//! StreamInlet: receives data from the network.

use crate::sample::Sample;
use crate::stream_info::StreamInfo;
use crate::types::*;
use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// The decoder of a compressed feed. Without the `compression` feature there is none to make,
/// and the inlet never asks for one.
#[cfg(feature = "compression")]
type FeedDecoder = crate::compress::StreamDecoder;
#[cfg(not(feature = "compression"))]
enum FeedDecoder {}
#[cfg(not(feature = "compression"))]
impl FeedDecoder {
    fn decode_payload(&mut self, _payload: &[u8], _sample_len: usize) -> Option<Vec<u8>> {
        match *self {}
    }
}

/// Whether this build can receive a compressed feed.
const FEED_COMPRESSION: bool = cfg!(feature = "compression");

#[cfg(feature = "compression")]
fn feed_decoder(value_bytes: usize) -> Option<FeedDecoder> {
    Some(FeedDecoder::new(value_bytes))
}
#[cfg(not(feature = "compression"))]
fn feed_decoder(_value_bytes: usize) -> Option<FeedDecoder> {
    None
}

/// Read one frame of a compressed feed and the sample in it.
async fn read_sample_compressed<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    decoder: &mut FeedDecoder,
    fmt: ChannelFormat,
    nch: u32,
) -> Result<Sample, String> {
    let mut header = [0u8; 4];
    reader
        .read_exact(&mut header)
        .await
        .map_err(|e| e.to_string())?;
    let payload_len = u16::from_le_bytes([header[0], header[1]]) as usize;
    let sample_len = u16::from_le_bytes([header[2], header[3]]) as usize;
    let mut payload = vec![0u8; payload_len];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|e| e.to_string())?;
    let raw = decoder
        .decode_payload(&payload, sample_len)
        .ok_or_else(|| "compressed stream is damaged".to_string())?;
    Sample::deserialize_110(&mut std::io::Cursor::new(raw), fmt, nch).map_err(|e| e.to_string())
}

/// What an inlet asks of the outlet's feed, and how much of it it keeps.
#[derive(Clone)]
struct FeedOptions {
    max_buflen: i32,
    max_chunklen: i32,
    request_compression: bool,
    /// Seconds of the stream's past to ask the outlet for (0: none).
    replay: f64,
    /// Where what the outlet grants of it is put.
    replay_granted: Arc<Mutex<f64>>,
    /// Samples held for a slow reader before the inlet stops taking more.
    buffer_cap: usize,
}

/// A stream inlet. Receives data from a stream outlet.
pub struct StreamInlet {
    info: StreamInfo,
    max_buflen: i32,
    max_chunklen: i32,
    recover: bool,
    sample_rx: Receiver<Sample>,
    sample_tx: crossbeam_channel::Sender<Sample>,
    connected: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    time_correction: Arc<Mutex<f64>>,
    /// When the time correction was last estimated (for the clock-sync post-processing).
    time_correction_at: Arc<Mutex<Option<std::time::Instant>>>,
    /// A background re-estimate is under way.
    time_correction_refreshing: Arc<AtomicBool>,
    /// Samples the inlet holds for a slow reader before it stops taking more from the outlet:
    /// `max_buflen` seconds of data (hundreds of samples for an irregular stream).
    buffer_cap: usize,
    /// [`set_clock_sync`](Self::set_clock_sync): the inlet keeps the correction itself.
    auto_clock_sync: AtomicBool,
    /// [`set_compression`](Self::set_compression): ask the outlet for a compressed feed.
    request_compression: Arc<AtomicBool>,
    /// The outlet agreed and the feed is compressed.
    compressed: Arc<AtomicBool>,
    /// [`set_replay`](Self::set_replay): seconds of the past to ask for at the first connection.
    replay: Arc<Mutex<f64>>,
    /// What the outlet granted of it.
    replay_granted: Arc<Mutex<f64>>,
    samples_available: Arc<AtomicU32>,
    post_processing: Arc<AtomicU32>,
    postproc: Mutex<crate::postproc::TimestampPostProcessor>,
    /// the outlet's full info, once fetched
    fullinfo: Mutex<Option<StreamInfo>>,
}

