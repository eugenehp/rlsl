//! Per-chunk compression for the iroh tunnel.
//!
//! # Framing
//!
//! Every compressed chunk on the wire is:
//!
//! ```text
//! [u16 LE compressed_len] [u16 LE uncompressed_len] [compressed payload]
//! ```
//!
//! 4 bytes of overhead per chunk. If `compressed_len == uncompressed_len`
//! the payload is stored verbatim (incompressible data fallback).
//!
//! # Available codecs
//!
//! | Mode          | Codec          | Speed          | Ratio  | Use case                        |
//! |---------------|----------------|----------------|--------|---------------------------------|
//! | `None`        | —              | —              | 1.0×   | LAN, localhost                  |
//! | `Lz4`         | LZ4 block      | ~3 GB/s enc    | 1.5–2× | Default when bandwidth-limited  |
//! | `Zstd1`       | Zstandard L1   | ~800 MB/s enc  | 2–3×   | Balanced speed / ratio          |
//! | `Zstd3`       | Zstandard L3   | ~400 MB/s enc  | 2.5–4× | Better ratio, still fast        |
//! | `Snappy`      | Snappy         | ~2 GB/s enc    | 1.5–2× | Similar to LZ4, Google standard |
//! | `DeltaLz4`    | XOR-delta+LZ4  | ~2 GB/s enc    | 3–8×   | Best for numeric physio signals |
//! | `StreamZstd`  | see below      | —              | 1.4×   | Sampled signals, sample by sample |
//!
//! # `StreamZstd`: compression that remembers
//!
//! The tunnel sends one chunk per sample, and every mode above compresses each chunk on its own:
//! a hundred-odd bytes with nothing to refer to, behind a 4-byte frame. What makes a sampled
//! signal compressible is the *previous sample*, which those modes never see.
//!
//! `StreamZstd` keeps state across chunks ([`ChunkEncoder`] / [`ChunkDecoder`], one pair per
//! stream; QUIC streams are ordered and lossless, so both sides stay in step):
//!
//! 1. each numeric sample is XORed with the one before it (values that barely moved become
//!    mostly-zero bytes, and so does the time stamp),
//! 2. the value bytes are regrouped by significance (all the high bytes of the channels
//!    together, …), and
//! 3. the result goes through one Zstandard stream that is flushed after every chunk, so each
//!    sample still leaves at once but is coded against everything sent before.
//!
//! Measured on a real 32-channel, 500 Hz float32 EEG recording sent one sample per chunk: the
//! per-sample modes put *more* bytes on the wire than no compression (103 %: nothing to gain in
//! one sample, plus the frame), `StreamZstd` 70 %. On a quiet synthetic signal it is 97 %.
//!
//! It is a new wire mode (6): both ends need a version that knows it. Nothing changes for the
//! other modes.

/// Compression mode negotiated in the stream header (1 byte on wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Compression {
    /// No compression (default). Raw protocol-1.10 bytes.
    None = 0,
    /// LZ4 block compression.
    Lz4 = 1,
    /// Zstandard level 1 (fast).
    Zstd1 = 2,
    /// Zstandard level 3 (balanced).
    Zstd3 = 3,
    /// Google Snappy.
    Snappy = 4,
    /// XOR-delta encoding followed by LZ4.
    /// Dramatically improves ratio on numeric physiological signals
    /// where consecutive samples differ only slightly.
    DeltaLz4 = 5,
    /// Delta against the previous sample, bytes grouped by significance, one Zstandard stream
    /// flushed per chunk. Stateful: use [`ChunkEncoder`] / [`ChunkDecoder`].
    StreamZstd = 6,
}

impl Compression {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Lz4,
            2 => Self::Zstd1,
            3 => Self::Zstd3,
            4 => Self::Snappy,
            5 => Self::DeltaLz4,
            6 => Self::StreamZstd,
            _ => Self::None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Lz4 => "lz4",
            Self::Zstd1 => "zstd1",
            Self::Zstd3 => "zstd3",
            Self::Snappy => "snappy",
            Self::DeltaLz4 => "delta-lz4",
            Self::StreamZstd => "stream-zstd",
        }
    }

    pub fn from_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "lz4" => Self::Lz4,
            "zstd" | "zstd1" => Self::Zstd1,
            "zstd3" | "zstd-high" => Self::Zstd3,
            "snappy" | "snap" => Self::Snappy,
            "delta-lz4" | "delta_lz4" | "dlz4" => Self::DeltaLz4,
            "stream-zstd" | "stream_zstd" | "szstd" => Self::StreamZstd,
            _ => Self::None,
        }
    }

    /// Returns true if this mode uses the chunked framing wire format.
    pub fn is_compressed(&self) -> bool {
        *self != Self::None
    }
}

