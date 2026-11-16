//! `lsl` — Unified CLI tool for Lab Streaming Layer.
//!
//! ```text
//! Usage:
//!   rlsl list    [--timeout SECS] [--query QUERY] [--json]
//!   rlsl gen     [--name N] [--type T] [--channels N] [--srate HZ] [--waveform W]
//!   rlsl record  [--output FILE] [--format xdf|parquet] [--query QUERY]
//!   rlsl convert <input> [--to parquet|csv|info] [-o output]
//!   rlsl bench   [--channels N] [--srate HZ] [--duration SECS]
//!   rlsl ntp     serve [--bind ADDR] [--port P] [--stratum N]   (feature `ntp`)
//!   rlsl ntp     query <server> [--probes N] [--timeout SECS]   (feature `ntp`)
//!   rlsl ntp     track <server> [--interval SECS] [--count N]   (feature `ntp`)
//!   rlsl ntp     mesh  [--id NAME] [--port P] [--peer HOST[:P]]... (feature `ntp`)
//!   rlsl info    <file>
//!   rlsl version
//!   rlsl help    [command]
//! ```

use anyhow::Result;
use rlsl::prelude::*;
use rlsl::{clock::local_clock, resolver};
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    let sub_args: Vec<String> = args.iter().skip(2).cloned().collect();

    match cmd {
        "list" | "ls" => cmd_list(&sub_args),
        "gen" | "generate" => cmd_gen(&sub_args),
        "bench" | "benchmark" => cmd_bench(&sub_args),
        #[cfg(feature = "ntp")]
        "ntp" => cmd_ntp(&sub_args),
        "version" | "--version" | "-V" => cmd_version(),
        "help" | "--help" | "-h" => cmd_help(sub_args.first().map(|s| s.as_str())),
        _ => {
            eprintln!("Unknown command: {}", cmd);
            eprintln!("Run `rrlsl help` for usage.");
            std::process::exit(1);
        }
    }
}

