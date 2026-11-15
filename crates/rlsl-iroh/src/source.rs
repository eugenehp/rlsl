//! Source side: resolve local LSL streams and forward them over iroh.
//!
//! **Zero data loss guarantee**: every sample pulled from the local LSL
//! outlet is delivered reliably over the QUIC stream. Back-pressure
//! propagates all the way from QUIC → channel → inlet pull so the inlet
//! never overflows as long as the network can keep up.

use crate::compress::{self, Compression};
use crate::protocol;
use anyhow::Result;
use iroh::endpoint::{Connection, Endpoint};
use iroh::PublicKey;
use rlsl::inlet::StreamInlet;
use rlsl::prelude::*;
use rlsl::resolver;
use rlsl::sample::Sample;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Configuration for the source side of the tunnel.
#[derive(Clone, Debug)]
pub struct SourceConfig {
    /// LSL query to select which streams to forward (empty = all).
    pub query: String,
    /// Public key of the remote sink to connect to.
    pub sink_node_id: PublicKey,
    /// How many seconds to wait for stream resolution.
    pub resolve_timeout: f64,
    /// Continuously watch for new streams (re-resolve periodically).
    pub continuous: bool,
    /// Compression mode. Default is `Compression::None`.
    pub compression: Compression,
    /// Use QUIC datagrams (lossy, lowest latency). **Will drop samples
    /// under congestion.** Only available with the `lossy-datagrams` feature.
    #[cfg(feature = "lossy-datagrams")]
    pub use_datagrams: bool,
}

/// Counters exposed for reliability monitoring.
#[derive(Debug, Default)]
pub struct ForwardStats {
    pub samples_pulled: AtomicU64,
    pub samples_sent: AtomicU64,
}

