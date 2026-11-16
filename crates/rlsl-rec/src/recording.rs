#![allow(clippy::too_many_arguments)]
//! Recording engine — resolves LSL streams, pulls data, writes XDF or Parquet.
//!
//! All heavy operations are spawned as tokio tasks (blocking I/O via
//! `spawn_blocking`, periodic work via `tokio::spawn`).

use crate::parquet_writer::{ParquetRecordingWriter, ToF64};
use exg::xdf::{NumericSample, XdfWriter};
use rlsl::clock::local_clock;
use rlsl::inlet::StreamInlet;
use rlsl::stream_info::StreamInfo;
use rlsl::types::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

// ── Recording format ─────────────────────────────────────────────────

/// Output format for recordings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingFormat {
    Xdf,
    Parquet,
    /// One `.h5` file (cargo feature `hdf5`: a pure-Rust HDF5 writer, no libhdf5 needed).
    #[cfg(feature = "hdf5")]
    Hdf5,
}

impl RecordingFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Xdf => "XDF",
            Self::Parquet => "Parquet",
            #[cfg(feature = "hdf5")]
            Self::Hdf5 => "HDF5",
        }
    }
}

// ── Writer abstraction ───────────────────────────────────────────────

/// Unifies the XDF, Parquet and HDF5 writers behind a common interface.
enum Writer {
    Xdf(Arc<XdfWriter>),
    Parquet(Arc<ParquetRecordingWriter>),
    #[cfg(feature = "hdf5")]
    Hdf5(Arc<crate::hdf5_writer::Hdf5RecordingWriter>),
}

impl Writer {
    fn write_stream_header(
        &self,
        stream_id: u32,
        info: &StreamInfo,
        header_xml: &str,
    ) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_stream_header(stream_id, header_xml),
            Writer::Parquet(w) => w.write_stream_header(stream_id, info, header_xml),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(w) => w.write_stream_header(stream_id, info, header_xml),
        }
    }

    fn write_samples_numeric<T: NumericSample + ToF64>(
        &self,
        stream_id: u32,
        timestamps: &[f64],
        data: &[T],
        n_channels: u32,
    ) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_samples_numeric(stream_id, timestamps, data, n_channels),
            Writer::Parquet(w) => w.write_samples_numeric(stream_id, timestamps, data, n_channels),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(w) => w.write_samples_numeric(stream_id, timestamps, data, n_channels),
        }
    }

    /// String samples (markers): `data` is `n × n_channels`, row-major, every sample with its
    /// own time stamp.
    fn write_samples_string(
        &self,
        stream_id: u32,
        timestamps: &[f64],
        data: &[String],
        n_channels: u32,
    ) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_samples_string(stream_id, timestamps, data, n_channels),
            Writer::Parquet(w) => w.write_samples_string(stream_id, timestamps, data),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(w) => w.write_samples_string(stream_id, timestamps, data),
        }
    }

    fn write_clock_offset(
        &self,
        stream_id: u32,
        collection_time: f64,
        offset: f64,
    ) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_clock_offset(stream_id, collection_time, offset),
            Writer::Parquet(w) => w.write_clock_offset(stream_id, collection_time, offset),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(w) => w.write_clock_offset(stream_id, collection_time, offset),
        }
    }

    fn write_stream_footer_xdf(&self, stream_id: u32, footer_xml: &str) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_stream_footer(stream_id, footer_xml),
            Writer::Parquet(_) => Ok(()),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(_) => Ok(()),
        }
    }

    fn write_stream_footer_parquet(
        &self,
        stream_id: u32,
        first_ts: f64,
        last_ts: f64,
        sample_count: u64,
    ) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(_) => Ok(()),
            Writer::Parquet(w) => w.write_stream_footer(stream_id, first_ts, last_ts, sample_count),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(w) => w.write_stream_footer(stream_id, first_ts, last_ts, sample_count),
        }
    }

    fn write_boundary(&self) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_boundary(),
            Writer::Parquet(_) => Ok(()),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(_) => Ok(()),
        }
    }
}

impl Clone for Writer {
    fn clone(&self) -> Self {
        match self {
            Writer::Xdf(w) => Writer::Xdf(w.clone()),
            Writer::Parquet(w) => Writer::Parquet(w.clone()),
            #[cfg(feature = "hdf5")]
            Writer::Hdf5(w) => Writer::Hdf5(w.clone()),
        }
    }
}