fn get_arg(args: &[String], key: &str, default: &str) -> String {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

// ── list ─────────────────────────────────────────────────────────────

fn cmd_list(args: &[String]) -> Result<()> {
    let timeout: f64 = get_arg(args, "--timeout", "2.0").parse()?;
    let query = get_arg(args, "--query", "");
    let json = has_flag(args, "--json");
    let continuous = has_flag(args, "--watch") || has_flag(args, "-w");

    if continuous {
        eprintln!("🔍 Watching for LSL streams (Ctrl-C to stop)...");
        loop {
            let streams = if query.is_empty() {
                resolver::resolve_all(timeout)
            } else {
                resolver::resolve_by_predicate(&query, 0, timeout)
            };

            // Clear screen
            eprint!("\x1b[2J\x1b[H");
            eprintln!("╔═══════════════════════════════════════════════════════════════════╗");
            eprintln!(
                "║  LSL Streams ({} found)                                          ║",
                streams.len()
            );
            eprintln!("╚═══════════════════════════════════════════════════════════════════╝");
            print_streams(&streams, json);
            std::thread::sleep(Duration::from_secs_f64(timeout));
        }
    } else {
        eprintln!("🔍 Resolving LSL streams ({}s)...", timeout);
        let streams = if query.is_empty() {
            resolver::resolve_all(timeout)
        } else {
            resolver::resolve_by_predicate(&query, 0, timeout)
        };

        if streams.is_empty() {
            eprintln!("No streams found.");
        } else {
            print_streams(&streams, json);
        }
    }
    Ok(())
}

fn print_streams(streams: &[StreamInfo], json: bool) {
    if json {
        print!("[");
        for (i, s) in streams.iter().enumerate() {
            if i > 0 {
                print!(",");
            }
            print!(
                r#"{{"name":"{}","type":"{}","channels":{},"srate":{},"format":"{}","source_id":"{}","hostname":"{}","uid":"{}"}}"#,
                s.name(),
                s.type_(),
                s.channel_count(),
                s.nominal_srate(),
                s.channel_format().as_str(),
                s.source_id(),
                s.hostname(),
                s.uid()
            );
        }
        println!("]");
    } else {
        for s in streams {
            eprintln!(
                "  {} [{}] — {}ch × {}Hz, {}, host={}, uid={}",
                s.name(),
                s.type_(),
                s.channel_count(),
                s.nominal_srate(),
                s.channel_format().as_str(),
                s.hostname(),
                &s.uid()[..8]
            );
        }
    }
}

// ── gen ──────────────────────────────────────────────────────────────

fn cmd_gen(args: &[String]) -> Result<()> {
    let name = get_arg(args, "--name", "LslGen");
    let type_ = get_arg(args, "--type", "EEG");
    let nch: u32 = get_arg(args, "--channels", "8").parse()?;
    let srate: f64 = get_arg(args, "--srate", "250").parse()?;
    let waveform = get_arg(args, "--waveform", "sine");
    let freq: f64 = get_arg(args, "--freq", "10.0").parse()?;
    let amplitude: f64 = get_arg(args, "--amplitude", "100.0").parse()?;
    // The stream's sample type: float32 (default), double64, int64, int32, int16, int8.
    let format_name = get_arg(args, "--format", "float32");

    let format = ChannelFormat::from_name(&format_name);
    if matches!(format, ChannelFormat::Undefined | ChannelFormat::String) {
        anyhow::bail!(
            "--format {format_name}: expected float32, double64, int64, int32, int16 or int8"
        );
    }
    let source_id = get_arg(args, "--source-id", "rlsl-cli-gen");
    let info = StreamInfo::new(&name, &type_, nch, srate, format, &source_id);
    let outlet = StreamOutlet::new(&info, 0, 360);
    #[cfg(feature = "ntp")]
    let _mesh = gen_clock_mesh(args, &outlet)?;

    eprintln!("🎵 rlsl gen — streaming:");
    eprintln!("   name={name}, type={type_}, {nch}ch, {srate}Hz, {format_name}");
    eprintln!("   waveform={waveform}, freq={freq}Hz, amplitude={amplitude}");
    eprintln!("   TCP port {}, Ctrl-C to stop", info.v4data_port());

    let interval = Duration::from_secs_f64(1.0 / srate);
    let mut sample_idx: u64 = 0;
    let mut sample = vec![0.0f64; nch as usize];

    loop {
        let t = sample_idx as f64 / srate;

        for (ch, s) in sample.iter_mut().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * freq * t + ch as f64 * 0.5;
            let value = match waveform.as_str() {
                "sine" => amplitude * phase.sin(),
                "square" => amplitude * phase.sin().signum(),
                "sawtooth" => amplitude * (2.0 * ((freq * t + ch as f64 * 0.1) % 1.0) - 1.0),
                "noise" => {
                    amplitude * (pseudo_random(sample_idx * nch as u64 + ch as u64) * 2.0 - 1.0)
                }
                "chirp" => {
                    let sweep = freq * (1.0 + 3.0 * (t % 10.0) / 10.0);
                    amplitude * (2.0 * std::f64::consts::PI * sweep * t).sin()
                }
                "counter" => (sample_idx * nch as u64 + ch as u64) as f64,
                _ => amplitude * phase.sin(),
            };
            *s = value;
        }

        // computed in double precision; the outlet stores each value in the stream's own type
        outlet.push_sample_d(&sample, 0.0, true);
        sample_idx += 1;
        std::thread::sleep(interval);
    }
}

