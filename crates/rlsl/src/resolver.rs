//! Stream resolver: discovers streams on the network via UDP.
//!
//! Sends LSL:shortinfo queries over IPv4 and IPv6 multicast/broadcast/unicast,
//! collects responses, and returns discovered StreamInfo objects.

use crate::config::CONFIG;
use crate::stream_info::StreamInfo;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Resolve streams matching a query string.
pub fn resolve_all(wait_time: f64) -> Vec<StreamInfo> {
    resolve_query("", 0, wait_time)
}

/// Resolve streams by property name and value.
pub fn resolve_by_property(prop: &str, value: &str, minimum: i32, timeout: f64) -> Vec<StreamInfo> {
    let query = if value.is_empty() {
        String::new()
    } else {
        format!("{}='{}'", prop, value)
    };
    resolve_query(&query, minimum, timeout)
}

/// Resolve streams by predicate.
pub fn resolve_by_predicate(pred: &str, minimum: i32, timeout: f64) -> Vec<StreamInfo> {
    resolve_query(pred, minimum, timeout)
}

/// Core resolve function.
pub fn resolve_query(query: &str, minimum: i32, timeout: f64) -> Vec<StreamInfo> {
    // Spawn on the RUNTIME and wait via channel (avoids block_on deadlock
    // when called from a thread that's already inside the RUNTIME).
    let query = query.to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    crate::RUNTIME.spawn(async move {
        let result = resolve_query_async(&query, minimum, timeout).await;
        let _ = tx.send(result);
    });
    let deadline = std::time::Duration::from_secs_f64(timeout + 2.0);
    rx.recv_timeout(deadline).unwrap_or_default()
}

