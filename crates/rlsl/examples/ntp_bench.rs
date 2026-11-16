//! Benchmarks of the `ntp` feature (needs it):
//!
//! ```text
//! cargo run --release -p rlsl --features ntp --example ntp_bench               # this machine
//! cargo run --release -p rlsl --features ntp --example ntp_bench host:port...  # + remote nodes
//! ```
//!
//! On this machine: record encoding and fitting, NTP and LSL-probe round trips on loopback,
//! and what in-band clock records cost a data stream (push → pull latency, throughput). With
//! addresses of NTP servers / clock mesh nodes: round trip and offset jitter against each.

use rlsl::clock::local_clock;
use rlsl::ntp::{
    self, ClockFit, ClockKind, ClockMesh, ClockMeshConfig, ClockRecord, ClockTracker,
    ClockTransport, NtpServer, OffsetSample,
};
use rlsl::prelude::*;
use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn summary(label: &str, mut v: Vec<f64>, unit: f64, unit_name: &str) {
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    println!(
        "  {label:<34} n={:<6} mean {:>9.3} p50 {:>9.3} p99 {:>9.3} max {:>9.3} {unit_name}",
        v.len(),
        mean * unit,
        pct(&v, 0.5) * unit,
        pct(&v, 0.99) * unit,
        pct(&v, 1.0) * unit
    );
}

fn sd(v: &[f64]) -> f64 {
    let m = v.iter().sum::<f64>() / v.len() as f64;
    (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (v.len() - 1).max(1) as f64).sqrt()
}

/// The spread of the offsets of the fastest fifth of the probes: the precision once a filter
/// keeps the least-delayed measurements.
fn fastest_sd(delays: &[f64], offsets: &[f64]) -> f64 {
    let mut pairs: Vec<(f64, f64)> = delays
        .iter()
        .copied()
        .zip(offsets.iter().copied())
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let best: Vec<f64> = pairs[..(pairs.len() / 5).max(2)]
        .iter()
        .map(|p| p.1)
        .collect();
    sd(&best)
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let r = f();
    (r, t.elapsed().as_secs_f64())
}

fn micro() {
    println!("micro-benchmarks");
    let rec = ClockRecord {
        from: "msi".into(),
        to: "amd".into(),
        clock: ClockKind::Mono,
        at: 2_390_189.068_787,
        offset: -1_429_709.011_316,
        delay: 0.001_61,
        at_unix: 1_794_797_633.841_073,
    };
    let n = 200_000;
    let (lines, t) = timed(|| (0..n).map(|_| rec.to_line()).collect::<Vec<_>>());
    println!(
        "  record -> line                     {:>9.0} ns/record",
        t / n as f64 * 1e9
    );
    let (parsed, t) = timed(|| {
        lines
            .iter()
            .filter_map(|l| ClockRecord::from_line(l))
            .count()
    });
    assert_eq!(parsed, n);
    println!(
        "  line -> record                     {:>9.0} ns/record",
        t / n as f64 * 1e9
    );
    let samples: Vec<OffsetSample> = (0..10_000)
        .map(|i| OffsetSample {
            at: i as f64,
            offset: 0.01 + 3e-6 * i as f64 + ((i * 7919) % 13) as f64 * 1e-5,
            delay: 0.001 + ((i * 31) % 7) as f64 * 1e-4,
        })
        .collect();
    let reps = 200;
    let (_, t) = timed(|| {
        for _ in 0..reps {
            std::hint::black_box(ClockFit::fit(&samples));
        }
    });
    println!(
        "  ClockFit over 10 000 samples       {:>9.1} µs/fit",
        t / reps as f64 * 1e6
    );
    let mut tracker = ClockTracker::new(360);
    let (_, t) = timed(|| {
        for s in samples.iter().cycle().take(100_000) {
            tracker.push(*s);
        }
    });
    println!(
        "  ClockTracker::push                 {:>9.0} ns/sample",
        t / 100_000.0 * 1e9
    );
    let (_, t) = timed(|| {
        for _ in 0..1000 {
            std::hint::black_box(tracker.stats());
        }
    });
    println!(
        "  ClockTracker::stats (360 samples)  {:>9.1} µs/call",
        t / 1000.0 * 1e6
    );
}