// Send + Sync are required for tokio tasks.
// Both XdfWriter (Mutex<BufWriter<File>>) and ParquetRecordingWriter
// (Mutex<HashMap<…>>) are Send + Sync.
unsafe impl Send for Writer {}
unsafe impl Sync for Writer {}

// ── Clock measurements ───────────────────────────────────────────────

/// How a recording gets the clock measurements that go with its streams, for correcting time
/// stamps afterwards (feature `ntp`). Any combination may be on.
#[cfg(feature = "ntp")]
#[derive(Clone, Default)]
pub struct ClockSyncOptions {
    /// Ask each outlet for its clock records in-band, inside the stream's own connection. An
    /// rlsl outlet with a clock mesh attached sends them; they are recorded as a stream of type
    /// `ClockSync` next to the data stream (`<name> (clock)`, its `<desc><clock_sync>` naming
    /// the data stream).
    pub in_band: bool,
    /// Record each stream's companion clock stream with it, if its outlet has one.
    pub companions: bool,
    /// Measure the machines the streams come from with this mesh node, and write those
    /// measurements as the streams' clock offsets (what XDF readers apply) — checked against
    /// LSL's own per-stream probe, which is used instead when the two disagree (an outlet that
    /// time-stamps with its process clock rather than the machine's).
    pub mesh: Option<Arc<rlsl::ntp::ClockMesh>>,
}

