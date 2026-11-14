//! StreamOutlet: publishes data on the lab network.

use crate::clock::local_clock;
use crate::sample::Sample;
use crate::send_buffer::SendBuffer;
use crate::stream_info::StreamInfo;
use crate::tcp_server::TcpServer;
use crate::types::*;
use crate::udp_server::UdpServer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A stream outlet. Creates a discoverable stream on the network.
pub struct StreamOutlet {
    info: StreamInfo,
    send_buffer: Arc<SendBuffer>,
    shutdown: Arc<AtomicBool>,
    #[allow(dead_code)]
    chunk_size: i32,
}

impl StreamOutlet {
    /// Create a new stream outlet.
    pub fn new(info: &StreamInfo, chunk_size: i32, _max_buffered: i32) -> Self {
        let send_buffer = SendBuffer::new();

        // Set up the stream identity
        info.reset_uid();
        info.set_created_at(local_clock());
        info.set_session_id(&crate::config::CONFIG.session_id);
        info.set_hostname(
            &hostname::get()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );

        // Start TCP data server (IPv4 + IPv6)
        let tcp = TcpServer::start(info.clone(), send_buffer.clone(), chunk_size);
        info.set_v4data_port(tcp.v4_port);
        info.set_v6data_port(tcp.v6_port);

        // Start UDP service (time sync, IPv4 + IPv6)
        let (v4_svc_port, v6_svc_port) =
            UdpServer::start_unicast(info.clone(), tcp.shutdown.clone());
        info.set_v4service_port(v4_svc_port);
        info.set_v6service_port(v6_svc_port);

        // Start multicast responders (IPv4 + IPv6 groups)
        UdpServer::start_multicast(info.clone(), tcp.shutdown.clone());
        let tcp_shutdown = tcp.shutdown;

        StreamOutlet {
            info: info.clone(),
            send_buffer,
            shutdown: tcp_shutdown,
            chunk_size,
        }
    }

    /// Get the stream info
    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// Build a sample of the stream's format, let `fill` put the values in, and queue it.
    /// A `timestamp` of 0 means now.
    fn push_sample_with(&self, timestamp: f64, pushthrough: bool, fill: impl FnOnce(&mut Sample)) {
        let ts = if timestamp == 0.0 {
            local_clock()
        } else {
            timestamp
        };
        let mut sample = Sample::new(self.info.channel_format(), self.info.channel_count(), ts);
        fill(&mut sample);
        sample.pushthrough = pushthrough;
        self.send_buffer.push_sample(sample);
    }

    /// Push a single float sample
    pub fn push_sample_f(&self, data: &[f32], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_f32(data));
    }

    /// Push a single double sample
    pub fn push_sample_d(&self, data: &[f64], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_f64(data));
    }

    /// Push a single 32-bit integer sample
    pub fn push_sample_i32(&self, data: &[i32], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_i32(data));
    }

    /// Push a single 16-bit integer sample
    pub fn push_sample_i16(&self, data: &[i16], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_i16(data));
    }

    /// Push a single 64-bit integer sample
    pub fn push_sample_i64(&self, data: &[i64], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_i64(data));
    }

    /// Push a single string sample
    pub fn push_sample_str(&self, data: &[String], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_strings(data));
    }

    /// Push a single sample given as the raw bytes of the stream's format
    pub fn push_sample_raw(&self, data: &[u8], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_raw(data));
    }

    /// Push a chunk of multiplexed double data (`n_samples × channel_count` values, row-major).
    ///
    /// `timestamp` is the time of the chunk's *last* sample (0 = now); with a regular rate the
    /// earlier samples are spaced back from it, like [`push_chunk_f`](Self::push_chunk_f).
    pub fn push_chunk_d(&self, data: &[f64], timestamp: f64, pushthrough: bool) {
        self.push_chunk_with(data, timestamp, pushthrough, Self::push_sample_d);
    }

    /// Push a chunk of multiplexed 64-bit integer data; see [`push_chunk_d`](Self::push_chunk_d).
    pub fn push_chunk_i64(&self, data: &[i64], timestamp: f64, pushthrough: bool) {
        self.push_chunk_with(data, timestamp, pushthrough, Self::push_sample_i64);
    }

    /// Push a chunk of multiplexed 32-bit integer data; see [`push_chunk_d`](Self::push_chunk_d).
    pub fn push_chunk_i32(&self, data: &[i32], timestamp: f64, pushthrough: bool) {
        self.push_chunk_with(data, timestamp, pushthrough, Self::push_sample_i32);
    }

    /// Push a chunk of multiplexed 16-bit integer data; see [`push_chunk_d`](Self::push_chunk_d).
    pub fn push_chunk_i16(&self, data: &[i16], timestamp: f64, pushthrough: bool) {
        self.push_chunk_with(data, timestamp, pushthrough, Self::push_sample_i16);
    }

    /// The chunk logic of [`push_chunk_f`](Self::push_chunk_f) for any sample type: the first
    /// sample carries the chunk's start time, the rest a deduced one, and only the last is
    /// pushed through.
    fn push_chunk_with<T>(
        &self,
        data: &[T],
        timestamp: f64,
        pushthrough: bool,
        push_sample: fn(&Self, &[T], f64, bool),
    ) {
        let nch = self.info.channel_count() as usize;
        if nch == 0 {
            return;
        }
        let n_samples = data.len() / nch;
        let srate = self.info.nominal_srate();
        let mut ts = if timestamp == 0.0 {
            local_clock()
        } else {
            timestamp
        };
        if srate != IRREGULAR_RATE && n_samples > 1 {
            ts -= (n_samples - 1) as f64 / srate;
        }
        for (i, row) in data.chunks_exact(nch).enumerate() {
            let sample_ts = if i == 0 { ts } else { DEDUCED_TIMESTAMP };
            push_sample(self, row, sample_ts, pushthrough && i == n_samples - 1);
        }
    }

    /// Push a chunk of multiplexed float data; see [`push_chunk_d`](Self::push_chunk_d).
    pub fn push_chunk_f(&self, data: &[f32], timestamp: f64, pushthrough: bool) {
        self.push_chunk_with(data, timestamp, pushthrough, Self::push_sample_f);
    }

    /// Check if there are consumers
    pub fn have_consumers(&self) -> bool {
        self.send_buffer.have_consumers()
    }

    /// Wait for consumers
    pub fn wait_for_consumers(&self, timeout: f64) -> bool {
        self.send_buffer.wait_for_consumers(timeout)
    }
}

impl Drop for StreamOutlet {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.send_buffer.push_sentinel();
    }
}
