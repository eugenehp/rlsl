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

/// Seconds of its own output an irregular stream keeps by default for late consumers
/// ([`StreamOutlet::set_history`]).
pub const DEFAULT_MARKER_HISTORY_SECS: f64 = 10.0;
/// The most samples an irregular stream's history holds.
const MARKER_HISTORY_MAX_SAMPLES: usize = 4096;

/// A stream outlet. Creates a discoverable stream on the network.
pub struct StreamOutlet {
    info: StreamInfo,
    send_buffer: Arc<SendBuffer>,
    shutdown: Arc<AtomicBool>,
    #[allow(dead_code)]
    chunk_size: i32,
    /// Clock records for consumers that ask for them in-band.
    #[cfg(feature = "ntp")]
    clock: Arc<crate::ntp::feed::ClockHub>,
    /// The companion stream the clock records also go out on, once there is one.
    #[cfg(feature = "ntp")]
    clock_companion: parking_lot::Mutex<Option<Arc<StreamOutlet>>>,
}

impl StreamOutlet {
    /// Create a new stream outlet.
    ///
    /// `max_buffered` is the most the outlet holds for a consumer that falls behind: seconds of
    /// data at the stream's rate, or hundreds of samples for an irregular one (0 or less: as
    /// much as each consumer asks for). A consumer further behind than that loses its oldest
    /// samples ([`dropped_samples`](Self::dropped_samples)).
    pub fn new(info: &StreamInfo, chunk_size: i32, max_buffered: i32) -> Self {
        let send_buffer = SendBuffer::new();
        send_buffer.set_max_buffered(max_buffered);
        // An irregular stream — markers, events — keeps its last seconds for consumers that
        // connect late and ask for them (see `set_history`). Costs a little memory, and
        // changes nothing for a consumer that does not ask.
        if info.nominal_srate() <= 0.0 {
            send_buffer.set_history(DEFAULT_MARKER_HISTORY_SECS, MARKER_HISTORY_MAX_SAMPLES, 0.0);
        }

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
        #[cfg(feature = "ntp")]
        let clock = Arc::new(crate::ntp::feed::ClockHub::default());
        #[cfg(feature = "ntp")]
        let clock_source = Some(clock.clone());
        #[cfg(not(feature = "ntp"))]
        let clock_source = None;
        let tcp = TcpServer::start_with_clock(
            info.clone(),
            send_buffer.clone(),
            chunk_size,
            clock_source,
        );
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
            #[cfg(feature = "ntp")]
            clock,
            #[cfg(feature = "ntp")]
            clock_companion: parking_lot::Mutex::new(None),
        }
    }

    /// Pass on the measurements of `mesh` — this machine's clock against the others' — with
    /// this stream, so that whoever records it can correct its time stamps afterwards
    /// (feature `ntp`). `transport` says how:
    ///
    /// * in-band: inside this stream's own connection, to rlsl inlets that ask
    ///   ([`StreamInlet::set_clock_records`](crate::inlet::StreamInlet::set_clock_records));
    ///   a consumer that connects later gets the measurements from before too;
    /// * companion: as a second stream, of type
    ///   [`CLOCK_STREAM_TYPE`](crate::ntp::CLOCK_STREAM_TYPE) and source id
    ///   [`clock_source_id`](crate::ntp::clock_source_id), which any LSL recorder can record.
    ///
    /// The measurements already made are passed on as well. Liblsl consumers see no
    /// difference in the stream itself.
    #[cfg(feature = "ntp")]
    pub fn attach_clock_mesh(
        &self,
        mesh: &crate::ntp::ClockMesh,
        transport: crate::ntp::ClockTransport,
    ) {
        let hub = transport.in_band.then(|| Arc::downgrade(&self.clock));
        let companion = transport.companion.then(|| {
            let mut slot = self.clock_companion.lock();
            let outlet = slot.get_or_insert_with(|| {
                let info = crate::ntp::clock_stream_info(&self.info, mesh.node_id());
                let outlet = StreamOutlet::new(&info, 0, 360);
                // the measurements of the last hour for a recorder that starts late
                outlet.set_history(3600.0);
                Arc::new(outlet)
            });
            Arc::downgrade(outlet)
        });
        mesh.subscribe(move |record| {
            let mut alive = false;
            if let Some(hub) = hub.as_ref().and_then(|h| h.upgrade()) {
                hub.push(record.clone());
                alive = true;
            }
            if let Some(outlet) = companion.as_ref().and_then(|c| c.upgrade()) {
                outlet.push_sample_str(&record.to_fields(), 0.0, true);
                alive = true;
            }
            alive
        });
    }

    /// The description of this stream's companion clock stream, once
    /// [`attach_clock_mesh`](Self::attach_clock_mesh) has made one.
    #[cfg(feature = "ntp")]
    pub fn clock_companion_info(&self) -> Option<StreamInfo> {
        self.clock_companion
            .lock()
            .as_ref()
            .map(|o| o.info().clone())
    }

    /// Clock records this outlet holds for consumers that ask for them in-band.
    #[cfg(feature = "ntp")]
    pub fn clock_records_held(&self) -> usize {
        self.clock.len()
    }

    /// Samples this outlet has had to drop because a consumer fell further behind than its
    /// buffer holds (`max_buflen` seconds of data, as the consumer asked for). Zero means every
    /// consumer has been sent every sample.
    pub fn dropped_samples(&self) -> u64 {
        self.send_buffer.dropped_samples()
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

    /// Push a single 8-bit integer sample
    pub fn push_sample_i8(&self, data: &[i8], timestamp: f64, pushthrough: bool) {
        self.push_sample_with(timestamp, pushthrough, |s| s.assign_i8(data));
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

    /// Push a chunk of multiplexed 8-bit integer data; see [`push_chunk_d`](Self::push_chunk_d).
    pub fn push_chunk_i8(&self, data: &[i8], timestamp: f64, pushthrough: bool) {
        self.push_chunk_with(data, timestamp, pushthrough, Self::push_sample_i8);
    }

    /// Push a chunk of multiplexed string data (`n_samples × channel_count` strings,
    /// row-major); see [`push_chunk_d`](Self::push_chunk_d).
    pub fn push_chunk_str(&self, data: &[String], timestamp: f64, pushthrough: bool) {
        self.push_chunk_with(data, timestamp, pushthrough, Self::push_sample_str);
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

    /// Keep what this outlet sends for `seconds`, for consumers that connect late: an rlsl
    /// inlet that asks ([`StreamInlet::set_replay`](crate::inlet::StreamInlet::set_replay))
    /// is then sent what it missed ahead of the live samples, each with its own time stamp.
    /// Without this a consumer gets only what is sent after it has connected, as in liblsl —
    /// and the first seconds of a stream that has just appeared, before anybody has found it,
    /// are lost to everybody.
    ///
    /// Irregular streams (rate 0: markers, events) keep their last
    /// [`DEFAULT_MARKER_HISTORY_SECS`] seconds by default, regular ones nothing; 0 turns it
    /// off. Nothing changes on the wire for consumers that do not ask, liblsl included.
    pub fn set_history(&self, seconds: f64) {
        let srate = self.info.nominal_srate();
        let max_samples = if srate > 0.0 {
            (seconds.max(0.0) * srate).ceil() as usize + 1
        } else {
            MARKER_HISTORY_MAX_SAMPLES
        };
        self.send_buffer.set_history(seconds, max_samples, srate);
    }

    /// The seconds of history this outlet keeps ([`set_history`](Self::set_history)).
    pub fn history(&self) -> f64 {
        self.send_buffer.history_seconds()
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
