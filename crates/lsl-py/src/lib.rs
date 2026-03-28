//! `pylsl` — Python bindings for `lsl-core`.
//!
//! Exposes the full LSL API to Python via PyO3:
//!
//! ```python
//! import pylsl
//!
//! # Discover streams
//! streams = pylsl.resolve_streams(timeout=2.0)
//!
//! # Create an outlet
//! info = pylsl.StreamInfo("MyStream", "EEG", 8, 250.0, pylsl.CF_FLOAT32, "src1")
//! outlet = pylsl.StreamOutlet(info)
//! outlet.push_sample([1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0])
//!
//! # Create an inlet
//! inlet = pylsl.StreamInlet(streams[0])
//! inlet.open_stream(timeout=10.0)
//! sample, timestamp = inlet.pull_sample(timeout=5.0)
//! chunk, timestamps = inlet.pull_chunk(timeout=0.0)  # numpy arrays!
//! ```
//!
//! Build with: `maturin develop -m crates/lsl-py/Cargo.toml`

use lsl_core::clock;
use lsl_core::inlet::StreamInlet as RsInlet;
use lsl_core::outlet::StreamOutlet as RsOutlet;
use lsl_core::resolver;
use lsl_core::stream_info::StreamInfo as RsStreamInfo;
use lsl_core::types::*;
use ndarray::Array2;
use numpy::{PyArray1, PyArray2};
use pyo3::exceptions::{PyRuntimeError, PyTimeoutError, PyValueError};
use pyo3::prelude::*;

// ── StreamInfo ───────────────────────────────────────────────────────

/// Stream metadata — describes a stream's name, type, channel count,
/// sample rate, and channel format.
#[pyclass(name = "StreamInfo")]
#[derive(Clone)]
struct PyStreamInfo {
    inner: RsStreamInfo,
}

#[pymethods]
impl PyStreamInfo {
    /// Create a new StreamInfo.
    ///
    /// Args:
    ///     name: Stream name (e.g. "EEG").
    ///     type_: Content type (e.g. "EEG", "Markers").
    ///     channel_count: Number of channels.
    ///     nominal_srate: Sampling rate in Hz (0.0 for irregular).
    ///     channel_format: One of CF_FLOAT32, CF_DOUBLE64, CF_INT32, etc.
    ///     source_id: Unique source identifier.
    #[new]
    #[pyo3(signature = (name, type_="", channel_count=1, nominal_srate=0.0, channel_format=1, source_id=""))]
    fn new(
        name: &str,
        type_: &str,
        channel_count: u32,
        nominal_srate: f64,
        channel_format: i32,
        source_id: &str,
    ) -> Self {
        PyStreamInfo {
            inner: RsStreamInfo::new(
                name,
                type_,
                channel_count,
                nominal_srate,
                ChannelFormat::from_i32(channel_format),
                source_id,
            ),
        }
    }

    /// Stream name.
    #[getter]
    fn name(&self) -> String { self.inner.name() }

    /// Content type.
    #[getter]
    fn type_(&self) -> String { self.inner.type_() }

    /// Number of channels.
    #[getter]
    fn channel_count(&self) -> u32 { self.inner.channel_count() }

    /// Nominal sampling rate (Hz). 0 = irregular.
    #[getter]
    fn nominal_srate(&self) -> f64 { self.inner.nominal_srate() }

    /// Channel format as integer constant.
    #[getter]
    fn channel_format(&self) -> i32 { self.inner.channel_format() as i32 }

    /// Unique stream identifier.
    #[getter]
    fn uid(&self) -> String { self.inner.uid() }

    /// Source identifier.
    #[getter]
    fn source_id(&self) -> String { self.inner.source_id() }

    /// Hostname of the machine providing the stream.
    #[getter]
    fn hostname(&self) -> String { self.inner.hostname() }

    /// Session identifier.
    #[getter]
    fn session_id(&self) -> String { self.inner.session_id() }

    /// Time when the stream was created.
    #[getter]
    fn created_at(&self) -> f64 { self.inner.created_at() }

    /// Full XML description of the stream.
    fn as_xml(&self) -> String { self.inner.to_fullinfo_message() }

    fn __repr__(&self) -> String {
        format!(
            "StreamInfo(name='{}', type='{}', ch={}, srate={}, fmt={})",
            self.inner.name(),
            self.inner.type_(),
            self.inner.channel_count(),
            self.inner.nominal_srate(),
            self.inner.channel_format().as_str(),
        )
    }
}

