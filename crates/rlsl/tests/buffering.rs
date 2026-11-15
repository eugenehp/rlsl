//! What happens to a stream whose reader falls behind: the buffers hold seconds of data, as
//! `max_buflen` says, and when they are full the oldest samples go — the stream itself goes on.

use rlsl::prelude::*;
use rlsl::types::ChannelFormat;
use std::time::{Duration, Instant};

fn connect(name: &str, max_buflen: i32) -> (StreamOutlet, StreamInlet) {
    let info = StreamInfo::new(name, "EEG", 4, 500.0, ChannelFormat::Float32, name);
    let outlet = StreamOutlet::new(&info, 0, 360);
    let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
    let inlet = StreamInlet::new(found.first().expect("stream resolves"), max_buflen, 0, true);
    inlet.open_stream(10.0).expect("data connection");
    let t0 = Instant::now();
    while !outlet.have_consumers() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    (outlet, inlet)
}

fn push(outlet: &StreamOutlet, from: u32, to: u32) {
    for k in from..to {
        outlet.push_sample_f(&[k as f32, 0.0, 0.0, 0.0], 0.0, true);
    }
}

/// Everything available within `quiet` of silence, as sample numbers.
fn drain(inlet: &StreamInlet, quiet: f64) -> Vec<u32> {
    let mut out = Vec::new();
    let mut buf = [0f32; 4];
    while inlet.pull_sample_f(&mut buf, quiet).unwrap() > 0.0 {
        out.push(buf[0] as u32);
    }
    out
}

/// A reader that looks away for six seconds of a 500 Hz stream finds all of it waiting.
/// (The buffers used to hold 360 *samples*, and the outlet ended the feed of a consumer that
/// fell further behind than that.)
#[test]
fn a_reader_that_falls_seconds_behind_loses_nothing() {
    let (outlet, inlet) = connect("rlsl-test-buffer-keep", 360);
    push(&outlet, 0, 3000);
    std::thread::sleep(Duration::from_millis(300));
    let got = drain(&inlet, 1.0);
    assert_eq!(
        got.len(),
        3000,
        "first {:?}, last {:?}",
        got.first(),
        got.last()
    );
    assert_eq!(outlet.dropped_samples(), 0);
    assert!(got.iter().enumerate().all(|(i, &k)| k == i as u32));
}

/// With one-second buffers and ten seconds of backlog, the oldest samples are dropped; what is
/// left is in order and ends with the newest, and the stream is still live afterwards.
#[test]
fn a_full_buffer_drops_the_oldest_and_the_stream_goes_on() {
    let (outlet, inlet) = connect("rlsl-test-buffer-drop", 1);
    push(&outlet, 0, 5000);
    std::thread::sleep(Duration::from_millis(500));
    let got = drain(&inlet, 1.0);
    assert!(
        got.len() >= 500 && got.len() < 5000,
        "{} samples kept",
        got.len()
    );
    assert!(got.windows(2).all(|w| w[1] > w[0]), "in order");
    assert_eq!(got.last(), Some(&4999), "the newest sample is there");
    // and the outlet knows how many it could not deliver
    assert_eq!(outlet.dropped_samples() as usize, 5000 - got.len());

    // not disconnected: what is pushed now arrives
    push(&outlet, 5000, 5010);
    let more = drain(&inlet, 1.0);
    assert_eq!(more, (5000..5010).collect::<Vec<_>>());
}

#[test]
fn buffer_lengths_are_seconds_or_hundreds_of_samples() {
    use rlsl::types::buffer_samples;
    assert_eq!(buffer_samples(360, 500.0), 180_000);
    assert_eq!(buffer_samples(1, 256.0), 256);
    assert_eq!(buffer_samples(360, 0.0), 36_000);
    assert_eq!(buffer_samples(0, 500.0), 1);
}