#[cfg(feature = "ntp")]
impl ClockSyncOptions {
    /// From the environment: `RLSL_REC_CLOCK` lists the ways (`in-band`, `companion`, `mesh`,
    /// comma-separated); for `mesh`, `RLSL_REC_CLOCK_PEERS` names the machines to measure
    /// (`host[:port]`, comma-separated), `RLSL_REC_CLOCK_PORT` this node's port (default
    /// 12300) and `RLSL_REC_CLOCK_ID` its name (default `hostname:port`).
    pub fn from_env() -> Self {
        let ways = std::env::var("RLSL_REC_CLOCK").unwrap_or_default();
        let has = |w: &str| ways.split(',').any(|x| x.trim().eq_ignore_ascii_case(w));
        let mesh = has("mesh").then(|| {
            let peers = std::env::var("RLSL_REC_CLOCK_PEERS").unwrap_or_default();
            let port = std::env::var("RLSL_REC_CLOCK_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(rlsl::ntp::DEFAULT_MESH_PORT);
            rlsl::ntp::ClockMesh::start(rlsl::ntp::ClockMeshConfig {
                node_id: std::env::var("RLSL_REC_CLOCK_ID").unwrap_or_default(),
                bind: std::net::SocketAddr::from(([0, 0, 0, 0], port)),
                peers: peers
                    .split(',')
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(String::from)
                    .collect(),
                ..Default::default()
            })
            .map_err(|e| eprintln!("clock mesh on port {port}: {e}"))
            .ok()
            .map(Arc::new)
        });
        ClockSyncOptions {
            in_band: has("in-band") || has("inband"),
            companions: has("companion") || has("companions"),
            mesh: mesh.flatten(),
        }
    }
}

#[cfg(feature = "ntp")]
type ClockOptions = ClockSyncOptions;
#[cfg(not(feature = "ntp"))]
#[derive(Clone, Default)]
struct ClockOptions;

#[cfg(feature = "ntp")]
fn clock_options_from_env() -> ClockOptions {
    ClockSyncOptions::from_env()
}
#[cfg(not(feature = "ntp"))]
fn clock_options_from_env() -> ClockOptions {
    ClockOptions
}

/// The streams to record: those chosen and, when asked for, their companion clock streams.
#[cfg(feature = "ntp")]
fn with_companions(streams: &[StreamInfo], clock: &ClockOptions) -> Vec<StreamInfo> {
    let mut all = streams.to_vec();
    if !clock.companions {
        return all;
    }
    for s in streams {
        if s.type_() == rlsl::ntp::CLOCK_STREAM_TYPE {
            continue;
        }
        let base = if s.source_id().is_empty() {
            s.uid()
        } else {
            s.source_id()
        };
        let id = rlsl::ntp::clock_source_id(&base);
        for c in rlsl::resolver::resolve_by_property("source_id", &id, 1, 1.0) {
            if !all.iter().any(|x| x.uid() == c.uid()) {
                all.push(c);
            }
        }
    }
    all
}
#[cfg(not(feature = "ntp"))]
fn with_companions(streams: &[StreamInfo], _clock: &ClockOptions) -> Vec<StreamInfo> {
    streams.to_vec()
}

/// What to add to the stream's time stamps to get this machine's clock, by the mesh: the
/// machine the stream comes from, measured, as a line through the measurements — if LSL's own
/// probe of the stream (`lsl`) agrees to within a quarter second, which it does when the outlet
/// time-stamps with its machine's clock.
#[cfg(feature = "ntp")]
fn mesh_clock_offset(clock: &ClockOptions, source_ip: &str, lsl: Option<f64>) -> Option<f64> {
    let mesh = clock.mesh.as_ref()?;
    let ip: std::net::IpAddr = source_ip.parse().ok()?;
    let fit = mesh.mono_fit_to(ip)?;
    let now = rlsl::clock::system_clock();
    // the source's clock is `offset_at(now)` ahead of this machine's monotonic clock, which is
    // `system_clock - local_clock` ahead of this recording's clock
    let offset = -fit.offset_at(now) - (now - local_clock());
    (lsl? - offset).abs().lt(&0.25).then_some(offset)
}
#[cfg(not(feature = "ntp"))]
fn mesh_clock_offset(_clock: &ClockOptions, _source_ip: &str, _lsl: Option<f64>) -> Option<f64> {
    None
}

/// The clock records a stream brought in-band, recorded as a clock stream of their own.
struct InBandClock {
    stream_id: u32,
    started: bool,
    count: u64,
    first_ts: f64,
    last_ts: f64,
}

impl InBandClock {
    #[cfg(feature = "ntp")]
    fn record(
        &mut self,
        inlet: &StreamInlet,
        info: &StreamInfo,
        writer: &Writer,
    ) -> anyhow::Result<()> {
        let records = inlet.pull_clock_records();
        let Some(first) = records.first() else {
            return Ok(());
        };
        if !self.started {
            let clock_info = rlsl::ntp::clock_stream_info(info, &first.from);
            writer.write_stream_header(
                self.stream_id,
                &clock_info,
                &clock_info.to_fullinfo_message(),
            )?;
            self.started = true;
        }
        let now = local_clock();
        let stamps = vec![now; records.len()];
        let fields: Vec<String> = records.iter().flat_map(|r| r.to_fields()).collect();
        let n = rlsl::ntp::ClockRecord::FIELDS.len() as u32;
        writer.write_samples_string(self.stream_id, &stamps, &fields, n)?;
        if self.count == 0 {
            self.first_ts = now;
        }
        self.last_ts = now;
        self.count += records.len() as u64;
        Ok(())
    }

    #[cfg(not(feature = "ntp"))]
    fn record(&mut self, _: &StreamInlet, _: &StreamInfo, _: &Writer) -> anyhow::Result<()> {
        Ok(())
    }

    fn finish(&self, writer: &Writer) -> anyhow::Result<()> {
        if !self.started {
            return Ok(());
        }
        let footer = format!(
            "<?xml version=\"1.0\"?><info>\
             <first_timestamp>{}</first_timestamp>\
             <last_timestamp>{}</last_timestamp>\
             <sample_count>{}</sample_count>\
             </info>",
            self.first_ts, self.last_ts, self.count
        );
        writer.write_stream_footer_xdf(self.stream_id, &footer)?;
        writer.write_stream_footer_parquet(self.stream_id, self.first_ts, self.last_ts, self.count)
    }
}

// ── Shared recording state ──────────────────────────────────────────

/// Shared recording state visible to the TUI/GUI.
pub struct RecordingState {
    pub sample_count: AtomicU64,
    pub stream_count: AtomicU64,
    /// Streams whose connection is open: from then on every sample their outlet sends is
    /// recorded.
    pub streams_open: AtomicU64,
    pub running: AtomicBool,
}

/// A running recording session.
///
/// Recording tasks are spawned on the tokio runtime. A runtime must be
/// available (via `#[tokio::main]`, `Runtime::enter()`, etc.) when
/// calling `start_with_format`.
pub struct Recording {
    pub state: Arc<RecordingState>,
    pub filename: String,
    pub format: RecordingFormat,
    shutdown: Arc<AtomicBool>,
    tasks: Vec<JoinHandle<()>>,
    writer: Option<Writer>,
}

impl Recording {
    /// Start a recording to `filename` (XDF) for the given resolved streams.
    pub fn start(filename: &str, streams: &[StreamInfo]) -> anyhow::Result<Self> {
        Self::start_with_format(filename, streams, RecordingFormat::Xdf)
    }