impl std::fmt::Display for Compression {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ── Compress ─────────────────────────────────────────────────────────

/// Compress a chunk of raw sample bytes using the given codec.
/// Output: `[u16 compressed_len][u16 uncompressed_len][payload]`.
///
/// Falls back to storing uncompressed if the codec expands the data.
pub fn compress_chunk(raw: &[u8], mode: Compression, out: &mut Vec<u8>) {
    debug_assert!(raw.len() <= u16::MAX as usize);
    let uncompressed_len = raw.len();

    let compressed_payload = match mode {
        Compression::None => unreachable!("compress_chunk called with None"),
        Compression::Lz4 => compress_lz4(raw),
        Compression::Zstd1 => compress_zstd(raw, 1),
        Compression::Zstd3 => compress_zstd(raw, 3),
        Compression::Snappy => compress_snappy(raw),
        Compression::DeltaLz4 => {
            let delta = delta_encode(raw);
            compress_lz4(&delta)
        }
        Compression::StreamZstd => {
            // Stateful mode: a chunk on its own is a stream of one (see `ChunkEncoder`).
            ChunkEncoder::new(Compression::StreamZstd, 1).compress(raw, out);
            return;
        }
    };

    if compressed_payload.len() < uncompressed_len {
        out.extend_from_slice(&(compressed_payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&(uncompressed_len as u16).to_le_bytes());
        out.extend_from_slice(&compressed_payload);
    } else {
        // Store uncompressed — sentinel: compressed_len == uncompressed_len
        out.extend_from_slice(&(uncompressed_len as u16).to_le_bytes());
        out.extend_from_slice(&(uncompressed_len as u16).to_le_bytes());
        out.extend_from_slice(raw);
    }
}

/// Decompress a framed chunk.
/// Returns `(decompressed_bytes, total_bytes_consumed_from_input)`.
/// Returns `None` if the input is too short (need more data).
pub fn decompress_chunk(input: &[u8], mode: Compression) -> Option<(Vec<u8>, usize)> {
    if input.len() < 4 {
        return None;
    }
    let compressed_len = u16::from_le_bytes([input[0], input[1]]) as usize;
    let uncompressed_len = u16::from_le_bytes([input[2], input[3]]) as usize;
    let total = 4 + compressed_len;

    if input.len() < total {
        return None;
    }

    let payload = &input[4..total];

    if mode == Compression::StreamZstd {
        // Stateful mode: a chunk on its own is a stream of one (see `ChunkDecoder`).
        return ChunkDecoder::new(Compression::StreamZstd, 1).decompress(input);
    }

    if compressed_len == uncompressed_len {
        // Stored uncompressed
        return Some((payload.to_vec(), total));
    }

    let decompressed = match mode {
        Compression::None => unreachable!(),
        Compression::Lz4 => decompress_lz4(payload, uncompressed_len)?,
        Compression::Zstd1 | Compression::Zstd3 => decompress_zstd(payload)?,
        Compression::Snappy => decompress_snappy(payload)?,
        Compression::DeltaLz4 => {
            let delta = decompress_lz4(payload, uncompressed_len)?;
            delta_decode(&delta)
        }
        Compression::StreamZstd => unreachable!("handled above"),
    };

    Some((decompressed, total))
}

// ── Stateful chunk codec ─────────────────────────────────────────────

/// Compresses the chunks of one stream, in order. For every mode but
/// [`Compression::StreamZstd`] it is [`compress_chunk`]; for that one it carries the previous
/// sample and the Zstandard stream from chunk to chunk ([`rlsl::compress::StreamEncoder`]).
pub struct ChunkEncoder {
    mode: Compression,
    stream: Option<rlsl::compress::StreamEncoder>,
}

impl ChunkEncoder {
    /// `value_bytes` is the size of one channel value of the stream
    /// (`ChannelFormat::channel_bytes()`; 0 for string streams).
    pub fn new(mode: Compression, value_bytes: usize) -> Self {
        Self {
            mode,
            stream: (mode == Compression::StreamZstd)
                .then(|| rlsl::compress::StreamEncoder::new(value_bytes)),
        }
    }

    /// Compress one chunk (a serialised sample) and append its frame to `out`:
    /// `[u16 compressed_len][u16 uncompressed_len][payload]`.
    pub fn compress(&mut self, raw: &[u8], out: &mut Vec<u8>) {
        match self.stream.as_mut() {
            Some(enc) => enc.encode(raw, out),
            None => compress_chunk(raw, self.mode, out),
        }
    }
}

/// Decompresses the chunks of one stream, in the order they were compressed. The counterpart of
/// [`ChunkEncoder`].
pub struct ChunkDecoder {
    mode: Compression,
    stream: Option<rlsl::compress::StreamDecoder>,
}

impl ChunkDecoder {
    pub fn new(mode: Compression, value_bytes: usize) -> Self {
        Self {
            mode,
            stream: (mode == Compression::StreamZstd)
                .then(|| rlsl::compress::StreamDecoder::new(value_bytes)),
        }
    }

    /// Decompress the framed chunk at the start of `input`.
    /// Returns `(bytes, input_consumed)`, or `None` when `input` does not hold a whole chunk
    /// yet — or, for `StreamZstd`, when the stream is damaged.
    pub fn decompress(&mut self, input: &[u8]) -> Option<(Vec<u8>, usize)> {
        match self.stream.as_mut() {
            Some(dec) => dec.decode(input),
            None => decompress_chunk(input, self.mode),
        }
    }
}

// ── Codec implementations ────────────────────────────────────────────

fn compress_lz4(data: &[u8]) -> Vec<u8> {
    let with_size = lz4_flex::compress_prepend_size(data);
    // Strip the 4-byte original-size prefix that lz4_flex adds
    with_size[4..].to_vec()
}

fn decompress_lz4(data: &[u8], uncompressed_len: usize) -> Option<Vec<u8>> {
    lz4_flex::decompress(data, uncompressed_len).ok()
}

fn compress_zstd(data: &[u8], level: i32) -> Vec<u8> {
    zstd::bulk::compress(data, level).unwrap_or_else(|_| data.to_vec())
}

fn decompress_zstd(data: &[u8]) -> Option<Vec<u8>> {
    // zstd frame carries its own uncompressed size
    zstd::bulk::decompress(data, 65536).ok()
}

fn compress_snappy(data: &[u8]) -> Vec<u8> {
    let mut enc = snap::raw::Encoder::new();
    enc.compress_vec(data).unwrap_or_else(|_| data.to_vec())
}

fn decompress_snappy(data: &[u8]) -> Option<Vec<u8>> {
    let mut dec = snap::raw::Decoder::new();
    dec.decompress_vec(data).ok()
}

// ── Delta encoding ───────────────────────────────────────────────────

/// XOR-delta encode: output[0] = input[0], output[i] = input[i] ^ input[i-1].
///
/// For numeric physiological signals where consecutive bytes are similar,
/// most delta bytes become zero → extremely compressible by LZ4/zstd.
fn delta_encode(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(data.len());
    out.push(data[0]);
    for i in 1..data.len() {
        out.push(data[i] ^ data[i - 1]);
    }
    out
}

/// Reverse XOR-delta encoding.
fn delta_decode(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(data.len());
    out.push(data[0]);
    for i in 1..data.len() {
        out.push(data[i] ^ out[i - 1]);
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(mode: Compression, data: &[u8]) {
        let mut compressed = Vec::new();
        compress_chunk(data, mode, &mut compressed);

        let (decompressed, consumed) = decompress_chunk(&compressed, mode).unwrap();
        assert_eq!(
            consumed,
            compressed.len(),
            "mode={:?}: consumed mismatch",
            mode
        );
        assert_eq!(decompressed, data, "mode={:?}: data mismatch", mode);
    }

    #[test]
    fn roundtrip_all_codecs_compressible() {
        let raw = vec![42u8; 1024];
        for mode in [
            Compression::Lz4,
            Compression::Zstd1,
            Compression::Zstd3,
            Compression::Snappy,
            Compression::DeltaLz4,
        ] {
            roundtrip(mode, &raw);
        }
    }

    #[test]
    fn roundtrip_all_codecs_random() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut raw = vec![0u8; 256];
        for (i, b) in raw.iter_mut().enumerate() {
            let mut h = DefaultHasher::new();
            i.hash(&mut h);
            *b = h.finish() as u8;
        }
        for mode in [
            Compression::Lz4,
            Compression::Zstd1,
            Compression::Zstd3,
            Compression::Snappy,
            Compression::DeltaLz4,
        ] {
            roundtrip(mode, &raw);
        }
    }

    #[test]
    fn roundtrip_all_codecs_tiny() {
        // Minimal: 1 byte
        for mode in [
            Compression::Lz4,
            Compression::Zstd1,
            Compression::Zstd3,
            Compression::Snappy,
            Compression::DeltaLz4,
        ] {
            roundtrip(mode, &[0xAB]);
        }
    }

    #[test]
    fn partial_input_returns_none() {
        for mode in [Compression::Lz4, Compression::Zstd1, Compression::Snappy] {
            assert!(decompress_chunk(&[], mode).is_none());
            assert!(decompress_chunk(&[1, 0, 2, 0], mode).is_none());
        }
    }

    #[test]
    fn delta_encode_decode_identity() {
        let data: Vec<u8> = (0..=255).collect();
        let encoded = delta_encode(&data);
        let decoded = delta_decode(&encoded);
        assert_eq!(decoded, data);
    }

    #[test]
    fn delta_encode_decode_empty() {
        assert!(delta_encode(&[]).is_empty());
        assert!(delta_decode(&[]).is_empty());
    }

    #[test]
    fn delta_lz4_compresses_better_than_plain_lz4() {
        // Slowly changing signal — delta should compress much better
        let mut raw = vec![0u8; 1024];
        for (i, b) in raw.iter_mut().enumerate() {
            *b = (100 + (i / 32) as u8).wrapping_add((i % 4) as u8);
        }

        let mut lz4_out = Vec::new();
        compress_chunk(&raw, Compression::Lz4, &mut lz4_out);

        let mut delta_out = Vec::new();
        compress_chunk(&raw, Compression::DeltaLz4, &mut delta_out);

        assert!(
            delta_out.len() <= lz4_out.len(),
            "delta-lz4 ({}) should be <= lz4 ({}) for correlated data",
            delta_out.len(),
            lz4_out.len()
        );
    }

    #[test]
    fn from_name_variants() {
        assert_eq!(Compression::from_name("none"), Compression::None);
        assert_eq!(Compression::from_name("lz4"), Compression::Lz4);
        assert_eq!(Compression::from_name("zstd"), Compression::Zstd1);
        assert_eq!(Compression::from_name("zstd1"), Compression::Zstd1);
        assert_eq!(Compression::from_name("zstd3"), Compression::Zstd3);
        assert_eq!(Compression::from_name("zstd-high"), Compression::Zstd3);
        assert_eq!(Compression::from_name("snappy"), Compression::Snappy);
        assert_eq!(Compression::from_name("snap"), Compression::Snappy);
        assert_eq!(Compression::from_name("delta-lz4"), Compression::DeltaLz4);
        assert_eq!(Compression::from_name("dlz4"), Compression::DeltaLz4);
        assert_eq!(Compression::from_name("garbage"), Compression::None);
    }

    // ── StreamZstd ───────────────────────────────────────────────────

    /// Serialised float32 samples of a 32-channel EEG as an amplifier delivers it (electrode
    /// offsets, mains pickup, a slow wander, broadband noise), as the tunnel sends them.
    fn eeg_samples(n: usize) -> Vec<Vec<u8>> {
        let mut state = 0x2545F4914F6CDD1Du64;
        let mut level = [0f64; 32];
        (0..n)
            .map(|k| {
                let mut raw = vec![rlsl::types::TAG_TRANSMITTED_TIMESTAMP];
                raw.extend_from_slice(&(12_345.678 + k as f64 / 500.0).to_le_bytes());
                for (c, l) in level.iter_mut().enumerate() {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    let w = (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
                    *l = 0.95 * *l + w * 20.0;
                    // quantised to an amplifier's step, like a real recording
                    let t = k as f64 / 500.0;
                    let mains = (300.0 + 40.0 * c as f64)
                        * (std::f64::consts::TAU * 50.0 * t + c as f64).sin();
                    let offset = -9000.0 - 700.0 * c as f64 + 2.0 * t;
                    let uv =
                        ((offset + mains + *l + w * 6.0) / 0.047_683_72).round() * 0.047_683_72;
                    raw.extend_from_slice(&(uv as f32).to_le_bytes());
                }
                raw
            })
            .collect()
    }

    fn wire_bytes(mode: Compression, value_bytes: usize, samples: &[Vec<u8>]) -> Vec<u8> {
        let mut enc = ChunkEncoder::new(mode, value_bytes);
        let mut wire = Vec::new();
        for s in samples {
            enc.compress(s, &mut wire);
        }
        wire
    }

    fn decode_all(mode: Compression, value_bytes: usize, wire: &[u8]) -> Vec<Vec<u8>> {
        let mut dec = ChunkDecoder::new(mode, value_bytes);
        let (mut out, mut at) = (Vec::new(), 0);
        while at < wire.len() {
            let (raw, used) = dec.decompress(&wire[at..]).expect("a whole chunk");
            out.push(raw);
            at += used;
        }
        out
    }

    #[test]
    fn stream_zstd_round_trips_every_kind_of_chunk() {
        let mut samples = eeg_samples(300);
        // a sample without its own time stamp, a different length, an empty chunk, a string
        // sample, then numbers again: the delta is dropped and picked up as the shape changes
        let mut deduced = vec![rlsl::types::TAG_DEDUCED_TIMESTAMP];
        deduced.extend_from_slice(&samples[5][9..]);
        samples.insert(100, deduced.clone());
        samples.insert(101, deduced);
        samples.insert(150, vec![rlsl::types::TAG_TRANSMITTED_TIMESTAMP, 1, 2, 3]);
        samples.insert(151, Vec::new());
        samples.insert(152, b"\x02\0\0\0\0\0\0\0\0\x01\x05hello".to_vec());
        for value_bytes in [0, 1, 2, 4, 8] {
            let wire = wire_bytes(Compression::StreamZstd, value_bytes, &samples);
            assert_eq!(
                decode_all(Compression::StreamZstd, value_bytes, &wire),
                samples,
                "value_bytes {value_bytes}"
            );
        }
        // partial input: nothing is consumed until the chunk is whole
        let wire = wire_bytes(Compression::StreamZstd, 4, &samples[..2]);
        let mut dec = ChunkDecoder::new(Compression::StreamZstd, 4);
        assert!(dec.decompress(&wire[..3]).is_none());
        let first = 4 + u16::from_le_bytes([wire[0], wire[1]]) as usize;
        assert!(dec.decompress(&wire[..first - 1]).is_none());
        assert_eq!(dec.decompress(&wire).unwrap(), (samples[0].clone(), first));
    }

    #[test]
    fn stateful_codec_is_the_stateless_one_for_the_older_modes() {
        let samples = eeg_samples(50);
        for mode in [
            Compression::Lz4,
            Compression::Zstd1,
            Compression::Zstd3,
            Compression::Snappy,
            Compression::DeltaLz4,
        ] {
            let mut stateless = Vec::new();
            for s in &samples {
                compress_chunk(s, mode, &mut stateless);
            }
            assert_eq!(wire_bytes(mode, 4, &samples), stateless, "{mode:?}");
            assert_eq!(decode_all(mode, 4, &stateless), samples, "{mode:?}");
        }
        // and a lone chunk in the new mode goes through the stateless calls
        let mut one = Vec::new();
        compress_chunk(&samples[0], Compression::StreamZstd, &mut one);
        assert_eq!(
            decompress_chunk(&one, Compression::StreamZstd).unwrap(),
            (samples[0].clone(), one.len())
        );
    }

    /// One minute of 32-channel float32 EEG at 500 Hz, sample by sample: the modes that
    /// compress each sample alone gain little or nothing; the one that remembers the previous
    /// samples sends clearly less.
    #[test]
    fn stream_zstd_beats_the_per_sample_modes_on_a_sampled_signal() {
        let samples = eeg_samples(500 * 60);
        let raw: usize = samples.iter().map(Vec::len).sum();
        let mut report = format!("raw {:.2} MB;", raw as f64 / 1e6);
        let mut best_other = usize::MAX;
        for mode in [
            Compression::Lz4,
            Compression::Zstd1,
            Compression::Zstd3,
            Compression::Snappy,
            Compression::DeltaLz4,
        ] {
            let n = wire_bytes(mode, 4, &samples).len();
            report += &format!(" {mode} {:.0}%;", n as f64 / raw as f64 * 100.0);
            best_other = best_other.min(n);
        }
        let wire = wire_bytes(Compression::StreamZstd, 4, &samples);
        report += &format!(
            " stream-zstd {:.0}%",
            wire.len() as f64 / raw as f64 * 100.0
        );
        eprintln!("32-channel float32 EEG, one chunk per sample: {report}");
        assert!(wire.len() < raw * 9 / 10, "{report}");
        assert!(wire.len() < best_other, "{report}");
        assert_eq!(decode_all(Compression::StreamZstd, 4, &wire), samples);
    }

    #[test]
    fn stream_zstd_names() {
        assert_eq!(Compression::from_u8(6), Compression::StreamZstd);
        assert_eq!(
            Compression::from_name("stream-zstd"),
            Compression::StreamZstd
        );
        assert_eq!(Compression::StreamZstd.as_str(), "stream-zstd");
        assert!(Compression::StreamZstd.is_compressed());
    }
}