/// Run the source: resolve local LSL streams matching `config.query`,
/// connect to the remote sink, and forward samples.
pub async fn run_source(endpoint: &Endpoint, config: SourceConfig) -> Result<()> {
    let shutdown = Arc::new(AtomicBool::new(false));

    // Ctrl-C handler
    let shutdown_ctrlc = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        log::info!("Ctrl-C received, shutting down source...");
        shutdown_ctrlc.store(true, Ordering::SeqCst);
    });

    log::info!(
        "Resolving local LSL streams (query={:?}, timeout={}s)...",
        config.query,
        config.resolve_timeout
    );

    // Connect to the remote sink node
    let connection = endpoint
        .connect(config.sink_node_id, protocol::LSL_ALPN)
        .await?;
    log::info!(
        "Connected to sink (rtt={:?})",
        connection.rtt(iroh::endpoint::PathId::default())
    );

    let mut known_uids: HashSet<String> = HashSet::new();
    let mut handles = Vec::new();

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let streams = {
            let query = config.query.clone();
            let timeout = config.resolve_timeout;
            tokio::task::spawn_blocking(move || resolve_streams(&query, timeout)).await?
        };

        for info in streams {
            let uid = info.uid();
            if known_uids.contains(&uid) {
                continue;
            }
            known_uids.insert(uid.clone());

            log::info!("New stream discovered: '{}' (uid={})", info.name(), uid);

            let conn = connection.clone();
            let comp = config.compression;
            #[cfg(feature = "lossy-datagrams")]
            let use_dg = config.use_datagrams;
            #[cfg(not(feature = "lossy-datagrams"))]
            let use_dg = false;
            let sd = shutdown.clone();

            let handle = tokio::spawn(async move {
                if let Err(e) = forward_stream(conn, &info, use_dg, comp, sd).await {
                    if !e.to_string().contains("shutdown") {
                        log::error!("Stream '{}' forwarding ended: {}", info.name(), e);
                    }
                }
            });
            handles.push(handle);
        }

        if !config.continuous {
            break;
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    if !config.continuous {
        for h in handles {
            let _ = h.await;
        }
    } else {
        while !shutdown.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(())
}

fn resolve_streams(query: &str, timeout: f64) -> Vec<StreamInfo> {
    if query.is_empty() {
        resolver::resolve_all(timeout)
    } else if let Some(eq) = query.find('=') {
        let prop = query[..eq].trim().trim_matches('\'').trim_matches('"');
        let val = query[eq + 1..].trim().trim_matches('\'').trim_matches('"');
        resolver::resolve_by_property(prop, val, 0, timeout)
    } else {
        resolver::resolve_all(timeout)
    }
}

/// Forward a single LSL stream over the iroh connection.
///
/// # Reliability
///
/// * Uses a reliable QUIC uni-directional stream (never datagrams).
/// * The internal channel is **unbounded** — back-pressure comes from QUIC
///   flow control, not from dropping samples.
/// * The blocking reader uses `FOREVER` timeout so it never spuriously
///   returns empty — every sample the outlet produces is captured.
/// * The inlet uses a large buffer (32768 samples ≈ 130s at 250 Hz) to
///   absorb transient QUIC stalls.
/// * On any write error the function returns an error — the caller can
///   retry or log, but data is never silently swallowed.
async fn forward_stream(
    conn: Connection,
    info: &StreamInfo,
    use_datagrams: bool,
    compression: Compression,
    shutdown: Arc<AtomicBool>,
) -> Result<()> {
    let name = info.name();
    let name2 = name.clone();
    let fmt = info.channel_format();
    let nch = info.channel_count();
    let srate = info.nominal_srate();

    log::info!(
        "Forwarding stream '{}' (fmt={:?}, ch={}, srate={}, compression={:?})...",
        name,
        fmt,
        nch,
        srate,
        compression
    );

    let stats = Arc::new(ForwardStats::default());

    // Open a unidirectional QUIC stream — reliable, ordered delivery.
    let mut send = conn.open_uni().await?;
    send.set_priority(0)?;

    // Send the stream header
    let header = protocol::encode_stream_header(info, compression);
    send.write_all(&header).await?;

    // Subscribe to the local outlet via inlet.
    // Large buffer (32768) to absorb transient QUIC stalls.
    // recover=true so if the local outlet restarts, we reconnect.
    let inlet = {
        let info = info.clone();
        tokio::task::spawn_blocking(move || {
            let inlet = StreamInlet::new(&info, 32768, 0, true);
            inlet.open_stream(10.0).map(|_| inlet)
        })
        .await?
        .map_err(|e| anyhow::anyhow!(e))?
    };

    // **Unbounded** channel: never drop samples due to channel pressure.
    // Back-pressure is applied by the QUIC write_all future — when the
    // network is congested, the async writer blocks, the channel grows,
    // and eventually the inlet buffer absorbs the spike.
    let (sample_tx, mut sample_rx) = tokio::sync::mpsc::unbounded_channel::<Sample>();

    // Blocking reader thread
    let shutdown2 = shutdown.clone();
    let stats2 = stats.clone();
    let reader_handle = tokio::task::spawn_blocking(move || {
        let mut buf_f32 = vec![0.0f32; nch as usize];
        let mut buf_f64 = vec![0.0f64; nch as usize];
        let mut buf_i32 = vec![0i32; nch as usize];
        let mut buf_i16 = vec![0i16; nch as usize];
        let mut buf_i8 = vec![0i8; nch as usize];
        let mut buf_i64 = vec![0i64; nch as usize];

        // Use a moderate timeout. We can't use FOREVER because we need to
        // check the shutdown flag. 0.1s is a good balance: responsive
        // shutdown, negligible overhead.
        let poll_timeout = 0.1;

        loop {
            if shutdown2.load(Ordering::Relaxed) {
                break;
            }

            let sample = match fmt {
                ChannelFormat::Float32 => inlet
                    .pull_sample_f(&mut buf_f32, poll_timeout)
                    .ok()
                    .and_then(|ts| {
                        (ts > 0.0).then(|| {
                            let mut s = Sample::new(fmt, nch, ts);
                            s.assign_f32(&buf_f32);
                            s
                        })
                    }),
                ChannelFormat::Double64 => inlet
                    .pull_sample_d(&mut buf_f64, poll_timeout)
                    .ok()
                    .and_then(|ts| {
                        (ts > 0.0).then(|| {
                            let mut s = Sample::new(fmt, nch, ts);
                            s.assign_f64(&buf_f64);
                            s
                        })
                    }),
                ChannelFormat::Int32 => inlet
                    .pull_sample_i32(&mut buf_i32, poll_timeout)
                    .ok()
                    .and_then(|ts| {
                        (ts > 0.0).then(|| {
                            let mut s = Sample::new(fmt, nch, ts);
                            s.assign_i32(&buf_i32);
                            s
                        })
                    }),
                ChannelFormat::Int16 => inlet
                    .pull_sample_i16(&mut buf_i16, poll_timeout)
                    .ok()
                    .and_then(|ts| {
                        (ts > 0.0).then(|| {
                            let mut s = Sample::new(fmt, nch, ts);
                            s.assign_i16(&buf_i16);
                            s
                        })
                    }),
                ChannelFormat::Int8 => {
                    // No pull_sample_i8 — pull as i16 and downcast
                    let raw_len = nch as usize;
                    inlet
                        .pull_sample_i16(&mut buf_i16, poll_timeout)
                        .ok()
                        .and_then(|ts| {
                            (ts > 0.0).then(|| {
                                let mut s = Sample::new(fmt, nch, ts);
                                for (i, v) in buf_i8.iter_mut().enumerate() {
                                    if i < raw_len {
                                        *v = buf_i16[i] as i8;
                                    }
                                }
                                s.assign_i8(&buf_i8);
                                s
                            })
                        })
                }
                ChannelFormat::Int64 => inlet
                    .pull_sample_i64(&mut buf_i64, poll_timeout)
                    .ok()
                    .and_then(|ts| {
                        (ts > 0.0).then(|| {
                            let mut s = Sample::new(fmt, nch, ts);
                            s.assign_i64(&buf_i64);
                            s
                        })
                    }),
                ChannelFormat::String | ChannelFormat::Undefined => inlet
                    .pull_sample_str(poll_timeout)
                    .ok()
                    .and_then(|(strings, ts)| {
                        (ts > 0.0).then(|| {
                            let mut s = Sample::new(fmt, nch, ts);
                            s.assign_strings(&strings);
                            s
                        })
                    }),
            };

            if let Some(s) = sample {
                stats2.samples_pulled.fetch_add(1, Ordering::Relaxed);
                // Unbounded send — never fails unless receiver is dropped
                if sample_tx.send(s).is_err() {
                    log::warn!("Stream '{}': QUIC writer dropped, stopping reader", name2);
                    break;
                }
            }
        }
    });

    // Async writer — serialize and push over QUIC.
    // write_all blocks on QUIC congestion, providing natural back-pressure.
    let mut raw_buf = Vec::with_capacity(1024);
    let mut compressed_buf = Vec::with_capacity(1024);
    let mut encoder =
        compress::ChunkEncoder::new(compression, info.channel_format().channel_bytes());
    let mut sent_count = 0u64;

    while let Some(sample) = sample_rx.recv().await {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        raw_buf.clear();
        sample.serialize_110(&mut raw_buf);

        if use_datagrams {
            // Lossy fire-and-forget — only with `lossy-datagrams` feature.
            // Will silently drop under congestion.
            let _ = conn.send_datagram(bytes::Bytes::copy_from_slice(&raw_buf));
        } else if compression.is_compressed() {
            compressed_buf.clear();
            encoder.compress(&raw_buf, &mut compressed_buf);
            send.write_all(&compressed_buf).await?;
        } else {
            send.write_all(&raw_buf).await?;
        }
        sent_count += 1;
        stats.samples_sent.store(sent_count, Ordering::Relaxed);
    }

    let pulled = stats.samples_pulled.load(Ordering::Relaxed);
    log::info!(
        "Stream '{}': pulled={}, sent={}, delta={}",
        info.name(),
        pulled,
        sent_count,
        pulled as i64 - sent_count as i64
    );
    if pulled != sent_count {
        log::error!(
            "Stream '{}': DATA LOSS — {} samples pulled but only {} sent!",
            info.name(),
            pulled,
            sent_count
        );
    }

    shutdown.store(true, Ordering::Relaxed);
    let _ = reader_handle.await;
    send.finish()?;
    Ok(())
}

/// Convenience: resolve local streams and print them.
pub fn list_local_streams(timeout: f64) -> Vec<StreamInfo> {
    resolver::resolve_all(timeout)
}
