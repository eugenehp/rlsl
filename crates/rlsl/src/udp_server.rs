//! UDP service responder for a stream outlet.
//!
//! Handles:
//! - LSL:shortinfo queries (discovery)
//! - LSL:timedata queries (time synchronization)
//!
//! Supports both IPv4 and IPv6.

use crate::clock::local_clock;
use crate::config::CONFIG;
use crate::stream_info::StreamInfo;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;

pub struct UdpServer;

impl UdpServer {
    /// Start the unicast UDP service (time sync + shortinfo on a dedicated port).
    /// Binds on both IPv4 and IPv6. Returns (v4_port, v6_port).
    pub fn start_unicast(info: StreamInfo, shutdown: Arc<AtomicBool>) -> (u16, u16) {
        // --- IPv4 unicast ---
        let v4_port = match crate::RUNTIME.block_on(bind_service(IpAddr::V4(Ipv4Addr::UNSPECIFIED)))
        {
            Ok(socket) => {
                let port = socket.local_addr().map_or(0, |a| a.port());
                let shortinfo = info.to_shortinfo_message();
                let info_clone = info.clone();
                let shutdown = shutdown.clone();

                crate::RUNTIME.spawn(async move {
                    run_unicast_loop(socket, &info_clone, &shortinfo, &shutdown).await;
                });
                port
            }
            Err(e) => {
                // no IPv4, or every allowed port taken: no IPv4 service, not a crash
                log::error!("no IPv4 service port for the outlet: {e}");
                0
            }
        };

        // --- IPv6 unicast ---
        let v6_port = if CONFIG.allow_ipv6 {
            match crate::RUNTIME.block_on(bind_service(IpAddr::V6(Ipv6Addr::UNSPECIFIED))) {
                Ok(socket) => {
                    let port = socket.local_addr().map_or(0, |a| a.port());
                    let shortinfo = info.to_shortinfo_message();
                    let info_clone = info.clone();
                    let shutdown = shutdown.clone();

                    crate::RUNTIME.spawn(async move {
                        run_unicast_loop(socket, &info_clone, &shortinfo, &shutdown).await;
                    });
                    port
                }
                Err(_) => 0,
            }
        } else {
            0
        };

        (v4_port, v6_port)
    }

    /// Start multicast/broadcast responders on the multicast port.
    /// Creates listeners for both IPv4 and IPv6 multicast groups.
    pub fn start_multicast(info: StreamInfo, shutdown: Arc<AtomicBool>) {
        let shortinfo = info.to_shortinfo_message();

        for &addr in &CONFIG.multicast_addresses {
            // Skip IPv6 addresses if disabled
            if addr.is_ipv6() && !CONFIG.allow_ipv6 {
                continue;
            }

            let shortinfo = shortinfo.clone();
            let info = info.clone();
            let shutdown = shutdown.clone();

            crate::RUNTIME.spawn(async move {
                let socket = match create_multicast_listener(addr, CONFIG.multicast_port).await {
                    Ok(s) => s,
                    Err(_) => return,
                };

                let mut buf = vec![0u8; 65536];
                loop {
                    if shutdown.load(Ordering::Relaxed) { break; }
                    tokio::select! {
                        result = socket.recv_from(&mut buf) => {
                            if let Ok((len, peer_addr)) = result {
                                let msg = std::str::from_utf8(&buf[..len]).unwrap_or("");
                                let mut lines = msg.lines();
                                let method = lines.next().unwrap_or("").trim();

                                if method == "LSL:shortinfo" {
                                    let query = lines.next().unwrap_or("").trim().to_string();
                                    let params_line = lines.next().unwrap_or("").trim().to_string();
                                    let parts: Vec<&str> = params_line.split_whitespace().collect();
                                    let return_port: u16 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                                    let query_id = parts.get(1).unwrap_or(&"").to_string();

                                    if info.matches_query(&query) {
                                        let reply = format!("{}\r\n{}", query_id, shortinfo);
                                        let return_addr = SocketAddr::new(peer_addr.ip(), return_port);
                                        let _ = socket.send_to(reply.as_bytes(), return_addr).await;
                                    }
                                }
                            }
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                            if shutdown.load(Ordering::Relaxed) { break; }
                        }
                    }
                }
            });
        }
    }
}

// ── Shared unicast handler loop ──────────────────────────────────────