async fn resolve_query_async(query: &str, minimum: i32, timeout: f64) -> Vec<StreamInfo> {
    let wait = Duration::from_secs_f64(timeout.max(0.01));

    // Create IPv4 receiver socket
    let v4_recv = UdpSocket::bind("0.0.0.0:0").await.ok();
    let v4_port = v4_recv
        .as_ref()
        .and_then(|s| s.local_addr().ok())
        .map_or(0, |a| a.port());

    // Create IPv6 receiver socket
    let v6_recv = if CONFIG.allow_ipv6 {
        UdpSocket::bind("[::]:0").await.ok()
    } else {
        None
    };
    let v6_port = v6_recv
        .as_ref()
        .and_then(|s| s.local_addr().ok())
        .map_or(0, |a| a.port());

    // Build query message. For IPv6 targets we use v6_port, for v4 we use v4_port.
    let query_id = format!("{}", fxhash::hash32(query));

    // Build target list: (address, message)
    let mut targets: Vec<(SocketAddr, String)> = Vec::new();

    for &addr in &CONFIG.multicast_addresses {
        let ret_port = if addr.is_ipv6() { v6_port } else { v4_port };
        if ret_port == 0 {
            continue;
        }
        let msg = format!(
            "LSL:shortinfo\r\n{}\r\n{} {}\r\n",
            query, ret_port, query_id
        );
        targets.push((SocketAddr::new(addr, CONFIG.multicast_port), msg));
    }

    // Unicast to base ports (IPv4)
    {
        let msg = format!("LSL:shortinfo\r\n{}\r\n{} {}\r\n", query, v4_port, query_id);
        for port in CONFIG.base_port..CONFIG.base_port + CONFIG.port_range {
            targets.push((
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
                msg.clone(),
            ));
        }
    }

    // Unicast to base ports (IPv6 loopback)
    if CONFIG.allow_ipv6 && v6_port != 0 {
        let msg = format!("LSL:shortinfo\r\n{}\r\n{} {}\r\n", query, v6_port, query_id);
        for port in CONFIG.base_port..CONFIG.base_port + CONFIG.port_range {
            targets.push((
                SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
                msg.clone(),
            ));
        }
    }

    // Known peers: every service port of each, asked directly. This is what finds streams
    // where multicast and broadcast do not get through.
    for peer in known_peers() {
        let Ok(addrs) = tokio::net::lookup_host((peer.as_str(), 0)).await else {
            log::debug!("known peer {peer}: name not resolved");
            continue;
        };
        for ip in addrs.map(|a| a.ip()) {
            let ret_port = if ip.is_ipv6() { v6_port } else { v4_port };
            if ret_port == 0
                || (ip.is_ipv6() && !CONFIG.allow_ipv6)
                || (ip.is_ipv4() && !CONFIG.allow_ipv4)
            {
                continue;
            }
            let msg = format!(
                "LSL:shortinfo\r\n{}\r\n{} {}\r\n",
                query, ret_port, query_id
            );
            for port in CONFIG.base_port..CONFIG.base_port.saturating_add(CONFIG.port_range) {
                targets.push((SocketAddr::new(ip, port), msg.clone()));
            }
        }
    }

    // Send queries with retry waves
    let end = tokio::time::Instant::now() + wait;
    let mut results: HashMap<String, StreamInfo> = HashMap::new();
    // Once `minimum` streams are known: how long to keep listening for their other address.
    let mut settle_until: Option<tokio::time::Instant> = None;
    let mut wave_interval = Duration::from_millis(500);
    let mut next_wave = tokio::time::Instant::now();

    let mut buf4 = vec![0u8; 65536];
    let mut buf6 = vec![0u8; 65536];

    // Prepare send sockets
    let v4_send = UdpSocket::bind("0.0.0.0:0").await.ok();
    if let Some(ref s) = v4_send {
        let _ = s.set_broadcast(true);
    }

    let v6_send = if CONFIG.allow_ipv6 {
        UdpSocket::bind("[::]:0").await.ok()
    } else {
        None
    };

    loop {
        let now = tokio::time::Instant::now();
        if now >= end {
            break;
        }
        if minimum > 0 && results.len() >= minimum as usize {
            // Enough streams. An outlet answers over IPv4 and IPv6, and whichever reply came
            // first may be the one whose address is of no use from here (a link-local IPv6
            // address): give the other a moment to arrive, so the stream is known by both.
            let complete = !CONFIG.allow_ipv4
                || !CONFIG.allow_ipv6
                || results.values().all(|i| !i.v4address().is_empty());
            let grace = *settle_until.get_or_insert(now + Duration::from_millis(100));
            if complete || now >= grace {
                break;
            }
        }

        // Send a wave of queries
        if now >= next_wave {
            for (target, msg) in &targets {
                let sock = if target.is_ipv4() { &v4_send } else { &v6_send };
                if let Some(s) = sock {
                    let _ = s.send_to(msg.as_bytes(), target).await;
                }
            }
            next_wave = now + wave_interval;
            wave_interval = (wave_interval * 2).min(Duration::from_secs(3));
        }

        // Receive replies from both sockets
        let remaining = end.saturating_duration_since(tokio::time::Instant::now());
        let recv_timeout = remaining.min(Duration::from_millis(100));

        tokio::select! {
            // IPv4 replies
            result = async {
                match &v4_recv {
                    Some(s) => s.recv_from(&mut buf4).await,
                    None => std::future::pending().await,
                }
            } => {
                if let Ok((len, addr)) = result {
                    if let Some(info) = parse_reply(&buf4[..len], &query_id) {
                        let uid = info.uid();
                        if !uid.is_empty() {
                            // Populate source address for cross-machine networking
                            if info.v4address().is_empty() {
                                info.set_v4address(&addr.ip().to_string());
                            }
                            merge_reply(&mut results, uid, info);
                        }
                    }
                }
            }
            // IPv6 replies
            result = async {
                match &v6_recv {
                    Some(s) => s.recv_from(&mut buf6).await,
                    None => std::future::pending().await,
                }
            } => {
                if let Ok((len, addr)) = result {
                    if let Some(info) = parse_reply(&buf6[..len], &query_id) {
                        let uid = info.uid();
                        if !uid.is_empty() {
                            if info.v6address().is_empty() {
                                info.set_v6address(&addr.ip().to_string());
                            }
                            merge_reply(&mut results, uid, info);
                        }
                    }
                }
            }
            _ = tokio::time::sleep(recv_timeout) => {}
        }
    }

    results.into_values().collect()
}

