//! Stream compression for sampled signals (feature `compression`).
//!
//! A sample of a multi-channel signal is a hundred-odd bytes with nothing in it to refer to:
//! compressing samples one at a time gains nothing. What makes the signal compressible is the
//! *previous sample*. [`StreamEncoder`] / [`StreamDecoder`] keep that state for one stream of
//! serialised samples (protocol 1.10 bytes), in order and without loss:
//!
//! 1. each sample is XORed with the one before it (values that barely moved become mostly-zero
//!    bytes, and so does the time stamp),
//! 2. the value bytes are regrouped by significance (all the high bytes of the channels
//!    together, …), and
//! 3. the result goes through one Zstandard stream that is flushed after every sample, so each
//!    sample still leaves at once but is coded against everything sent before.
//!
//! Every sample becomes one frame: `[u16 LE payload_len][u16 LE sample_len][payload]`.
//!
//! On a real 32-channel, 500 Hz float32 EEG recording this puts 70 % of the bytes on the wire.
//!
//! It is used between an rlsl inlet and an rlsl outlet when the inlet asks for it (see
//! [`StreamInlet::set_compression`](crate::inlet::StreamInlet::set_compression)), and by the
//! `rlsl-iroh` tunnel's `stream-zstd` mode.

use std::io::Write as _;

/// The request / response header an inlet and an outlet agree on compression with.
pub const HEADER: &str = "Rlsl-Compression";
/// The one scheme there is.
pub const STREAM_ZSTD: &str = "stream-zstd";
/// Largest sample (serialised) a frame can carry.
pub const MAX_SAMPLE_BYTES: usize = u16::MAX as usize;

/// Compresses the samples of one stream, in order.
pub struct StreamEncoder {
    /// Bytes per channel value (4 for float32, 8 for double64, …); 0 or 1 = do not regroup.
    value_bytes: usize,
    prev: Vec<u8>,
    scratch: Vec<u8>,
    zstd: zstd::stream::write::Encoder<'static, Vec<u8>>,
}

impl StreamEncoder {
    /// `value_bytes` is the size of one channel value of the stream
    /// (`ChannelFormat::channel_bytes()`; 0 for string streams).
    pub fn new(value_bytes: usize) -> Self {
        let mut zstd = zstd::stream::write::Encoder::new(Vec::new(), 3).expect("zstd encoder");
        // samples are small: a modest window is all the history that helps
        let _ = zstd.window_log(17);
        let _ = zstd.include_checksum(false);
        Self {
            value_bytes,
            prev: Vec::new(),
            scratch: Vec::new(),
            zstd,
        }
    }

    /// Compress one serialised sample (at most [`MAX_SAMPLE_BYTES`]) and append its frame to
    /// `out`.
    pub fn encode(&mut self, raw: &[u8], out: &mut Vec<u8>) {
        debug_assert!(raw.len() <= MAX_SAMPLE_BYTES);
        transform(raw, &self.prev, self.value_bytes, &mut self.scratch);
        self.prev.clear();
        self.prev.extend_from_slice(raw);
        // A flush ends a block: everything of this sample is in the encoder's output, coded
        // against all the samples before it.
        let written = self
            .zstd
            .write_all(&self.scratch)
            .and_then(|()| self.zstd.flush());
        let payload = self.zstd.get_mut();
        if written.is_err() || payload.len() > MAX_SAMPLE_BYTES {
            // Cannot happen with an in-memory sink and samples this size; an empty payload
            // makes the receiver stop rather than read a broken stream.
            payload.clear();
        }
        out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        out.extend_from_slice(payload);
        payload.clear();
    }
}

/// Decompresses the samples of one stream, in the order they were compressed.
pub struct StreamDecoder {
    value_bytes: usize,
    prev: Vec<u8>,
    zstd: zstd::stream::write::Decoder<'static, Vec<u8>>,
}

impl StreamDecoder {
    pub fn new(value_bytes: usize) -> Self {
        Self {
            value_bytes,
            prev: Vec::new(),
            zstd: zstd::stream::write::Decoder::new(Vec::new()).expect("zstd decoder"),
        }
    }

    /// Decompress the frame at the start of `input`.
    /// Returns `(sample_bytes, input_consumed)`, or `None` when `input` does not hold a whole
    /// frame yet, or the stream is damaged.
    pub fn decode(&mut self, input: &[u8]) -> Option<(Vec<u8>, usize)> {
        if input.len() < 4 {
            return None;
        }
        let total = 4 + u16::from_le_bytes([input[0], input[1]]) as usize;
        if input.len() < total {
            return None;
        }
        let sample_len = u16::from_le_bytes([input[2], input[3]]) as usize;
        let raw = self.decode_payload(&input[4..total], sample_len)?;
        Some((raw, total))
    }

