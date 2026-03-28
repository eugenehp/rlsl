//! API configuration, matching liblsl's `lsl_api.cfg`.
//!
//! Loads settings from (in priority order):
//! 1. Environment variables (`LSL_IPV6`, `LSL_MULTICAST_PORT`, etc.)
//! 2. `lsl_api.cfg` in current dir, home dir, or `/etc/lsl_api/lsl_api.cfg`
//! 3. Built-in defaults

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

pub static CONFIG: Lazy<ApiConfig> = Lazy::new(ApiConfig::load);

pub struct ApiConfig {
    pub multicast_port: u16,
    pub base_port: u16,
    pub port_range: u16,
    pub allow_random_ports: bool,
    pub allow_ipv4: bool,
    pub allow_ipv6: bool,
    pub multicast_addresses: Vec<IpAddr>,
    pub multicast_ttl: u32,
    pub session_id: String,
    pub use_protocol_version: i32,
    pub smoothing_halftime: f32,
    pub time_update_interval: f64,
    pub time_probe_count: i32,
    pub time_probe_interval: f64,
    pub time_probe_max_rtt: f64,
    pub time_update_minprobes: i32,
}

impl ApiConfig {
    fn load() -> Self {
        let file_cfg = load_config_file();

        let get = |key: &str| -> Option<String> {
            // Env var first (LSL_ prefix, uppercase)
            std::env::var(format!("LSL_{}", key.to_uppercase()))
                .ok()
                .or_else(|| file_cfg.get(key).cloned())
        };

        let get_bool = |key: &str, default: bool| -> bool {
            get(key)
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(default)
        };
        let get_u16 = |key: &str, default: u16| -> u16 {
            get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
        };
        let get_u32 = |key: &str, default: u32| -> u32 {
            get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
        };
        let get_f32 = |key: &str, default: f32| -> f32 {
            get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
        };
        let get_f64 = |key: &str, default: f64| -> f64 {
            get(key).and_then(|v| v.parse().ok()).unwrap_or(default)
        };

        let allow_ipv6 = get_bool("ipv6", true);

        // Parse multicast addresses from config or use defaults
        let multicast_addresses = if let Some(addrs_str) = get("multicast_addresses") {
            addrs_str
                .split(&[',', ' '][..])
                .filter_map(|s| s.trim().parse::<IpAddr>().ok())
                .collect()
        } else {
            let mut addrs: Vec<IpAddr> = vec![
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                IpAddr::V4(Ipv4Addr::BROADCAST),
                IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
                IpAddr::V4(Ipv4Addr::new(224, 0, 0, 183)),
                IpAddr::V4(Ipv4Addr::new(239, 255, 172, 215)),
            ];
            if allow_ipv6 {
                addrs.extend([
                    IpAddr::V6(Ipv6Addr::LOCALHOST),
                    IpAddr::V6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1)),
                    IpAddr::V6(Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0x113a)),
                    IpAddr::V6(Ipv6Addr::new(0xff05, 0, 0, 0, 0, 0, 0, 0x113a)),
                ]);
            }
            addrs
        };

        Self {
            multicast_port: get_u16("multicast_port", 16571),
            base_port: get_u16("base_port", 16572),
            port_range: get_u16("port_range", 32),
            allow_random_ports: get_bool("allow_random_ports", true),
            allow_ipv4: get_bool("ipv4", true),
            allow_ipv6,
            multicast_addresses,
            multicast_ttl: get_u32("multicast_ttl", 24),
            session_id: get("session_id").unwrap_or_else(|| "default".into()),
            use_protocol_version: get("protocol_version")
                .and_then(|v| v.parse().ok())
                .unwrap_or(110),
            smoothing_halftime: get_f32("smoothing_halftime", 90.0),
            time_update_interval: get_f64("time_update_interval", 2.0),
            time_probe_count: get("time_probe_count")
                .and_then(|v| v.parse().ok())
                .unwrap_or(8),
            time_probe_interval: get_f64("time_probe_interval", 0.064),
            time_probe_max_rtt: get_f64("time_probe_max_rtt", 0.128),
            time_update_minprobes: get("time_update_minprobes")
                .and_then(|v| v.parse().ok())
                .unwrap_or(6),
        }
    }
}

/// Load key=value pairs from `lsl_api.cfg` (INI-like format).
fn load_config_file() -> HashMap<String, String> {
    let candidates = [
        Some(std::path::PathBuf::from("lsl_api.cfg")),
        dirs_path("lsl_api.cfg"),
        Some(std::path::PathBuf::from("/etc/lsl_api/lsl_api.cfg")),
    ];

    for candidate in candidates.iter().flatten() {
        if candidate.exists() {
            if let Ok(contents) = std::fs::read_to_string(candidate) {
                log::info!("Loaded LSL config from {}", candidate.display());
                return parse_ini(&contents);
            }
        }
    }

    HashMap::new()
}

fn dirs_path(filename: &str) -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|h| Path::new(&h).join(".lsl").join(filename))
}

fn parse_ini(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty()
            || line.starts_with('#')
            || line.starts_with(';')
            || line.starts_with('[')
        {
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim().to_lowercase().replace('-', "_");
            let val = line[eq + 1..].trim().to_string();
            map.insert(key, val);
        }
    }
    map
}