impl StreamInlet {
    pub fn new(info: &StreamInfo, max_buflen: i32, max_chunklen: i32, recover: bool) -> Self {
        // Unbounded, with the limit enforced where the samples are put in (see `buffer_cap`):
        // a bounded queue of that many slots would be allocated in full up front.
        let (tx, rx) = crossbeam_channel::unbounded();
        let connected = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));
        let time_correction = Arc::new(Mutex::new(0.0f64));
        let samples_available = Arc::new(AtomicU32::new(0));
        let post_processing = Arc::new(AtomicU32::new(PROC_NONE));

        let srate = info.nominal_srate();
        let halftime = crate::config::CONFIG.smoothing_halftime;
        StreamInlet {
            info: info.clone(),
            max_buflen,
            max_chunklen,
            recover,
            sample_rx: rx,
            sample_tx: tx,
            connected: connected.clone(),
            shutdown: shutdown.clone(),
            time_correction,
            time_correction_at: Arc::new(Mutex::new(None)),
            time_correction_refreshing: Arc::new(AtomicBool::new(false)),
            buffer_cap: buffer_samples(max_buflen, srate),
            auto_clock_sync: AtomicBool::new(cfg!(feature = "liblsl-defaults")),
            request_compression: Arc::new(AtomicBool::new(
                crate::config::CONFIG.compression == "stream-zstd",
            )),
            compressed: Arc::new(AtomicBool::new(false)),
            replay: Arc::new(Mutex::new(crate::config::CONFIG.replay)),
            replay_granted: Arc::new(Mutex::new(0.0)),
            samples_available,
            post_processing: post_processing.clone(),
            postproc: Mutex::new(crate::postproc::TimestampPostProcessor::new(
                PROC_NONE, srate, halftime,
            )),
            fullinfo: Mutex::new(None),
        }
    }

    /// Open the data stream (connect to the outlet's TCP port).
    pub fn open_stream(&self, timeout: f64) -> Result<(), String> {
        if self.connected.load(Ordering::Relaxed) {
            return Ok(());
        }

        let info = self.info.clone();
        let tx = self.sample_tx.clone();
        let connected = self.connected.clone();
        let shutdown = self.shutdown.clone();
        let max_buflen = self.max_buflen;
        let max_chunklen = self.max_chunklen;
        let samples_avail = self.samples_available.clone();
        let request_compression = self.request_compression.clone();
        let replay = self.replay.clone();
        let replay_granted = self.replay_granted.clone();
        let compressed = self.compressed.clone();
        let buffer_cap = self.buffer_cap;

        // Start the data receiver in a dedicated thread with its own runtime.
        // When `recover` is true, automatically re-resolves and reconnects on loss.
        let connected2 = self.connected.clone();
        let recover = self.recover;
        let source_uid = info.uid();
        std::thread::Builder::new()
            .name("lsl_data_recv".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        // out of threads or descriptors: no data connection, not a crash
                        log::error!("inlet: no runtime for the data connection: {e}");
                        return;
                    }
                };
                rt.block_on(async move {
                    let mut current_info = info.clone();
                    loop {
                        if shutdown.load(Ordering::Relaxed) {
                            break;
                        }
                        // the past is asked for once: a reconnection that asked again would
                        // be sent samples it already has
                        let replay_now = std::mem::take(&mut *replay.lock());
                        match Self::connect_and_receive(
                            &current_info,
                            &tx,
                            &connected,
                            &shutdown,
                            &samples_avail,
                            &compressed,
                            FeedOptions {
                                max_buflen,
                                max_chunklen,
                                request_compression: request_compression.load(Ordering::Relaxed),
                                replay: replay_now,
                                replay_granted: replay_granted.clone(),
                                buffer_cap,
                            },
                        )
                        .await
                        {
                            Ok(()) => break,
                            Err(e) => {
                                connected.store(false, Ordering::SeqCst);
                                if shutdown.load(Ordering::Relaxed) {
                                    break;
                                }
                                if !recover {
                                    log::trace!("[inlet] Connection lost, recovery disabled");
                                    break;
                                }
                                log::trace!("[inlet] Connection lost: {}, re-resolving...", e);
                                // Try to re-resolve the stream by UID
                                tokio::time::sleep(Duration::from_millis(500)).await;
                                let uid = source_uid.clone();
                                match tokio::task::spawn_blocking(move || {
                                    crate::resolver::resolve_by_property("uid", &uid, 1, 3.0)
                                })
                                .await
                                {
                                    Ok(mut found) if !found.is_empty() => {
                                        current_info = found.swap_remove(0);
                                        log::trace!("[inlet] Re-resolved, reconnecting...");
                                    }
                                    _ => {
                                        log::trace!("[inlet] Could not re-resolve, will retry...");
                                        tokio::time::sleep(Duration::from_secs(1)).await;
                                    }
                                }
                            }
                        }
                    }
                });
            })
            .map_err(|e| e.to_string())?;

        // Wait for connection
        let deadline = std::time::Instant::now() + Duration::from_secs_f64(timeout.max(0.001));
        while !connected2.load(Ordering::SeqCst) {
            if std::time::Instant::now() >= deadline {
                return Err("open_stream timed out".to_string());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    async fn connect_and_receive(
        info: &StreamInfo,
        tx: &crossbeam_channel::Sender<Sample>,
        connected: &Arc<AtomicBool>,
        shutdown: &Arc<AtomicBool>,
        samples_avail: &Arc<AtomicU32>,
        compressed: &Arc<AtomicBool>,
        feed: FeedOptions,
    ) -> Result<(), String> {
        let FeedOptions {
            max_buflen,
            max_chunklen,
            request_compression,
            replay,
            replay_granted,
            buffer_cap,
        } = feed;
        // Try connecting: prefer IPv6 if available, fall back to IPv4
        let stream = Self::try_connect(info).await?;
        stream.set_nodelay(true).ok();

        let mut reader = BufReader::new(stream);

        // Protocol negotiation (1.10)
        let fmt = info.channel_format();
        let nch = info.channel_count();
        let proto_version = info.version().min(LSL_PROTOCOL_VERSION);

        // Whether the outlet sends in big-endian byte order (its choice, stated in its answer).
        let mut big_endian = false;
        // Which protocol the data is in: 1.00 when either end speaks no better.
        let mut data_version = proto_version;
        // The decoder of a compressed feed, once the outlet has agreed to one.
        let mut decoder: Option<FeedDecoder> = None;
        compressed.store(false, Ordering::SeqCst);

        if proto_version >= 110 {
            // An extra header only an rlsl outlet built with compression acts on; any other
            // outlet (liblsl included) skips headers it does not know and feeds plain data.
            let ask = if request_compression && FEED_COMPRESSION {
                "Rlsl-Compression: stream-zstd\r\n"
            } else {
                ""
            };
            // Likewise: only an rlsl outlet that keeps a history acts on this one.
            let ask_replay = if replay > 0.0 {
                format!("Rlsl-Replay: {replay}\r\n")
            } else {
                String::new()
            };
            let request = format!(
                "LSL:streamfeed/{} {}\r\nNative-Byte-Order: 1234\r\nEndian-Performance: 0\r\nHas-IEEE754-Floats: 1\r\nSupports-Subnormals: 1\r\nValue-Size: {}\r\nData-Protocol-Version: {}\r\nMax-Buffer-Length: {}\r\nMax-Chunk-Length: {}\r\n{}{}\r\n",
                proto_version,
                info.uid(),
                fmt.channel_bytes(),
                proto_version,
                max_buflen,
                max_chunklen,
                ask,
                ask_replay,
            );
            reader
                .get_mut()
                .write_all(request.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            reader.get_mut().flush().await.map_err(|e| e.to_string())?;

            // Read response line
            let mut response_line = String::new();
            reader
                .read_line(&mut response_line)
                .await
                .map_err(|e| e.to_string())?;
            if !response_line.contains("200") {
                return Err(format!("Server error: {}", response_line.trim()));
            }

            // Read response headers
            loop {
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| e.to_string())?;
                if line.trim().is_empty() {
                    break;
                }
                // the outlet says so when — and only when — it will compress
                if let Some((key, value)) = line.split_once(':') {
                    if key.trim().eq_ignore_ascii_case("rlsl-replay") {
                        *replay_granted.lock() = value.trim().parse().unwrap_or(0.0);
                    }
                    if key.trim().eq_ignore_ascii_case("data-protocol-version") {
                        data_version = value.trim().parse().unwrap_or(data_version);
                    }
                    if key.trim().eq_ignore_ascii_case("byte-order") {
                        big_endian = value.trim() == "4321";
                    }
                    if !ask.is_empty()
                        && key.trim().eq_ignore_ascii_case("rlsl-compression")
                        && value.trim().eq_ignore_ascii_case("stream-zstd")
                    {
                        decoder = feed_decoder(fmt.channel_bytes());
                    }
                }
            }
            compressed.store(decoder.is_some(), Ordering::SeqCst);
        } else {
            let request = format!("LSL:streamfeed\r\n{} {}\r\n", max_buflen, max_chunklen);
            reader
                .get_mut()
                .write_all(request.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            reader.get_mut().flush().await.map_err(|e| e.to_string())?;
            data_version = 100;
        }

        // Read and validate test patterns
        let use_proto_100 = data_version < 110;
        if use_proto_100 {
            // The answer is one portable archive: its header (magic byte, version), the
            // stream's short description, and the class information of the first sample.
            let mut magic = [0u8; 1];
            reader
                .read_exact(&mut magic)
                .await
                .map_err(|e| e.to_string())?;
            if magic[0] != crate::sample::portable::HEADER[0] {
                return Err("not a protocol 1.00 feed".to_string());
            }
            let _archive_version = read_portable_int(&mut reader).await?;
            let described = read_portable_string(&mut reader).await?;
            // the wrong stream at a reused address: look for ours again
            if !described.contains(&info.uid()) {
                return Err("the outlet at this address serves another stream".to_string());
            }
            let mut class_info = [0u8; 2];
            reader
                .read_exact(&mut class_info)
                .await
                .map_err(|e| e.to_string())?;
        }
        for test_offset in [4, 2] {
            let received = if let Some(dec) = decoder.as_mut() {
                read_sample_compressed(&mut reader, dec, fmt, nch).await?
            } else if use_proto_100 {
                read_sample_async_100(&mut reader, fmt, nch).await?
            } else {
                read_sample_async(&mut reader, fmt, nch, big_endian).await?
            };
            let mut expected = Sample::new(fmt, nch, 0.0);
            expected.assign_test_pattern(test_offset);
            if received != expected {
                return Err("Test pattern mismatch".to_string());
            }
        }

        connected.store(true, Ordering::SeqCst);

        // Receive loop
        let srate = info.nominal_srate();
        let mut last_timestamp = 0.0f64;

        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            let mut sample = if let Some(dec) = decoder.as_mut() {
                read_sample_compressed(&mut reader, dec, fmt, nch).await?
            } else if use_proto_100 {
                read_sample_async_100(&mut reader, fmt, nch).await?
            } else {
                read_sample_async(&mut reader, fmt, nch, big_endian).await?
            };

            // Deduce timestamp if needed
            if sample.timestamp == DEDUCED_TIMESTAMP {
                sample.timestamp = last_timestamp;
                if srate != IRREGULAR_RATE {
                    sample.timestamp += 1.0 / srate;
                }
            }
            last_timestamp = sample.timestamp;

            // A reader that has not taken `buffer_cap` samples yet: wait for it rather than
            // pile up more (the outlet then buffers on its side, and drops its oldest when that
            // is full too).
            while tx.len() >= buffer_cap {
                if shutdown.load(Ordering::Relaxed) {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            samples_avail.fetch_add(1, Ordering::Relaxed);
            if tx.send(sample).is_err() {
                break;
            }
        }

        Ok(())
    }

    /// The addresses to reach the outlet's data port at, most promising first: an address the
    /// resolver actually saw the outlet at before a loopback guess, IPv4 before IPv6 (a
    /// link-local IPv6 address is only usable with its interface, which is not kept).
    fn data_addresses(info: &StreamInfo) -> Vec<String> {
        let (v4, v6) = (info.v4address(), info.v6address());
        let (v4_port, v6_port) = (info.v4data_port(), info.v6data_port());
        let mut out = Vec::new();
        if !v4.is_empty() && v4_port > 0 {
            out.push(format!("{v4}:{v4_port}"));
        }
        if !v6.is_empty() && v6_port > 0 {
            out.push(format!("[{v6}]:{v6_port}"));
        }
        // same machine
        if v4.is_empty() && v4_port > 0 {
            out.push(format!("127.0.0.1:{v4_port}"));
        }
        if v6.is_empty() && v6_port > 0 {
            out.push(format!("[::1]:{v6_port}"));
        }
        out
    }

    /// Connect to the outlet's data port, trying [`data_addresses`](Self::data_addresses) in
    /// order.
    async fn try_connect(info: &StreamInfo) -> Result<TcpStream, String> {
        let mut last_error = "the stream has no data port".to_string();
        for addr in Self::data_addresses(info) {
            log::trace!("[inlet] Connecting {}...", addr);
            match TcpStream::connect(&addr).await {
                Ok(stream) => {
                    log::trace!("[inlet] Connected via {}", addr);
                    return Ok(stream);
                }
                Err(e) => {
                    log::trace!("[inlet] Connect to {} failed: {}", addr, e);
                    last_error = e.to_string();
                }
            }
        }
        Err(last_error)
    }

    /// Pull one sample and hand it to `take`; the time stamp, or 0 on timeout.
    fn pull_sample_with(&self, timeout: f64, take: impl FnOnce(&Sample)) -> Result<f64, String> {
        match self.pull_sample_raw(timeout)? {
            Some(s) => {
                take(&s);
                self.samples_available.fetch_sub(1, Ordering::Relaxed);
                Ok(self.postprocess_timestamp(s.timestamp))
            }
            None => Ok(0.0),
        }
    }

    /// Pull a single float sample. Returns the timestamp, or 0 on timeout.
    pub fn pull_sample_f(&self, buffer: &mut [f32], timeout: f64) -> Result<f64, String> {
        self.pull_sample_with(timeout, |s| s.retrieve_f32(buffer))
    }

    /// Pull a single double sample. Returns the timestamp, or 0 on timeout.
    pub fn pull_sample_d(&self, buffer: &mut [f64], timeout: f64) -> Result<f64, String> {
        self.pull_sample_with(timeout, |s| s.retrieve_f64(buffer))
    }

    /// Pull a single 32-bit integer sample. Returns the timestamp, or 0 on timeout.
    pub fn pull_sample_i32(&self, buffer: &mut [i32], timeout: f64) -> Result<f64, String> {
        self.pull_sample_with(timeout, |s| s.retrieve_i32(buffer))
    }

    /// Pull a single 16-bit integer sample. Returns the timestamp, or 0 on timeout.
    pub fn pull_sample_i16(&self, buffer: &mut [i16], timeout: f64) -> Result<f64, String> {
        self.pull_sample_with(timeout, |s| s.retrieve_i16(buffer))
    }

    /// Pull a single 8-bit integer sample. Returns the timestamp, or 0 on timeout.
    pub fn pull_sample_i8(&self, buffer: &mut [i8], timeout: f64) -> Result<f64, String> {
        self.pull_sample_with(timeout, |s| s.retrieve_i8(buffer))
    }

    /// Pull a single 64-bit integer sample. Returns the timestamp, or 0 on timeout.
    pub fn pull_sample_i64(&self, buffer: &mut [i64], timeout: f64) -> Result<f64, String> {
        self.pull_sample_with(timeout, |s| s.retrieve_i64(buffer))
    }

    pub fn pull_sample_str(&self, timeout: f64) -> Result<(Vec<String>, f64), String> {
        let sample = self.pull_sample_raw(timeout)?;
        match sample {
            Some(s) => {
                let strings = s.retrieve_strings();
                self.samples_available.fetch_sub(1, Ordering::Relaxed);
                Ok((strings, self.postprocess_timestamp(s.timestamp)))
            }
            None => Ok((Vec::new(), 0.0)),
        }
    }

    fn pull_sample_raw(&self, timeout: f64) -> Result<Option<Sample>, String> {
        if timeout <= 0.0 {
            match self.sample_rx.try_recv() {
                Ok(s) => Ok(Some(s)),
                Err(_) => Ok(None),
            }
        } else if timeout >= FOREVER {
            match self.sample_rx.recv() {
                Ok(s) => Ok(Some(s)),
                Err(_) => Err("channel closed".to_string()),
            }
        } else {
            match self
                .sample_rx
                .recv_timeout(Duration::from_secs_f64(timeout))
            {
                Ok(s) => Ok(Some(s)),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => Ok(None),
                Err(_) => Err("channel closed".to_string()),
            }
        }
    }

    fn postprocess_timestamp(&self, ts: f64) -> f64 {
        let flags = self.post_processing.load(Ordering::Relaxed);
        if flags == PROC_NONE {
            return ts;
        }
        if flags & PROC_CLOCKSYNC != 0 && self.auto_clock_sync.load(Ordering::Relaxed) {
            self.keep_time_correction_fresh();
        }
        let mut proc = self.postproc.lock();
        proc.set_clock_offset(*self.time_correction.lock());
        proc.process(ts)
    }

    /// For automatic clock synchronisation: make sure there is a correction and that it is not
    /// older than the configured update interval. The first estimate is waited for (the first
    /// samples would otherwise come out on the outlet's clock and the later ones on this
    /// machine's); after that the estimate is renewed in the background.
    fn keep_time_correction_fresh(&self) {
        let age = self.time_correction_at.lock().map(|t| t.elapsed());
        match age {
            None => {
                let _ = self.clock_offset(2.0);
                // tried: do not block every sample when the outlet does not answer
                self.time_correction_at
                    .lock()
                    .get_or_insert_with(std::time::Instant::now);
            }
            Some(age) if age.as_secs_f64() >= crate::config::CONFIG.time_update_interval => {
                if self.time_correction_refreshing.swap(true, Ordering::SeqCst) {
                    return;
                }
                let (host, port) = self.time_service_address();
                let (value, at, refreshing) = (
                    self.time_correction.clone(),
                    self.time_correction_at.clone(),
                    self.time_correction_refreshing.clone(),
                );
                let spawned = std::thread::Builder::new()
                    .name("lsl_time_sync".to_string())
                    .spawn(move || {
                        if let Some(offset) = crate::time_receiver::clock_offset(&host, port, 2.0) {
                            *value.lock() = offset;
                        }
                        *at.lock() = Some(std::time::Instant::now());
                        refreshing.store(false, Ordering::SeqCst);
                    });
                if spawned.is_err() {
                    self.time_correction_refreshing
                        .store(false, Ordering::SeqCst);
                }
            }
            Some(_) => {}
        }
    }

    pub fn close_stream(&self) {
        // The receiver thread will notice shutdown
    }

    /// Where the outlet answers time queries.
    fn time_service_address(&self) -> (String, u16) {
        let v4 = self.info.v4address();
        let host = if v4.is_empty() {
            "127.0.0.1".to_string()
        } else {
            v4
        };
        (host, self.info.v4service_port())
    }

    /// Estimate the clock offset between this machine and the outlet's machine.
    /// Uses NTP-like probing against the outlet's UDP service port.
    ///
    /// Returns the outlet's clock minus this machine's (0 when the outlet does not answer), and
    /// keeps that value as the offset the `PROC_CLOCKSYNC` post-processing adds — both exactly
    /// as in every release so far. Note the sign: it is the opposite of liblsl's
    /// `time_correction()`. For the liblsl convention use [`clock_offset`](Self::clock_offset);
    /// for clock-synchronised time stamps, [`set_clock_sync`](Self::set_clock_sync).
    ///
    /// With the cargo feature `liblsl-defaults` this is liblsl's `time_correction()` instead:
    /// [`clock_offset`](Self::clock_offset), or `0.0` when the outlet does not answer.
    pub fn time_correction(&self, timeout: f64) -> f64 {
        if cfg!(feature = "liblsl-defaults") {
            return self.clock_offset(timeout).unwrap_or(0.0);
        }
        let (host, port) = self.time_service_address();
        let offset = crate::time_receiver::time_correction(&host, port, timeout);
        *self.time_correction.lock() = offset;
        offset
    }

    /// The number of seconds to **add** to this stream's time stamps to get this machine's
    /// [`local_clock`](crate::clock::local_clock) time (this clock minus the outlet's) — what
    /// liblsl's `time_correction()` returns. `None` when the outlet does not answer within
    /// `timeout`. A successful estimate becomes the offset `PROC_CLOCKSYNC` adds.
    pub fn clock_offset(&self, timeout: f64) -> Option<f64> {
        let (host, port) = self.time_service_address();
        let offset = crate::time_receiver::clock_offset(&host, port, timeout)?;
        *self.time_correction.lock() = offset;
        *self.time_correction_at.lock() = Some(std::time::Instant::now());
        Some(offset)
    }

    /// Ask the outlet to compress the data it sends this inlet (before
    /// [`open_stream`](Self::open_stream)); on by default when the configuration says
    /// `compression = stream-zstd` (`LSL_COMPRESSION=stream-zstd`).
    ///
    /// It takes effect when both ends are rlsl built with the `compression` feature: each
    /// sample is then coded against the ones before it (see `rlsl::compress`), which on a real
    /// 32-channel EEG stream leaves about 70 % of the bytes on the network, with nothing held
    /// back. Any other outlet — an older rlsl, one built without the feature, liblsl — ignores
    /// the request and the stream is the plain protocol, as is every stream of an inlet that
    /// does not ask. [`is_compressed`](Self::is_compressed) tells which it is.
    pub fn set_compression(&self, on: bool) {
        self.request_compression.store(on, Ordering::Relaxed);
    }

    /// Whether the open stream is being received compressed.
    pub fn is_compressed(&self) -> bool {
        self.compressed.load(Ordering::Relaxed)
    }

    /// Ask the outlet, when the stream is opened, for what it sent in the `seconds` before:
    /// a consumer that connects late — a recorder that found a marker stream a second after it
    /// appeared — then gets the samples it missed, each with its own time stamp, ahead of the
    /// live ones. Call before [`open_stream`](Self::open_stream) / the first pull.
    ///
    /// It takes effect with an rlsl outlet that keeps a history
    /// ([`StreamOutlet::set_history`](crate::outlet::StreamOutlet::set_history); irregular
    /// streams do by default); any other outlet, liblsl included, ignores the request and the
    /// feed starts now. [`replay_granted`](Self::replay_granted) tells which. Asked once: a
    /// connection that is lost and recovered goes on from where the outlet is.
    ///
    /// Off by default (as in liblsl, a consumer gets what is sent after it connects), or
    /// `replay = <seconds>` in `lsl_api.cfg` / `LSL_REPLAY=<seconds>`.
    pub fn set_replay(&self, seconds: f64) {
        *self.replay.lock() = if seconds.is_finite() {
            seconds.max(0.0)
        } else {
            0.0
        };
    }

    /// The seconds of its past the outlet agreed to send first ([`set_replay`](Self::set_replay));
    /// 0 when it was not asked, cannot, or the stream is not open yet.
    pub fn replay_granted(&self) -> f64 {
        *self.replay_granted.lock()
    }

    /// Let the inlet keep its clock offset itself: with this on and `PROC_CLOCKSYNC` set, time
    /// stamps come out on this machine's clock without any call to
    /// [`time_correction`](Self::time_correction) — the offset is estimated before the first
    /// sample is stamped and renewed in the background every `time_update_interval` (2 s).
    ///
    /// Off by default: `PROC_CLOCKSYNC` then adds whatever the last
    /// [`time_correction`](Self::time_correction) / [`clock_offset`](Self::clock_offset) call
    /// stored, as it always has. (On by default, as in liblsl, with the cargo feature
    /// `liblsl-defaults`.)
    pub fn set_clock_sync(&self, automatic: bool) {
        self.auto_clock_sync.store(automatic, Ordering::Relaxed);
    }

    pub fn set_postprocessing(&self, flags: u32) {
        self.post_processing.store(flags, Ordering::Relaxed);
        let srate = self.info.nominal_srate();
        let halftime = crate::config::CONFIG.smoothing_halftime;
        *self.postproc.lock() =
            crate::postproc::TimestampPostProcessor::new(flags, srate, halftime);
    }

    /// Pull all available samples (up to `max_samples`) into a flat buffer.
    ///
    /// Returns `(timestamps, data)` where `data` is a flat vec of
    /// `n_samples × n_channels` f64 values in row-major order, and
    /// `timestamps` has one entry per sample.
    ///
    /// If no samples are available, returns empty vecs (never blocks when
    /// `timeout == 0.0`).
    pub fn pull_chunk_d(
        &self,
        max_samples: usize,
        timeout: f64,
    ) -> Result<(Vec<f64>, Vec<f64>), String> {
        self.pull_chunk_with(max_samples, timeout, 0f64, Sample::retrieve_f64)
    }

    /// [`pull_chunk_d`](Self::pull_chunk_d) for 64-bit integer data: `(timestamps, data)` with
    /// the values exact over the whole `i64` range (a double holds integers only up to 2^53).
    pub fn pull_chunk_i64(
        &self,
        max_samples: usize,
        timeout: f64,
    ) -> Result<(Vec<f64>, Vec<i64>), String> {
        self.pull_chunk_with(max_samples, timeout, 0i64, Sample::retrieve_i64)
    }

    /// [`pull_chunk_d`](Self::pull_chunk_d) for 32-bit integer data.
    pub fn pull_chunk_i32(
        &self,
        max_samples: usize,
        timeout: f64,
    ) -> Result<(Vec<f64>, Vec<i32>), String> {
        self.pull_chunk_with(max_samples, timeout, 0i32, Sample::retrieve_i32)
    }

    /// [`pull_chunk_d`](Self::pull_chunk_d) for single-precision data.
    pub fn pull_chunk_f(
        &self,
        max_samples: usize,
        timeout: f64,
    ) -> Result<(Vec<f64>, Vec<f32>), String> {
        self.pull_chunk_with(max_samples, timeout, 0f32, Sample::retrieve_f32)
    }

    /// [`pull_chunk_d`](Self::pull_chunk_d) for 16-bit integer data.
    pub fn pull_chunk_i16(
        &self,
        max_samples: usize,
        timeout: f64,
    ) -> Result<(Vec<f64>, Vec<i16>), String> {
        self.pull_chunk_with(max_samples, timeout, 0i16, Sample::retrieve_i16)
    }

    /// [`pull_chunk_d`](Self::pull_chunk_d) for 8-bit integer data.
    pub fn pull_chunk_i8(
        &self,
        max_samples: usize,
        timeout: f64,
    ) -> Result<(Vec<f64>, Vec<i8>), String> {
        self.pull_chunk_with(max_samples, timeout, 0i8, Sample::retrieve_i8)
    }

    /// [`pull_chunk_d`](Self::pull_chunk_d) for string data: `(timestamps, data)` with
    /// `n_samples × channel_count` strings, row-major.
    pub fn pull_chunk_str(
        &self,
        max_samples: usize,
        timeout: f64,
    ) -> Result<(Vec<f64>, Vec<String>), String> {
        let mut timestamps = Vec::new();
        let mut data = Vec::new();
        for i in 0..max_samples {
            let Some(s) = self.pull_sample_raw(if i == 0 { timeout } else { 0.0 })? else {
                break;
            };
            self.samples_available.fetch_sub(1, Ordering::Relaxed);
            timestamps.push(self.postprocess_timestamp(s.timestamp));
            data.extend(s.retrieve_strings());
        }
        Ok((timestamps, data))
    }

    /// The chunk logic of [`pull_chunk_d`](Self::pull_chunk_d) for any sample type: the first
    /// sample may block up to `timeout`, the rest are taken only if already there.
    fn pull_chunk_with<T: Copy>(
        &self,
        max_samples: usize,
        timeout: f64,
        zero: T,
        retrieve: fn(&Sample, &mut [T]),
    ) -> Result<(Vec<f64>, Vec<T>), String> {
        let n_ch = self.info.channel_count() as usize;
        let mut timestamps = Vec::with_capacity(max_samples);
        let mut data = Vec::with_capacity(max_samples * n_ch);
        let mut buf = vec![zero; n_ch];
        for i in 0..max_samples {
            let Some(s) = self.pull_sample_raw(if i == 0 { timeout } else { 0.0 })? else {
                break;
            };
            retrieve(&s, &mut buf);
            self.samples_available.fetch_sub(1, Ordering::Relaxed);
            timestamps.push(self.postprocess_timestamp(s.timestamp));
            data.extend_from_slice(&buf);
        }
        Ok((timestamps, data))
    }

    pub fn samples_available(&self) -> u32 {
        self.sample_rx.len() as u32
    }

    pub fn flush(&self) -> u32 {
        let mut count = 0u32;
        while self.sample_rx.try_recv().is_ok() {
            count += 1;
        }
        count
    }

    pub fn was_clock_reset(&self) -> bool {
        false
    }

    pub fn smoothing_halftime(&self, value: f32) {
        let flags = self.post_processing.load(Ordering::Relaxed);
        let srate = self.info.nominal_srate();
        *self.postproc.lock() = crate::postproc::TimestampPostProcessor::new(flags, srate, value);
    }

    /// The stream info this inlet was created from (the resolver's short info: no
    /// description).  Cheap, no network.
    pub fn info(&self) -> StreamInfo {
        self.info.clone()
    }

    /// `lsl_get_fullinfo`: the outlet's full info, description included, fetched once with an
    /// `LSL:fullinfo` request (`timeout` in seconds, ≤ 0 = up to 10 s) and then cached.  If the
    /// outlet cannot be reached the short info is returned (and the fetch retried next time).
    pub fn get_fullinfo(&self, timeout: f64) -> StreamInfo {
        if let Some(f) = self.fullinfo.lock().as_ref() {
            return f.clone();
        }
        let t = if timeout > 0.0 {
            Duration::from_secs_f64(timeout)
        } else {
            Duration::from_secs(10)
        };
        match Self::fetch_fullinfo(&self.info, t) {
            Some(full) => {
                // keep the addresses the resolver saw (the outlet may not know its own)
                if full.v4address().is_empty() {
                    full.set_v4address(&self.info.v4address());
                }
                if full.v6address().is_empty() {
                    full.set_v6address(&self.info.v6address());
                }
                if full.hostname().is_empty() {
                    full.set_hostname(&self.info.hostname());
                }
                *self.fullinfo.lock() = Some(full.clone());
                full
            }
            None => self.info.clone(),
        }
    }

    /// One `LSL:fullinfo` round trip, at the addresses the data connection uses.
    fn fetch_fullinfo(info: &StreamInfo, timeout: Duration) -> Option<StreamInfo> {
        use std::io::{Read, Write};
        let deadline = std::time::Instant::now() + timeout;
        let addrs = Self::data_addresses(info);
        for a in addrs {
            let Ok(sa) = a.parse::<std::net::SocketAddr>() else {
                continue;
            };
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            let Ok(mut c) = std::net::TcpStream::connect_timeout(&sa, left) else {
                continue;
            };
            let _ = c.set_read_timeout(Some(left));
            if c.write_all(b"LSL:fullinfo\r\n").is_err() {
                continue;
            }
            let mut xml = Vec::new();
            let mut buf = [0u8; 8192];
            while std::time::Instant::now() < deadline {
                match c.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        xml.extend_from_slice(&buf[..n]);
                        if xml.windows(7).any(|w| w == b"</info>") {
                            break;
                        }
                    }
                }
            }
            if let Some(full) = StreamInfo::from_fullinfo_message(&String::from_utf8_lossy(&xml)) {
                return Some(full);
            }
        }
        None
    }
}

impl Drop for StreamInlet {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

/// Read a sample from an async reader using protocol 1.10. `big_endian`: the outlet said it
/// sends in big-endian byte order (`Byte-Order: 4321`); every multi-byte number is then turned
/// round as it is read.
async fn read_sample_async<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    fmt: ChannelFormat,
    num_channels: u32,
    big_endian: bool,
) -> Result<Sample, String> {
    use crate::sample::SampleData;

    let mut tag = [0u8; 1];
    reader
        .read_exact(&mut tag)
        .await
        .map_err(|e| e.to_string())?;

    let timestamp = if tag[0] == TAG_DEDUCED_TIMESTAMP {
        DEDUCED_TIMESTAMP
    } else {
        let mut ts_bytes = [0u8; 8];
        reader
            .read_exact(&mut ts_bytes)
            .await
            .map_err(|e| e.to_string())?;
        if big_endian {
            ts_bytes.reverse();
        }
        f64::from_le_bytes(ts_bytes)
    };

    let n = num_channels as usize;
    let data = match fmt {
        ChannelFormat::Float32 => {
            let mut raw = vec![0u8; n * 4];
            reader
                .read_exact(&mut raw)
                .await
                .map_err(|e| e.to_string())?;
            if big_endian {
                for value in raw.as_chunks_mut::<4>().0 {
                    value.reverse();
                }
            }
            SampleData::Float32(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect(),
            )
        }
        ChannelFormat::Double64 => {
            let mut raw = vec![0u8; n * 8];
            reader
                .read_exact(&mut raw)
                .await
                .map_err(|e| e.to_string())?;
            if big_endian {
                for value in raw.as_chunks_mut::<8>().0 {
                    value.reverse();
                }
            }
            SampleData::Double64(
                raw.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| f64::from_le_bytes(*c))
                    .collect(),
            )
        }
        ChannelFormat::Int32 => {
            let mut raw = vec![0u8; n * 4];
            reader
                .read_exact(&mut raw)
                .await
                .map_err(|e| e.to_string())?;
            if big_endian {
                for value in raw.as_chunks_mut::<4>().0 {
                    value.reverse();
                }
            }
            SampleData::Int32(
                raw.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| i32::from_le_bytes(*c))
                    .collect(),
            )
        }
        ChannelFormat::Int16 => {
            let mut raw = vec![0u8; n * 2];
            reader
                .read_exact(&mut raw)
                .await
                .map_err(|e| e.to_string())?;
            if big_endian {
                for value in raw.as_chunks_mut::<2>().0 {
                    value.reverse();
                }
            }
            SampleData::Int16(
                raw.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| i16::from_le_bytes(*c))
                    .collect(),
            )
        }
        ChannelFormat::Int8 => {
            let mut raw = vec![0u8; n];
            reader
                .read_exact(&mut raw)
                .await
                .map_err(|e| e.to_string())?;
            SampleData::Int8(raw.into_iter().map(|b| b as i8).collect())
        }
        ChannelFormat::Int64 => {
            let mut raw = vec![0u8; n * 8];
            reader
                .read_exact(&mut raw)
                .await
                .map_err(|e| e.to_string())?;
            if big_endian {
                for value in raw.as_chunks_mut::<8>().0 {
                    value.reverse();
                }
            }
            SampleData::Int64(
                raw.as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| i64::from_le_bytes(*c))
                    .collect(),
            )
        }
        ChannelFormat::String | ChannelFormat::Undefined => {
            let mut strings = Vec::with_capacity(n);
            for _ in 0..n {
                let mut lenbytes = [0u8; 1];
                reader
                    .read_exact(&mut lenbytes)
                    .await
                    .map_err(|e| e.to_string())?;
                let len: usize = match lenbytes[0] {
                    1 => {
                        let mut b = [0u8; 1];
                        reader.read_exact(&mut b).await.map_err(|e| e.to_string())?;
                        b[0] as usize
                    }
                    4 => {
                        let mut b = [0u8; 4];
                        reader.read_exact(&mut b).await.map_err(|e| e.to_string())?;
                        if big_endian {
                            b.reverse();
                        }
                        u32::from_le_bytes(b) as usize
                    }
                    8 => {
                        let mut b = [0u8; 8];
                        reader.read_exact(&mut b).await.map_err(|e| e.to_string())?;
                        if big_endian {
                            b.reverse();
                        }
                        u64::from_le_bytes(b) as usize
                    }
                    _ => return Err("invalid varlen int".to_string()),
                };
                let mut sbuf = vec![0u8; len];
                reader
                    .read_exact(&mut sbuf)
                    .await
                    .map_err(|e| e.to_string())?;
                strings.push(String::from_utf8_lossy(&sbuf).into_owned());
            }
            SampleData::StringData(strings)
        }
    };

    Ok(Sample {
        timestamp,
        pushthrough: true,
        data,
    })
}

