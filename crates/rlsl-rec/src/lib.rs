//! `rlsl-rec` library — shared recording engine and Parquet writer.
//!
//! The XDF writer and `NumericSample` trait live in the [`exg`] crate.

#[cfg(feature = "hdf5")]
pub mod hdf5_writer;
/// Without the `hdf5` feature there is no HDF5 writer; the name it had up to 0.0.8 — an alias
/// of the Parquet writer — is kept so that code using it still builds.
#[cfg(not(feature = "hdf5"))]
pub mod hdf5_writer {
    #[deprecated(note = "this is the Parquet writer; build with the `hdf5` feature for HDF5 files")]
    pub type Hdf5RecordingWriter = crate::parquet_writer::ParquetRecordingWriter;
}
pub mod markers;
pub mod parquet_writer;
pub mod recording;
