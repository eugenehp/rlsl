//! In-process iroh tunnel benchmark.
//!
//! Measures end-to-end latency and bandwidth of the iroh QUIC path.
//! Creates two iroh endpoints in the same process, wires them together,
//! and streams LSL samples through the full pipeline.

use crate::compress::{self, Compression};
use crate::protocol;
use anyhow::Result;
use iroh::endpoint::presets::N0;
use iroh::endpoint::{
    AckFrequencyConfig, Connection, Endpoint, QuicTransportConfig, RecvStream, VarInt,
};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use rlsl::clock::local_clock;
use rlsl::prelude::*;
use rlsl::sample::Sample;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Barrier};

// ── Configuration ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BenchConfig {
    pub channels: u32,
    pub sample_rate: f64,
    pub duration_secs: f64,
    pub format: ChannelFormat,
    pub use_datagrams: bool,
    pub compression: Compression,
}

impl Default for BenchConfig {
    fn default() -> Self {
        BenchConfig {
            channels: 8,
            sample_rate: 1000.0,
            duration_secs: 5.0,
            format: ChannelFormat::Float32,
            use_datagrams: false,
            compression: Compression::None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BenchResults {
    pub pushed: u64,
    pub received: u64,
    pub elapsed_secs: f64,
    pub throughput_samples_sec: f64,
    pub data_rate_mb_sec: f64,
    pub loss_pct: f64,
    pub latency_min_us: f64,
    pub latency_mean_us: f64,
    pub latency_p50_us: f64,
    pub latency_p95_us: f64,
    pub latency_p99_us: f64,
    pub latency_max_us: f64,
    pub rtt_us: f64,
}

impl std::fmt::Display for BenchResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "╔═══════════════════════════════════════════╗")?;
        writeln!(f, "║       rlsl-iroh Benchmark Results         ║")?;
        writeln!(f, "╠═══════════════════════════════════════════╣")?;
        writeln!(f, "║ Pushed:     {:>10} samples            ║", self.pushed)?;
        writeln!(
            f,
            "║ Received:   {:>10} samples            ║",
            self.received
        )?;
        writeln!(
            f,
            "║ Loss:       {:>9.2}%                  ║",
            self.loss_pct
        )?;
        writeln!(
            f,
            "║ Duration:   {:>9.2}s                  ║",
            self.elapsed_secs
        )?;
        writeln!(
            f,
            "║ Throughput: {:>9.0} samples/s           ║",
            self.throughput_samples_sec
        )?;
        writeln!(
            f,
            "║ Data rate:  {:>9.2} MB/s               ║",
            self.data_rate_mb_sec
        )?;
        writeln!(f, "║ QUIC RTT:   {:>9.0} µs                 ║", self.rtt_us)?;
        writeln!(f, "╠═══════════════════════════════════════════╣")?;
        writeln!(f, "║ Latency (source push → sink receive):    ║")?;
        writeln!(
            f,
            "║   min:  {:>9.1} µs                     ║",
            self.latency_min_us
        )?;
        writeln!(
            f,
            "║   mean: {:>9.1} µs                     ║",
            self.latency_mean_us
        )?;
        writeln!(
            f,
            "║   p50:  {:>9.1} µs                     ║",
            self.latency_p50_us
        )?;
        writeln!(
            f,
            "║   p95:  {:>9.1} µs                     ║",
            self.latency_p95_us
        )?;
        writeln!(
            f,
            "║   p99:  {:>9.1} µs                     ║",
            self.latency_p99_us
        )?;
        writeln!(
            f,
            "║   max:  {:>9.1} µs                     ║",
            self.latency_max_us
        )?;
        writeln!(f, "╚═══════════════════════════════════════════╝")
    }
}

// ── Low-latency QUIC transport config ────────────────────────────────