/// With `--clock-peers`, run a clock mesh node next to the stream and pass its measurements on
/// with it (`--clock-transport in-band|companion|both`, default both).
#[cfg(feature = "ntp")]
fn gen_clock_mesh(args: &[String], outlet: &StreamOutlet) -> Result<Option<rlsl::ntp::ClockMesh>> {
    use rlsl::ntp::{ClockMesh, ClockMeshConfig, ClockTransport, DEFAULT_MESH_PORT};
    if !has_flag(args, "--clock-peers") {
        return Ok(None);
    }
    let peers: Vec<String> = get_arg(args, "--clock-peers", "")
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    let port: u16 = get_arg(args, "--clock-port", &DEFAULT_MESH_PORT.to_string()).parse()?;
    let interval: f64 = get_arg(args, "--clock-interval", "10").parse()?;
    let transport = match get_arg(args, "--clock-transport", "both").as_str() {
        "in-band" | "inband" => ClockTransport::IN_BAND,
        "companion" => ClockTransport::COMPANION,
        "both" => ClockTransport::BOTH,
        other => anyhow::bail!("--clock-transport {other}: expected in-band, companion or both"),
    };
    let mesh = ClockMesh::start(ClockMeshConfig {
        node_id: get_arg(args, "--clock-id", ""),
        bind: std::net::SocketAddr::from(([0, 0, 0, 0], port)),
        peers,
        interval: Duration::from_secs_f64(interval.max(1.0)),
        ..ClockMeshConfig::default()
    })
    .map_err(|e| anyhow::anyhow!("clock mesh on port {port}: {e}"))?;
    outlet.attach_clock_mesh(&mesh, transport);
    eprintln!(
        "   clock mesh node {} on {}, records {}{}",
        mesh.node_id(),
        mesh.local_addr(),
        if transport.in_band { "in-band" } else { "" },
        if transport.companion {
            if transport.in_band {
                " + companion stream"
            } else {
                "companion stream"
            }
        } else {
            ""
        }
    );
    Ok(Some(mesh))
}

fn pseudo_random(seed: u64) -> f64 {
    let x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (x >> 33) as f64 / (1u64 << 31) as f64
}

// ── bench ────────────────────────────────────────────────────────────

fn cmd_bench(args: &[String]) -> Result<()> {
    let nch: u32 = get_arg(args, "--channels", "8").parse()?;
    let srate: f64 = get_arg(args, "--srate", "1000").parse()?;
    let duration_secs: f64 = get_arg(args, "--duration", "5").parse()?;
    let format_str = get_arg(args, "--format", "float32");
    let fmt = ChannelFormat::from_name(&format_str);
    let n_samples = (srate * duration_secs) as u64;

    eprintln!(
        "⚡ rlsl bench: {}ch × {}Hz × {:.1}s = {} samples ({})",
        nch, srate, duration_secs, n_samples, format_str
    );

    let info = StreamInfo::new("LslCliBench", "Benchmark", nch, srate, fmt, "cli_bench");
    let outlet = StreamOutlet::new(&info, 0, 360);
    std::thread::sleep(Duration::from_secs(1));

    eprint!("  Resolving...");
    let streams = resolver::resolve_all(2.0);
    let bench_stream = streams
        .iter()
        .find(|s| s.name() == "LslCliBench")
        .ok_or_else(|| anyhow::anyhow!("Could not find LslCliBench stream"))?;
    let inlet = StreamInlet::new(bench_stream, 360, 0, false);
    inlet.open_stream(5.0).map_err(|e| anyhow::anyhow!(e))?;
    eprintln!(" connected.");

    let sample_data = vec![0.0f32; nch as usize];
    let mut recv_buf = vec![0.0f32; nch as usize];
    let mut latencies: Vec<f64> = Vec::with_capacity(n_samples as usize);

    // Warmup
    for _ in 0..100 {
        outlet.push_sample_f(&sample_data, 0.0, true);
        let _ = inlet.pull_sample_f(&mut recv_buf, 1.0);
    }

    let start = std::time::Instant::now();

    let outlet_handle = {
        let sample_data = sample_data.clone();
        std::thread::spawn(move || {
            let interval = Duration::from_secs_f64(1.0 / srate);
            for _ in 0..n_samples {
                let ts = local_clock();
                outlet.push_sample_f(&sample_data, ts, true);
                std::thread::sleep(interval);
            }
        })
    };

    let deadline = std::time::Instant::now() + Duration::from_secs_f64(duration_secs + 2.0);
    let mut pulled = 0u64;

    while std::time::Instant::now() < deadline && pulled < n_samples {
        match inlet.pull_sample_f(&mut recv_buf, 0.5) {
            Ok(ts) if ts > 0.0 => {
                latencies.push(local_clock() - ts);
                pulled += 1;
            }
            _ => {}
        }
    }
    let elapsed = start.elapsed();
    outlet_handle.join().unwrap();

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = latencies.iter().sum::<f64>() / latencies.len().max(1) as f64;
    let p50 = latencies.get(latencies.len() / 2).copied().unwrap_or(0.0);
    let p95 = latencies
        .get(latencies.len() * 95 / 100)
        .copied()
        .unwrap_or(0.0);
    let p99 = latencies
        .get(latencies.len() * 99 / 100)
        .copied()
        .unwrap_or(0.0);
    let throughput = pulled as f64 / elapsed.as_secs_f64();
    let data_rate = throughput * nch as f64 * fmt.channel_bytes() as f64 / 1_000_000.0;
    let loss = 1.0 - pulled as f64 / n_samples as f64;

    eprintln!();
    eprintln!("  ╔══════════════════════════════════════╗");
    eprintln!("  ║       rlsl bench — Results            ║");
    eprintln!("  ╠══════════════════════════════════════╣");
    eprintln!("  ║ Pushed:    {:>10} samples         ║", n_samples);
    eprintln!("  ║ Pulled:    {:>10} samples         ║", pulled);
    eprintln!("  ║ Loss:      {:>9.2}%               ║", loss * 100.0);
    eprintln!("  ║ Throughput:{:>9.0} samples/s      ║", throughput);
    eprintln!("  ║ Data rate: {:>9.2} MB/s            ║", data_rate);
    eprintln!("  ╠══════════════════════════════════════╣");
    eprintln!("  ║ Latency (push→pull):                ║");
    eprintln!("  ║   mean: {:>9.3} ms                ║", mean * 1000.0);
    eprintln!("  ║   p50:  {:>9.3} ms                ║", p50 * 1000.0);
    eprintln!("  ║   p95:  {:>9.3} ms                ║", p95 * 1000.0);
    eprintln!("  ║   p99:  {:>9.3} ms                ║", p99 * 1000.0);
    eprintln!("  ╚══════════════════════════════════════╝");

    Ok(())
}

