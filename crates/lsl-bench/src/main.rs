//! `lsl-bench` — LSL throughput & latency benchmark.
//!
//! Measures end-to-end push→pull latency and throughput.
//!
//! Usage: lsl-bench [--channels N] [--srate HZ] [--duration SECS] [--format FMT]

use lsl_core::clock::local_clock;
use lsl_core::prelude::*;
use lsl_core::resolver;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get = |key: &str, default: &str| -> String {
        args.iter()
            .position(|a| a == key)
            .and_then(|i| args.get(i + 1))
            .map(|s| s.clone())
            .unwrap_or_else(|| default.to_string())
    };

    let nch: u32 = get("--channels", "8").parse()?;
    let srate: f64 = get("--srate", "1000").parse()?;
    let duration_secs: f64 = get("--duration", "5").parse()?;
    let format_str = get("--format", "float32");

    let fmt = ChannelFormat::from_name(&format_str);
    let n_samples = (srate * duration_secs) as u64;

    eprintln!(
        "⚡ lsl-bench: {}ch × {}Hz × {:.1}s = {} samples ({})",
        nch, srate, duration_secs, n_samples, format_str
    );
    eprintln!();

    // ── Create outlet ──
    let info = StreamInfo::new("LslBench", "Benchmark", nch, srate, fmt, "bench_src");
    let outlet = StreamOutlet::new(&info, 0, 360);
    std::thread::sleep(Duration::from_secs(1));

    // ── Resolve and create inlet ──
    eprint!("  Resolving...");
    let streams = resolver::resolve_all(2.0);
    let bench_stream = streams
        .iter()
        .find(|s| s.name() == "LslBench")
        .expect("Could not find LslBench stream");
    let inlet = StreamInlet::new(bench_stream, 360, 0, false);
    inlet.open_stream(5.0).expect("Failed to open stream");
    eprintln!(" connected.");
    eprintln!();

    // ── Benchmark: push + pull ──
    let sample_data = vec![0.0f32; nch as usize];
    let mut recv_buf = vec![0.0f32; nch as usize];
    let mut latencies: Vec<f64> = Vec::with_capacity(n_samples as usize);

    // Warmup
    for _ in 0..100 {
        outlet.push_sample_f(&sample_data, 0.0, true);
        let _ = inlet.pull_sample_f(&mut recv_buf, 1.0);
    }

    let start = Instant::now();
    let mut pushed = 0u64;
    let mut pulled = 0u64;

    // Push thread
    let outlet_handle = {
        let sample_data = sample_data.clone();
        let n = n_samples;
        std::thread::spawn(move || {
            let interval = Duration::from_secs_f64(1.0 / srate);
            for _ in 0..n {
                let ts = local_clock();
                outlet.push_sample_f(&sample_data, ts, true);
                std::thread::sleep(interval);
            }
        })
    };

    // Pull loop
    let pull_start = Instant::now();
    let deadline = pull_start + Duration::from_secs_f64(duration_secs + 2.0);

    while Instant::now() < deadline && pulled < n_samples {
        match inlet.pull_sample_f(&mut recv_buf, 0.5) {
            Ok(ts) if ts > 0.0 => {
                let now = local_clock();
                latencies.push(now - ts);
                pulled += 1;
            }
            _ => {}
        }
    }
    let elapsed = start.elapsed();
    outlet_handle.join().unwrap();

    // ── Report ──
    pushed = n_samples;
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let mean_latency = latencies.iter().sum::<f64>() / latencies.len().max(1) as f64;
    let p50 = latencies.get(latencies.len() / 2).copied().unwrap_or(0.0);
    let p95 = latencies
        .get(latencies.len() * 95 / 100)
        .copied()
        .unwrap_or(0.0);
    let p99 = latencies
        .get(latencies.len() * 99 / 100)
        .copied()
        .unwrap_or(0.0);
    let min_lat = latencies.first().copied().unwrap_or(0.0);
    let max_lat = latencies.last().copied().unwrap_or(0.0);

    let throughput = pulled as f64 / elapsed.as_secs_f64();
    let data_rate_mb = throughput * nch as f64 * fmt.channel_bytes() as f64 / 1_000_000.0;
    let loss = if pushed > 0 {
        1.0 - pulled as f64 / pushed as f64
    } else {
        0.0
    };

    eprintln!("  ╔══════════════════════════════════════╗");
    eprintln!("  ║         lsl-bench Results            ║");
    eprintln!("  ╠══════════════════════════════════════╣");
    eprintln!("  ║ Pushed:    {:>10} samples         ║", pushed);
    eprintln!("  ║ Pulled:    {:>10} samples         ║", pulled);
    eprintln!("  ║ Loss:      {:>9.2}%               ║", loss * 100.0);
    eprintln!(
        "  ║ Duration:  {:>9.2}s               ║",
        elapsed.as_secs_f64()
    );
    eprintln!("  ║ Throughput:{:>9.0} samples/s      ║", throughput);
    eprintln!("  ║ Data rate: {:>9.2} MB/s            ║", data_rate_mb);
    eprintln!("  ╠══════════════════════════════════════╣");
    eprintln!("  ║ Latency (push→pull):                ║");
    eprintln!("  ║   min:  {:>9.3} ms                ║", min_lat * 1000.0);
    eprintln!(
        "  ║   mean: {:>9.3} ms                ║",
        mean_latency * 1000.0
    );
    eprintln!("  ║   p50:  {:>9.3} ms                ║", p50 * 1000.0);
    eprintln!("  ║   p95:  {:>9.3} ms                ║", p95 * 1000.0);
    eprintln!("  ║   p99:  {:>9.3} ms                ║", p99 * 1000.0);
    eprintln!("  ║   max:  {:>9.3} ms                ║", max_lat * 1000.0);
    eprintln!("  ╚══════════════════════════════════════╝");

    Ok(())
}