/// Build a QuicTransportConfig tuned for minimal latency:
/// - ACK every packet immediately (ack_eliciting_threshold=1)
/// - Zero max ACK delay — don't batch acknowledgements
/// - Small initial RTT (1ms) so congestion controller ramps up fast
/// - Large windows to avoid flow-control stalls
pub fn low_latency_transport() -> QuicTransportConfig {
    let mut ack_freq = AckFrequencyConfig::default();
    ack_freq
        .ack_eliciting_threshold(VarInt::from_u32(1))
        .max_ack_delay(Some(Duration::ZERO));

    QuicTransportConfig::builder()
        .initial_rtt(Duration::from_millis(1))
        .ack_frequency_config(Some(ack_freq))
        .send_window(2 * 1024 * 1024)
        .receive_window(VarInt::from_u32(2 * 1024 * 1024))
        .stream_receive_window(VarInt::from_u32(1024 * 1024))
        .build()
}

// ── Benchmark sink handler ───────────────────────────────────────────

#[derive(Debug, Clone)]
struct BenchSinkHandler {
    tx: mpsc::UnboundedSender<(Sample, f64)>,
}

impl ProtocolHandler for BenchSinkHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let tx = self.tx.clone();

        let recv = match connection.accept_uni().await {
            Ok(r) => r,
            Err(e) => {
                log::debug!("accept_uni error: {}", e);
                return Ok(());
            }
        };

        if let Err(e) = receive_loop(recv, tx).await {
            log::debug!("Bench receive loop ended: {}", e);
        }

        connection.closed().await;
        Ok(())
    }
}

async fn receive_loop(
    mut recv: RecvStream,
    tx: mpsc::UnboundedSender<(Sample, f64)>,
) -> Result<()> {
    // Read header (12 bytes)
    let mut header_prefix = [0u8; 12];
    recv.read_exact(&mut header_prefix).await?;
    anyhow::ensure!(&header_prefix[..4] == protocol::MAGIC, "bad magic");
    let compression = Compression::from_u8(header_prefix[4]);
    let xml_len = u32::from_le_bytes(header_prefix[8..12].try_into()?) as usize;
    let mut xml_buf = vec![0u8; xml_len];
    recv.read_exact(&mut xml_buf).await?;
    let xml = std::str::from_utf8(&xml_buf)?;
    let info =
        StreamInfo::from_shortinfo_message(xml).ok_or_else(|| anyhow::anyhow!("bad header"))?;

    let fmt = info.channel_format();
    let nch = info.channel_count();
    let sample_data_bytes = fmt.channel_bytes() * nch as usize;

    // Large read buffer: grab many samples per syscall
    let mut read_buf = vec![0u8; 128 * 1024];
    let mut leftover: Vec<u8> = Vec::new();
    let mut decoder = compress::ChunkDecoder::new(compression, fmt.channel_bytes());

    loop {
        let n = recv.read(&mut read_buf).await?;
        match n {
            Some(0) | None => break,
            Some(n) => {
                // Stamp arrival ASAP — before any parsing
                let arrival = local_clock();

                let data = if leftover.is_empty() {
                    &read_buf[..n]
                } else {
                    leftover.extend_from_slice(&read_buf[..n]);
                    leftover.as_slice()
                };

                let consumed = if compression.is_compressed() {
                    let mut offset = 0;
                    while offset < data.len() {
                        match decoder.decompress(&data[offset..]) {
                            Some((decompressed, chunk_consumed)) => {
                                offset += chunk_consumed;
                                parse_samples(
                                    &decompressed,
                                    fmt,
                                    nch,
                                    sample_data_bytes,
                                    arrival,
                                    &tx,
                                );
                            }
                            None => break,
                        }
                    }
                    offset
                } else {
                    parse_samples(data, fmt, nch, sample_data_bytes, arrival, &tx)
                };

                if consumed < data.len() {
                    leftover = data[consumed..].to_vec();
                } else {
                    leftover.clear();
                }
            }
        }
    }
    Ok(())
}