    /// Start a recording in the given format.
    ///
    /// For XDF: `filename` is the `.xdf` path.
    /// For Parquet: `filename` is the output directory name.
    ///
    /// Requires a tokio runtime context.
    pub fn start_with_format(
        filename: &str,
        streams: &[StreamInfo],
        format: RecordingFormat,
    ) -> anyhow::Result<Self> {
        Self::start_with_parquet_options(
            filename,
            streams,
            format,
            crate::parquet_writer::ParquetOptions::from_env(),
        )
    }

    /// [`start_with_format`](Self::start_with_format), choosing how Parquet files are written
    /// (ignored for XDF).
    pub fn start_with_parquet_options(
        filename: &str,
        streams: &[StreamInfo],
        format: RecordingFormat,
        parquet: crate::parquet_writer::ParquetOptions,
    ) -> anyhow::Result<Self> {
        Self::start_inner(filename, streams, format, parquet, clock_options_from_env())
    }

    /// [`start_with_parquet_options`](Self::start_with_parquet_options), choosing how the
    /// clock measurements that go with the streams are recorded (feature `ntp`; without this,
    /// the `RLSL_REC_CLOCK` environment variable chooses — see
    /// [`ClockSyncOptions::from_env`]).
    #[cfg(feature = "ntp")]
    pub fn start_with_clock_sync(
        filename: &str,
        streams: &[StreamInfo],
        format: RecordingFormat,
        parquet: crate::parquet_writer::ParquetOptions,
        clock: ClockSyncOptions,
    ) -> anyhow::Result<Self> {
        Self::start_inner(filename, streams, format, parquet, clock)
    }