fn round_trips() {
    println!("\nround trips on loopback");
    let server = NtpServer::start("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = server.local_addr();
    let n = 2000;
    let mut wall = Vec::new();
    let mut client = Vec::new();
    let t0 = Instant::now();
    for _ in 0..n {
        let t = Instant::now();
        let s = ntp::query(&addr.to_string(), 1.0).unwrap();
        client.push(t.elapsed().as_secs_f64());
        wall.push(s.delay);
    }
    let rate = n as f64 / t0.elapsed().as_secs_f64();
    summary("NTP delay (server-measured RTT)", wall, 1e6, "µs");
    summary("ntp::query call (incl. socket)", client, 1e6, "µs");
    println!("  ntp::query rate                    {rate:>9.0} queries/s (sequential)");
    let mut mono = Vec::new();
    let mut offs = Vec::new();
    for _ in 0..n {
        let (o, d, _) = ntp::mono_query(addr, 1, 1.0).unwrap();
        mono.push(d);
        offs.push(o);
    }
    summary("LSL probe delay", mono, 1e6, "µs");
    println!(
        "  LSL probe offset (true 0)          mean {:+.3} µs, sd {:.3} µs",
        offs.iter().sum::<f64>() / offs.len() as f64 * 1e6,
        sd(&offs) * 1e6
    );
    println!(
        "  requests served                    {}",
        server.requests_served()
    );
}

/// Push 8-channel float32 at `rate` Hz for `secs` and pull it; return latencies (s) and the
/// samples received.
fn stream_run(
    name: &str,
    transport: Option<(&ClockMesh, ClockTransport)>,
    records: bool,
) -> (Vec<f64>, usize, usize) {
    let (rate, secs) = (1000.0, 5.0);
    let info = StreamInfo::new(name, "EEG", 8, rate, ChannelFormat::Float32, name);
    let outlet = StreamOutlet::new(&info, 0, 360);
    if let Some((mesh, t)) = transport {
        outlet.attach_clock_mesh(mesh, t);
    }
    let inlet = StreamInlet::new(outlet.info(), 360, 0, false);
    inlet.set_clock_records(records);
    inlet.open_stream(5.0).unwrap();
    let n = (rate * secs) as usize;
    let done = Arc::new(AtomicBool::new(false));
    let pusher = {
        let done = done.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            for i in 0..n {
                let due = start + Duration::from_secs_f64(i as f64 / rate);
                while Instant::now() < due {
                    std::thread::sleep(Duration::from_micros(200));
                }
                outlet.push_sample_f(&[i as f32; 8], local_clock(), true);
            }
            std::thread::sleep(Duration::from_millis(500));
            done.store(true, Ordering::Relaxed);
        })
    };
    let mut lat = Vec::with_capacity(n);
    let mut buf = [0.0f32; 8];
    let mut clock_records = 0;
    while !done.load(Ordering::Relaxed) || inlet.samples_available() > 0 {
        if let Ok(ts) = inlet.pull_sample_f(&mut buf, 0.2) {
            if ts > 0.0 {
                lat.push(local_clock() - ts);
            }
        }
        clock_records += inlet.pull_clock_records().len();
    }
    pusher.join().unwrap();
    (lat, n, clock_records)
}

fn data_path() {
    println!("\nin-band clock records on a data stream (8 ch float32, 1 kHz, 5 s, loopback)");
    let b = ClockMesh::start(ClockMeshConfig {
        node_id: "peer".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        ..ClockMeshConfig::default()
    })
    .unwrap();
    // a node measuring its peer ten times a second: far more records than a real mesh makes
    let a = ClockMesh::start(ClockMeshConfig {
        node_id: "bench".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        peers: vec![b.local_addr().to_string()],
        interval: Duration::from_millis(100),
        probes: 1,
        ..ClockMeshConfig::default()
    })
    .unwrap();
    for (label, transport, ask) in [
        ("no mesh", None, false),
        (
            "mesh, records not asked for",
            Some((&a, ClockTransport::IN_BAND)),
            false,
        ),
        (
            "mesh, in-band records",
            Some((&a, ClockTransport::IN_BAND)),
            true,
        ),
        (
            "mesh, in-band + companion",
            Some((&a, ClockTransport::BOTH)),
            true,
        ),
    ] {
        let name = format!("NtpBench {label}");
        let (lat, pushed, records) = stream_run(&name, transport, ask);
        let got = lat.len();
        summary(label, lat, 1e3, "ms");
        println!(
            "  {:<34} received {got}/{pushed} samples, {records} clock records",
            ""
        );
    }
}

fn remote(targets: &[String]) {
    for t in targets {
        let Some(addr) = t.to_socket_addrs().ok().and_then(|mut a| a.next()) else {
            println!("\n{t}: cannot resolve");
            continue;
        };
        println!("\nagainst {t} (200 probes each, 20 ms apart)");
        let (mut d, mut o) = (Vec::new(), Vec::new());
        let (mut md, mut mo) = (Vec::new(), Vec::new());
        for _ in 0..200 {
            if let Ok(s) = ntp::query(t, 1.0) {
                d.push(s.delay);
                o.push(s.offset);
            }
            if let Ok((off, del, _)) = ntp::mono_query(addr, 1, 1.0) {
                md.push(del);
                mo.push(off);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !d.is_empty() {
            summary("NTP delay", d.clone(), 1e3, "ms");
            println!(
                "  NTP offset (wall)                  mean {:+.3} ms, sd {:.3} ms (all), sd {:.3} ms (fastest 20%)",
                o.iter().sum::<f64>() / o.len() as f64 * 1e3,
                sd(&o) * 1e3,
                fastest_sd(&d, &o) * 1e3
            );
        }
        if !md.is_empty() {
            summary("LSL probe delay", md.clone(), 1e3, "ms");
            println!(
                "  LSL probe offset (mono) jitter     sd {:.3} ms (all), sd {:.3} ms (fastest 20%)",
                sd(&mo) * 1e3,
                fastest_sd(&md, &mo) * 1e3
            );
            let samples: Vec<OffsetSample> = md
                .iter()
                .zip(&mo)
                .enumerate()
                .map(|(i, (&delay, &offset))| OffsetSample {
                    at: i as f64 * 0.02,
                    offset,
                    delay,
                })
                .collect();
            if let Some(fit) = ClockFit::fit(&samples) {
                println!(
                    "  delay-weighted fit                 residual {:.3} ms over {} probes",
                    fit.residual * 1e3,
                    fit.count
                );
            }
        }
    }
}

fn main() {
    let targets: Vec<String> = std::env::args().skip(1).collect();
    println!(
        "rlsl ntp bench on {}",
        hostname::get().map_or("?".into(), |h| h.to_string_lossy().into_owned())
    );
    if std::env::var_os("NTP_BENCH_REMOTE_ONLY").is_none() {
        micro();
        round_trips();
        data_path();
    }
    remote(&targets);
}