/// One number of a protocol 1.00 feed (see [`crate::sample::portable`]).
async fn read_portable_int<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Result<i64, String> {
    let mut size = [0u8; 1];
    reader
        .read_exact(&mut size)
        .await
        .map_err(|e| e.to_string())?;
    let size = size[0] as i8;
    if size == 0 {
        return Ok(0);
    }
    let n = size.unsigned_abs() as usize;
    if n > 8 {
        return Err("a number of more than 8 bytes in the feed".to_string());
    }
    let mut bytes = if size < 0 { [0xffu8; 8] } else { [0u8; 8] };
    reader
        .read_exact(&mut bytes[..n])
        .await
        .map_err(|e| e.to_string())?;
    Ok(i64::from_le_bytes(bytes))
}

/// One string of a protocol 1.00 feed.
async fn read_portable_string<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<String, String> {
    let len = read_portable_int(reader).await? as u64;
    if len > 64 << 20 {
        return Err("an impossibly long string in the feed".to_string());
    }
    let mut bytes = vec![0u8; len as usize];
    reader
        .read_exact(&mut bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Read a sample from an async reader using protocol 1.00 (liblsl's portable archive).
async fn read_sample_async_100<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
    fmt: ChannelFormat,
    num_channels: u32,
) -> Result<Sample, String> {
    use crate::sample::SampleData;
    let tag = read_portable_int(reader).await?;
    let timestamp = if tag == i64::from(TAG_DEDUCED_TIMESTAMP) {
        DEDUCED_TIMESTAMP
    } else {
        f64::from_bits(read_portable_int(reader).await? as u64)
    };
    let n = num_channels as usize;
    let data = if fmt == ChannelFormat::String {
        let mut strings = Vec::with_capacity(n);
        for _ in 0..n {
            strings.push(read_portable_string(reader).await?);
        }
        SampleData::StringData(strings)
    } else {
        let mut ints = Vec::with_capacity(n);
        for _ in 0..n {
            ints.push(read_portable_int(reader).await?);
        }
        match fmt {
            ChannelFormat::Float32 => {
                SampleData::Float32(ints.into_iter().map(|b| f32::from_bits(b as u32)).collect())
            }
            ChannelFormat::Double64 => {
                SampleData::Double64(ints.into_iter().map(|b| f64::from_bits(b as u64)).collect())
            }
            ChannelFormat::Int8 => SampleData::Int8(ints.into_iter().map(|v| v as i8).collect()),
            ChannelFormat::Int16 => SampleData::Int16(ints.into_iter().map(|v| v as i16).collect()),
            ChannelFormat::Int32 => SampleData::Int32(ints.into_iter().map(|v| v as i32).collect()),
            _ => SampleData::Int64(ints),
        }
    };
    Ok(Sample {
        timestamp,
        pushthrough: true,
        data,
    })
}

#[cfg(test)]
mod byte_order_tests {
    use super::*;
    use crate::sample::SampleData;

    /// The bytes a big-endian outlet sends for `sample`: every multi-byte number of the
    /// little-endian form turned round.
    fn big_endian_bytes(sample: &Sample, value_bytes: usize) -> Vec<u8> {
        let mut le = Vec::new();
        sample.serialize_110(&mut le);
        let mut out = vec![le[0]];
        let mut at = 1;
        if le[0] != TAG_DEDUCED_TIMESTAMP {
            out.extend(le[1..9].iter().rev());
            at = 9;
        }
        if let SampleData::StringData(strings) = &sample.data {
            for s in strings {
                let width = le[at] as usize;
                out.push(le[at]);
                out.extend(le[at + 1..at + 1 + width].iter().rev());
                at += 1 + width;
                out.extend(&le[at..at + s.len()]);
                at += s.len();
            }
        } else {
            for value in le[at..].chunks(value_bytes) {
                out.extend(value.iter().rev());
            }
        }
        out
    }

    #[test]
    fn a_big_endian_feed_is_read_as_the_values_that_were_sent() {
        let cases = [
            (
                SampleData::Float32(vec![1.5, -2.25e-3, f32::MAX]),
                ChannelFormat::Float32,
            ),
            (
                SampleData::Double64(vec![1.0e-300, -7.0, 0.1]),
                ChannelFormat::Double64,
            ),
            (
                SampleData::Int32(vec![1, -2, i32::MIN]),
                ChannelFormat::Int32,
            ),
            (
                SampleData::Int16(vec![258, -2, i16::MAX]),
                ChannelFormat::Int16,
            ),
            (SampleData::Int8(vec![1, -2, 127]), ChannelFormat::Int8),
            (
                SampleData::Int64(vec![1, -2, (1 << 53) + 1]),
                ChannelFormat::Int64,
            ),
            (
                SampleData::StringData(vec!["a".into(), String::new(), "x".repeat(300)]),
                ChannelFormat::String,
            ),
        ];
        for (data, fmt) in cases {
            let mut sample = Sample::new(fmt, 3, 1234.5678);
            sample.data = data;
            let bytes = big_endian_bytes(&sample, fmt.channel_bytes().max(1));
            let got = crate::RUNTIME
                .block_on(read_sample_async(&mut &bytes[..], fmt, 3, true))
                .unwrap();
            assert_eq!(got, sample, "{fmt:?}");
            // and the same bytes read as little-endian are not it (except where there is
            // nothing to turn round)
            if fmt != ChannelFormat::Int8 {
                let wrong =
                    crate::RUNTIME.block_on(read_sample_async(&mut &bytes[..], fmt, 3, false));
                assert!(wrong.map_or(true, |w| w != sample), "{fmt:?}");
            }
        }
    }
}