// ── StreamOutlet ─────────────────────────────────────────────────────

/// A stream outlet. Publishes data on the network.
#[pyclass(name = "StreamOutlet")]
struct PyStreamOutlet {
    inner: RsOutlet,
    nch: usize,
    fmt: ChannelFormat,
}

#[pymethods]
impl PyStreamOutlet {
    /// Create a new outlet from a StreamInfo.
    #[new]
    #[pyo3(signature = (info, chunk_size=0, max_buffered=360))]
    fn new(info: &PyStreamInfo, chunk_size: i32, max_buffered: i32) -> Self {
        let fmt = info.inner.channel_format();
        let nch = info.inner.channel_count() as usize;
        PyStreamOutlet {
            inner: RsOutlet::new(&info.inner, chunk_size, max_buffered),
            nch,
            fmt,
        }
    }

    /// Push a single sample. `data` is a list of numbers (or strings for string streams).
    ///
    /// Args:
    ///     data: Sample values (list of float/int).
    ///     timestamp: Optional timestamp (0 = auto, -1 = deduced).
    ///     pushthrough: Flush immediately (default True).
    #[pyo3(signature = (data, timestamp=0.0, pushthrough=true))]
    fn push_sample(
        &self,
        data: Vec<f64>,
        timestamp: f64,
        pushthrough: bool,
    ) -> PyResult<()> {
        if data.len() != self.nch {
            return Err(PyValueError::new_err(format!(
                "Expected {} values, got {}",
                self.nch,
                data.len()
            )));
        }
        match self.fmt {
            ChannelFormat::Float32 => {
                let buf: Vec<f32> = data.iter().map(|&v| v as f32).collect();
                self.inner.push_sample_f(&buf, timestamp, pushthrough);
            }
            ChannelFormat::Double64 => {
                self.inner.push_sample_d(&data, timestamp, pushthrough);
            }
            ChannelFormat::Int32 => {
                let buf: Vec<i32> = data.iter().map(|&v| v as i32).collect();
                self.inner.push_sample_i32(&buf, timestamp, pushthrough);
            }
            ChannelFormat::Int16 => {
                let buf: Vec<i16> = data.iter().map(|&v| v as i16).collect();
                self.inner.push_sample_i16(&buf, timestamp, pushthrough);
            }
            ChannelFormat::Int64 => {
                let buf: Vec<i64> = data.iter().map(|&v| v as i64).collect();
                self.inner.push_sample_i64(&buf, timestamp, pushthrough);
            }
            _ => {
                return Err(PyValueError::new_err("Unsupported channel format for push_sample"));
            }
        }
        Ok(())
    }

    /// Push a chunk of samples as a flat list or 2D array.
    ///
    /// Args:
    ///     data: Flat list of values [s0ch0, s0ch1, …, s1ch0, …].
    ///     timestamp: Timestamp for the last sample (0 = auto).
    ///     pushthrough: Flush immediately (default True).
    #[pyo3(signature = (data, timestamp=0.0, pushthrough=true))]
    fn push_chunk(
        &self,
        data: Vec<f64>,
        timestamp: f64,
        pushthrough: bool,
    ) -> PyResult<()> {
        if data.len() % self.nch != 0 {
            return Err(PyValueError::new_err(format!(
                "Data length {} is not a multiple of channel_count {}",
                data.len(),
                self.nch
            )));
        }
        let n_samples = data.len() / self.nch;
        for i in 0..n_samples {
            let ts = if i == n_samples - 1 { timestamp } else { 0.0 };
            let chunk = &data[i * self.nch..(i + 1) * self.nch];
            match self.fmt {
                ChannelFormat::Float32 => {
                    let buf: Vec<f32> = chunk.iter().map(|&v| v as f32).collect();
                    self.inner.push_sample_f(&buf, ts, pushthrough && i == n_samples - 1);
                }
                ChannelFormat::Double64 => {
                    self.inner.push_sample_d(chunk, ts, pushthrough && i == n_samples - 1);
                }
                _ => {
                    return Err(PyValueError::new_err("Unsupported format for push_chunk"));
                }
            }
        }
        Ok(())
    }

    /// Return True if any consumers are connected.
    fn have_consumers(&self) -> bool {
        self.inner.have_consumers()
    }

    /// Wait until a consumer connects or timeout.
    #[pyo3(signature = (timeout=32000000.0))]
    fn wait_for_consumers(&self, timeout: f64) -> bool {
        self.inner.wait_for_consumers(timeout)
    }