    fn start_inner(
        filename: &str,
        streams: &[StreamInfo],
        format: RecordingFormat,
        parquet: crate::parquet_writer::ParquetOptions,
        clock: ClockOptions,
    ) -> anyhow::Result<Self> {
        let streams = &with_companions(streams, &clock)[..];
        let writer = match format {
            RecordingFormat::Xdf => Writer::Xdf(Arc::new(XdfWriter::new(filename)?)),
            RecordingFormat::Parquet => Writer::Parquet(Arc::new(
                ParquetRecordingWriter::new_with_options(filename, parquet)?,
            )),
            #[cfg(feature = "hdf5")]
            RecordingFormat::Hdf5 => Writer::Hdf5(Arc::new(
                crate::hdf5_writer::Hdf5RecordingWriter::new(filename)?,
            )),
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RecordingState {
            sample_count: AtomicU64::new(0),
            stream_count: AtomicU64::new(0),
            streams_open: AtomicU64::new(0),
            running: AtomicBool::new(true),
        });

        let handle = tokio::runtime::Handle::try_current()
            .unwrap_or_else(|_| rlsl::RUNTIME.handle().clone());

        let mut tasks: Vec<JoinHandle<()>> = Vec::new();

        for (idx, info) in streams.iter().enumerate() {
            let sid = (idx + 1) as u32;
            // the clock records a stream brings in-band, if it does, are a stream of their own
            let clock_sid = (streams.len() + idx + 1) as u32;
            let clock = clock.clone();
            let info = info.clone();
            let writer = writer.clone();
            let shutdown = shutdown.clone();
            let st = state.clone();

            state.stream_count.fetch_add(1, Ordering::Relaxed);

            // Each stream recorder runs as a blocking task because
            // inlet.open_stream() and pull_sample_* are blocking calls.
            let task = handle.spawn_blocking(move || {
                if let Err(e) =
                    record_stream(sid, clock_sid, &info, &writer, &shutdown, &st, &clock)
                {
                    eprintln!("Stream {} error: {}", info.name(), e);
                }
            });
            tasks.push(task);
        }

        // Boundary chunk task — async (only sleeps + tiny write)
        {
            let bw = writer.clone();
            let bs = shutdown.clone();
            let task = handle.spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    if bs.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = bw.write_boundary();
                }
            });
            tasks.push(task);
        }

        Ok(Recording {
            state,
            filename: filename.to_string(),
            format,
            shutdown,
            tasks,
            writer: Some(writer),
        })
    }

    /// Signal all tasks to stop (non-blocking).
    pub fn signal_stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.state.running.store(false, Ordering::Relaxed);
    }

    /// Stop the recording: signal shutdown, await all tasks, finalize files.
    pub async fn stop(mut self) {
        self.signal_stop();

        // Await all recording + boundary tasks
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }

        match self.writer.take() {
            // Finalize Parquet (close files, write JSON sidecar)
            Some(Writer::Parquet(arc)) => {
                if let Ok(pw) = Arc::try_unwrap(arc) {
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Err(e) = pw.close() {
                            eprintln!("Error finalizing Parquet: {}", e);
                        }
                    })
                    .await;
                }
            }
            // An HDF5 file is only whole once it is closed.
            #[cfg(feature = "hdf5")]
            Some(Writer::Hdf5(h5)) => {
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = h5.close() {
                        eprintln!("Error finalizing HDF5: {}", e);
                    }
                })
                .await;
            }
            _ => {}
        }
    }

    /// Synchronous stop — blocks the current thread until done.
    /// Use when you don't have an async context (e.g. Drop-like cleanup).
    pub fn stop_sync(self) {
        let handle = tokio::runtime::Handle::try_current()
            .unwrap_or_else(|_| rlsl::RUNTIME.handle().clone());
        handle.block_on(self.stop());
    }

    pub fn file_size(&self) -> u64 {
        match &self.writer {
            Some(Writer::Xdf(_)) => std::fs::metadata(&self.filename)
                .map(|m| m.len())
                .unwrap_or(0),
            #[cfg(feature = "hdf5")]
            Some(Writer::Hdf5(_)) => std::fs::metadata(&self.filename)
                .map(|m| m.len())
                .unwrap_or(0),
            Some(Writer::Parquet(_)) => std::fs::read_dir(&self.filename)
                .map(|entries| {
                    entries
                        .filter_map(|e| e.ok())
                        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
                        .sum()
                })
                .unwrap_or(0),
            None => 0,
        }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.state.running.store(false, Ordering::Relaxed);
        // Abort any still-running tasks so they don't leak
        for task in &self.tasks {
            task.abort();
        }
    }
}

// ── Stream recording loop (runs inside spawn_blocking) ──────────────

