//! TCP data server for a stream outlet.
//!
//! Implements the liblsl TCP protocol:
//! - LSL:shortinfo - returns shortinfo XML if query matches
//! - LSL:fullinfo  - returns full info XML
//! - LSL:streamfeed/110 - negotiates and streams samples

use crate::config::CONFIG;
use crate::sample::Sample;
use crate::send_buffer::SendBuffer;
use crate::stream_info::StreamInfo;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

pub struct TcpServer {
    info: StreamInfo,
    send_buffer: Arc<SendBuffer>,
    shutdown: Arc<AtomicBool>,
    /// Where the outlet's clock records come from, for consumers that ask for them in-band.
    clock: Option<ClockSource>,
}

/// The clock records of an outlet (feature `ntp`; without it there are none to send, and a
/// request for them is not understood).
#[cfg(feature = "ntp")]
pub type ClockSource = Arc<crate::ntp::feed::ClockHub>;
#[cfg(not(feature = "ntp"))]
pub type ClockSource = ();
#[cfg(feature = "ntp")]
type ClockFeed = crossbeam_channel::Receiver<Arc<crate::ntp::ClockRecord>>;
#[cfg(not(feature = "ntp"))]
type ClockFeed = ();

#[cfg(feature = "ntp")]
fn clock_feed(source: &ClockSource) -> ClockFeed {
    source.subscribe()
}
#[cfg(not(feature = "ntp"))]
fn clock_feed(_source: &ClockSource) -> ClockFeed {}

/// Append the clock records that have come in to `out`; whether there were any.
#[cfg(feature = "ntp")]
fn drain_clock_feed(feed: &ClockFeed, compressed: bool, out: &mut Vec<u8>) -> bool {
    let mut any = false;
    for r in feed.try_iter() {
        if compressed {
            crate::ntp::feed::encode_compressed(&r, out);
        } else {
            crate::ntp::feed::encode_plain(&r, out);
        }
        any = true;
    }
    any
}
#[cfg(not(feature = "ntp"))]
fn drain_clock_feed(_feed: &ClockFeed, _compressed: bool, _out: &mut Vec<u8>) -> bool {
    false
}

/// Ports returned by `TcpServer::start`.
pub struct TcpPorts {
    pub v4_port: u16,
    pub v6_port: u16,
    pub shutdown: Arc<AtomicBool>,
}

/// The encoder of a compressed feed. Without the `compression` feature there is none to make:
/// requests for compression are not understood and the feed is the plain protocol.
#[cfg(feature = "compression")]
type FeedEncoder = crate::compress::StreamEncoder;
#[cfg(not(feature = "compression"))]
enum FeedEncoder {}
#[cfg(not(feature = "compression"))]
impl FeedEncoder {
    fn encode(&mut self, _raw: &[u8], _out: &mut Vec<u8>) {
        match *self {}
    }
}

/// The encoder for a consumer that asked for `requested` compression, when this outlet can
/// give it: a known scheme, protocol 1.10, and samples that fit a frame.
#[cfg(feature = "compression")]
fn feed_encoder(
    requested: &str,
    protocol: i32,
    fmt: crate::types::ChannelFormat,
    channels: u32,
) -> Option<FeedEncoder> {
    let sample_bytes = 9 + fmt.channel_bytes() * channels as usize;
    (requested.eq_ignore_ascii_case(crate::compress::STREAM_ZSTD)
        && protocol >= 110
        && fmt != crate::types::ChannelFormat::String
        && fmt != crate::types::ChannelFormat::Undefined
        && sample_bytes <= crate::compress::MAX_SAMPLE_BYTES)
        .then(|| FeedEncoder::new(fmt.channel_bytes()))
}
#[cfg(not(feature = "compression"))]
fn feed_encoder(
    _requested: &str,
    _protocol: i32,
    _fmt: crate::types::ChannelFormat,
    _channels: u32,
) -> Option<FeedEncoder> {
    None
}

/// Bind the data listener like liblsl: on the first free port of the LSL range (so that a
/// firewall can be opened for it), any free port when the range is taken.
async fn bind_service(ip: &str) -> std::io::Result<TcpListener> {
    let ip: std::net::IpAddr = ip.parse().map_err(std::io::Error::other)?;
    let mut last = std::io::Error::other("no service port to try");
    for port in crate::config::service_ports() {
        match TcpListener::bind(std::net::SocketAddr::new(ip, port)).await {
            Ok(listener) => return Ok(listener),
            Err(e) => last = e,
        }
    }
    Err(last)
}

impl TcpServer {
    /// Start the TCP server on both IPv4 and IPv6.
    /// Returns ports and a shared shutdown handle.
    pub fn start(info: StreamInfo, send_buffer: Arc<SendBuffer>, chunk_size: i32) -> TcpPorts {
        Self::start_with_clock(info, send_buffer, chunk_size, None)
    }