    /// Stream info.
    fn info(&self) -> PyStreamInfo {
        PyStreamInfo {
            inner: self.inner.info().clone(),
        }
    }
}

// ── StreamInlet ──────────────────────────────────────────────────────

/// A stream inlet. Receives data from a stream outlet.
#[pyclass(name = "StreamInlet")]
struct PyStreamInlet {
    inner: RsInlet,
    nch: usize,
    _fmt: ChannelFormat,
}

#[pymethods]
impl PyStreamInlet {
    /// Create a new inlet from a StreamInfo.
    #[new]
    #[pyo3(signature = (info, max_buflen=360, max_chunklen=0, recover=true))]
    fn new(info: &PyStreamInfo, max_buflen: i32, max_chunklen: i32, recover: bool) -> Self {
        let fmt = info.inner.channel_format();
        let nch = info.inner.channel_count() as usize;
        PyStreamInlet {
            inner: RsInlet::new(&info.inner, max_buflen, max_chunklen, recover),
            nch,
            _fmt: fmt,
        }
    }

    /// Open the stream connection.
    #[pyo3(signature = (timeout=32000000.0))]
    fn open_stream(&self, timeout: f64) -> PyResult<()> {
        self.inner
            .open_stream(timeout)
            .map_err(|e| PyTimeoutError::new_err(e))
    }

    /// Close the stream connection.
    fn close_stream(&self) {
        self.inner.close_stream();
    }

    /// Pull a single sample. Returns (data_list, timestamp).
    ///
    /// Args:
    ///     timeout: Max seconds to wait (0 = non-blocking).
    ///
    /// Returns:
    ///     (list[float], float) — sample values and timestamp.
    ///     Returns ([], 0.0) if timeout.
    #[pyo3(signature = (timeout=32000000.0))]
    fn pull_sample(&self, timeout: f64) -> PyResult<(Vec<f64>, f64)> {
        let mut buf = vec![0.0f64; self.nch];
        match self.inner.pull_sample_d(&mut buf, timeout) {
            Ok(ts) if ts > 0.0 => Ok((buf, ts)),
            Ok(_) => Ok((vec![], 0.0)),
            Err(e) => Err(PyRuntimeError::new_err(e)),
        }
    }

    /// Pull all available samples as numpy arrays.
    ///
    /// Returns:
    ///     (data: numpy.ndarray[float64, (N, nch)], timestamps: numpy.ndarray[float64, (N,)])
    ///     Empty arrays if no data available.
    #[pyo3(signature = (timeout=0.0, max_samples=4096))]
    fn pull_chunk<'py>(
        &self,
        py: Python<'py>,
        timeout: f64,
        max_samples: usize,
    ) -> PyResult<(Bound<'py, PyArray2<f64>>, Bound<'py, PyArray1<f64>>)> {
        let nch = self.nch;
        let mut data = vec![0.0f64; max_samples * nch];
        let mut timestamps = vec![0.0f64; max_samples];
        let mut n_pulled = 0usize;

        // Pull first sample with the requested timeout
        if n_pulled < max_samples {
            let buf = &mut data[0..nch];
            match self.inner.pull_sample_d(buf, timeout) {
                Ok(ts) if ts > 0.0 => {
                    timestamps[0] = ts;
                    n_pulled = 1;
                }
                Ok(_) => {}
                Err(e) => return Err(PyRuntimeError::new_err(e)),
            }
        }

        // Pull remaining with timeout=0 (non-blocking)
        while n_pulled < max_samples {
            let buf = &mut data[n_pulled * nch..(n_pulled + 1) * nch];
            match self.inner.pull_sample_d(buf, 0.0) {
                Ok(ts) if ts > 0.0 => {
                    timestamps[n_pulled] = ts;
                    n_pulled += 1;
                }
                _ => break,
            }
        }

        let ts_array = PyArray1::from_slice(py, &timestamps[..n_pulled]);
        let data_slice = &data[..n_pulled * nch];
        let nd = Array2::from_shape_vec((n_pulled, nch), data_slice.to_vec())
            .map_err(|e| PyRuntimeError::new_err(format!("Array error: {}", e)))?;
        let data_array = PyArray2::from_owned_array(py, nd);
        Ok((data_array, ts_array))
    }

    /// Get the clock offset correction value.
    #[pyo3(signature = (timeout=2.0))]
    fn time_correction(&self, timeout: f64) -> f64 {
        self.inner.time_correction(timeout)
    }

    /// Number of samples available for immediate read.
    fn samples_available(&self) -> u32 {
        self.inner.samples_available()
    }

    /// Discard all buffered samples and return the count.
    fn flush(&self) -> u32 {
        self.inner.flush()
    }

    /// Stream info.
    fn info(&self) -> PyStreamInfo {
        PyStreamInfo {
            inner: self.inner.get_fullinfo(0.0),
        }
    }
}