/// Record a reply. A stream that is already known (it answers once per address family and per
/// query wave) keeps its entry and gains whichever address the earlier replies did not carry.
fn merge_reply(results: &mut HashMap<String, StreamInfo>, uid: String, info: StreamInfo) {
    match results.get(&uid) {
        Some(known) => {
            if known.v4address().is_empty() && !info.v4address().is_empty() {
                known.set_v4address(&info.v4address());
            }
            if known.v6address().is_empty() && !info.v6address().is_empty() {
                known.set_v6address(&info.v6address());
            }
        }
        None => {
            results.insert(uid, info);
        }
    }
}

fn parse_reply(data: &[u8], expected_id: &str) -> Option<StreamInfo> {
    if let Some(newline_pos) = data.iter().position(|&b| b == b'\n') {
        let returned_id = std::str::from_utf8(&data[..newline_pos])
            .unwrap_or("")
            .trim();
        if returned_id == expected_id {
            let xml = std::str::from_utf8(&data[newline_pos + 1..]).unwrap_or("");
            return StreamInfo::from_shortinfo_message(xml);
        }
    }
    None
}

static EXTRA_PEERS: once_cell::sync::Lazy<parking_lot::Mutex<Vec<String>>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(Vec::new()));

/// Ask `host` (a name or an address) for its streams directly from now on, as if it were
/// listed in `KnownPeers` — for a host on a network that multicast does not cross.
pub fn add_known_peer(host: &str) {
    let host = host.trim();
    let mut peers = EXTRA_PEERS.lock();
    if !host.is_empty()
        && !peers.iter().any(|p| p == host)
        && !CONFIG.known_peers.iter().any(|p| p == host)
    {
        peers.push(host.to_string());
    }
}

/// The hosts that are asked directly: the configured `KnownPeers` and those added with
/// [`add_known_peer`].
pub fn known_peers() -> Vec<String> {
    let mut peers = CONFIG.known_peers.clone();
    peers.extend(EXTRA_PEERS.lock().iter().cloned());
    peers
}

/// Continuous resolver that keeps discovering streams in the background.
///
/// A stream stays in [`results`](Self::results) from the moment it is first seen until it has
/// not answered for `forget_after` seconds — one missed answer does not make it disappear.
/// [`take_new`](Self::take_new) and [`take_lost`](Self::take_lost) say what has changed since
/// they were last asked, for a program that follows streams as they come and go.
pub struct ContinuousResolver {
    state: std::sync::Arc<parking_lot::Mutex<ContinuousState>>,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
struct ContinuousState {
    /// uid → (the stream, when it last answered)
    streams: HashMap<String, (StreamInfo, std::time::Instant)>,
    new: Vec<StreamInfo>,
    lost: Vec<StreamInfo>,
}

impl ContinuousState {
    /// Take in the streams that answered a round at `now`, and forget those silent for
    /// `forget_after`.
    fn update(&mut self, found: Vec<StreamInfo>, now: std::time::Instant, forget_after: Duration) {
        for info in found {
            let uid = info.uid();
            if self.streams.insert(uid, (info.clone(), now)).is_none() {
                self.new.push(info);
            }
        }
        let gone: Vec<String> = self
            .streams
            .iter()
            .filter(|(_, (_, seen))| now.saturating_duration_since(*seen) > forget_after)
            .map(|(uid, _)| uid.clone())
            .collect();
        for uid in gone {
            if let Some((info, _)) = self.streams.remove(&uid) {
                // never reported as new: not worth reporting as lost
                match self.new.iter().position(|n| n.uid() == uid) {
                    Some(k) => drop(self.new.remove(k)),
                    None => self.lost.push(info),
                }
            }
        }
    }
}

impl ContinuousResolver {
    /// `forget_after`: seconds without an answer after which a stream is dropped (liblsl's
    /// default is 5; anything not positive is taken as that).
    pub fn new(query: &str, forget_after: f64) -> Self {
        let state = std::sync::Arc::new(parking_lot::Mutex::new(ContinuousState::default()));
        let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let forget_after = Duration::from_secs_f64(if forget_after > 0.0 {
            forget_after
        } else {
            5.0
        });

        let state_clone = state.clone();
        let shutdown_clone = shutdown.clone();
        let query = query.to_string();

        std::thread::spawn(move || {
            while !shutdown_clone.load(std::sync::atomic::Ordering::Relaxed) {
                let found = resolve_query(&query, 0, 1.0);
                state_clone
                    .lock()
                    .update(found, std::time::Instant::now(), forget_after);
                std::thread::sleep(Duration::from_secs_f64(0.5));
            }
        });

        ContinuousResolver { state, shutdown }
    }