/// The outlet's own `max_buffered` is the most it holds for any consumer, whatever the consumer
/// asks for: a client that asks for six minutes and then reads nothing is held one second by an
/// outlet made with `max_buffered = 1`, and six minutes by one made with 360.
#[test]
fn the_outlet_s_own_limit_caps_what_a_consumer_can_ask_for() {
    use std::io::Write;
    let dropped_for = |name: &str, outlet_limit: i32| -> u64 {
        // large samples, so that the backlog outgrows what the sockets themselves hold
        let info = StreamInfo::new(name, "Test", 64, 500.0, ChannelFormat::Double64, name);
        let outlet = StreamOutlet::new(&info, 0, outlet_limit);
        let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
        let mut client =
            std::net::TcpStream::connect(format!("127.0.0.1:{}", found[0].v4data_port())).unwrap();
        let request = format!(
            "LSL:streamfeed/110 {}\r\nNative-Byte-Order: 1234\r\nEndian-Performance: 0\r\nHas-IEEE754-Floats: 1\r\nSupports-Subnormals: 1\r\nValue-Size: 8\r\nData-Protocol-Version: 110\r\nMax-Buffer-Length: 360\r\nMax-Chunk-Length: 0\r\n\r\n",
            found[0].uid()
        );
        client.write_all(request.as_bytes()).unwrap();
        let t0 = Instant::now();
        while !outlet.have_consumers() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        // 40 s of the stream, about 20 MB, to a client that never reads
        let sample = [1.5f64; 64];
        for _ in 0..20_000 {
            outlet.push_sample_d(&sample, 0.0, true);
        }
        std::thread::sleep(Duration::from_millis(300));
        let dropped = outlet.dropped_samples();
        drop(client);
        dropped
    };
    assert_eq!(
        dropped_for("rlsl-test-limit-wide", 360),
        0,
        "six minutes of room: nothing dropped"
    );
    let dropped = dropped_for("rlsl-test-limit-tight", 1);
    assert!(
        dropped > 10_000,
        "one second of room: {dropped} of 20000 dropped"
    );
}

/// 8-bit integer streams have their own push and pull.
#[test]
fn eight_bit_samples_round_trip() {
    let info = StreamInfo::new(
        "rlsl-test-int8",
        "Test",
        4,
        0.0,
        ChannelFormat::Int8,
        "int8",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let found = rlsl::resolver::resolve_by_property("name", "rlsl-test-int8", 1, 10.0);
    let inlet = StreamInlet::new(&found[0], 360, 0, true);
    inlet.open_stream(10.0).unwrap();
    let t0 = Instant::now();
    while !outlet.have_consumers() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    outlet.push_sample_i8(&[i8::MIN, -1, 0, i8::MAX], 0.0, true);
    let mut got = [0i8; 4];
    assert!(inlet.pull_sample_i8(&mut got, 5.0).unwrap() > 0.0);
    assert_eq!(got, [i8::MIN, -1, 0, i8::MAX]);
}

/// liblsl's handshake: a consumer whose machine has no subnormal numbers says so, and is sent
/// zeros in their place; everyone else gets the numbers as they are.
#[test]
fn subnormals_are_flushed_for_a_consumer_that_cannot_hold_them() {
    use std::io::{BufRead, BufReader, Read, Write};
    let name = "rlsl-test-subnormals";
    let info = StreamInfo::new(name, "Test", 2, 0.0, ChannelFormat::Double64, "subnormals");
    let outlet = StreamOutlet::new(&info, 0, 360);
    let port = outlet.info().v4data_port();
    let tiny = f64::MIN_POSITIVE / 4.0;
    assert!(tiny.is_subnormal());

    for (supports, expect) in [("1", tiny), ("0", 0.0)] {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "LSL:streamfeed/110 {}\r\nNative-Byte-Order: 1234\r\nEndian-Performance: 0\r\nHas-IEEE754-Floats: 1\r\nSupports-Subnormals: {supports}\r\nValue-Size: 8\r\nData-Protocol-Version: 110\r\nMax-Buffer-Length: 360\r\nMax-Chunk-Length: 0\r\n\r\n",
            outlet.info().uid()
        )
        .unwrap();
        let mut reader = BufReader::new(stream);
        let mut said = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Suppress-Subnormals:") {
                said = Some(v.trim().to_string());
            }
        }
        assert_eq!(
            said.as_deref(),
            Some(if supports == "0" { "1" } else { "0" })
        );
        // two test patterns: tag, time stamp, two doubles each
        let mut patterns = [0u8; 2 * (1 + 8 + 16)];
        reader.read_exact(&mut patterns).unwrap();
        // wait until the outlet has this consumer, then one sample
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            outlet.push_sample_d(&[tiny, 1.5], 7.0, true);
            let mut sample = [0u8; 1 + 8 + 16];
            if reader.read_exact(&mut sample).is_ok() {
                let value = |at: usize| f64::from_le_bytes(sample[at..at + 8].try_into().unwrap());
                assert_eq!(
                    (value(9), value(17)),
                    (expect, 1.5),
                    "Supports-Subnormals: {supports}"
                );
                break;
            }
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "no sample arrived");
    }
}