    /// [`start`](Self::start), with the outlet's clock records for consumers that ask for them.
    pub fn start_with_clock(
        info: StreamInfo,
        send_buffer: Arc<SendBuffer>,
        chunk_size: i32,
        clock: Option<ClockSource>,
    ) -> TcpPorts {
        let shutdown = Arc::new(AtomicBool::new(false));

        let server = Arc::new(TcpServer {
            info: info.clone(),
            send_buffer,
            shutdown: shutdown.clone(),
            clock,
        });

        // --- IPv4 listener ---
        // A machine without IPv4, or with every allowed port taken, must not take the program
        // down: the outlet then has no IPv4 data port (0), and says so.
        let v4_port = match crate::RUNTIME.block_on(bind_service("0.0.0.0")) {
            Ok(listener) => {
                let port = listener.local_addr().map_or(0, |a| a.port());
                let srv = server.clone();
                crate::RUNTIME.spawn(async move {
                    Self::accept_loop(listener, srv, chunk_size).await;
                });
                port
            }
            Err(e) => {
                log::error!("no IPv4 data port for the outlet: {e}");
                0
            }
        };

        // --- IPv6 listener ---
        let v6_port = if crate::config::CONFIG.allow_ipv6 {
            match crate::RUNTIME.block_on(bind_service("::")) {
                Ok(listener) => {
                    let port = listener.local_addr().map_or(0, |a| a.port());
                    let srv = server.clone();
                    crate::RUNTIME.spawn(async move {
                        Self::accept_loop(listener, srv, chunk_size).await;
                    });
                    port
                }
                Err(_) => 0,
            }
        } else {
            0
        };

        TcpPorts {
            v4_port,
            v6_port,
            shutdown,
        }
    }