/// Parse raw (uncompressed) sample bytes, send to channel. Returns bytes consumed.
fn parse_samples(
    data: &[u8],
    fmt: ChannelFormat,
    nch: u32,
    sample_data_bytes: usize,
    arrival: f64,
    tx: &mpsc::UnboundedSender<(Sample, f64)>,
) -> usize {
    let mut cursor = Cursor::new(data);
    let mut last_consumed = 0usize;
    loop {
        let pos = cursor.position() as usize;
        if data.len() - pos < 1 + sample_data_bytes {
            break;
        }
        let before = pos;
        match Sample::deserialize_110(&mut cursor, fmt, nch) {
            Ok(sample) => {
                last_consumed = cursor.position() as usize;
                let _ = tx.send((sample, arrival));
            }
            Err(_) => {
                cursor.set_position(before as u64);
                last_consumed = before;
                break;
            }
        }
    }
    last_consumed
}

// ── Public benchmark runner ──────────────────────────────────────────

pub async fn run_bench(config: BenchConfig) -> Result<BenchResults> {
    let n_samples = (config.sample_rate * config.duration_secs) as u64;
    let fmt = config.format;
    let nch = config.channels;

    eprintln!(
        "⚡ rlsl-iroh bench: {}ch × {}Hz × {:.1}s = {} samples ({})",
        nch,
        config.sample_rate,
        config.duration_secs,
        n_samples,
        fmt.as_str()
    );

    let (sample_tx, mut sample_rx) = mpsc::unbounded_channel::<(Sample, f64)>();

    let transport = low_latency_transport();

    // ── Sink endpoint ────────────────────────────────────────────────
    let sink_ep: Endpoint = Endpoint::builder(N0)
        .alpns(vec![protocol::LSL_ALPN.to_vec()])
        .transport_config(transport.clone())
        .bind()
        .await?;

    let sink_handler = BenchSinkHandler { tx: sample_tx };
    let _router = Router::builder(sink_ep.clone())
        .accept(protocol::LSL_ALPN, sink_handler)
        .spawn();

    let sink_addr = sink_ep.addr();
    eprintln!("  Sink node: {}", sink_ep.id());

    // ── Source endpoint ──────────────────────────────────────────────
    let source_ep: Endpoint = Endpoint::builder(N0)
        .transport_config(transport)
        .bind()
        .await?;
    eprintln!("  Source node: {}", source_ep.id());

    eprintln!("  Waiting for endpoints to come online...");
    tokio::time::timeout(Duration::from_secs(15), sink_ep.online()).await?;
    tokio::time::timeout(Duration::from_secs(15), source_ep.online()).await?;
    eprintln!("  Both endpoints online.");

    let conn = source_ep.connect(sink_addr, protocol::LSL_ALPN).await?;
    let rtt = conn
        .rtt(iroh::endpoint::PathId::default())
        .map(|d| d.as_micros() as f64)
        .unwrap_or(0.0);
    eprintln!("  Connected (RTT: {:.0} µs)", rtt);

    let mut send_stream = conn.open_uni().await?;
    send_stream.set_priority(0)?;

    let info = StreamInfo::new(
        "IrohBench",
        "Benchmark",
        nch,
        config.sample_rate,
        fmt,
        "bench",
    );
    let header = protocol::encode_stream_header(&info, config.compression);
    send_stream.write_all(&header).await?;

    tokio::time::sleep(Duration::from_millis(50)).await;

    eprintln!("  Streaming {} samples...", n_samples);
    let barrier = Arc::new(Barrier::new(2));

    // ── Sender task ──────────────────────────────────────────────────
    // Direct async write — no intermediate channel hop.
    // yield_now() cost doesn't matter because timestamp is stamped AFTER yield.
    let send_barrier = barrier.clone();
    let sample_rate = config.sample_rate;
    let compression = config.compression;
    let sender = tokio::spawn(async move {
        send_barrier.wait().await;

        // Pre-allocate — reuse across iterations
        let mut write_buf = Vec::with_capacity(2048);
        let mut compressed_buf = Vec::with_capacity(2048);
        let mut encoder = compress::ChunkEncoder::new(compression, fmt.channel_bytes());
        let mut sample = Sample::new(fmt, nch, 0.0);
        let interval_ns = (1_000_000_000.0 / sample_rate) as u64;
        let mut next_send = Instant::now();

        for i in 0..n_samples {
            while Instant::now() < next_send {
                tokio::task::yield_now().await;
            }

            // Stamp time right before serialization+write
            sample.timestamp = local_clock();
            match &mut sample.data {
                rlsl::sample::SampleData::Float32(d) => {
                    for (c, v) in d.iter_mut().enumerate() {
                        *v = (i as f32) + (c as f32) * 0.001;
                    }
                }
                rlsl::sample::SampleData::Double64(d) => {
                    for (c, v) in d.iter_mut().enumerate() {
                        *v = (i as f64) + (c as f64) * 0.001;
                    }
                }
                rlsl::sample::SampleData::Int32(d) => {
                    for (c, v) in d.iter_mut().enumerate() {
                        *v = i as i32 + c as i32;
                    }
                }
                _ => {}
            }

            write_buf.clear();
            sample.serialize_110(&mut write_buf);

            let wire_data = if compression.is_compressed() {
                compressed_buf.clear();
                encoder.compress(&write_buf, &mut compressed_buf);
                compressed_buf.as_slice()
            } else {
                write_buf.as_slice()
            };

            if send_stream.write_all(wire_data).await.is_err() {
                return i;
            }

            next_send += Duration::from_nanos(interval_ns);
        }

        send_stream.finish().ok();
        n_samples
    });

    // ── Receiver task ────────────────────────────────────────────────
    let recv_barrier = barrier.clone();
    let duration_secs = config.duration_secs;
    let receiver = tokio::spawn(async move {
        recv_barrier.wait().await;

        let mut latencies = Vec::with_capacity(n_samples as usize);
        let deadline = Instant::now() + Duration::from_secs_f64(duration_secs + 10.0);

        // Hot loop — no per-sample timeout wrapper
        loop {
            if Instant::now() >= deadline || (latencies.len() as u64) >= n_samples {
                break;
            }
            match sample_rx.recv().await {
                Some((sample, arrival)) => {
                    latencies.push(arrival - sample.timestamp);
                }
                None => break,
            }
        }

        let elapsed = Instant::now() - (deadline - Duration::from_secs_f64(duration_secs + 10.0));
        (latencies, elapsed)
    });

    let pushed = sender.await?;
    let (mut latencies, elapsed) = receiver.await?;
    let received = latencies.len() as u64;

    eprintln!("  Done: pushed={}, received={}", pushed, received);

    // ── Statistics ───────────────────────────────────────────────────
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let to_us = 1_000_000.0;
    let mean = if latencies.is_empty() {
        0.0
    } else {
        latencies.iter().sum::<f64>() / latencies.len() as f64
    };
    let percentile = |p: usize| -> f64 {
        if latencies.is_empty() {
            return 0.0;
        }
        latencies[(latencies.len() * p / 100).min(latencies.len() - 1)]
    };

    let throughput = received as f64 / elapsed.as_secs_f64();
    let data_rate = throughput * nch as f64 * fmt.channel_bytes().max(1) as f64 / 1_000_000.0;
    let loss = if pushed > 0 {
        (1.0 - received as f64 / pushed as f64) * 100.0
    } else {
        0.0
    };

    let rtt_final = conn
        .rtt(iroh::endpoint::PathId::default())
        .map(|d| d.as_micros() as f64)
        .unwrap_or(rtt);

    conn.close(0u32.into(), b"done");
    source_ep.close().await;
    sink_ep.close().await;

    Ok(BenchResults {
        pushed,
        received,
        elapsed_secs: elapsed.as_secs_f64(),
        throughput_samples_sec: throughput,
        data_rate_mb_sec: data_rate,
        loss_pct: loss,
        latency_min_us: latencies.first().copied().unwrap_or(0.0) * to_us,
        latency_mean_us: mean * to_us,
        latency_p50_us: percentile(50) * to_us,
        latency_p95_us: percentile(95) * to_us,
        latency_p99_us: percentile(99) * to_us,
        latency_max_us: latencies.last().copied().unwrap_or(0.0) * to_us,
        rtt_us: rtt_final,
    })
}