/// Chunks of every remaining type: 8-bit and 16-bit integers and strings, pushed as a chunk and
/// pulled as one.
#[test]
fn small_integer_and_string_chunks_round_trip() {
    fn pair(name: &str, format: ChannelFormat) -> (StreamOutlet, StreamInlet) {
        let info = StreamInfo::new(name, "Test", 2, 0.0, format, name);
        let outlet = StreamOutlet::new(&info, 0, 360);
        let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
        let inlet = StreamInlet::new(&found[0], 360, 0, true);
        inlet.open_stream(10.0).unwrap();
        let t0 = Instant::now();
        while !outlet.have_consumers() && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        (outlet, inlet)
    }
    /// Pull until `n` values have arrived (a chunk may come in pieces).
    fn pull<T>(n: usize, mut chunk: impl FnMut() -> (Vec<f64>, Vec<T>)) -> (Vec<f64>, Vec<T>) {
        let (mut stamps, mut values) = (Vec::new(), Vec::new());
        let t0 = Instant::now();
        while values.len() < n && t0.elapsed() < Duration::from_secs(5) {
            let (t, v) = chunk();
            stamps.extend(t);
            values.extend(v);
        }
        (stamps, values)
    }

    let (outlet, inlet) = pair("rlsl-test-chunk-i8", ChannelFormat::Int8);
    let sent: Vec<i8> = vec![i8::MIN, -1, 0, 1, 2, i8::MAX];
    outlet.push_chunk_i8(&sent, 0.0, true);
    let (stamps, got) = pull(6, || inlet.pull_chunk_i8(16, 1.0).unwrap());
    assert_eq!((stamps.len(), got), (3, sent));

    let (outlet, inlet) = pair("rlsl-test-chunk-i16", ChannelFormat::Int16);
    let sent: Vec<i16> = vec![i16::MIN, -1, 0, 258, 2, i16::MAX];
    outlet.push_chunk_i16(&sent, 0.0, true);
    let (stamps, got) = pull(6, || inlet.pull_chunk_i16(16, 1.0).unwrap());
    assert_eq!((stamps.len(), got), (3, sent));

    let (outlet, inlet) = pair("rlsl-test-chunk-str", ChannelFormat::String);
    let sent: Vec<String> = ["start", "", "héllo", &"x".repeat(300), "a;b", "end"]
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    outlet.push_chunk_str(&sent, 0.0, true);
    let (stamps, got) = pull(6, || inlet.pull_chunk_str(16, 1.0).unwrap());
    assert_eq!(stamps.len(), 3);
    assert_eq!(got, sent);
    assert!(stamps.windows(2).all(|w| w[1] >= w[0]));
}

