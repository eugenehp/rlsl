//! Sink side: accept iroh connections and re-publish LSL streams locally.

use crate::compress::{self, Compression};
use crate::protocol;
use anyhow::Result;
use iroh::endpoint::{Connection, Endpoint, RecvStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use rlsl::outlet::StreamOutlet;
use rlsl::prelude::*;
use rlsl::sample::Sample;
use rlsl::stream_info::StreamInfo;
use std::io::Cursor;
use std::sync::Arc;

/// The iroh protocol handler that accepts incoming LSL tunnel connections.
#[derive(Debug, Clone)]
pub struct LslSinkHandler;

impl ProtocolHandler for LslSinkHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        log::info!(
            "Incoming tunnel connection (rtt={:?})",
            connection.rtt(iroh::endpoint::PathId::default())
        );

        let conn = connection.clone();

        // Spawn datagram receiver
        let dg_conn = connection.clone();
        tokio::spawn(async move {
            handle_datagrams(dg_conn).await;
        });

        // Accept uni streams — one per LSL stream
        loop {
            match conn.accept_uni().await {
                Ok(recv) => {
                    tokio::spawn(async move {
                        if let Err(e) = handle_stream(recv).await {
                            log::error!("Stream handler error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    log::info!("Connection closed: {}", e);
                    break;
                }
            }
        }
        Ok(())
    }
}

// ── Reliable stream path ─────────────────────────────────────────────

/// Handle a single incoming uni stream: read header, create outlet, relay samples.
async fn handle_stream(mut recv: RecvStream) -> Result<()> {
    let (info, compression) = read_stream_header(&mut recv).await?;

    let name = info.name();
    let fmt = info.channel_format();
    let nch = info.channel_count();

    log::info!(
        "Receiving stream '{}' (fmt={:?}, ch={}, srate={}, compression={:?}) — creating local outlet",
        name, fmt, nch, info.nominal_srate(), compression
    );

    let outlet = Arc::new(StreamOutlet::new(&info, 0, 360));
    log::info!("Local outlet '{}' on TCP port {}", name, info.v4data_port());

    if compression.is_compressed() {
        receive_compressed(&mut recv, &outlet, fmt, nch, compression).await
    } else {
        receive_raw(&mut recv, &outlet, fmt, nch).await
    }
}

/// Receive raw (uncompressed) samples.
async fn receive_raw(
    recv: &mut RecvStream,
    outlet: &StreamOutlet,
    fmt: ChannelFormat,
    nch: u32,
) -> Result<()> {
    let mut read_buf = vec![0u8; 8192];
    let mut leftover: Vec<u8> = Vec::new();

    loop {
        let n = recv.read(&mut read_buf).await?;
        match n {
            Some(0) | None => break,
            Some(n) => {
                let data = if leftover.is_empty() {
                    &read_buf[..n]
                } else {
                    leftover.extend_from_slice(&read_buf[..n]);
                    leftover.as_slice()
                };

                let consumed = push_samples(data, outlet, fmt, nch)?;
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

/// Receive compressed chunked samples (any codec).
async fn receive_compressed(
    recv: &mut RecvStream,
    outlet: &StreamOutlet,
    fmt: ChannelFormat,
    nch: u32,
    compression: Compression,
) -> Result<()> {
    let mut read_buf = vec![0u8; 8192];
    let mut leftover: Vec<u8> = Vec::new();

    loop {
        let n = recv.read(&mut read_buf).await?;
        match n {
            Some(0) | None => break,
            Some(n) => {
                let data = if leftover.is_empty() {
                    &read_buf[..n]
                } else {
                    leftover.extend_from_slice(&read_buf[..n]);
                    leftover.as_slice()
                };

                let mut offset = 0;
                while offset < data.len() {
                    match compress::decompress_chunk(&data[offset..], compression) {
                        Some((decompressed, consumed)) => {
                            offset += consumed;
                            let _ = push_samples(&decompressed, outlet, fmt, nch)?;
                        }
                        None => break, // need more data
                    }
                }

                if offset < data.len() {
                    leftover = data[offset..].to_vec();
                } else {
                    leftover.clear();
                }
            }
        }
    }
    Ok(())
}

/// Read the stream header from the recv stream.
async fn read_stream_header(recv: &mut RecvStream) -> Result<(StreamInfo, Compression)> {
    // magic(4) + compression(1) + reserved(3) + xml_len(4) = 12 bytes
    let mut header_prefix = [0u8; 12];
    recv.read_exact(&mut header_prefix).await?;
    anyhow::ensure!(&header_prefix[..4] == protocol::MAGIC, "bad magic");
    let compression = Compression::from_u8(header_prefix[4]);
    let xml_len = u32::from_le_bytes(header_prefix[8..12].try_into()?) as usize;
    anyhow::ensure!(xml_len < 1_000_000, "XML header too large");

    let mut xml_buf = vec![0u8; xml_len];
    recv.read_exact(&mut xml_buf).await?;

    let xml = std::str::from_utf8(&xml_buf)?;
    let info =
        StreamInfo::from_shortinfo_message(xml).ok_or_else(|| anyhow::anyhow!("bad XML header"))?;
    Ok((info, compression))
}

// ── Datagram (lossy) path ────────────────────────────────────────────

async fn handle_datagrams(conn: Connection) {
    while let Ok(bytes) = conn.read_datagram().await {
        log::trace!("Received datagram ({} bytes)", bytes.len());
        let _ = bytes;
    }
}

// ── Sample deserialization ───────────────────────────────────────────

/// Deserialize as many complete samples as possible from `data`.
/// Returns bytes consumed.
fn push_samples(data: &[u8], outlet: &StreamOutlet, fmt: ChannelFormat, nch: u32) -> Result<usize> {
    let mut cursor = Cursor::new(data);
    let sample_data_bytes = fmt.channel_bytes() * nch as usize;

    loop {
        let pos = cursor.position() as usize;
        let remaining = data.len() - pos;

        if fmt != ChannelFormat::String && fmt != ChannelFormat::Undefined {
            if remaining < 1 + sample_data_bytes {
                return Ok(pos);
            }
        } else if remaining < 1 {
            return Ok(pos);
        }

        let before = pos;
        match Sample::deserialize_110(&mut cursor, fmt, nch) {
            Ok(sample) => push_sample_to_outlet(outlet, &sample),
            Err(_) => {
                cursor.set_position(before as u64);
                return Ok(before);
            }
        }
    }
}

fn push_sample_to_outlet(outlet: &StreamOutlet, sample: &Sample) {
    let ts = sample.timestamp;
    let pt = sample.pushthrough;
    match &sample.data {
        rlsl::sample::SampleData::Float32(d) => outlet.push_sample_f(d, ts, pt),
        rlsl::sample::SampleData::Double64(d) => outlet.push_sample_d(d, ts, pt),
        rlsl::sample::SampleData::Int32(d) => outlet.push_sample_i32(d, ts, pt),
        rlsl::sample::SampleData::Int16(d) => outlet.push_sample_i16(d, ts, pt),
        rlsl::sample::SampleData::Int64(d) => outlet.push_sample_i64(d, ts, pt),
        rlsl::sample::SampleData::Int8(_) => {
            let raw = sample.retrieve_raw();
            outlet.push_sample_raw(&raw, ts, pt);
        }
        rlsl::sample::SampleData::StringData(d) => outlet.push_sample_str(d, ts, pt),
    }
}

// ── Public entry point ───────────────────────────────────────────────

pub async fn run_sink(endpoint: &Endpoint) -> Result<()> {
    let node_id = endpoint.id();
    log::info!("Sink ready. Node ID: {}", node_id);

    println!();
    println!("╔══════════════════════════════════════════════════╗");
    println!("║  LSL Iroh Sink                                  ║");
    println!("║  Node ID: {}  ║", node_id);
    println!("╚══════════════════════════════════════════════════╝");
    println!();
    println!("Share the Node ID with the source to start tunneling.");
    println!("Local LSL outlets will appear automatically.");
    println!("Existing LSL clients connect as usual (TCP/UDP).");
    println!();

    tokio::signal::ctrl_c().await?;
    log::info!("Shutting down sink...");
    Ok(())
}
