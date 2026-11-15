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
    /// This sample with its subnormal numbers (nonzero, but smaller than the smallest normal
    /// float) replaced by zero, for a consumer whose machine has none — or `None` when there
    /// is nothing to replace, which is nearly always.
    pub fn without_subnormals(&self) -> Option<Sample> {
        let subnormal = match &self.data {
            SampleData::Float32(v) => v.iter().any(|x| x.is_subnormal()),
            SampleData::Double64(v) => v.iter().any(|x| x.is_subnormal()),
            _ => false,
        };
        if !subnormal {
            return None;
        }
        let mut flushed = self.clone();
        match &mut flushed.data {
            SampleData::Float32(v) => v
                .iter_mut()
                .filter(|x| x.is_subnormal())
                .for_each(|x| *x = 0.0),
            SampleData::Double64(v) => v
                .iter_mut()
                .filter(|x| x.is_subnormal())
                .for_each(|x| *x = 0.0),
            _ => {}
        }
        Some(flushed)
    }

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
                    raw.as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| f32::from_le_bytes(*c))
                        .collect(),
                )
            }
            ChannelFormat::Double64 => {
                let mut raw = vec![0u8; n * 8];
                reader.read_exact(&mut raw)?;
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
                reader.read_exact(&mut raw)?;
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
                reader.read_exact(&mut raw)?;
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
                reader.read_exact(&mut raw)?;
                SampleData::Int8(raw.into_iter().map(|b| b as i8).collect())
            }
            ChannelFormat::Int64 => {
                let mut raw = vec![0u8; n * 8];
                reader.read_exact(&mut raw)?;
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

    /// Serialize the sample in protocol 1.00: liblsl's "portable archive" form, in which every
    /// number is written as a size byte and that many little-endian bytes (zero as a single
    /// zero byte, a negative integer with a negative size), floats as their IEEE 754 bits, and
    /// strings as a length and their bytes. Checked byte for byte against liblsl 1.18 set to
    /// protocol 100. (Up to 0.0.8 this wrote a form of rlsl's own that no liblsl could read.)
    pub fn serialize_100(&self, buf: &mut Vec<u8>) {
        if self.timestamp == DEDUCED_TIMESTAMP {
            portable::put_uint(buf, u64::from(TAG_DEDUCED_TIMESTAMP));
        } else {
            portable::put_uint(buf, u64::from(TAG_TRANSMITTED_TIMESTAMP));
            portable::put_uint(buf, self.timestamp.to_bits());
        }
        match &self.data {
            SampleData::Float32(v) => v
                .iter()
                .for_each(|x| portable::put_uint(buf, u64::from(x.to_bits()))),
            SampleData::Double64(v) => v.iter().for_each(|x| portable::put_uint(buf, x.to_bits())),
            SampleData::Int8(v) => v.iter().for_each(|x| portable::put_int(buf, i64::from(*x))),
            SampleData::Int16(v) => v.iter().for_each(|x| portable::put_int(buf, i64::from(*x))),
            SampleData::Int32(v) => v.iter().for_each(|x| portable::put_int(buf, i64::from(*x))),
            SampleData::Int64(v) => v.iter().for_each(|x| portable::put_int(buf, *x)),
            SampleData::StringData(strings) => {
                for s in strings {
                    portable::put_uint(buf, s.len() as u64);
                    buf.extend_from_slice(s.as_bytes());
                }
            }
        }
    }

    /// Deserialize a sample from a reader (protocol 1.00, see [`serialize_100`](Self::serialize_100)).
    pub fn deserialize_100<R: Read>(
        reader: &mut R,
        fmt: ChannelFormat,
        num_channels: u32,
    ) -> IoResult<Sample> {
        let tag = portable::get_int(reader)?;
        let timestamp = if tag == i64::from(TAG_DEDUCED_TIMESTAMP) {
            DEDUCED_TIMESTAMP
        } else {
            f64::from_bits(portable::get_int(reader)? as u64)
        };
        let n = num_channels as usize;
        let ints = |reader: &mut R| -> IoResult<Vec<i64>> {
            (0..n).map(|_| portable::get_int(reader)).collect()
        };
        let data = match fmt {
            ChannelFormat::Float32 => SampleData::Float32(
                ints(reader)?
                    .into_iter()
                    .map(|b| f32::from_bits(b as u32))
                    .collect(),
            ),
            ChannelFormat::Double64 => SampleData::Double64(
                ints(reader)?
                    .into_iter()
                    .map(|b| f64::from_bits(b as u64))
                    .collect(),
            ),
            ChannelFormat::Int8 => {
                SampleData::Int8(ints(reader)?.into_iter().map(|v| v as i8).collect())
            }
            ChannelFormat::Int16 => {
                SampleData::Int16(ints(reader)?.into_iter().map(|v| v as i16).collect())
            }
            ChannelFormat::Int32 => {
                SampleData::Int32(ints(reader)?.into_iter().map(|v| v as i32).collect())
            }
            ChannelFormat::Int64 => SampleData::Int64(ints(reader)?),
            _ => {
                let mut strings = Vec::with_capacity(n);
                for _ in 0..n {
                    strings.push(portable::get_string(reader)?);
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

/// liblsl's "portable binary archive" (protocol 1.00): the pieces of it a stream feed uses.
pub mod portable {
    use std::io::{Error, ErrorKind, Read, Result};

    /// The start of an archive: the magic byte, and the archive version (9) as a number.
    pub const HEADER: [u8; 3] = [0x7f, 0x01, 0x09];
    /// What precedes the first sample of a feed: its class information (not tracked, version 0).
    pub const SAMPLE_CLASS_INFO: [u8; 2] = [0, 0];
    /// The longest string a feed is trusted to announce.
    const MAX_STRING: u64 = 64 << 20;

    /// An unsigned number: zero as one zero byte, else the count of its significant bytes and
    /// those bytes, least significant first.
    pub fn put_uint(buf: &mut Vec<u8>, value: u64) {
        if value == 0 {
            buf.push(0);
            return;
        }
        let size = (8 - value.leading_zeros() as usize / 8).max(1);
        buf.push(size as u8);
        buf.extend_from_slice(&value.to_le_bytes()[..size]);
    }

    /// A signed number: as [`put_uint`] when positive; when negative, the size is negative
    /// too and the bytes dropped are the all-ones ones.
    pub fn put_int(buf: &mut Vec<u8>, value: i64) {
        if value >= 0 {
            return put_uint(buf, value as u64);
        }
        // as liblsl counts: shift a byte at a time until nothing but the sign is left
        let (mut rest, mut size) = (value, 0i8);
        loop {
            rest >>= 8;
            size += 1;
            if rest == -1 || rest == 0 {
                break;
            }
        }
        buf.push((-size) as u8);
        buf.extend_from_slice(&value.to_le_bytes()[..size as usize]);
    }

    /// A number as [`put_int`] / [`put_uint`] wrote it (an unsigned one comes back as its bits).
    pub fn get_int<R: Read>(reader: &mut R) -> Result<i64> {
        let mut size = [0u8; 1];
        reader.read_exact(&mut size)?;
        let size = size[0] as i8;
        if size == 0 {
            return Ok(0);
        }
        let n = size.unsigned_abs() as usize;
        if n > 8 {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "a number of more than 8 bytes",
            ));
        }
        let mut bytes = if size < 0 { [0xffu8; 8] } else { [0u8; 8] };
        reader.read_exact(&mut bytes[..n])?;
        Ok(i64::from_le_bytes(bytes))
    }

    /// A string: its length, then its bytes.
    pub fn put_string(buf: &mut Vec<u8>, s: &str) {
        put_uint(buf, s.len() as u64);
        buf.extend_from_slice(s.as_bytes());
    }

    pub fn get_string<R: Read>(reader: &mut R) -> Result<String> {
        let len = get_int(reader)? as u64;
        if len > MAX_STRING {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "an impossibly long string",
            ));
        }
        let mut bytes = vec![0u8; len as usize];
        reader.read_exact(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The bytes liblsl 1.18 (set to protocol 100) put on the wire, sample by sample.
        #[test]
        fn numbers_are_written_as_liblsl_writes_them() {
            let bytes = |f: &dyn Fn(&mut Vec<u8>)| {
                let mut b = Vec::new();
                f(&mut b);
                b
            };
            let int = |v: i64| bytes(&|b| put_int(b, v));
            let bits = |v: u64| bytes(&|b| put_uint(b, v));
            assert_eq!(int(0), [0x00]);
            assert_eq!(int(5), [0x01, 0x05]);
            assert_eq!(int(-6), [0xff, 0xfa]);
            assert_eq!(int(127), [0x01, 0x7f]);
            assert_eq!(int(300), [0x02, 0x2c, 0x01]);
            assert_eq!(int(-300), [0xfe, 0xd4, 0xfe]);
            assert_eq!(int(32767), [0x02, 0xff, 0x7f]);
            assert_eq!(int(1 << 40), [0x06, 0, 0, 0, 0, 0, 1]);
            assert_eq!(int(-(1 << 40)), [0xfb, 0, 0, 0, 0, 0]);
            // floats, as their bits: 1.0f32, -2.5f32, 0.001f32, 0.0, and the test pattern's time
            assert_eq!(
                bits(u64::from(1.0f32.to_bits())),
                [0x04, 0x00, 0x00, 0x80, 0x3f]
            );
            assert_eq!(
                bits(u64::from((-2.5f32).to_bits())),
                [0x04, 0x00, 0x00, 0x20, 0xc0]
            );
            assert_eq!(
                bits(u64::from(0.001f32.to_bits())),
                [0x04, 0x6f, 0x12, 0x83, 0x3a]
            );
            assert_eq!(bits(0.0f64.to_bits()), [0x00]);
            assert_eq!(
                bits(123456.789f64.to_bits()),
                [0x08, 0xc9, 0x76, 0xbe, 0x9f, 0x0c, 0x24, 0xfe, 0x40]
            );
            for v in [
                0i64,
                1,
                -1,
                255,
                256,
                -129,
                -32768,
                i64::MAX,
                i64::MIN,
                (1 << 53) + 1,
            ] {
                assert_eq!(get_int(&mut &int(v)[..]).unwrap(), v, "{v}");
            }
            let s = bytes(&|b| put_string(b, &"x".repeat(300)));
            assert_eq!(s[..3], [0x02, 0x2c, 0x01]);
            assert_eq!(get_string(&mut &s[..]).unwrap().len(), 300);
            assert_eq!(bytes(&|b| put_string(b, "")), [0x00]);
        }
    }
}
