#![allow(clippy::too_many_arguments)]
//! Recording engine — resolves LSL streams, pulls data, writes XDF or Parquet.
//!
//! All heavy operations are spawned as tokio tasks (blocking I/O via
//! `spawn_blocking`, periodic work via `tokio::spawn`).

use crate::parquet_writer::{ParquetRecordingWriter, ToF64};
use exg::{NumericSample, XdfWriter};
use lsl_core::clock::local_clock;
use lsl_core::inlet::StreamInlet;
use lsl_core::stream_info::StreamInfo;
use lsl_core::types::*;
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
}

impl RecordingFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Xdf => "XDF",
            Self::Parquet => "Parquet",
        }
    }
}

// ── Writer abstraction ───────────────────────────────────────────────

/// Unifies XDF and Parquet writers behind a common interface.
enum Writer {
    Xdf(Arc<XdfWriter>),
    Parquet(Arc<ParquetRecordingWriter>),
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
        }
    }

    fn write_stream_footer_xdf(&self, stream_id: u32, footer_xml: &str) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_stream_footer(stream_id, footer_xml),
            Writer::Parquet(_) => Ok(()),
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
        }
    }

    fn write_boundary(&self) -> anyhow::Result<()> {
        match self {
            Writer::Xdf(w) => w.write_boundary(),
            Writer::Parquet(_) => Ok(()),
        }
    }
}

impl Clone for Writer {
    fn clone(&self) -> Self {
        match self {
            Writer::Xdf(w) => Writer::Xdf(w.clone()),
            Writer::Parquet(w) => Writer::Parquet(w.clone()),
        }
    }
}

// Send + Sync are required for tokio tasks.
// Both XdfWriter (Mutex<BufWriter<File>>) and ParquetRecordingWriter
// (Mutex<HashMap<…>>) are Send + Sync.
unsafe impl Send for Writer {}
unsafe impl Sync for Writer {}

// ── Shared recording state ──────────────────────────────────────────

/// Shared recording state visible to the TUI/GUI.
pub struct RecordingState {
    pub sample_count: AtomicU64,
    pub stream_count: AtomicU64,
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
        let writer = match format {
            RecordingFormat::Xdf => Writer::Xdf(Arc::new(XdfWriter::new(filename)?)),
            RecordingFormat::Parquet => {
                Writer::Parquet(Arc::new(ParquetRecordingWriter::new(filename)?))
            }
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = Arc::new(RecordingState {
            sample_count: AtomicU64::new(0),
            stream_count: AtomicU64::new(0),
            running: AtomicBool::new(true),
        });

        let handle = tokio::runtime::Handle::try_current()
            .unwrap_or_else(|_| lsl_core::RUNTIME.handle().clone());

        let mut tasks: Vec<JoinHandle<()>> = Vec::new();

        for (idx, info) in streams.iter().enumerate() {
            let sid = (idx + 1) as u32;
            let info = info.clone();
            let writer = writer.clone();
            let shutdown = shutdown.clone();
            let st = state.clone();

            state.stream_count.fetch_add(1, Ordering::Relaxed);

            // Each stream recorder runs as a blocking task because
            // inlet.open_stream() and pull_sample_* are blocking calls.
            let task = handle.spawn_blocking(move || {
                if let Err(e) = record_stream(sid, &info, &writer, &shutdown, &st) {
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

        // Finalize Parquet (close files, write JSON sidecar)
        if let Some(Writer::Parquet(arc)) = self.writer.take() {
            if let Ok(pw) = Arc::try_unwrap(arc) {
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = pw.close() {
                        eprintln!("Error finalizing Parquet: {}", e);
                    }
                })
                .await;
            }
        }
    }

    /// Synchronous stop — blocks the current thread until done.
    /// Use when you don't have an async context (e.g. Drop-like cleanup).
    pub fn stop_sync(self) {
        let handle = tokio::runtime::Handle::try_current()
            .unwrap_or_else(|_| lsl_core::RUNTIME.handle().clone());
        handle.block_on(self.stop());
    }

    pub fn file_size(&self) -> u64 {
        match &self.writer {
            Some(Writer::Xdf(_)) => std::fs::metadata(&self.filename)
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
    info: &StreamInfo,
    writer: &Writer,
    shutdown: &Arc<AtomicBool>,
    state: &Arc<RecordingState>,
) -> anyhow::Result<()> {
    let inlet = StreamInlet::new(info, 360, 0, true);
    inlet
        .open_stream(10.0)
        .map_err(|e| anyhow::anyhow!("open_stream: {}", e))?;

    let header_xml = info.to_fullinfo_message();
    writer.write_stream_header(stream_id, info, &header_xml)?;

    let fmt = info.channel_format();
    let nch = info.channel_count() as usize;
    let srate = info.nominal_srate();

    let mut first_ts = 0.0f64;
    let mut last_ts = 0.0f64;
    let mut total_samples = 0u64;
    let mut last_offset_time = local_clock();

    while !shutdown.load(Ordering::Relaxed) {
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
            _ => {
                std::thread::sleep(Duration::from_millis(500));
            }
        }

        // Periodic clock offset
        let now = local_clock();
        if now - last_offset_time > 5.0 {
            let offset = inlet.time_correction(2.0);
            writer.write_clock_offset(stream_id, now, offset)?;
            last_offset_time = now;
        }

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
         </info>"
    );
    writer.write_stream_footer_xdf(stream_id, &footer)?;
    writer.write_stream_footer_parquet(stream_id, first_ts, last_ts, total_samples)?;

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
        Writer::Parquet(_) => {
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
impl PullSample for i64 {
    fn pull_one(inlet: &StreamInlet, buf: &mut [Self], timeout: f64) -> anyhow::Result<f64> {
        inlet
            .pull_sample_i64(buf, timeout)
            .map_err(|e| anyhow::anyhow!(e))
    }
}