fn record_stream(
    stream_id: u32,
    clock_stream_id: u32,
    info: &StreamInfo,
    writer: &Writer,
    shutdown: &Arc<AtomicBool>,
    state: &Arc<RecordingState>,
    clock: &ClockOptions,
) -> anyhow::Result<()> {
    let inlet = StreamInlet::new(info, 360, 0, true);
    // the address the stream was found at: the machine whose clock its time stamps are on
    let source_ip = info.v4address();
    let mut in_band = InBandClock {
        stream_id: clock_stream_id,
        started: false,
        count: 0,
        first_ts: 0.0,
        last_ts: 0.0,
    };
    #[cfg(feature = "ntp")]
    {
        if info.type_() == rlsl::ntp::CLOCK_STREAM_TYPE {
            // a clock stream keeps its last hour for a recorder that starts late
            inlet.set_replay(3600.0);
        } else if clock.in_band {
            inlet.set_clock_records(true);
        }
    }
    inlet
        .open_stream(10.0)
        .map_err(|e| anyhow::anyhow!("open_stream: {}", e))?;

    // The stream as its outlet describes it: a resolved stream carries no description, and
    // with it the channel labels and everything else an outlet says about itself would be
    // missing from the recording.
    state.streams_open.fetch_add(1, Ordering::Relaxed);
    let full = inlet.get_fullinfo(5.0);
    let info = &full;
    let header_xml = info.to_fullinfo_message();
    writer.write_stream_header(stream_id, info, &header_xml)?;

    let fmt = info.channel_format();
    let nch = info.channel_count() as usize;
    let srate = info.nominal_srate();

    let mut first_ts = 0.0f64;
    let mut last_ts = 0.0f64;
    let mut total_samples = 0u64;
    let mut last_offset_time = local_clock();
    // where the clock offsets written came from: the clock mesh, or LSL's probe of the stream
    let (mut offsets_from_mesh, mut offsets_from_probe) = (0u64, 0u64);

    loop {
        // once more after being told to stop: what has arrived by then is recorded too
        let stopping = shutdown.load(Ordering::Relaxed);
        match fmt {
            ChannelFormat::Float32 => {
                pull_chunk_typed::<f32>(
                    &inlet,
                    nch,
                    srate,
                    stream_id,
                    writer,
                    state,
                    &mut first_ts,
                    &mut last_ts,
                    &mut total_samples,
                )?;
            }
            ChannelFormat::Double64 => {
                pull_chunk_typed::<f64>(
                    &inlet,
                    nch,
                    srate,
                    stream_id,
                    writer,
                    state,
                    &mut first_ts,
                    &mut last_ts,
                    &mut total_samples,
                )?;
            }
            ChannelFormat::Int32 => {
                pull_chunk_typed::<i32>(
                    &inlet,
                    nch,
                    srate,
                    stream_id,
                    writer,
                    state,
                    &mut first_ts,
                    &mut last_ts,
                    &mut total_samples,
                )?;
            }
            ChannelFormat::Int16 => {
                pull_chunk_typed::<i16>(
                    &inlet,
                    nch,
                    srate,
                    stream_id,
                    writer,
                    state,
                    &mut first_ts,
                    &mut last_ts,
                    &mut total_samples,
                )?;
            }
            ChannelFormat::Int8 => {
                pull_chunk_typed::<i8>(
                    &inlet,
                    nch,
                    srate,
                    stream_id,
                    writer,
                    state,
                    &mut first_ts,
                    &mut last_ts,
                    &mut total_samples,
                )?;
            }
            ChannelFormat::Int64 => {
                pull_chunk_typed::<i64>(
                    &inlet,
                    nch,
                    srate,
                    stream_id,
                    writer,
                    state,
                    &mut first_ts,
                    &mut last_ts,
                    &mut total_samples,
                )?;
            }
            ChannelFormat::String => {
                // Markers and other text: every sample carries its own time stamp (there is
                // no rate to deduce one from). Up to 0.0.8 string streams were not recorded.
                let (stamps, texts) = inlet
                    .pull_chunk_str(4096, 0.0)
                    .map_err(|e| anyhow::anyhow!("pull: {e}"))?;
                if let (Some(first), Some(last)) = (stamps.first(), stamps.last()) {
                    if first_ts == 0.0 {
                        first_ts = *first;
                    }
                    last_ts = *last;
                    writer.write_samples_string(stream_id, &stamps, &texts, nch as u32)?;
                    total_samples += stamps.len() as u64;
                    state
                        .sample_count
                        .fetch_add(stamps.len() as u64, Ordering::Relaxed);
                }
            }
            _ => {
                std::thread::sleep(Duration::from_millis(500));
            }
        }

        if stopping {
            break;
        }

        // Periodic clock offset
        let now = local_clock();
        if now - last_offset_time > 5.0 {
            // The value to ADD to the stream's time stamps to get this machine's clock — what
            // an XDF ClockOffset chunk holds and what readers (pyxdf, EEGLAB) apply. Nothing is
            // written when the outlet does not answer: a made-up zero would be applied as a
            // measurement. (Up to 0.0.8 the opposite sign was written, and 0 on no answer.)
            let lsl = inlet.clock_offset(2.0);
            let mesh = mesh_clock_offset(clock, &source_ip, lsl);
            if let Some(offset) = mesh.or(lsl) {
                writer.write_clock_offset(stream_id, now, offset)?;
                if mesh.is_some() {
                    offsets_from_mesh += 1;
                } else {
                    offsets_from_probe += 1;
                }
            }
            last_offset_time = now;
        }
        in_band.record(&inlet, info, writer)?;

        // Sleep inside spawn_blocking is fine — it doesn't block the
        // async executor, only this blocking-pool thread.
        std::thread::sleep(Duration::from_millis(500));
    }

    // Stream footer
    let footer = format!(
        "<?xml version=\"1.0\"?><info>\
         <first_timestamp>{first_ts}</first_timestamp>\
         <last_timestamp>{last_ts}</last_timestamp>\
         <sample_count>{total_samples}</sample_count>\
         <clock_offsets><from_clock_mesh>{offsets_from_mesh}</from_clock_mesh>\
         <from_lsl_probe>{offsets_from_probe}</from_lsl_probe></clock_offsets>\
         </info>"
    );
    writer.write_stream_footer_xdf(stream_id, &footer)?;
    writer.write_stream_footer_parquet(stream_id, first_ts, last_ts, total_samples)?;
    in_band.record(&inlet, info, writer)?;
    state.streams_open.fetch_sub(1, Ordering::Relaxed);
    in_band.finish(writer)?;

    Ok(())
}

