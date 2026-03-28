//! `lsl` — Unified CLI tool for Lab Streaming Layer.
//!
//! ```text
//! Usage:
//!   lsl list    [--timeout SECS] [--query QUERY] [--json]
//!   lsl gen     [--name N] [--type T] [--channels N] [--srate HZ] [--waveform W]
//!   lsl record  [--output FILE] [--format xdf|parquet] [--query QUERY]
//!   lsl convert <input> [--to parquet|csv|info] [-o output]
//!   lsl bench   [--channels N] [--srate HZ] [--duration SECS]
//!   lsl info    <file>
//!   lsl version
//!   lsl help    [command]
//! ```

use anyhow::Result;
use lsl_core::prelude::*;
use lsl_core::{clock::local_clock, resolver};
use std::time::Duration;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");
    let sub_args: Vec<String> = args.iter().skip(2).cloned().collect();

    match cmd {
        "list" | "ls" => cmd_list(&sub_args),
        "gen" | "generate" => cmd_gen(&sub_args),
        "bench" | "benchmark" => cmd_bench(&sub_args),
        "version" | "--version" | "-V" => cmd_version(),
        "help" | "--help" | "-h" => cmd_help(sub_args.first().map(|s| s.as_str())),
        _ => {
            eprintln!("Unknown command: {}", cmd);
            eprintln!("Run `lsl help` for usage.");
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

    let info = StreamInfo::new(
        &name,
        &type_,
        nch,
        srate,
        ChannelFormat::Float32,
        "lsl-cli-gen",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);

    eprintln!("🎵 lsl gen — streaming:");
    eprintln!("   name={name}, type={type_}, {nch}ch, {srate}Hz");
    eprintln!("   waveform={waveform}, freq={freq}Hz, amplitude={amplitude}");
    eprintln!("   TCP port {}, Ctrl-C to stop", info.v4data_port());

    let interval = Duration::from_secs_f64(1.0 / srate);
    let mut sample_idx: u64 = 0;
    let mut sample = vec![0.0f32; nch as usize];

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
            *s = value as f32;
        }

        outlet.push_sample_f(&sample, 0.0, true);
        sample_idx += 1;
        std::thread::sleep(interval);
    }
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
        "⚡ lsl bench: {}ch × {}Hz × {:.1}s = {} samples ({})",
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
    eprintln!("  ║       lsl bench — Results            ║");
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

// ── version ──────────────────────────────────────────────────────────

fn cmd_version() -> Result<()> {
    println!("lsl {} (lsl-rs)", env!("CARGO_PKG_VERSION"));
    println!(
        "Protocol version: {}",
        lsl_core::types::LSL_PROTOCOL_VERSION
    );
    println!("Library version:  {}", lsl_core::types::LSL_LIBRARY_VERSION);
    Ok(())
}

// ── help ─────────────────────────────────────────────────────────────

fn cmd_help(sub: Option<&str>) -> Result<()> {
    match sub {
        Some("list") | Some("ls") => {
            eprintln!("lsl list — Discover LSL streams on the network");
            eprintln!();
            eprintln!("Usage: lsl list [OPTIONS]");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  --timeout SECS    Discovery timeout (default: 2.0)");
            eprintln!("  --query QUERY     XPath-like filter (e.g. \"type='EEG'\")");
            eprintln!("  --json            Output as JSON");
            eprintln!("  -w, --watch       Continuously refresh");
        }
        Some("gen") | Some("generate") => {
            eprintln!("lsl gen — Generate synthetic LSL streams");
            eprintln!();
            eprintln!("Usage: lsl gen [OPTIONS]");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  --name NAME        Stream name (default: LslGen)");
            eprintln!("  --type TYPE        Content type (default: EEG)");
            eprintln!("  --channels N       Channel count (default: 8)");
            eprintln!("  --srate HZ         Sample rate (default: 250)");
            eprintln!("  --waveform WAVE    sine|square|noise|chirp|sawtooth|counter");
            eprintln!("  --freq HZ          Waveform frequency (default: 10.0)");
            eprintln!("  --amplitude A      Amplitude (default: 100.0)");
        }
        Some("bench") | Some("benchmark") => {
            eprintln!("lsl bench — Measure push→pull latency and throughput");
            eprintln!();
            eprintln!("Usage: lsl bench [OPTIONS]");
            eprintln!();
            eprintln!("Options:");
            eprintln!("  --channels N       Channel count (default: 8)");
            eprintln!("  --srate HZ         Sample rate (default: 1000)");
            eprintln!("  --duration SECS    Test duration (default: 5)");
            eprintln!("  --format FMT       Channel format (default: float32)");
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
            eprintln!("  version   Show version information");
            eprintln!("  help      Show help for a command");
            eprintln!();
            eprintln!("Run `lsl help <command>` for detailed usage.");
        }
    }
    Ok(())
}