    async fn accept_loop(listener: TcpListener, server: Arc<TcpServer>, chunk_size: i32) {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _addr)) => {
                            let srv = server.clone();
                            let cs = chunk_size;
                            tokio::spawn(async move {
                                let _ = srv.handle_connection(stream, cs).await;
                            });
                        }
                        Err(_) => {
                            if server.shutdown.load(Ordering::Relaxed) { break; }
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    if server.shutdown.load(Ordering::Relaxed) { break; }
                }
            }
        }
    }

    async fn handle_connection(&self, stream: TcpStream, chunk_size: i32) -> std::io::Result<()> {
        stream.set_nodelay(true)?;
        let mut reader = BufReader::new(stream);

        // Read the command line
        let mut command = String::new();
        reader.read_line(&mut command).await?;
        let command = command.trim().to_string();

        if command == "LSL:shortinfo" {
            self.handle_shortinfo(&mut reader).await
        } else if command == "LSL:fullinfo" {
            self.handle_fullinfo(&mut reader).await
        } else if command.starts_with("LSL:streamfeed") {
            self.handle_streamfeed(reader, &command, chunk_size).await
        } else {
            Ok(())
        }
    }

    async fn handle_shortinfo(&self, reader: &mut BufReader<TcpStream>) -> std::io::Result<()> {
        let mut query = String::new();
        reader.read_line(&mut query).await?;
        let query = query.trim().to_string();

        if self.info.matches_query(&query) {
            let msg = self.info.to_shortinfo_message();
            reader.get_mut().write_all(msg.as_bytes()).await?;
        }
        Ok(())
    }

    /// Send the consumer's samples until it disconnects or the outlet shuts down.
    #[allow(clippy::too_many_arguments)]
    fn feed(
        mut stream: std::net::TcpStream,
        consumer: crate::send_buffer::BufferedConsumer,
        shutdown: &AtomicBool,
        data_protocol_version: i32,
        effective_chunk: i32,
        mut encoder: Option<FeedEncoder>,
        suppress_subnormals: bool,
        clock_feed: Option<ClockFeed>,
    ) {
        use std::io::Write;
        let mut chunk_count = 0;
        let mut chunk_buf = Vec::with_capacity(4096);
        let mut sample_buf = Vec::with_capacity(512);
        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            match consumer
                .receiver
                .recv_timeout(std::time::Duration::from_millis(100))
            {
                Ok(Some(sample)) => {
                    // clock records go between samples, ahead of this one
                    let clock_records = clock_feed
                        .as_ref()
                        .is_some_and(|f| drain_clock_feed(f, encoder.is_some(), &mut chunk_buf));
                    // a consumer whose machine cannot hold subnormal numbers gets zeros for them
                    let flushed = if suppress_subnormals {
                        sample.without_subnormals()
                    } else {
                        None
                    };
                    let sample = flushed.as_ref().unwrap_or(&sample);
                    if let Some(enc) = encoder.as_mut() {
                        // one frame per sample, coded against the samples before it
                        sample_buf.clear();
                        sample.serialize_110(&mut sample_buf);
                        enc.encode(&sample_buf, &mut chunk_buf);
                    } else if data_protocol_version >= 110 {
                        sample.serialize_110(&mut chunk_buf);
                    } else {
                        sample.serialize_100(&mut chunk_buf);
                    }
                    chunk_count += 1;
                    if sample.pushthrough || chunk_count >= effective_chunk || clock_records {
                        if stream.write_all(&chunk_buf).is_err() {
                            break;
                        }
                        chunk_buf.clear();
                        chunk_count = 0;
                    }
                }
                Ok(None) => break, // sentinel
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    if let Some(f) = clock_feed.as_ref() {
                        drain_clock_feed(f, encoder.is_some(), &mut chunk_buf);
                    }
                    if !chunk_buf.is_empty() {
                        if stream.write_all(&chunk_buf).is_err() {
                            break;
                        }
                        chunk_buf.clear();
                        chunk_count = 0;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    async fn handle_fullinfo(&self, reader: &mut BufReader<TcpStream>) -> std::io::Result<()> {
        let msg = self.info.to_fullinfo_message();
        reader.get_mut().write_all(msg.as_bytes()).await?;
        Ok(())
    }

    async fn handle_streamfeed(
        &self,
        mut reader: BufReader<TcpStream>,
        command: &str,
        chunk_size: i32,
    ) -> std::io::Result<()> {
        let mut max_buffered = 360;
        let mut max_chunklen = 0;
        let mut _request_uid = String::new();
        let mut data_protocol_version = 110;
        let mut encoder: Option<FeedEncoder> = None;
        let mut suppress_subnormals = false;
        let mut replay_seconds = 0.0f64;
        // the consumer asked for the outlet's clock records in-band, and there are some to send
        let mut send_clock = false;

        if command.starts_with("LSL:streamfeed/") {
            // Parse version and optional UID from command line
            let parts: Vec<&str> = command.split_whitespace().collect();
            if let Some(ver_str) = parts
                .first()
                .and_then(|s| s.strip_prefix("LSL:streamfeed/"))
            {
                // the lower of what the consumer speaks and what this outlet is set to
                data_protocol_version = ver_str
                    .parse::<i32>()
                    .unwrap_or(110)
                    .min(CONFIG.use_protocol_version);
            }
            if parts.len() > 1 {
                _request_uid = parts[1].to_string();
            }

            // Read feed parameters (key: value headers until empty line)
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await?;
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    break;
                }
                let line = trimmed;
                if let Some(colon) = line.find(':') {
                    let key = line[..colon].trim().to_lowercase();
                    let val = line[colon + 1..].trim().to_string();
                    match key.as_str() {
                        "max-buffer-length" => {
                            max_buffered = val.parse().unwrap_or(360);
                        }
                        "max-chunk-length" => {
                            max_chunklen = val.parse().unwrap_or(0);
                        }
                        // the consumer's machine has no subnormal numbers: it must not be
                        // sent any (liblsl's negotiation; this outlet always sends little-endian
                        // IEEE 754, which the consumer converts from as it needs)
                        // an rlsl inlet that connected late asking for what it missed
                        // (liblsl never does): granted as far as this outlet keeps a history
                        "rlsl-replay" => {
                            replay_seconds = val
                                .trim()
                                .parse::<f64>()
                                .ok()
                                .filter(|s| s.is_finite() && *s > 0.0)
                                .map_or(0.0, |s| s.min(self.send_buffer.history_seconds()));
                        }
                        // an rlsl inlet asking for clock records with the samples (liblsl
                        // never does)
                        "rlsl-clock-records" => {
                            send_clock =
                                val.trim() == "1" && cfg!(feature = "ntp") && self.clock.is_some();
                        }
                        "supports-subnormals" => {
                            suppress_subnormals = val.trim() == "0";
                        }
                        // an rlsl inlet asking for a compressed feed (liblsl never does)
                        "rlsl-compression" => {
                            encoder = feed_encoder(
                                &val,
                                data_protocol_version,
                                self.info.channel_format(),
                                self.info.channel_count(),
                            );
                        }
                        _ => {}
                    }
                }
            }

            // The consumer names the stream it wants. This port may have belonged to another
            // outlet a moment ago — services sit in a small range of ports, and an inlet that
            // lost its stream comes back to the address it knew — so a request for another
            // stream is refused, as liblsl refuses it, and the inlet looks for its stream
            // again instead of being fed somebody else's samples.
            if !_request_uid.is_empty() && _request_uid != self.info.uid() {
                let refusal = format!("LSL/{} 404 Not found\r\n\r\n", CONFIG.use_protocol_version);
                reader.get_mut().write_all(refusal.as_bytes()).await?;
                reader.get_mut().flush().await?;
                return Ok(());
            }

            // Send response. Agreeing to compress is said here; from the blank line on, the
            // feed (test patterns included) is then compressed frames.
            let mut compression = if encoder.is_some() {
                "Rlsl-Compression: stream-zstd\r\n".to_string()
            } else {
                String::new()
            };
            if replay_seconds > 0.0 {
                // how far back the feed starts
                compression.push_str(&format!("Rlsl-Replay: {replay_seconds}\r\n"));
            }
            // records only in protocol 1.10 feeds, which have room for them between samples
            send_clock &= data_protocol_version >= 110;
            if send_clock {
                compression.push_str("Rlsl-Clock-Records: 1\r\n");
            }
            let response = format!(
                "LSL/{} 200 OK\r\nUID: {}\r\nByte-Order: 1234\r\nSuppress-Subnormals: {}\r\nData-Protocol-Version: {}\r\n{}\r\n",
                CONFIG.use_protocol_version,
                self.info.uid(),
                u8::from(suppress_subnormals),
                data_protocol_version,
                compression
            );
            reader.get_mut().write_all(response.as_bytes()).await?;
            reader.get_mut().flush().await?;
        } else {
            // Protocol 1.00 fallback - read two integers
            let mut params = String::new();
            reader.read_line(&mut params).await?;
            let parts: Vec<&str> = params.split_whitespace().collect();
            if parts.len() >= 2 {
                max_buffered = parts[0].parse().unwrap_or(360);
                max_chunklen = parts[1].parse().unwrap_or(0);
            }
            // a request without a version is protocol 1.00
            data_protocol_version = 100;
        }

        // Subscribe to the send buffer — before the test patterns go out: the consumer counts
        // itself connected once it has read them, and every sample pushed from then on must
        // reach it. (Subscribed after them, a sample pushed in between was lost.)
        // The consumer's buffer: `max_buffered` is seconds of data at the stream's rate, or
        // hundreds of samples for an irregular stream, as in liblsl. (It used to be taken as a
        // number of samples, and a consumer that fell 360 samples behind — 0.7 s at 500 Hz —
        // had its feed ended.)
        let consumer = (max_buffered > 0).then(|| {
            self.send_buffer.new_replaying_consumer(
                crate::types::buffer_samples(
                    self.send_buffer.granted_buffer(max_buffered),
                    self.info.nominal_srate(),
                ),
                replay_seconds,
            )
        });

        if data_protocol_version < 110 {
            // The feed of protocol 1.00 is one "portable archive": its header, the stream's
            // short description as a string, and then the samples — the first of them
            // preceded by its class information, as liblsl's serialisation library writes it.
            let mut opening = crate::sample::portable::HEADER.to_vec();
            crate::sample::portable::put_string(&mut opening, &self.info.to_shortinfo_message());
            opening.extend_from_slice(&crate::sample::portable::SAMPLE_CLASS_INFO);
            reader.get_mut().write_all(&opening).await?;
        }

        // Send test pattern samples using the negotiated protocol version
        let fmt = self.info.channel_format();
        let nch = self.info.channel_count();
        for test_offset in [4, 2] {
            let mut test_sample = Sample::new(fmt, nch, 0.0);
            test_sample.assign_test_pattern(test_offset);
            let mut buf = Vec::new();
            if let Some(enc) = encoder.as_mut() {
                let mut raw = Vec::new();
                test_sample.serialize_110(&mut raw);
                enc.encode(&raw, &mut buf);
            } else if data_protocol_version >= 110 {
                test_sample.serialize_110(&mut buf);
            } else {
                test_sample.serialize_100(&mut buf);
            }
            reader.get_mut().write_all(&buf).await?;
        }
        reader.get_mut().flush().await?;

        let Some(consumer) = consumer else {
            return Ok(());
        };

        let effective_chunk = if max_chunklen > 0 {
            max_chunklen
        } else if chunk_size > 0 {
            chunk_size
        } else {
            i32::MAX
        };

        // Waiting for samples blocks, so the feed runs on the blocking pool (one thread per
        // consumer, like liblsl): on an async worker it would keep that worker from the other
        // connections (`LSL:fullinfo` / `LSL:shortinfo` requests, further consumers).
        let stream = reader.into_inner().into_std()?;
        stream.set_nonblocking(false)?;
        let shutdown = self.shutdown.clone();
        let clock_feed = self.clock.as_ref().filter(|_| send_clock).map(clock_feed);
        let _ = tokio::task::spawn_blocking(move || {
            Self::feed(
                stream,
                consumer,
                &shutdown,
                data_protocol_version,
                effective_chunk,
                encoder,
                suppress_subnormals,
                clock_feed,
            )
        })
        .await;
        Ok(())
    }
}
