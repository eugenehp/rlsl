//! HDF5 file writer for LSL recordings (cargo feature `hdf5`).
//!
//! One HDF5 file per recording, with a group per stream:
//!
//! ```text
//! recording.h5
//! ├── stream_1_EEG/
//! │   ├── timestamps      float64[N]
//! │   ├── data            the stream's own type [N × channels] (strings for a string stream)
//! │   ├── channel_labels  string[channels]
//! │   ├── clock_offsets   float64[M × 2]: collection time, offset (what to add to the stamps)
//! │   └── attributes      name, type, channel_count, nominal_srate, channel_format, source_id,
//! │                       hostname, uid, session_id, created_at, header_xml,
//! │                       first_timestamp, last_timestamp, sample_count
//! ├── stream_2_Markers/
//! │   └── ...
//! └── attributes          format ("rlsl-rec hdf5 1")
//! ```
//!
//! The file is written by `hdf5-no-c`, a pure-Rust HDF5 implementation: there is no libhdf5 to
//! install, and the result is ordinary HDF5 (h5py, MATLAB, HDFView read it). Datasets are
//! chunked, extendable and compressed (shuffle + deflate), and grow as samples arrive, so a
//! recording does not sit in memory.
//!
//! String streams (markers) are the exception to "grow as samples arrive": their time stamps go
//! to the file as they come, but the strings are kept in memory and written when the stream
//! ends. A variable-length string dataset written piece by piece is valid HDF5 (h5py reads it),
//! but `hdf5-no-c` 0.1.1 itself reads empty strings back from it; written whole, every reader
//! gets them, the crate that wrote them included.
//!
//! Unlike XDF and Parquet-with-a-journal, an HDF5 file is only whole once it has been closed:
//! a recorder that is killed leaves a file that may not open. The file is flushed after every
//! block of samples, which is as much as the format allows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use hdf5::types::VarLenUnicode;
use ndarray::{s, Array1, Array2};
use rlsl::stream_info::StreamInfo;
use rlsl::types::ChannelFormat;

use crate::parquet_writer::ToF64;

/// Rows of a stream held in memory before they go to the file.
const BLOCK_ROWS: usize = 1024;

fn text(s: &str) -> VarLenUnicode {
    // an interior NUL cannot be stored in a string; nothing an LSL description should contain
    s.replace('\0', " ").parse().unwrap_or_default()
}

struct StreamGroup {
    group: hdf5::Group,
    timestamps: hdf5::Dataset,
    /// `None` for a string stream, whose `data` is written whole at the end.
    data: Option<hdf5::Dataset>,
    format: ChannelFormat,
    nch: usize,
    rows: usize,
    ts_buf: Vec<f64>,
    float_buf: Vec<f64>,
    int_buf: Vec<i64>,
    text_buf: Vec<VarLenUnicode>,
    clock_offsets: Vec<[f64; 2]>,
}

impl StreamGroup {
    fn flush(&mut self) -> anyhow::Result<()> {
        let n = self.ts_buf.len();
        if n == 0 {
            return Ok(());
        }
        let (from, to, nch) = (self.rows, self.rows + n, self.nch);
        self.timestamps.resize(to)?;
        self.timestamps.write_slice(
            &Array1::from(std::mem::take(&mut self.ts_buf)),
            s![from..to],
        )?;
        self.rows = to;
        let Some(dataset) = self.data.as_ref() else {
            // strings: kept in `text_buf` until the stream ends
            return Ok(());
        };
        dataset.resize((to, nch))?;
        let floats = std::mem::take(&mut self.float_buf);
        let ints = std::mem::take(&mut self.int_buf);
        macro_rules! write {
            ($values:expr, $t:ty) => {{
                let block =
                    Array2::from_shape_vec((n, nch), $values.iter().map(|v| *v as $t).collect())?;
                dataset.write_slice(&block, s![from..to, ..])?;
            }};
        }
        match self.format {
            ChannelFormat::Float32 => write!(floats, f32),
            ChannelFormat::Int8 => write!(ints, i8),
            ChannelFormat::Int16 => write!(ints, i16),
            ChannelFormat::Int32 => write!(ints, i32),
            ChannelFormat::Int64 => write!(ints, i64),
            _ => write!(floats, f64),
        }
        Ok(())
    }