/// Pull available samples from an inlet and write them.
fn pull_chunk_typed<T>(
    inlet: &StreamInlet,
    nch: usize,
    srate: f64,
    stream_id: u32,
    writer: &Writer,
    state: &Arc<RecordingState>,
    first_ts: &mut f64,
    last_ts: &mut f64,
    total_samples: &mut u64,
) -> anyhow::Result<()>
where
    T: Default + Copy + NumericSample + PullSample + ToF64,
{
    let max_chunk = 4096;
    let mut data = vec![T::default(); max_chunk * nch];
    let mut timestamps = vec![0.0f64; max_chunk];

    let mut n_pulled = 0usize;
    loop {
        if n_pulled >= max_chunk {
            break;
        }
        let buf = &mut data[n_pulled * nch..(n_pulled + 1) * nch];
        let ts = T::pull_one(inlet, buf, 0.0)?;
        if ts == 0.0 {
            break;
        }
        timestamps[n_pulled] = ts;
        n_pulled += 1;
    }

    if n_pulled == 0 {
        return Ok(());
    }

    let full_timestamps: Vec<f64> = timestamps[..n_pulled].to_vec();

    // Deduce timestamps for XDF (zero out deducible ones)
    let sample_interval = if srate > 0.0 { 1.0 / srate } else { 0.0 };
    for ts in timestamps[1..n_pulled].iter_mut() {
        if sample_interval > 0.0 && (*last_ts + sample_interval - *ts).abs() < 1e-9 {
            *ts = 0.0;
        }
        if *ts != 0.0 {
            *last_ts = *ts;
        } else {
            *last_ts += sample_interval;
        }
    }
    if *first_ts == 0.0 {
        *first_ts = full_timestamps[0];
    }
    *last_ts = full_timestamps.last().copied().unwrap_or(*last_ts);

    match writer {
        Writer::Xdf(_) => {
            writer.write_samples_numeric(
                stream_id,
                &timestamps[..n_pulled],
                &data[..n_pulled * nch],
                nch as u32,
            )?;
        }
        // every sample with its own time stamp
        _ => {
            let mut parquet_ts = full_timestamps;
            let mut running_ts = parquet_ts[0];
            for ts in parquet_ts[1..].iter_mut() {
                if *ts == 0.0 {
                    running_ts += sample_interval;
                    *ts = running_ts;
                } else {
                    running_ts = *ts;
                }
            }
            writer.write_samples_numeric(
                stream_id,
                &parquet_ts,
                &data[..n_pulled * nch],
                nch as u32,
            )?;
        }
    }

    *total_samples += n_pulled as u64;
    state
        .sample_count
        .fetch_add(n_pulled as u64, Ordering::Relaxed);

    Ok(())
}

/// Trait to pull one sample from an inlet into a typed buffer.
pub trait PullSample: Sized {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64>;
}

impl PullSample for f32 {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64> {
        inlet
            .pull_sample_f(buf, timeout)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
impl PullSample for f64 {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64> {
        inlet
            .pull_sample_d(buf, timeout)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
impl PullSample for i32 {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64> {
        inlet
            .pull_sample_i32(buf, timeout)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
impl PullSample for i16 {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64> {
        inlet
            .pull_sample_i16(buf, timeout)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
impl PullSample for i8 {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64> {
        inlet
            .pull_sample_i8(buf, timeout)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
impl PullSample for i64 {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64> {
        inlet
            .pull_sample_i64(buf, timeout)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
