//! 64-bit streams end to end: doubles and 64-bit integers go from an outlet to an inlet over
//! the network without losing a bit, by sample and by chunk.

use rlsl::prelude::*;
use rlsl::types::ChannelFormat;
use std::time::{Duration, Instant};

/// Values a 32-bit float (24-bit mantissa) cannot hold.
const DOUBLES: [f64; 4] = [
    1_700_000_000.123_456_7,       // Unix time with sub-microsecond digits
    std::f64::consts::PI,          // all 53 mantissa bits
    -1.0e-300,                     // far below f32's range
    9_007_199_254_740_993.0 - 2.0, // 2^53 - 1: the largest odd integer a double holds
];

/// Values a double (53-bit mantissa) cannot hold.
const INTS: [i64; 4] = [
    i64::MAX,
    i64::MIN,
    (1 << 53) + 1,
    -1_234_567_890_123_456_789,
];

fn connect(name: &str, fmt: ChannelFormat, rate: f64) -> (StreamOutlet, StreamInlet) {
    let info = StreamInfo::new(name, "Test", 4, rate, fmt, &format!("{name}-src"));
    let outlet = StreamOutlet::new(&info, 0, 360);
    let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
    let inlet = StreamInlet::new(found.first().expect("stream resolves"), 360, 0, true);
    inlet.open_stream(10.0).expect("data connection");
    // samples pushed before the outlet has registered the consumer are not delivered
    let t0 = Instant::now();
    while !outlet.have_consumers() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(outlet.have_consumers());
    (outlet, inlet)
}

#[test]
fn doubles_arrive_bit_for_bit() {
    let (outlet, inlet) = connect("rlsl-test-double64", ChannelFormat::Double64, 100.0);
    assert_eq!(inlet.info().channel_format(), ChannelFormat::Double64);

    // by sample
    outlet.push_sample_d(&DOUBLES, 0.0, true);
    let mut got = [0f64; 4];
    let ts = inlet.pull_sample_d(&mut got, 5.0).unwrap();
    assert!(ts > 0.0, "a sample within the timeout");
    assert_eq!(got.map(f64::to_bits), DOUBLES.map(f64::to_bits));

    // by chunk: three samples, each value offset by a step far below f32 resolution
    let chunk: Vec<f64> = (0..3)
        .flat_map(|k| DOUBLES.map(|v| v + k as f64 * 1e-9))
        .collect();
    outlet.push_chunk_d(&chunk, 0.0, true);
    let (mut stamps, mut data) = (Vec::new(), Vec::new());
    let t0 = Instant::now();
    while stamps.len() < 3 && t0.elapsed() < Duration::from_secs(5) {
        let (t, d) = inlet.pull_chunk_d(16, 0.5).unwrap();
        stamps.extend(t);
        data.extend(d);
    }
    assert_eq!(stamps.len(), 3);
    assert_eq!(
        data.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        chunk.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    // a chunk's samples are one sample period apart
    assert!((stamps[2] - stamps[0] - 0.02).abs() < 1e-6, "{stamps:?}");

    // read as single precision, the same stream gives the nearest floats
    outlet.push_sample_d(&DOUBLES, 0.0, true);
    let (_, narrow) = inlet.pull_chunk_f(1, 5.0).unwrap();
    assert_eq!(narrow, DOUBLES.map(|v| v as f32));
}

#[test]
fn sixty_four_bit_integers_arrive_exactly() {
    let (outlet, inlet) = connect("rlsl-test-int64", ChannelFormat::Int64, 0.0);
    assert_eq!(inlet.info().channel_format(), ChannelFormat::Int64);

    // by sample
    outlet.push_sample_i64(&INTS, 0.0, true);
    let mut got = [0i64; 4];
    assert!(inlet.pull_sample_i64(&mut got, 5.0).unwrap() > 0.0);
    assert_eq!(got, INTS);

    // by chunk
    let chunk: Vec<i64> = (0..5)
        .flat_map(|k| INTS.map(|v| v.wrapping_add(k)))
        .collect();
    outlet.push_chunk_i64(&chunk, 0.0, true);
    let (mut stamps, mut data) = (Vec::new(), Vec::new());
    let t0 = Instant::now();
    while stamps.len() < 5 && t0.elapsed() < Duration::from_secs(5) {
        let (t, d) = inlet.pull_chunk_i64(16, 0.5).unwrap();
        stamps.extend(t);
        data.extend(d);
    }
    assert_eq!(data, chunk);

    // the double view of the same values is the nearest double, as documented
    outlet.push_sample_i64(&INTS, 0.0, true);
    let (_, wide) = inlet.pull_chunk_d(1, 5.0).unwrap();
    assert_eq!(wide, INTS.map(|v| v as f64));
    assert_ne!(wide[2] as i64, INTS[2], "2^53 + 1 does not fit a double");
}

/// The narrower integer chunk calls share the same code path.
#[test]
fn thirty_two_bit_integer_chunks_round_trip() {
    let (outlet, inlet) = connect("rlsl-test-int32", ChannelFormat::Int32, 50.0);
    let chunk: Vec<i32> = vec![i32::MAX, i32::MIN, 0, -1, 1, 2, 3, 4];
    outlet.push_chunk_i32(&chunk, 0.0, true);
    let (mut stamps, mut data) = (Vec::new(), Vec::new());
    let t0 = Instant::now();
    while stamps.len() < 2 && t0.elapsed() < Duration::from_secs(5) {
        let (t, d) = inlet.pull_chunk_i32(16, 0.5).unwrap();
        stamps.extend(t);
        data.extend(d);
    }
    assert_eq!(data, chunk);
}
