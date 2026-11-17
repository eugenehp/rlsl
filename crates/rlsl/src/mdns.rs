//! Streams announced by DNS-SD — Bonjour on Apple systems, Avahi on Linux — as well as by
//! LSL's own discovery (cargo feature `mdns`).
//!
//! Every outlet registers a `_lsl._tcp` service: its instance name is the stream's name and
//! host, its port the outlet's data port, and its TXT record says what the stream is (`name`,
//! `type`, `channels`, `srate`, `format`, `source_id`, `uid`, `service_port`). Anything that
//! browses DNS-SD sees it: `dns-sd -B _lsl._tcp`, `avahi-browse -r _lsl._tcp`.
//!
//! A resolver browses for these services too, and asks each machine that announces one the
//! ordinary LSL question, directly ([`crate::resolver::add_known_peer`]). So what a resolver
//! returns is exactly what it returns without this feature — the announcement only says where
//! to ask — and streams are found on networks that carry mDNS but not LSL's multicast groups.
//! Like those, mDNS stays on the local link; across routers it needs an mDNS reflector, or
//! `KnownPeers`.
//!
//! `LSL_MDNS=0` (or `false`, `off`) switches both off at run time. A machine where the mDNS
//! port cannot be opened goes on without it.
use crate::stream_info::StreamInfo;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// The DNS-SD service type of an LSL stream.
pub const SERVICE_TYPE: &str = "_lsl._tcp.local.";

fn enabled() -> bool {
    !matches!(
        std::env::var("LSL_MDNS").ok().as_deref().map(str::trim),
        Some("0" | "false" | "off" | "no")
    )
}

/// One mDNS responder for the process, made when first needed.
fn daemon() -> Option<&'static ServiceDaemon> {
    static DAEMON: once_cell::sync::Lazy<Option<ServiceDaemon>> =
        once_cell::sync::Lazy::new(|| {
            if !enabled() {
                return None;
            }
            ServiceDaemon::new()
                .inspect_err(|e| {
                    log::warn!(
                        "mDNS: not available ({e}); streams are announced by LSL discovery only"
                    )
                })
                .ok()
        });
    DAEMON.as_ref()
}

/// A stream's announcement; it is withdrawn when this is dropped.
pub struct Announcement {
    fullname: String,
}

impl Drop for Announcement {
    fn drop(&mut self) {
        if let Some(d) = daemon() {
            let _ = d.unregister(&self.fullname);
        }
    }
}

fn host_label(host: &str) -> String {
    let label: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if label.is_empty() {
        "lsl-host".into()
    } else {
        label
    }
}

/// Announce an outlet's stream (`info` with its ports set). `None` when mDNS is switched off
/// or not available here.
pub fn announce(info: &StreamInfo) -> Option<Announcement> {
    let daemon = daemon()?;
    let host = host_label(&info.hostname());
    let uid = info.uid();
    // unique on the network: two outlets of one name on one machine differ in their uid
    let instance = format!("{} @ {host} ({})", info.name(), &uid[..uid.len().min(8)]);
    let txt = [
        ("name", info.name()),
        ("type", info.type_()),
        ("channels", info.channel_count().to_string()),
        ("srate", info.nominal_srate().to_string()),
        ("format", info.channel_format().as_str().to_string()),
        ("source_id", info.source_id()),
        ("uid", uid),
        ("service_port", info.v4service_port().to_string()),
    ];
    let service = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &format!("{host}.local."),
        "",
        info.v4data_port(),
        &txt[..],
    )
    .inspect_err(|e| log::warn!("mDNS: '{instance}' not announced: {e}"))
    .ok()?
    .enable_addr_auto();
    let fullname = service.get_fullname().to_string();
    daemon
        .register(service)
        .inspect_err(|e| log::warn!("mDNS: '{instance}' not announced: {e}"))
        .ok()?;
    Some(Announcement { fullname })
}

/// A stream as DNS-SD announces it.
#[derive(Debug, Clone, PartialEq)]
pub struct Announced {
    pub name: String,
    pub stream_type: String,
    pub source_id: String,
    pub uid: String,
    /// The machine's mDNS name (`pi.local.`).
    pub host: String,
    pub addresses: Vec<IpAddr>,
    /// The outlet's TCP data port.
    pub port: u16,
}

fn from_resolved(s: &mdns_sd::ResolvedService) -> Announced {
    let txt = |key: &str| {
        s.txt_properties
            .get_property_val_str(key)
            .unwrap_or_default()
            .to_string()
    };
    let mut addresses: Vec<IpAddr> = s
        .addresses
        .iter()
        .map(mdns_sd::ScopedIp::to_ip_addr)
        .collect();
    addresses.sort();
    Announced {
        name: txt("name"),
        stream_type: txt("type"),
        source_id: txt("source_id"),
        uid: txt("uid"),
        host: s.host.clone(),
        addresses,
        port: s.port,
    }
}

/// Browse in the background from now on, and have every machine that announces a stream asked
/// directly by the resolvers of this process. Called by the resolver itself; calling it again
/// does nothing.
pub fn follow_announcements() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let Some(daemon) = daemon() else { return };
        let Ok(events) = daemon.browse(SERVICE_TYPE) else {
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("lsl_mdns".into())
            .spawn(move || {
                while let Ok(event) = events.recv() {
                    if let ServiceEvent::ServiceResolved(service) = event {
                        for ip in service.addresses.iter().map(mdns_sd::ScopedIp::to_ip_addr) {
                            // a link-local IPv6 address is no use without its interface
                            if matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
                            {
                                continue;
                            }
                            crate::resolver::add_known_peer(&ip.to_string());
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            log::warn!("mDNS: no thread to browse on: {e}");
        }
    });
}

/// The streams announced by DNS-SD that answer within `wait`: what `dns-sd -B _lsl._tcp`
/// would show, for tools. To *use* streams, resolve them as always — the resolver follows
/// these announcements by itself.
pub fn browse(wait: Duration) -> Vec<Announced> {
    let Some(daemon) = daemon() else {
        return Vec::new();
    };
    let Ok(events) = daemon.browse(SERVICE_TYPE) else {
        return Vec::new();
    };
    let until = Instant::now() + wait;
    let mut found: Vec<Announced> = Vec::new();
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(service)) => {
                let a = from_resolved(&service);
                match found
                    .iter_mut()
                    .find(|f| f.uid == a.uid && f.host == a.host)
                {
                    Some(f) => *f = a,
                    None => found.push(a),
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    found
}