    /// Decompress the payload of one frame whose header said `sample_len`.
    pub fn decode_payload(&mut self, payload: &[u8], sample_len: usize) -> Option<Vec<u8>> {
        self.zstd.write_all(payload).ok()?;
        self.zstd.flush().ok()?;
        let transformed = std::mem::take(self.zstd.get_mut());
        let raw = restore(&transformed, &self.prev, self.value_bytes)?;
        if raw.len() != sample_len {
            return None;
        }
        self.prev.clear();
        self.prev.extend_from_slice(&raw);
        Some(raw)
    }
}

/// Length of the part of a serialised sample before the channel values: the tag, and the time
/// stamp when the sample carries one.
fn head_len(raw: &[u8]) -> usize {
    match raw.first() {
        Some(&crate::types::TAG_TRANSMITTED_TIMESTAMP) => 9.min(raw.len()),
        Some(_) => 1,
        None => 0,
    }
}

/// `out = [delta flag][tag][rest]`: `rest` is the sample after its tag — XORed with the same
/// bytes of `prev` when the two have the same shape (flag 1) — with the channel values
/// regrouped by byte significance when they are a whole number of `value_bytes`.
fn transform(raw: &[u8], prev: &[u8], value_bytes: usize, out: &mut Vec<u8>) {
    out.clear();
    let delta = !raw.is_empty() && prev.len() == raw.len() && prev[0] == raw[0];
    out.push(delta as u8);
    let Some((&tag, rest)) = raw.split_first() else {
        return;
    };
    out.push(tag);
    let head = head_len(raw);
    let start = out.len();
    if delta {
        out.extend(rest.iter().zip(&prev[1..]).map(|(a, b)| a ^ b));
    } else {
        out.extend_from_slice(rest);
    }
    // the values begin `head - 1` bytes into `rest`
    let body = start + head - 1;
    let n = out.len() - body;
    if value_bytes > 1 && n > 0 && n.is_multiple_of(value_bytes) {
        let values: Vec<u8> = out[body..].to_vec();
        let count = n / value_bytes;
        for (i, byte) in values.iter().enumerate() {
            out[body + (i % value_bytes) * count + i / value_bytes] = *byte;
        }
    }
}

/// The inverse of [`transform`]; `None` when the bytes cannot be one.
fn restore(transformed: &[u8], prev: &[u8], value_bytes: usize) -> Option<Vec<u8>> {
    let (&flag, rest) = transformed.split_first()?;
    let Some((&tag, rest)) = rest.split_first() else {
        return (flag == 0).then(Vec::new);
    };
    let mut raw = Vec::with_capacity(1 + rest.len());
    raw.push(tag);
    raw.extend_from_slice(rest);
    let head = head_len(&raw);
    let n = raw.len() - head;
    if value_bytes > 1 && n > 0 && n.is_multiple_of(value_bytes) {
        let grouped: Vec<u8> = raw[head..].to_vec();
        let count = n / value_bytes;
        for (i, slot) in raw[head..].iter_mut().enumerate() {
            *slot = grouped[(i % value_bytes) * count + i / value_bytes];
        }
    }
    match flag {
        0 => {}
        1 => {
            if prev.len() != raw.len() || prev[0] != tag {
                return None;
            }
            for (a, b) in raw[1..].iter_mut().zip(&prev[1..]) {
                *a ^= b;
            }
        }
        _ => return None,
    }
    Some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_of_every_shape_round_trip() {
        let sample = |k: u32, tag: u8| -> Vec<u8> {
            let mut raw = vec![tag];
            if tag == crate::types::TAG_TRANSMITTED_TIMESTAMP {
                raw.extend_from_slice(&(100.0 + f64::from(k) / 250.0).to_le_bytes());
            }
            for c in 0..8u32 {
                raw.extend_from_slice(&((k * 3 + c) as f32 * 0.25).to_le_bytes());
            }
            raw
        };
        let mut samples: Vec<Vec<u8>> = (0..200)
            .map(|k| sample(k, crate::types::TAG_TRANSMITTED_TIMESTAMP))
            .collect();
        samples.insert(50, sample(7, crate::types::TAG_DEDUCED_TIMESTAMP));
        samples.insert(51, sample(8, crate::types::TAG_DEDUCED_TIMESTAMP));
        samples.insert(90, Vec::new());
        samples.insert(91, b"\x02\0\0\0\0\0\0\0\0\x01\x05hello".to_vec());
        for value_bytes in [0, 1, 2, 4, 8] {
            let (mut enc, mut dec) = (
                StreamEncoder::new(value_bytes),
                StreamDecoder::new(value_bytes),
            );
            let mut wire = Vec::new();
            for s in &samples {
                enc.encode(s, &mut wire);
            }
            let mut at = 0;
            for s in &samples {
                assert!(
                    dec.decode(&wire[at..at + 3]).is_none(),
                    "a partial frame is not consumed"
                );
                let (raw, used) = dec.decode(&wire[at..]).expect("a whole frame");
                assert_eq!(&raw, s, "value_bytes {value_bytes}");
                at += used;
            }
            assert_eq!(at, wire.len());
        }
    }
}
