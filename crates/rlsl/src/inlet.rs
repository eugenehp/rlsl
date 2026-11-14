//! StreamInlet: receives data from the network.

use crate::sample::Sample;
use crate::stream_info::StreamInfo;
use crate::types::*;
use crossbeam_channel::{bounded, Receiver};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

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
    /// [`set_clock_sync`](Self::set_clock_sync): the inlet keeps the correction itself.
    auto_clock_sync: AtomicBool,
    samples_available: Arc<AtomicU32>,
    post_processing: Arc<AtomicU32>,
    postproc: Mutex<crate::postproc::TimestampPostProcessor>,
    /// the outlet's full info, once fetched
    fullinfo: Mutex<Option<StreamInfo>>,
}

impl StreamInlet {
    pub fn new(info: &StreamInfo, max_buflen: i32, max_chunklen: i32, recover: bool) -> Self {
        let (tx, rx) = bounded(max_buflen.max(1) as usize);
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
            auto_clock_sync: AtomicBool::new(false),
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

        // Start the data receiver in a dedicated thread with its own runtime.
        // When `recover` is true, automatically re-resolves and reconnects on loss.
        let connected2 = self.connected.clone();
        let recover = self.recover;
        let source_uid = info.uid();
        std::thread::Builder::new()
            .name("lsl_data_recv".to_string())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    let mut current_info = info.clone();
                    loop {
                        if shutdown.load(Ordering::Relaxed) {
                            break;
                        }
                        match Self::connect_and_receive(
                            &current_info,
                            &tx,
                            &connected,
                            &shutdown,
                            max_buflen,
                            max_chunklen,
                            &samples_avail,
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
                                    Ok(found) if !found.is_empty() => {
                                        current_info = found.into_iter().next().unwrap();
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
        max_buflen: i32,
        max_chunklen: i32,
        samples_avail: &Arc<AtomicU32>,
    ) -> Result<(), String> {
        // Try connecting: prefer IPv6 if available, fall back to IPv4
        let stream = Self::try_connect(info).await?;
        stream.set_nodelay(true).ok();

        let mut reader = BufReader::new(stream);

        // Protocol negotiation (1.10)
        let fmt = info.channel_format();
        let nch = info.channel_count();
        let proto_version = info.version().min(LSL_PROTOCOL_VERSION);

        if proto_version >= 110 {
            let request = format!(
                "LSL:streamfeed/{} {}\r\nNative-Byte-Order: 1234\r\nEndian-Performance: 0\r\nHas-IEEE754-Floats: 1\r\nSupports-Subnormals: 1\r\nValue-Size: {}\r\nData-Protocol-Version: {}\r\nMax-Buffer-Length: {}\r\nMax-Chunk-Length: {}\r\n\r\n",
                proto_version,
                info.uid(),
                fmt.channel_bytes(),
                proto_version,
                max_buflen,
                max_chunklen,
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
            }
        } else {
            let request = format!("LSL:streamfeed\r\n{} {}\r\n", max_buflen, max_chunklen);
            reader
                .get_mut()
                .write_all(request.as_bytes())
                .await
                .map_err(|e| e.to_string())?;
            reader.get_mut().flush().await.map_err(|e| e.to_string())?;
        }

        // Read and validate test patterns
        let use_proto_100 = proto_version < 110;
        for test_offset in [4, 2] {
            let received = if use_proto_100 {
                read_sample_async_100(&mut reader, fmt, nch).await?
            } else {
                read_sample_async(&mut reader, fmt, nch).await?
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

            let mut sample = if use_proto_100 {
                read_sample_async_100(&mut reader, fmt, nch).await?
            } else {
                read_sample_async(&mut reader, fmt, nch).await?
            };

            // Deduce timestamp if needed
            if sample.timestamp == DEDUCED_TIMESTAMP {
                sample.timestamp = last_timestamp;
                if srate != IRREGULAR_RATE {
                    sample.timestamp += 1.0 / srate;
                }
            }
            last_timestamp = sample.timestamp;

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
    pub fn time_correction(&self, timeout: f64) -> f64 {
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

    /// Let the inlet keep its clock offset itself: with this on and `PROC_CLOCKSYNC` set, time
    /// stamps come out on this machine's clock without any call to
    /// [`time_correction`](Self::time_correction) — the offset is estimated before the first
    /// sample is stamped and renewed in the background every `time_update_interval` (2 s).
    ///
    /// Off by default: `PROC_CLOCKSYNC` then adds whatever the last
    /// [`time_correction`](Self::time_correction) / [`clock_offset`](Self::clock_offset) call
    /// stored, as it always has.
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

/// Read a sample from an async reader using protocol 1.10
async fn read_sample_async(
    reader: &mut BufReader<TcpStream>,
    fmt: ChannelFormat,
    num_channels: u32,
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
                        u32::from_le_bytes(b) as usize
                    }
                    8 => {
                        let mut b = [0u8; 8];
                        reader.read_exact(&mut b).await.map_err(|e| e.to_string())?;
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

/// Read a sample from an async reader using protocol 1.00 (always 8-byte timestamp, 4-byte string lengths).
async fn read_sample_async_100(
    reader: &mut BufReader<TcpStream>,
    fmt: ChannelFormat,
    num_channels: u32,
) -> Result<Sample, String> {
    use crate::sample::SampleData;

    // Protocol 1.00: always 8-byte timestamp
    let mut ts_bytes = [0u8; 8];
    reader
        .read_exact(&mut ts_bytes)
        .await
        .map_err(|e| e.to_string())?;
    let timestamp = f64::from_le_bytes(ts_bytes);

    let n = num_channels as usize;
    let data = match fmt {
        ChannelFormat::Float32 => {
            let mut raw = vec![0u8; n * 4];
            reader
                .read_exact(&mut raw)
                .await
                .map_err(|e| e.to_string())?;
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
                // Protocol 1.00: always 4-byte length
                let mut len_bytes = [0u8; 4];
                reader
                    .read_exact(&mut len_bytes)
                    .await
                    .map_err(|e| e.to_string())?;
                let len = u32::from_le_bytes(len_bytes) as usize;
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