// ── Module-level functions ───────────────────────────────────────────

/// Resolve all streams on the network.
///
/// Args:
///     timeout: Seconds to wait for stream responses (default 1.0).
///
/// Returns:
///     List of StreamInfo objects.
#[pyfunction]
#[pyo3(signature = (timeout=1.0))]
fn resolve_streams(timeout: f64) -> Vec<PyStreamInfo> {
    resolver::resolve_all(timeout)
        .into_iter()
        .map(|info| PyStreamInfo { inner: info })
        .collect()
}

/// Resolve streams matching a property value.
///
/// Args:
///     prop: Property name ("name", "type", "source_id", …).
///     value: Value to match.
///     minimum: Minimum number of streams to find (default 1).
///     timeout: Max seconds to wait (default 5.0).
#[pyfunction]
#[pyo3(signature = (prop, value, minimum=1, timeout=5.0))]
fn resolve_byprop(prop: &str, value: &str, minimum: i32, timeout: f64) -> Vec<PyStreamInfo> {
    resolver::resolve_by_property(prop, value, minimum, timeout)
        .into_iter()
        .map(|info| PyStreamInfo { inner: info })
        .collect()
}

/// Resolve streams matching a predicate.
///
/// Args:
///     pred: Predicate string (e.g. "name='EEG' and type='EEG'").
///     minimum: Minimum number of streams to find (default 1).
///     timeout: Max seconds to wait (default 5.0).
#[pyfunction]
#[pyo3(signature = (pred, minimum=1, timeout=5.0))]
fn resolve_bypred(pred: &str, minimum: i32, timeout: f64) -> Vec<PyStreamInfo> {
    resolver::resolve_by_predicate(pred, minimum, timeout)
        .into_iter()
        .map(|info| PyStreamInfo { inner: info })
        .collect()
}

/// Return the local clock time (seconds, monotonic).
#[pyfunction]
fn local_clock() -> f64 {
    clock::local_clock()
}

/// Return the library version.
#[pyfunction]
fn library_version() -> i32 {
    LSL_LIBRARY_VERSION
}

/// Return the protocol version.
#[pyfunction]
fn protocol_version() -> i32 {
    LSL_PROTOCOL_VERSION
}

// ── Python module ────────────────────────────────────────────────────

#[pymodule]
fn pylsl(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Classes
    m.add_class::<PyStreamInfo>()?;
    m.add_class::<PyStreamOutlet>()?;
    m.add_class::<PyStreamInlet>()?;

    // Functions
    m.add_function(wrap_pyfunction!(resolve_streams, m)?)?;
    m.add_function(wrap_pyfunction!(resolve_byprop, m)?)?;
    m.add_function(wrap_pyfunction!(resolve_bypred, m)?)?;
    m.add_function(wrap_pyfunction!(local_clock, m)?)?;
    m.add_function(wrap_pyfunction!(library_version, m)?)?;
    m.add_function(wrap_pyfunction!(protocol_version, m)?)?;

    // Channel format constants (matching pylsl conventions)
    m.add("CF_UNDEFINED", ChannelFormat::Undefined as i32)?;
    m.add("CF_FLOAT32", ChannelFormat::Float32 as i32)?;
    m.add("CF_DOUBLE64", ChannelFormat::Double64 as i32)?;
    m.add("CF_STRING", ChannelFormat::String as i32)?;
    m.add("CF_INT32", ChannelFormat::Int32 as i32)?;
    m.add("CF_INT16", ChannelFormat::Int16 as i32)?;
    m.add("CF_INT8", ChannelFormat::Int8 as i32)?;
    m.add("CF_INT64", ChannelFormat::Int64 as i32)?;

    // Useful constants
    m.add("IRREGULAR_RATE", IRREGULAR_RATE)?;
    m.add("DEDUCED_TIMESTAMP", DEDUCED_TIMESTAMP)?;
    m.add("FOREVER", FOREVER)?;

    Ok(())
}