// ── ntp ──────────────────────────────────────────────────────────────

#[cfg(feature = "ntp")]
fn cmd_ntp(args: &[String]) -> Result<()> {
    use rlsl::ntp::{self, NtpServer, NtpServerConfig};

    match args.first().map(|s| s.as_str()) {
        Some("serve") | Some("server") => {
            let bind = get_arg(args, "--bind", "0.0.0.0");
            let port: u16 = get_arg(args, "--port", &ntp::NTP_PORT.to_string()).parse()?;
            let stratum: u8 = get_arg(args, "--stratum", "10").parse()?;
            let addr: std::net::SocketAddr = if bind.contains(':') {
                format!("[{bind}]:{port}").parse()?
            } else {
                format!("{bind}:{port}").parse()?
            };
            let mut config = NtpServerConfig {
                stratum,
                ..NtpServerConfig::default()
            };
            if stratum == 1 {
                // a stratum-1 server names its reference clock in ASCII
                config.reference_id = *b"LOCL";
            }
            let server = NtpServer::start_with(addr, config).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied && port < 1024 {
                    anyhow::anyhow!(
                        "{addr}: {e} (ports below 1024 need privileges; try --port 12300, or on macOS the default --bind 0.0.0.0)"
                    )
                } else {
                    anyhow::anyhow!("{addr}: {e}")
                }
            })?;
            eprintln!(
                "🕒 rlsl ntp — serving this machine's clock on {} (stratum {stratum}), Ctrl-C to stop",
                server.local_addr()
            );
            let mut reported = 0;
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let served = server.requests_served();
                if served != reported {
                    eprintln!("   {served} requests answered");
                    reported = served;
                }
            }
        }
        Some("query") | Some("client") => {
            let server = args
                .get(1)
                .filter(|s| !s.starts_with("--"))
                .cloned()
                .unwrap_or_else(|| "pool.ntp.org".to_string());
            let probes: usize = get_arg(args, "--probes", "4").parse()?;
            let timeout: f64 = get_arg(args, "--timeout", "5.0").parse()?;
            let json = has_flag(args, "--json");
            let s = ntp::sync(&server, probes, timeout)
                .map_err(|e| anyhow::anyhow!("{server}: {e}"))?;
            if json {
                println!(
                    r#"{{"server":"{}","offset":{},"lsl_offset":{},"delay":{},"stratum":{},"reference_id":"{}","root_distance":{},"leap":{}}}"#,
                    s.server,
                    s.offset,
                    s.lsl_offset,
                    s.delay,
                    s.stratum,
                    s.reference_id_string(),
                    s.root_distance,
                    s.leap
                );
            } else {
                eprintln!("🕒 {server} ({})", s.server);
                eprintln!(
                    "   stratum {}, reference {}",
                    s.stratum,
                    s.reference_id_string()
                );
                eprintln!(
                    "   offset      {:+.6} s  (server minus this machine's wall clock)",
                    s.offset
                );
                eprintln!("   delay       {:.6} s", s.delay);
                eprintln!("   root dist.  {:.6} s", s.root_distance);
                eprintln!(
                    "   lsl offset  {:+.6} s  (add to local_clock() for the server's Unix time)",
                    s.lsl_offset
                );
            }
            Ok(())
        }
        Some("track") => {
            let server = args
                .get(1)
                .filter(|s| !s.starts_with("--"))
                .cloned()
                .unwrap_or_else(|| "pool.ntp.org".to_string());
            let interval: f64 = get_arg(args, "--interval", "16").parse()?;
            let count: u64 = get_arg(args, "--count", "0").parse()?;
            let window: usize = get_arg(args, "--window", "64").parse()?;
            let probes: usize = get_arg(args, "--probes", "4").parse()?;
            let json = has_flag(args, "--json");
            let mut tracker = ntp::ClockTracker::new(window);
            if !json {
                eprintln!(
                    "🕒 tracking {server} every {interval} s (window {window}), Ctrl-C to stop"
                );
                eprintln!(
                    "   {:>4} {:>12} {:>10} │ {:>12} {:>10} {:>10} {:>12} {:>10}",
                    "n",
                    "offset ms",
                    "delay ms",
                    "mean ms",
                    "jitter ms",
                    "drift ppm",
                    "min..max ms",
                    "span s"
                );
            }
            let mut n = 0u64;
            loop {
                match ntp::sync(&server, probes, 5.0) {
                    Ok(sample) => {
                        tracker.push_ntp(&sample);
                        n += 1;
                        let st = tracker.stats().expect("one sample at least");
                        let drift = st.drift_ppm.map_or("-".to_string(), |d| format!("{d:+.3}"));
                        if json {
                            println!(
                                r#"{{"n":{n},"server":"{}","offset":{},"delay":{},"mean_offset":{},"jitter":{},"drift_ppm":{},"min_offset":{},"max_offset":{},"span":{}}}"#,
                                sample.server,
                                sample.offset,
                                sample.delay,
                                st.mean_offset,
                                st.jitter,
                                st.drift_ppm.map_or("null".to_string(), |d| d.to_string()),
                                st.min_offset,
                                st.max_offset,
                                st.span
                            );
                        } else {
                            eprintln!(
                                "   {n:>4} {:>+12.3} {:>10.3} │ {:>+12.3} {:>10.3} {:>10} {:>5.1}..{:<6.1} {:>10.0}",
                                sample.offset * 1e3,
                                sample.delay * 1e3,
                                st.mean_offset * 1e3,
                                st.jitter * 1e3,
                                drift,
                                st.min_offset * 1e3,
                                st.max_offset * 1e3,
                                st.span
                            );
                        }
                    }
                    Err(e) => eprintln!("   {server}: {e}"),
                }
                if count > 0 && n >= count {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_secs_f64(interval.max(1.0)));
            }
        }
        Some("mesh") => {
            let bind = get_arg(args, "--bind", "0.0.0.0");
            let port: u16 = get_arg(args, "--port", &ntp::DEFAULT_MESH_PORT.to_string()).parse()?;
            let interval: f64 = get_arg(args, "--interval", "10").parse()?;
            let json = has_flag(args, "--json");
            // --peer may be given several times, each also a comma-separated list
            let peers: Vec<String> = args
                .windows(2)
                .filter(|w| w[0] == "--peer" || w[0] == "--peers")
                .flat_map(|w| w[1].split(',').map(str::to_string).collect::<Vec<_>>())
                .collect();
            let addr: std::net::SocketAddr = if bind.contains(':') {
                format!("[{bind}]:{port}").parse()?
            } else {
                format!("{bind}:{port}").parse()?
            };
            let mesh = ntp::ClockMesh::start(ntp::ClockMeshConfig {
                node_id: get_arg(args, "--id", ""),
                bind: addr,
                peers,
                interval: Duration::from_secs_f64(interval.max(1.0)),
                ..ntp::ClockMeshConfig::default()
            })
            .map_err(|e| anyhow::anyhow!("{addr}: {e}"))?;
            eprintln!(
                "🕸  rlsl ntp mesh — node {} on {} (NTP/UDP + tables/TCP), measuring every {interval} s",
                mesh.node_id(),
                mesh.local_addr()
            );
            loop {
                std::thread::sleep(Duration::from_secs_f64(interval.max(1.0)));
                if json {
                    println!("{}", mesh.matrix_json());
                    continue;
                }
                for p in mesh.peers() {
                    if let Some(e) = p.last_error {
                        eprintln!("   ⚠ {}: {e}", p.addr);
                    }
                }
                eprintln!(
                    "   {:<22} {:<22} {:<5} {:>6} {:>12} {:>10} {:>10} {:>10} {:>8}",
                    "from",
                    "to",
                    "clock",
                    "n",
                    "offset ms",
                    "jitter ms",
                    "drift ppm",
                    "delay ms",
                    "age s"
                );
                let now = rlsl::clock::system_clock();
                for e in mesh.matrix() {
                    let s = e.stats;
                    eprintln!(
                        "   {:<22} {:<22} {:<5} {:>6} {:>+12.3} {:>10.3} {:>10} {:>10.3} {:>8.0}{}",
                        e.from,
                        e.to,
                        e.clock.as_str(),
                        e.seq,
                        s.mean_offset * 1e3,
                        s.jitter * 1e3,
                        s.drift_ppm.map_or("-".to_string(), |d| format!("{d:+.3}")),
                        s.mean_delay * 1e3,
                        (now - e.updated).max(0.0),
                        if e.direct { "" } else { "  (learned)" }
                    );
                }
                eprintln!();
            }
        }
        _ => cmd_help(Some("ntp")),
    }
}

