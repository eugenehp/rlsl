//! Sample types and serialization matching liblsl protocol 1.10.
//!
//! Samples are the unit of data transfer. Each contains a timestamp and channel data.

use crate::types::*;
use std::io::{Error as IoError, ErrorKind, Read, Result as IoResult};

/// A timestamped multi-channel sample.
#[derive(Debug, Clone)]
pub struct Sample {
    pub timestamp: f64,
    pub pushthrough: bool,
    pub data: SampleData,
}

/// The payload of a sample, typed by channel format.
#[derive(Debug, Clone)]
pub enum SampleData {
    Float32(Vec<f32>),
    Double64(Vec<f64>),
    Int32(Vec<i32>),
    Int16(Vec<i16>),
    Int8(Vec<i8>),
    Int64(Vec<i64>),
    StringData(Vec<String>),
}

impl Sample {
    /// Create a new sample with the given format, channel count, timestamp, and default data.
    pub fn new(fmt: ChannelFormat, num_channels: u32, timestamp: f64) -> Self {
        let n = num_channels as usize;
        let data = match fmt {
            ChannelFormat::Float32 => SampleData::Float32(vec![0.0; n]),
            ChannelFormat::Double64 => SampleData::Double64(vec![0.0; n]),
            ChannelFormat::Int32 => SampleData::Int32(vec![0; n]),
            ChannelFormat::Int16 => SampleData::Int16(vec![0; n]),
            ChannelFormat::Int8 => SampleData::Int8(vec![0; n]),
            ChannelFormat::Int64 => SampleData::Int64(vec![0; n]),
            ChannelFormat::String | ChannelFormat::Undefined => {
                SampleData::StringData(vec![String::new(); n])
            }
        };
        Sample {
            timestamp,
            pushthrough: true,
            data,
        }
    }