/// An inlet that comes back to an address it knew may find another outlet there (services share
/// a small range of ports). An outlet asked for a stream that is not its own refuses, as liblsl
/// does, rather than feed the consumer somebody else's samples.
#[test]
fn an_outlet_refuses_a_request_for_another_stream() {
    use std::io::{BufRead, BufReader, Write};
    let info = StreamInfo::new(
        "rlsl-test-uid",
        "Test",
        1,
        0.0,
        ChannelFormat::Float32,
        "uid",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let port = outlet.info().v4data_port();
    let ask = |uid: &str| {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write!(
            stream,
            "LSL:streamfeed/110 {uid}\r\nNative-Byte-Order: 1234\r\nValue-Size: 4\r\nData-Protocol-Version: 110\r\nMax-Buffer-Length: 360\r\nMax-Chunk-Length: 0\r\n\r\n"
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        line
    };
    assert!(ask(&outlet.info().uid()).contains("200"), "its own stream");
    let other = ask("00000000-0000-4000-8000-000000000000");
    assert!(other.contains("404"), "another stream: {other}");

    // and an inlet whose stream was replaced at its address does not take the newcomer's data
    let first = StreamInfo::new(
        "rlsl-test-uid-first",
        "Markers",
        1,
        0.0,
        ChannelFormat::String,
        "first",
    );
    let first_outlet = StreamOutlet::new(&first, 0, 360);
    let found = rlsl::resolver::resolve_by_property("name", "rlsl-test-uid-first", 1, 10.0);
    let stale = found[0].clone();
    drop(first_outlet);
    // a different stream that the stale description now points at
    stale.set_v4address("127.0.0.1");
    stale.set_v4data_port(port);
    stale.set_v6data_port(0);
    let inlet = StreamInlet::new(&stale, 360, 0, false);
    let _ = inlet.open_stream(2.0);
    outlet.push_sample_f(&[42.0], 0.0, true);
    let mut buf = [0f32; 1];
    let got = inlet.pull_sample_f(&mut buf, 1.0);
    assert!(
        got.is_err() || got == Ok(0.0),
        "a sample of another stream was delivered: {got:?}"
    );
}

/// Protocol 1.00, the form liblsl falls back to with old or unusual peers: a request without a
/// version is answered with liblsl's "portable archive" — header, the stream's description,
/// and samples in which every number carries its size. The bytes are the ones liblsl 1.18 sends.
#[test]
fn protocol_100_is_liblsl_s_portable_archive() {
    use std::io::{Read, Write};
    let info = StreamInfo::new(
        "rlsl-test-p100",
        "Test",
        2,
        100.0,
        ChannelFormat::Float32,
        "p100",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", outlet.info().v4data_port())).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(b"LSL:streamfeed\r\n360 0\r\n").unwrap();

    let mut head = [0u8; 3];
    stream.read_exact(&mut head).unwrap();
    assert_eq!(head, [0x7f, 0x01, 0x09], "magic byte and archive version");
    let description = rlsl::sample::portable::get_string(&mut stream).unwrap();
    assert!(
        description.contains("<name>rlsl-test-p100</name>"),
        "{description}"
    );
    let mut class_info = [0xffu8; 2];
    stream.read_exact(&mut class_info).unwrap();
    assert_eq!(class_info, [0, 0]);
    // the two test patterns, as liblsl sends them for two float32 channels
    let mut patterns = [0u8; 42];
    stream.read_exact(&mut patterns).unwrap();
    assert_eq!(
        patterns[..21],
        [
            0x01, 0x02, 0x08, 0xc9, 0x76, 0xbe, 0x9f, 0x0c, 0x24, 0xfe, 0x40, 0x04, 0x00, 0x00,
            0x80, 0x40, 0x04, 0x00, 0x00, 0xa0, 0xc0
        ]
    );
    assert_eq!(
        patterns[21..],
        [
            0x01, 0x02, 0x08, 0xc9, 0x76, 0xbe, 0x9f, 0x0c, 0x24, 0xfe, 0x40, 0x04, 0x00, 0x00,
            0x00, 0x40, 0x04, 0x00, 0x00, 0x40, 0xc0
        ]
    );
    // and a sample: [0.0, 3.0] at t = 1000
    let t0 = Instant::now();
    while !outlet.have_consumers() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    outlet.push_sample_f(&[0.0, 3.0], 1000.0, true);
    let mut sample = [0u8; 17];
    stream.read_exact(&mut sample).unwrap();
    assert_eq!(
        sample,
        [0x01, 0x02, 0x08, 0, 0, 0, 0, 0, 0x40, 0x8f, 0x40, 0x00, 0x04, 0x00, 0x00, 0x40, 0x40]
    );
}