// ── version ──────────────────────────────────────────────────────────

fn cmd_version() -> Result<()> {
    println!("lsl {} (lsl-rs)", env!("CARGO_PKG_VERSION"));
    println!("Protocol version: {}", rlsl::types::LSL_PROTOCOL_VERSION);
    println!("Library version:  {}", rlsl::types::LSL_LIBRARY_VERSION);
    Ok(())
}

// ── help ─────────────────────────────────────────────────────────────

fn cmd_help(sub: Option<&str>) -> Result<()> {
    match sub {
        Some("list") | Some("ls") => {
            eprintln!("rlsl list — Discover LSL streams on the network");
            eprintln!();
            eprintln!("Usage: rlsl list [OPTIONS]");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  --timeout SECS    Discovery timeout (default: 2.0)");
            eprintln!("  --query QUERY     XPath-like filter (e.g. \"type='EEG'\")");
            eprintln!("  --json            Output as JSON");
            eprintln!("  -w, --watch       Continuously refresh");
        }
        Some("gen") | Some("generate") => {
            eprintln!("rlsl gen — Generate synthetic LSL streams");
            eprintln!();
            eprintln!("Usage: rlsl gen [OPTIONS]");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  --name NAME        Stream name (default: LslGen)");
            eprintln!("  --type TYPE        Content type (default: EEG)");
            eprintln!("  --channels N       Channel count (default: 8)");
            eprintln!("  --srate HZ         Sample rate (default: 250)");
            eprintln!("  --waveform WAVE    sine|square|noise|chirp|sawtooth|counter");
            eprintln!("  --freq HZ          Waveform frequency (default: 10.0)");
            eprintln!("  --amplitude A      Amplitude (default: 100.0)");
            eprintln!(
                "  --format F         float32|double64|int64|int32|int16|int8 (default: float32)"
            );
            eprintln!("  --source-id ID     Source id (default: rlsl-cli-gen)");
            #[cfg(feature = "ntp")]
            {
                eprintln!(
                    "  --clock-peers A,B  Run a clock mesh node measuring these machines and"
                );
                eprintln!("                     pass its measurements on with the stream");
                eprintln!("  --clock-transport  in-band|companion|both (default: both)");
                eprintln!("  --clock-port P     Mesh port (default: 12300)");
                eprintln!("  --clock-id NAME    Mesh node name (default: hostname:port)");
                eprintln!("  --clock-interval S Seconds between measurements (default: 10)");
            }
        }
        Some("bench") | Some("benchmark") => {
            eprintln!("rlsl bench — Measure push→pull latency and throughput");
            eprintln!();
            eprintln!("Usage: rlsl bench [OPTIONS]");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  --channels N       Channel count (default: 8)");
            eprintln!("  --srate HZ         Sample rate (default: 1000)");
            eprintln!("  --duration SECS    Test duration (default: 5)");
            eprintln!("  --format FMT       Channel format (default: float32)");
        }
        #[cfg(feature = "ntp")]
        Some("ntp") => {
            eprintln!("rlsl ntp — NTP server and client");
            eprintln!();
            eprintln!("Usage: rlsl ntp serve [OPTIONS]");
            eprintln!("       rlsl ntp query [SERVER] [OPTIONS]");
            eprintln!();
            eprintln!("serve options:");
            eprintln!("  --bind ADDR        Address to listen on (default: 0.0.0.0)");
            eprintln!("  --port P           UDP port (default: 123, needs privileges)");
            eprintln!("  --stratum N        Stratum announced, 1-15 (default: 10)");
            eprintln!();
            eprintln!("query options:");
            eprintln!("  SERVER             host[:port] (default: pool.ntp.org)");
            eprintln!(
                "  --probes N         Requests sent; the fastest answer is kept (default: 4)"
            );
            eprintln!("  --timeout SECS     Time allowed for all probes (default: 5.0)");
            eprintln!("  --json             Output as JSON");
            eprintln!();
            eprintln!(
                "Usage: rlsl ntp track [SERVER] [OPTIONS]   follow one server's offset over time"
            );
            eprintln!("  --interval SECS    Time between measurements (default: 16)");
            eprintln!("  --count N          Stop after N measurements (default: 0, never)");
            eprintln!("  --window N         Measurements the statistics cover (default: 64)");
            eprintln!("  --probes N         Requests per measurement (default: 4)");
            eprintln!("  --json             One JSON object per measurement");
            eprintln!();
            eprintln!("Usage: rlsl ntp mesh [OPTIONS]   measure peers and share the offset matrix");
            eprintln!("  --id NAME          This node's name (default: hostname:port)");
            eprintln!("  --bind ADDR        Address to listen on (default: 0.0.0.0)");
            eprintln!("  --port P           UDP (NTP) and TCP (tables) port (default: 12300)");
            eprintln!("  --peer HOST[:P]    Another node; repeat or comma-separate for more");
            eprintln!("  --interval SECS    Time between measurement rounds (default: 10)");
            eprintln!("  --json             Print the matrix as JSON each round");
        }
        _ => {
            eprintln!("lsl — Lab Streaming Layer CLI (lsl-rs)");
            eprintln!();
            eprintln!("Usage: lsl <COMMAND> [OPTIONS]");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  list      Discover LSL streams on the network");
            eprintln!("  gen       Generate synthetic LSL streams");
            eprintln!("  bench     Measure push→pull latency and throughput");
            #[cfg(feature = "ntp")]
            eprintln!("  ntp       NTP server and client");
            eprintln!("  version   Show version information");
            eprintln!("  help      Show help for a command");
            eprintln!();
            eprintln!("Run `rrlsl help <command>` for detailed usage.");
        }
    }
    Ok(())
}