    /// The streams known at the moment.
    pub fn results(&self) -> Vec<StreamInfo> {
        self.state
            .lock()
            .streams
            .values()
            .map(|(info, _)| info.clone())
            .collect()
    }

    /// The streams that have appeared since this was last called.
    pub fn take_new(&self) -> Vec<StreamInfo> {
        std::mem::take(&mut self.state.lock().new)
    }

    /// The streams that have been forgotten since this was last called.
    pub fn take_lost(&self) -> Vec<StreamInfo> {
        std::mem::take(&mut self.state.lock().lost)
    }
}

impl Drop for ContinuousResolver {
    fn drop(&mut self) {
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
mod continuous_tests {
    use super::*;
    use crate::types::ChannelFormat;
    use std::time::Instant;

    fn stream(name: &str) -> StreamInfo {
        StreamInfo::new(name, "Test", 1, 0.0, ChannelFormat::Float32, name)
    }

    #[test]
    fn streams_stay_until_forgotten_and_changes_are_reported_once() {
        let (a, b) = (stream("a"), stream("b"));
        let forget = Duration::from_secs(5);
        let t0 = Instant::now();
        let mut state = ContinuousState::default();

        state.update(vec![a.clone(), b.clone()], t0, forget);
        assert_eq!(state.streams.len(), 2);
        assert_eq!(std::mem::take(&mut state.new).len(), 2);

        // b misses a few rounds: still there, nothing new, nothing lost
        state.update(vec![a.clone()], t0 + Duration::from_secs(3), forget);
        assert_eq!(state.streams.len(), 2);
        assert!(state.new.is_empty() && state.lost.is_empty());

        // b answers again in time: its silence starts over
        state.update(
            vec![a.clone(), b.clone()],
            t0 + Duration::from_secs(4),
            forget,
        );
        state.update(vec![a.clone()], t0 + Duration::from_secs(8), forget);
        assert_eq!(state.streams.len(), 2);

        // silent for longer than `forget_after`: gone, and said so once
        state.update(vec![a.clone()], t0 + Duration::from_secs(10), forget);
        assert_eq!(state.streams.len(), 1);
        let lost = std::mem::take(&mut state.lost);
        assert_eq!((lost.len(), lost[0].uid()), (1, b.uid()));
        state.update(vec![a.clone()], t0 + Duration::from_secs(11), forget);
        assert!(state.lost.is_empty() && state.new.is_empty());

        // back: new again
        state.update(vec![a, b.clone()], t0 + Duration::from_secs(12), forget);
        assert_eq!(std::mem::take(&mut state.new)[0].uid(), b.uid());
    }
}
