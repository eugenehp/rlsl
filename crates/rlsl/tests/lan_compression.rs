//! Compression between an inlet and an outlet on the network: asked for by the inlet, given by
//! an outlet that can, invisible to everyone else — and lossless.

use rlsl::prelude::*;
use rlsl::types::ChannelFormat;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Whether this build of rlsl has the `compression` feature.
const BUILT_WITH_COMPRESSION: bool = cfg!(feature = "compression");

fn resolve(name: &str) -> StreamInfo {
    rlsl::resolver::resolve_by_property("name", name, 1, 10.0)
        .into_iter()
        .next()
        .expect("stream resolves")
}

fn wait_for_consumers(outlet: &StreamOutlet, n: usize) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(5) {
        if outlet.have_consumers() && n <= 1 {
            return;
        }
        if n > 1 && t0.elapsed() > Duration::from_millis(500) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A 32-channel EEG sample like an amplifier's: offsets, mains pickup, noise.
fn eeg_sample(k: u64, state: &mut u64) -> Vec<f32> {
    (0..32)
        .map(|c| {
            *state ^= *state << 13;
            *state ^= *state >> 7;
            *state ^= *state << 17;
            let w = (*state >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            let t = k as f64 / 500.0;
            let mains =
                (300.0 + 40.0 * c as f64) * (std::f64::consts::TAU * 50.0 * t + c as f64).sin();
            let uv = -9000.0 - 700.0 * c as f64 + mains + w * 30.0;
            ((uv / 0.047_683_72).round() * 0.047_683_72) as f32
        })
        .collect()
}

/// An inlet that asks gets a compressed feed from an outlet built for it (and a plain one
/// otherwise), an inlet that does not ask gets the plain protocol from the same outlet, and
/// both receive every value bit for bit.
#[test]
fn a_compressed_and_a_plain_inlet_receive_the_same_samples() {
    let info = StreamInfo::new(
        "rlsl-test-lan-f32",
        "EEG",
        32,
        500.0,
        ChannelFormat::Float32,
        "lan-f32",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let found = resolve("rlsl-test-lan-f32");

    let squeezed = StreamInlet::new(&found, 360, 0, true);
    squeezed.set_compression(true);
    squeezed
        .open_stream(10.0)
        .expect("compressed inlet connects");
    let plain = StreamInlet::new(&found, 360, 0, true);
    plain.open_stream(10.0).expect("plain inlet connects");
    assert_eq!(squeezed.is_compressed(), BUILT_WITH_COMPRESSION);
    assert!(!plain.is_compressed());
    wait_for_consumers(&outlet, 2);

    let mut state = 0x2545F4914F6CDD1Du64;
    let sent: Vec<Vec<f32>> = (0..600).map(|k| eeg_sample(k, &mut state)).collect();
    let mut buf = vec![0f32; 32];
    // a hundred at a time, read by both before the next hundred (as a live stream is)
    for (batch, samples) in sent.chunks(100).enumerate() {
        for (i, s) in samples.iter().enumerate() {
            outlet.push_sample_f(s, 100.0 + (batch * 100 + i) as f64 / 500.0, true);
        }
        for (name, inlet) in [("compressed", &squeezed), ("plain", &plain)] {
            for (i, want) in samples.iter().enumerate() {
                let k = batch * 100 + i;
                let ts = inlet.pull_sample_f(&mut buf, 5.0).unwrap();
                assert!(
                    (ts - (100.0 + k as f64 / 500.0)).abs() < 1e-9,
                    "{name} inlet, sample {k}: stamp {ts}"
                );
                assert_eq!(
                    buf.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    want.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                    "{name} inlet, sample {k}"
                );
            }
        }
    }
}

/// 64-bit values and chunks go through the compressed feed exactly too.
#[test]
fn sixty_four_bit_streams_survive_compression() {
    for (name, fmt) in [
        ("rlsl-test-lan-i64", ChannelFormat::Int64),
        ("rlsl-test-lan-f64", ChannelFormat::Double64),
    ] {
        let info = StreamInfo::new(name, "Test", 3, 100.0, fmt, name);
        let outlet = StreamOutlet::new(&info, 0, 360);
        let inlet = StreamInlet::new(&resolve(name), 360, 0, true);
        inlet.set_compression(true);
        inlet.open_stream(10.0).unwrap();
        assert_eq!(inlet.is_compressed(), BUILT_WITH_COMPRESSION);
        wait_for_consumers(&outlet, 1);
        if fmt == ChannelFormat::Int64 {
            let chunk: Vec<i64> = (0..60)
                .map(|k| [i64::MAX - k, i64::MIN + k, (1 << 53) + 1 + k][(k % 3) as usize])
                .collect();
            outlet.push_chunk_i64(&chunk, 0.0, true);
            let (mut got, t0) = (Vec::new(), Instant::now());
            while got.len() < chunk.len() && t0.elapsed() < Duration::from_secs(5) {
                got.extend(inlet.pull_chunk_i64(64, 0.5).unwrap().1);
            }
            assert_eq!(got, chunk);
        } else {
            let chunk: Vec<f64> = (0..60)
                .map(|k| std::f64::consts::PI * 1e-9 * k as f64 + 1_700_000_000.123_456_7)
                .collect();
            outlet.push_chunk_d(&chunk, 0.0, true);
            let (mut got, t0) = (Vec::new(), Instant::now());
            while got.len() < chunk.len() && t0.elapsed() < Duration::from_secs(5) {
                got.extend(inlet.pull_chunk_d(64, 0.5).unwrap().1);
            }
            assert_eq!(
                got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                chunk.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
    }
}

/// A string stream is not compressed even when asked: the inlet is told so and reads it as
/// ever.
#[test]
fn string_streams_stay_plain() {
    let info = StreamInfo::new(
        "rlsl-test-lan-str",
        "Markers",
        1,
        0.0,
        ChannelFormat::String,
        "lan-str",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let inlet = StreamInlet::new(&resolve("rlsl-test-lan-str"), 360, 0, true);
    inlet.set_compression(true);
    inlet.open_stream(10.0).unwrap();
    assert!(!inlet.is_compressed());
    wait_for_consumers(&outlet, 1);
    outlet.push_sample_str(&["stimulus, left".to_string()], 0.0, true);
    let (values, ts) = inlet.pull_sample_str(5.0).unwrap();
    assert!(ts > 0.0);
    assert_eq!(values, ["stimulus, left"]);
}

/// The bytes an outlet sends a client, read straight off the socket: a client that sends the
/// plain LSL request (as liblsl does) gets the plain protocol, with no sign of compression in
/// the reply; one that adds the request header gets fewer bytes for the same samples.
#[test]
fn the_wire_is_untouched_unless_asked_and_smaller_when_asked() {
    let info = StreamInfo::new(
        "rlsl-test-lan-wire",
        "EEG",
        32,
        500.0,
        ChannelFormat::Float32,
        "lan-wire",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let found = resolve("rlsl-test-lan-wire");
    let addr = format!("127.0.0.1:{}", found.v4data_port());
    let connect = |extra: &str| {
        let mut s = std::net::TcpStream::connect(&addr).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(300)))
            .unwrap();
        let request = format!(
            "LSL:streamfeed/110 {}\r\nNative-Byte-Order: 1234\r\nEndian-Performance: 0\r\nHas-IEEE754-Floats: 1\r\nSupports-Subnormals: 1\r\nValue-Size: 4\r\nData-Protocol-Version: 110\r\nMax-Buffer-Length: 360\r\nMax-Chunk-Length: 0\r\n{extra}\r\n",
            found.uid()
        );
        s.write_all(request.as_bytes()).unwrap();
        s
    };
    let plain = connect("");
    let asked = connect("Rlsl-Compression: stream-zstd\r\n");
    std::thread::sleep(Duration::from_millis(500));

    // each client reads on its own thread while the samples are pushed at a steady pace
    let read_all = |mut s: std::net::TcpStream| {
        std::thread::spawn(move || {
            let (mut out, mut buf) = (Vec::new(), [0u8; 65536]);
            let mut quiet = 0;
            while quiet < 5 {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        out.extend_from_slice(&buf[..n]);
                        quiet = 0;
                    }
                    Err(_) => quiet += 1,
                }
            }
            out
        })
    };
    let (plain_reader, asked_reader) = (read_all(plain), read_all(asked));
    let mut state = 0x2545F4914F6CDD1Du64;
    const N: u64 = 1500;
    for k in 0..N {
        outlet.push_sample_f(&eeg_sample(k, &mut state), 100.0 + k as f64 / 500.0, true);
        if k % 50 == 49 {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let (plain_bytes, asked_bytes) = (plain_reader.join().unwrap(), asked_reader.join().unwrap());
    let split = |b: &[u8]| -> (String, usize) {
        let end = b
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("reply headers")
            + 4;
        (
            String::from_utf8_lossy(&b[..end]).to_string(),
            b.len() - end,
        )
    };
    let (plain_head, plain_body) = split(&plain_bytes);
    let (asked_head, asked_body) = split(&asked_bytes);

    // plain: the reply liblsl expects, then two test patterns and N samples, each a tag, a time
    // stamp and 32 floats
    assert!(
        plain_head.starts_with("LSL/110 200 OK\r\n") || plain_head.contains("200 OK"),
        "{plain_head}"
    );
    assert!(
        !plain_head.to_ascii_lowercase().contains("compression"),
        "{plain_head}"
    );
    assert_eq!(plain_body, (N as usize + 2) * (9 + 128), "plain feed size");

    assert_eq!(
        asked_head
            .to_ascii_lowercase()
            .contains("rlsl-compression: stream-zstd"),
        BUILT_WITH_COMPRESSION,
        "{asked_head}"
    );
    if BUILT_WITH_COMPRESSION {
        eprintln!(
            "[lan] {N} samples of 32-channel float32 EEG: plain {plain_body} bytes, compressed {asked_body} bytes ({:.0}%)",
            asked_body as f64 / plain_body as f64 * 100.0
        );
        assert!(
            asked_body * 100 < plain_body * 85,
            "compressed {asked_body} vs plain {plain_body}"
        );
    } else {
        assert_eq!(
            asked_body, plain_body,
            "without the feature the request changes nothing"
        );
    }
}