/// Bind the service socket where liblsl binds its own: on the first free port of the LSL range,
/// so that a peer that knows this host only by name (`KnownPeers`) finds the stream by asking
/// those ports, and a firewall can be opened for them; any free port when the range is taken.
async fn bind_service(ip: IpAddr) -> std::io::Result<UdpSocket> {
    let mut last = std::io::Error::other("no service port to try");
    for port in crate::config::service_ports() {
        match UdpSocket::bind(SocketAddr::new(ip, port)).await {
            Ok(socket) => return Ok(socket),
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn run_unicast_loop(
    socket: UdpSocket,
    info: &StreamInfo,
    shortinfo: &str,
    shutdown: &Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; 65536];
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        tokio::select! {
            result = socket.recv_from(&mut buf) => {
                if let Ok((len, addr)) = result {
                    let msg = std::str::from_utf8(&buf[..len]).unwrap_or("");
                    let mut lines = msg.lines();
                    let method = lines.next().unwrap_or("").trim();

                    if method == "LSL:shortinfo" {
                        let query = lines.next().unwrap_or("").trim().to_string();
                        let params_line = lines.next().unwrap_or("").trim().to_string();
                        let parts: Vec<&str> = params_line.split_whitespace().collect();
                        let return_port: u16 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                        let query_id = parts.get(1).unwrap_or(&"").to_string();

                        if info.matches_query(&query) {
                            // The description as it is now, not as it was when this
                            // responder was started: the service ports are only known once
                            // every socket is bound, which is after that.
                            let _ = shortinfo;
                            let reply = format!("{}\r\n{}", query_id, info.to_shortinfo_message());
                            let return_addr = SocketAddr::new(addr.ip(), return_port);
                            let _ = socket.send_to(reply.as_bytes(), return_addr).await;
                        }
                    } else if method == "LSL:timedata" {
                        let t1 = local_clock();
                        let params = lines.next().unwrap_or("").trim().to_string();
                        let parts: Vec<&str> = params.split_whitespace().collect();
                        let wave_id = parts.first().unwrap_or(&"0");
                        let t0 = parts.get(1).unwrap_or(&"0");
                        let t2 = local_clock();
                        let reply = format!(" {} {} {} {}", wave_id, t0, t1, t2);
                        let _ = socket.send_to(reply.as_bytes(), addr).await;
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                if shutdown.load(Ordering::Relaxed) { break; }
            }
        }
    }
}

// ── Multicast socket helpers ─────────────────────────────────────────

async fn create_multicast_listener(addr: IpAddr, port: u16) -> std::io::Result<UdpSocket> {
    match addr {
        IpAddr::V4(v4) => create_multicast_listener_v4(v4, port).await,
        IpAddr::V6(v6) => create_multicast_listener_v6(v6, port).await,
    }
}

async fn create_multicast_listener_v4(addr: Ipv4Addr, port: u16) -> std::io::Result<UdpSocket> {
    let socket2 = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket2.set_reuse_address(true)?;
    #[cfg(unix)]
    socket2.set_reuse_port(true)?;

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    socket2.bind(&bind_addr.into())?;

    if addr.is_multicast() {
        let _ = socket2.join_multicast_v4(&addr, &Ipv4Addr::UNSPECIFIED);
        let _ = socket2.set_multicast_ttl_v4(CONFIG.multicast_ttl);
    }

    socket2.set_nonblocking(true)?;
    let std_socket: std::net::UdpSocket = socket2.into();
    UdpSocket::from_std(std_socket)
}

async fn create_multicast_listener_v6(addr: Ipv6Addr, port: u16) -> std::io::Result<UdpSocket> {
    let socket2 = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket2.set_reuse_address(true)?;
    #[cfg(unix)]
    socket2.set_reuse_port(true)?;
    // Don't use dual-stack on the multicast listener — we have separate v4 listeners.
    let _ = socket2.set_only_v6(true);

    let bind_addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);
    socket2.bind(&bind_addr.into())?;

    if is_ipv6_multicast(&addr) {
        // interface 0 = all interfaces
        let _ = socket2.join_multicast_v6(&addr, 0);
        let _ = socket2.set_multicast_hops_v6(CONFIG.multicast_ttl);
    }

    socket2.set_nonblocking(true)?;
    let std_socket: std::net::UdpSocket = socket2.into();
    UdpSocket::from_std(std_socket)
}

fn is_ipv6_multicast(addr: &Ipv6Addr) -> bool {
    addr.segments()[0] & 0xff00 == 0xff00
}