    /// Assign float data
    pub fn assign_f32(&mut self, src: &[f32]) {
        match &mut self.data {
            SampleData::Float32(d) => d.copy_from_slice(src),
            SampleData::Double64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f64;
                }
            }
            SampleData::Int32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i32;
                }
            }
            SampleData::Int16(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i16;
                }
            }
            SampleData::Int8(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i8;
                }
            }
            SampleData::Int64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i64;
                }
            }
            SampleData::StringData(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = s.to_string();
                }
            }
        }
    }

    /// Retrieve float data
    pub fn retrieve_f32(&self, dst: &mut [f32]) {
        match &self.data {
            SampleData::Float32(d) => dst.copy_from_slice(d),
            SampleData::Double64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f32;
                }
            }
            SampleData::Int32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f32;
                }
            }
            SampleData::Int16(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f32;
                }
            }
            SampleData::Int8(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f32;
                }
            }
            SampleData::Int64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f32;
                }
            }
            SampleData::StringData(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = s.parse().unwrap_or(0.0);
                }
            }
        }
    }

    /// Assign double data
    pub fn assign_f64(&mut self, src: &[f64]) {
        match &mut self.data {
            SampleData::Double64(d) => d.copy_from_slice(src),
            SampleData::Float32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f32;
                }
            }
            SampleData::Int32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i32;
                }
            }
            SampleData::Int16(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i16;
                }
            }
            SampleData::Int8(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i8;
                }
            }
            SampleData::Int64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i64;
                }
            }
            SampleData::StringData(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = s.to_string();
                }
            }
        }
    }

    /// Retrieve double data
    pub fn retrieve_f64(&self, dst: &mut [f64]) {
        match &self.data {
            SampleData::Double64(d) => dst.copy_from_slice(d),
            SampleData::Float32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f64;
                }
            }
            SampleData::Int32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f64;
                }
            }
            SampleData::Int16(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f64;
                }
            }
            SampleData::Int8(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f64;
                }
            }
            SampleData::Int64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as f64;
                }
            }
            SampleData::StringData(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = s.parse().unwrap_or(0.0);
                }
            }
        }
    }

    pub fn assign_i32(&mut self, src: &[i32]) {
        match &mut self.data {
            SampleData::Int32(d) => d.copy_from_slice(src),
            SampleData::Float32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f32;
                }
            }
            SampleData::Double64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f64;
                }
            }
            SampleData::Int16(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i16;
                }
            }
            SampleData::Int8(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i8;
                }
            }
            SampleData::Int64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i64;
                }
            }
            SampleData::StringData(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = s.to_string();
                }
            }
        }
    }

    pub fn retrieve_i32(&self, dst: &mut [i32]) {
        match &self.data {
            SampleData::Int32(d) => dst.copy_from_slice(d),
            SampleData::Float32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i32;
                }
            }
            SampleData::Double64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i32;
                }
            }
            SampleData::Int16(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i32;
                }
            }
            SampleData::Int8(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i32;
                }
            }
            SampleData::Int64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i32;
                }
            }
            SampleData::StringData(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = s.parse().unwrap_or(0);
                }
            }
        }
    }

    pub fn assign_i64(&mut self, src: &[i64]) {
        match &mut self.data {
            SampleData::Int64(d) => d.copy_from_slice(src),
            SampleData::Float32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f32;
                }
            }
            SampleData::Double64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f64;
                }
            }
            SampleData::Int32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i32;
                }
            }
            SampleData::Int16(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i16;
                }
            }
            SampleData::Int8(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i8;
                }
            }
            SampleData::StringData(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = s.to_string();
                }
            }
        }
    }

    pub fn retrieve_i64(&self, dst: &mut [i64]) {
        match &self.data {
            SampleData::Int64(d) => dst.copy_from_slice(d),
            SampleData::Float32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i64;
                }
            }
            SampleData::Double64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i64;
                }
            }
            SampleData::Int32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i64;
                }
            }
            SampleData::Int16(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i64;
                }
            }
            SampleData::Int8(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i64;
                }
            }
            SampleData::StringData(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = s.parse().unwrap_or(0);
                }
            }
        }
    }

    pub fn assign_i16(&mut self, src: &[i16]) {
        match &mut self.data {
            SampleData::Int16(d) => d.copy_from_slice(src),
            SampleData::Float32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f32;
                }
            }
            SampleData::Double64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f64;
                }
            }
            SampleData::Int32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i32;
                }
            }
            SampleData::Int8(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i8;
                }
            }
            SampleData::Int64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i64;
                }
            }
            SampleData::StringData(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = s.to_string();
                }
            }
        }
    }

    pub fn retrieve_i16(&self, dst: &mut [i16]) {
        match &self.data {
            SampleData::Int16(d) => dst.copy_from_slice(d),
            SampleData::Float32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i16;
                }
            }
            SampleData::Double64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i16;
                }
            }
            SampleData::Int32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i16;
                }
            }
            SampleData::Int8(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i16;
                }
            }
            SampleData::Int64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i16;
                }
            }
            SampleData::StringData(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = s.parse().unwrap_or(0);
                }
            }
        }
    }

    pub fn assign_i8(&mut self, src: &[i8]) {
        match &mut self.data {
            SampleData::Int8(d) => d.copy_from_slice(src),
            SampleData::Float32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f32;
                }
            }
            SampleData::Double64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as f64;
                }
            }
            SampleData::Int32(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i32;
                }
            }
            SampleData::Int16(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i16;
                }
            }
            SampleData::Int64(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = *s as i64;
                }
            }
            SampleData::StringData(d) => {
                for (dst, s) in d.iter_mut().zip(src) {
                    *dst = s.to_string();
                }
            }
        }
    }

    pub fn retrieve_i8(&self, dst: &mut [i8]) {
        match &self.data {
            SampleData::Int8(d) => dst.copy_from_slice(d),
            SampleData::Float32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i8;
                }
            }
            SampleData::Double64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i8;
                }
            }
            SampleData::Int32(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i8;
                }
            }
            SampleData::Int16(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i8;
                }
            }
            SampleData::Int64(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = *s as i8;
                }
            }
            SampleData::StringData(d) => {
                for (o, s) in dst.iter_mut().zip(d) {
                    *o = s.parse().unwrap_or(0);
                }
            }
        }
    }

    pub fn assign_strings(&mut self, src: &[String]) {
        if let SampleData::StringData(d) = &mut self.data {
            d.clone_from_slice(src);
        }
    }

    pub fn retrieve_strings(&self) -> Vec<String> {
        match &self.data {
            SampleData::StringData(d) => d.clone(),
            SampleData::Float32(d) => d.iter().map(|v| v.to_string()).collect(),
            SampleData::Double64(d) => d.iter().map(|v| v.to_string()).collect(),
            SampleData::Int32(d) => d.iter().map(|v| v.to_string()).collect(),
            SampleData::Int16(d) => d.iter().map(|v| v.to_string()).collect(),
            SampleData::Int8(d) => d.iter().map(|v| v.to_string()).collect(),
            SampleData::Int64(d) => d.iter().map(|v| v.to_string()).collect(),
        }
    }

    /// Assign raw bytes (for numeric non-string formats)
    pub fn assign_raw(&mut self, data: &[u8]) {
        match &mut self.data {
            SampleData::Float32(d) => {
                for (i, v) in d.iter_mut().enumerate() {
                    let off = i * 4;
                    if off + 4 <= data.len() {
                        *v = f32::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                        ]);
                    }
                }
            }
            SampleData::Double64(d) => {
                for (i, v) in d.iter_mut().enumerate() {
                    let off = i * 8;
                    if off + 8 <= data.len() {
                        *v = f64::from_le_bytes(data[off..off + 8].try_into().unwrap());
                    }
                }
            }
            SampleData::Int32(d) => {
                for (i, v) in d.iter_mut().enumerate() {
                    let off = i * 4;
                    if off + 4 <= data.len() {
                        *v = i32::from_le_bytes([
                            data[off],
                            data[off + 1],
                            data[off + 2],
                            data[off + 3],
                        ]);
                    }
                }
            }
            SampleData::Int16(d) => {
                for (i, v) in d.iter_mut().enumerate() {
                    let off = i * 2;
                    if off + 2 <= data.len() {
                        *v = i16::from_le_bytes([data[off], data[off + 1]]);
                    }
                }
            }
            SampleData::Int8(d) => {
                for (i, v) in d.iter_mut().enumerate() {
                    if i < data.len() {
                        *v = data[i] as i8;
                    }
                }
            }
            SampleData::Int64(d) => {
                for (i, v) in d.iter_mut().enumerate() {
                    let off = i * 8;
                    if off + 8 <= data.len() {
                        *v = i64::from_le_bytes(data[off..off + 8].try_into().unwrap());
                    }
                }
            }
            _ => {}
        }
    }

    /// Retrieve raw bytes
    pub fn retrieve_raw(&self) -> Vec<u8> {
        match &self.data {
            SampleData::Float32(d) => d.iter().flat_map(|v| v.to_le_bytes()).collect(),
            SampleData::Double64(d) => d.iter().flat_map(|v| v.to_le_bytes()).collect(),
            SampleData::Int32(d) => d.iter().flat_map(|v| v.to_le_bytes()).collect(),
            SampleData::Int16(d) => d.iter().flat_map(|v| v.to_le_bytes()).collect(),
            SampleData::Int8(d) => d.iter().map(|v| *v as u8).collect(),
            SampleData::Int64(d) => d.iter().flat_map(|v| v.to_le_bytes()).collect(),
            SampleData::StringData(_) => Vec::new(),
        }
    }

    /// Serialize a sample to bytes (protocol 1.10, little-endian, no byte-order swap).
    pub fn serialize_110(&self, buf: &mut Vec<u8>) {
        if self.timestamp == DEDUCED_TIMESTAMP {
            buf.push(TAG_DEDUCED_TIMESTAMP);
        } else {
            buf.push(TAG_TRANSMITTED_TIMESTAMP);
            buf.extend_from_slice(&self.timestamp.to_le_bytes());
        }

        match &self.data {
            SampleData::StringData(strings) => {
                for s in strings {
                    let len = s.len();
                    if len <= 0xFF {
                        buf.push(1); // sizeof(u8)
                        buf.push(len as u8);
                    } else if len <= 0xFFFFFFFF {
                        buf.push(4); // sizeof(u32)
                        buf.extend_from_slice(&(len as u32).to_le_bytes());
                    } else {
                        buf.push(8); // sizeof(u64)
                        buf.extend_from_slice(&(len as u64).to_le_bytes());
                    }
                    buf.extend_from_slice(s.as_bytes());
                }
            }
            _ => {
                buf.extend_from_slice(&self.retrieve_raw());
            }
        }
    }

    /// Deserialize a sample from a reader (protocol 1.10, little-endian).
    pub fn deserialize_110<R: Read>(
        reader: &mut R,
        fmt: ChannelFormat,
        num_channels: u32,
    ) -> IoResult<Sample> {
        let mut tag = [0u8; 1];
        reader.read_exact(&mut tag)?;

        let timestamp = if tag[0] == TAG_DEDUCED_TIMESTAMP {
            DEDUCED_TIMESTAMP
        } else {
            let mut ts_bytes = [0u8; 8];
            reader.read_exact(&mut ts_bytes)?;
            f64::from_le_bytes(ts_bytes)
        };

        let n = num_channels as usize;
        let data = match fmt {
            ChannelFormat::Float32 => {
                let mut raw = vec![0u8; n * 4];
                reader.read_exact(&mut raw)?;
                SampleData::Float32(
                    raw.chunks_exact(4)
                        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Double64 => {
                let mut raw = vec![0u8; n * 8];
                reader.read_exact(&mut raw)?;
                SampleData::Double64(
                    raw.chunks_exact(8)
                        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Int32 => {
                let mut raw = vec![0u8; n * 4];
                reader.read_exact(&mut raw)?;
                SampleData::Int32(
                    raw.chunks_exact(4)
                        .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Int16 => {
                let mut raw = vec![0u8; n * 2];
                reader.read_exact(&mut raw)?;
                SampleData::Int16(
                    raw.chunks_exact(2)
                        .map(|c| i16::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Int8 => {
                let mut raw = vec![0u8; n];
                reader.read_exact(&mut raw)?;
                SampleData::Int8(raw.into_iter().map(|b| b as i8).collect())
            }
            ChannelFormat::Int64 => {
                let mut raw = vec![0u8; n * 8];
                reader.read_exact(&mut raw)?;
                SampleData::Int64(
                    raw.chunks_exact(8)
                        .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::String | ChannelFormat::Undefined => {
                let mut strings = Vec::with_capacity(n);
                for _ in 0..n {
                    let mut lenbytes = [0u8; 1];
                    reader.read_exact(&mut lenbytes)?;
                    let len: usize = match lenbytes[0] {
                        1 => {
                            let mut b = [0u8; 1];
                            reader.read_exact(&mut b)?;
                            b[0] as usize
                        }
                        4 => {
                            let mut b = [0u8; 4];
                            reader.read_exact(&mut b)?;
                            u32::from_le_bytes(b) as usize
                        }
                        8 => {
                            let mut b = [0u8; 8];
                            reader.read_exact(&mut b)?;
                            u64::from_le_bytes(b) as usize
                        }
                        _ => {
                            return Err(IoError::new(ErrorKind::InvalidData, "invalid varlen int"))
                        }
                    };
                    let mut sbuf = vec![0u8; len];
                    reader.read_exact(&mut sbuf)?;
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

    /// Serialize a sample to bytes (protocol 1.00).
    /// Protocol 1.00: every sample has an 8-byte timestamp (no tag byte),
    /// strings use 4-byte length prefix.
    pub fn serialize_100(&self, buf: &mut Vec<u8>) {
        // Always write the full 8-byte timestamp
        buf.extend_from_slice(&self.timestamp.to_le_bytes());

        match &self.data {
            SampleData::StringData(strings) => {
                for s in strings {
                    let len = s.len() as u32;
                    buf.extend_from_slice(&len.to_le_bytes());
                    buf.extend_from_slice(s.as_bytes());
                }
            }
            _ => {
                buf.extend_from_slice(&self.retrieve_raw());
            }
        }
    }

    /// Deserialize a sample from a reader (protocol 1.00).
    pub fn deserialize_100<R: Read>(
        reader: &mut R,
        fmt: ChannelFormat,
        num_channels: u32,
    ) -> IoResult<Sample> {
        // Always read 8-byte timestamp
        let mut ts_bytes = [0u8; 8];
        reader.read_exact(&mut ts_bytes)?;
        let timestamp = f64::from_le_bytes(ts_bytes);

        let n = num_channels as usize;
        let data = match fmt {
            ChannelFormat::Float32 => {
                let mut raw = vec![0u8; n * 4];
                reader.read_exact(&mut raw)?;
                SampleData::Float32(
                    raw.chunks_exact(4)
                        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Double64 => {
                let mut raw = vec![0u8; n * 8];
                reader.read_exact(&mut raw)?;
                SampleData::Double64(
                    raw.chunks_exact(8)
                        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Int32 => {
                let mut raw = vec![0u8; n * 4];
                reader.read_exact(&mut raw)?;
                SampleData::Int32(
                    raw.chunks_exact(4)
                        .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Int16 => {
                let mut raw = vec![0u8; n * 2];
                reader.read_exact(&mut raw)?;
                SampleData::Int16(
                    raw.chunks_exact(2)
                        .map(|c| i16::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::Int8 => {
                let mut raw = vec![0u8; n];
                reader.read_exact(&mut raw)?;
                SampleData::Int8(raw.into_iter().map(|b| b as i8).collect())
            }
            ChannelFormat::Int64 => {
                let mut raw = vec![0u8; n * 8];
                reader.read_exact(&mut raw)?;
                SampleData::Int64(
                    raw.chunks_exact(8)
                        .map(|c| i64::from_le_bytes(c.try_into().unwrap()))
                        .collect(),
                )
            }
            ChannelFormat::String | ChannelFormat::Undefined => {
                let mut strings = Vec::with_capacity(n);
                for _ in 0..n {
                    let mut len_bytes = [0u8; 4];
                    reader.read_exact(&mut len_bytes)?;
                    let len = u32::from_le_bytes(len_bytes) as usize;
                    let mut sbuf = vec![0u8; len];
                    reader.read_exact(&mut sbuf)?;
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

    /// Generate a test pattern matching liblsl's assign_test_pattern
    pub fn assign_test_pattern(&mut self, offset: i32) {
        self.timestamp = 123456.789;
        self.pushthrough = true;
        match &mut self.data {
            SampleData::Float32(d) => {
                for (k, v) in d.iter_mut().enumerate() {
                    let val = (k as i32 + offset) as f32;
                    *v = if k % 2 == 0 { val } else { -val };
                }
            }
            SampleData::Double64(d) => {
                for (k, v) in d.iter_mut().enumerate() {
                    let val = (k as i64 + offset as i64 + 16777217) as f64;
                    *v = if k % 2 == 0 { val } else { -val };
                }
            }
            SampleData::Int32(d) => {
                for (k, v) in d.iter_mut().enumerate() {
                    let val = (k as i32 + offset + 65537) % i32::MAX;
                    *v = if k % 2 == 0 { val } else { -val };
                }
            }
            SampleData::Int16(d) => {
                for (k, v) in d.iter_mut().enumerate() {
                    let val = ((k as i32 + offset + 257) % i16::MAX as i32) as i16;
                    *v = if k % 2 == 0 { val } else { -val };
                }
            }
            SampleData::Int8(d) => {
                for (k, v) in d.iter_mut().enumerate() {
                    let val = ((k as i32 + offset + 1) % i8::MAX as i32) as i8;
                    *v = if k % 2 == 0 { val } else { -val };
                }
            }
            SampleData::Int64(d) => {
                for (k, v) in d.iter_mut().enumerate() {
                    let val = k as i64 + 2147483649i64 + offset as i64;
                    *v = if k % 2 == 0 { val } else { -val };
                }
            }
            SampleData::StringData(d) => {
                for (k, v) in d.iter_mut().enumerate() {
                    let val = (k as i32 + 10) * if k % 2 == 0 { 1 } else { -1 };
                    *v = val.to_string();
                }
            }
        }
    }

    pub fn num_channels(&self) -> usize {
        match &self.data {
            SampleData::Float32(d) => d.len(),
            SampleData::Double64(d) => d.len(),
            SampleData::Int32(d) => d.len(),
            SampleData::Int16(d) => d.len(),
            SampleData::Int8(d) => d.len(),
            SampleData::Int64(d) => d.len(),
            SampleData::StringData(d) => d.len(),
        }
    }

    pub fn format(&self) -> ChannelFormat {
        match &self.data {
            SampleData::Float32(_) => ChannelFormat::Float32,
            SampleData::Double64(_) => ChannelFormat::Double64,
            SampleData::Int32(_) => ChannelFormat::Int32,
            SampleData::Int16(_) => ChannelFormat::Int16,
            SampleData::Int8(_) => ChannelFormat::Int8,
            SampleData::Int64(_) => ChannelFormat::Int64,
            SampleData::StringData(_) => ChannelFormat::String,
        }
    }
}

impl PartialEq for Sample {
    fn eq(&self, other: &Self) -> bool {
        if self.timestamp != other.timestamp {
            return false;
        }
        match (&self.data, &other.data) {
            (SampleData::Float32(a), SampleData::Float32(b)) => a == b,
            (SampleData::Double64(a), SampleData::Double64(b)) => a == b,
            (SampleData::Int32(a), SampleData::Int32(b)) => a == b,
            (SampleData::Int16(a), SampleData::Int16(b)) => a == b,
            (SampleData::Int8(a), SampleData::Int8(b)) => a == b,
            (SampleData::Int64(a), SampleData::Int64(b)) => a == b,
            (SampleData::StringData(a), SampleData::StringData(b)) => a == b,
            _ => false,
        }
    }
}