    /// Everything that is written once, when the stream ends: what is still in memory, a
    /// string stream's strings, the clock offsets.
    fn finish(&mut self) -> anyhow::Result<()> {
        self.flush()?;
        if self.format == ChannelFormat::String && self.data.is_none() {
            let texts = std::mem::take(&mut self.text_buf);
            let table = Array2::from_shape_vec((texts.len() / self.nch.max(1), self.nch), texts)?;
            self.data = Some(
                self.group
                    .new_dataset_builder()
                    .with_data(&table)
                    .create("data")?,
            );
        }
        if !self.clock_offsets.is_empty() {
            let flat: Vec<f64> = self.clock_offsets.iter().flatten().copied().collect();
            let table = Array2::from_shape_vec((self.clock_offsets.len(), 2), flat)?;
            self.group
                .new_dataset_builder()
                .with_data(&table)
                .create("clock_offsets")?;
            self.clock_offsets.clear();
        }
        Ok(())
    }
}

/// Thread-safe HDF5 recording writer. Mirrors the Parquet and XDF writers.
pub struct Hdf5RecordingWriter {
    path: PathBuf,
    file: Mutex<Option<hdf5::File>>,
    streams: Mutex<HashMap<u32, StreamGroup>>,
}

impl Hdf5RecordingWriter {
    /// Create `path` (an `.h5` file; an existing one is replaced).
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        let file = hdf5::File::create(&path)?;
        file.new_attr::<VarLenUnicode>()
            .create("format")?
            .write_scalar(&text("rlsl-rec hdf5 1"))?;
        Ok(Self {
            path,
            file: Mutex::new(Some(file)),
            streams: Mutex::new(HashMap::new()),
        })
    }

    /// Register a stream and create its group. Call before writing samples.
    pub fn write_stream_header(
        &self,
        stream_id: u32,
        info: &StreamInfo,
        header_xml: &str,
    ) -> anyhow::Result<()> {
        let guard = self.file.lock().unwrap();
        let file = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the recording is closed"))?;
        let nch = info.channel_count() as usize;
        let format = info.channel_format();
        let kind: String = info
            .type_()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let group = file.create_group(&format!("stream_{stream_id}_{kind}"))?;

        let attr = |name: &str, value: &str| -> anyhow::Result<()> {
            group
                .new_attr::<VarLenUnicode>()
                .create(name)?
                .write_scalar(&text(value))?;
            Ok(())
        };
        attr("name", &info.name())?;
        attr("type", &info.type_())?;
        attr("channel_format", format.as_str())?;
        attr("source_id", &info.source_id())?;
        attr("hostname", &info.hostname())?;
        attr("uid", &info.uid())?;
        attr("session_id", &info.session_id())?;
        attr("header_xml", header_xml)?;
        group
            .new_attr::<u32>()
            .create("channel_count")?
            .write_scalar(&info.channel_count())?;
        group
            .new_attr::<f64>()
            .create("nominal_srate")?
            .write_scalar(&info.nominal_srate())?;
        group
            .new_attr::<f64>()
            .create("created_at")?
            .write_scalar(&info.created_at())?;

        let labels: Vec<VarLenUnicode> = crate::parquet_writer::channel_labels(info, nch)
            .iter()
            .map(|l| text(l))
            .collect();
        group
            .new_dataset_builder()
            .with_data(&labels)
            .create("channel_labels")?;

        let timestamps = group
            .new_dataset::<f64>()
            .shape((0..,))
            .chunk((BLOCK_ROWS * 4,))
            .shuffle()
            .deflate(4)
            .create("timestamps")?;
        // about a megabyte of samples per chunk, whatever the channel count
        let chunk_rows = (131_072 / nch.max(1)).clamp(64, 16_384);
        macro_rules! dataset {
            ($t:ty) => {
                group
                    .new_dataset::<$t>()
                    .shape((0.., nch))
                    .chunk((chunk_rows, nch.max(1)))
                    .shuffle()
                    .deflate(4)
                    .create("data")?
            };
        }
        let data = match format {
            ChannelFormat::Float32 => Some(dataset!(f32)),
            ChannelFormat::Int8 => Some(dataset!(i8)),
            ChannelFormat::Int16 => Some(dataset!(i16)),
            ChannelFormat::Int32 => Some(dataset!(i32)),
            ChannelFormat::Int64 => Some(dataset!(i64)),
            ChannelFormat::String => None,
            _ => Some(dataset!(f64)),
        };
        self.streams.lock().unwrap().insert(
            stream_id,
            StreamGroup {
                group,
                timestamps,
                data,
                format,
                nch,
                rows: 0,
                ts_buf: Vec::new(),
                float_buf: Vec::new(),
                int_buf: Vec::new(),
                text_buf: Vec::new(),
                clock_offsets: Vec::new(),
            },
        );
        Ok(())
    }

    /// Write numeric samples for a stream (`data` is `n × n_channels`, row-major).
    pub fn write_samples_numeric<T: ToF64>(
        &self,
        stream_id: u32,
        timestamps: &[f64],
        data: &[T],
        n_channels: u32,
    ) -> anyhow::Result<()> {
        if timestamps.is_empty() {
            return Ok(());
        }
        let mut streams = self.streams.lock().unwrap();
        let Some(st) = streams.get_mut(&stream_id) else {
            return Ok(());
        };
        anyhow::ensure!(
            n_channels as usize == st.nch && data.len() == timestamps.len() * st.nch,
            "sample block of the wrong shape"
        );
        st.ts_buf.extend_from_slice(timestamps);
        let integer = matches!(
            st.format,
            ChannelFormat::Int8
                | ChannelFormat::Int16
                | ChannelFormat::Int32
                | ChannelFormat::Int64
        );
        if integer {
            st.int_buf.extend(data.iter().map(ToF64::to_i64));
        } else {
            st.float_buf.extend(data.iter().map(ToF64::to_f64));
        }
        if st.ts_buf.len() >= BLOCK_ROWS {
            st.flush()?;
            drop(streams);
            self.flush_file();
        }
        Ok(())
    }

    /// Write string samples for a stream (`data` is `n × channels`, row-major). Markers are
    /// few and each one matters: they go to the file at once.
    pub fn write_samples_string(
        &self,
        stream_id: u32,
        timestamps: &[f64],
        data: &[String],
    ) -> anyhow::Result<()> {
        if timestamps.is_empty() {
            return Ok(());
        }
        let mut streams = self.streams.lock().unwrap();
        let Some(st) = streams.get_mut(&stream_id) else {
            return Ok(());
        };
        anyhow::ensure!(
            data.len() == timestamps.len() * st.nch,
            "sample block of the wrong shape"
        );
        st.ts_buf.extend_from_slice(timestamps);
        st.text_buf.extend(data.iter().map(|s| text(s)));
        st.flush()?;
        drop(streams);
        self.flush_file();
        Ok(())
    }

    /// Record a clock offset measurement (what to add to the stream's time stamps).
    pub fn write_clock_offset(
        &self,
        stream_id: u32,
        collection_time: f64,
        offset_value: f64,
    ) -> anyhow::Result<()> {
        if let Some(st) = self.streams.lock().unwrap().get_mut(&stream_id) {
            st.clock_offsets.push([collection_time, offset_value]);
        }
        Ok(())
    }

    /// Finish a stream: what is still in memory, its clock offsets and its totals.
    pub fn write_stream_footer(
        &self,
        stream_id: u32,
        first_ts: f64,
        last_ts: f64,
        sample_count: u64,
    ) -> anyhow::Result<()> {
        let mut streams = self.streams.lock().unwrap();
        let Some(st) = streams.get_mut(&stream_id) else {
            return Ok(());
        };
        st.finish()?;
        st.group
            .new_attr::<f64>()
            .create("first_timestamp")?
            .write_scalar(&first_ts)?;
        st.group
            .new_attr::<f64>()
            .create("last_timestamp")?
            .write_scalar(&last_ts)?;
        st.group
            .new_attr::<u64>()
            .create("sample_count")?
            .write_scalar(&sample_count)?;
        Ok(())
    }

    fn flush_file(&self) {
        if let Some(file) = self.file.lock().unwrap().as_ref() {
            let _ = file.flush();
        }
    }

    /// Finalize the file. Streams whose footer was never written are flushed first.
    pub fn close(&self) -> anyhow::Result<()> {
        let mut streams = self.streams.lock().unwrap();
        for st in streams.values_mut() {
            st.finish()?;
        }
        streams.clear();
        if let Some(file) = self.file.lock().unwrap().take() {
            file.close()?;
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Hdf5RecordingWriter {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
