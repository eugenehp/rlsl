//! Clock measurements travelling with a data stream: in its own connection to an inlet that
//! asks, and on a companion stream — while the data itself, and any consumer that does not
//! ask, are as before.
#![cfg(feature = "ntp")]

use rlsl::ntp::{
    clock_source_id, ClockKind, ClockMesh, ClockMeshConfig, ClockRecord, ClockTransport,
    CLOCK_STREAM_TYPE,
};
use rlsl::prelude::*;
use std::time::{Duration, Instant};

fn node(id: &str, peers: Vec<String>) -> ClockMesh {
    ClockMesh::start(ClockMeshConfig {
        node_id: id.into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        peers,
        interval: Duration::from_secs(3600),
        probes: 2,
        timeout: 1.0,
        ..ClockMeshConfig::default()
    })
    .unwrap()
}

/// Two nodes, `a` measuring `b`: once, in the round a node makes when it starts (the next is
/// an hour away; the tests ask for more with `poll_now`).
fn mesh() -> (ClockMesh, ClockMesh) {
    let b = node("b", vec![]);
    let a = node("a", vec![b.local_addr().to_string()]);
    let t0 = Instant::now();
    while a.records().len() < 2 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(a.records().len(), 2, "{:?}", a.peers());
    (a, b)
}

/// Push samples and pull them until `inlet` has `n` clock records (or 5 s are up).
fn records_through(
    outlet: &StreamOutlet,
    inlet: &StreamInlet,
    n: usize,
    got: &mut Vec<ClockRecord>,
) {
    let t0 = Instant::now();
    let mut buf = [0.0f32; 4];
    let mut k = 0.0f32;
    while got.len() < n && t0.elapsed() < Duration::from_secs(5) {
        outlet.push_sample_f(&[k, k + 1.0, k + 2.0, k + 3.0], 0.0, true);
        let ts = inlet.pull_sample_f(&mut buf, 2.0).unwrap();
        assert!(ts > 0.0);
        assert_eq!(buf, [k, k + 1.0, k + 2.0, k + 3.0], "data unharmed");
        k += 4.0;
        got.extend(inlet.pull_clock_records());
    }
}

fn in_band(compressed: bool) {
    let (a, _b) = mesh();
    let info = StreamInfo::new(
        &format!("ClockInBand{compressed}"),
        "EEG",
        4,
        100.0,
        ChannelFormat::Float32,
        &format!("clock-in-band-{compressed}"),
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    outlet.attach_clock_mesh(&a, ClockTransport::IN_BAND);
    assert_eq!(
        outlet.clock_records_held(),
        2,
        "the measurements made before"
    );
    assert!(outlet.clock_companion_info().is_none());

    let inlet = StreamInlet::new(outlet.info(), 360, 0, true);
    inlet.set_clock_records(true);
    inlet.set_compression(compressed);
    inlet.open_stream(5.0).unwrap();
    assert!(inlet.has_clock_records());
    assert_eq!(
        inlet.is_compressed(),
        compressed && cfg!(feature = "compression")
    );

    // the history first, then a new measurement as it is made
    let mut got = Vec::new();
    records_through(&outlet, &inlet, 2, &mut got);
    a.poll_now();
    records_through(&outlet, &inlet, 4, &mut got);
    assert_eq!(got.len(), 4, "{got:?}");
    assert_eq!(got, a.records());
    let mono: Vec<&ClockRecord> = got.iter().filter(|r| r.clock == ClockKind::Mono).collect();
    assert_eq!(mono.len(), 2);
    // one machine, one monotonic clock: no measured offset beyond what the delay allows
    assert!(
        mono.iter().all(|r| r.offset.abs() <= r.delay / 2.0 + 1e-4),
        "{mono:?}"
    );
    assert!(got.iter().all(|r| r.from == "a" && r.to == "b"));

    // a consumer that does not ask gets the plain stream
    let plain = StreamInlet::new(outlet.info(), 360, 0, true);
    plain.open_stream(5.0).unwrap();
    assert!(!plain.has_clock_records());
    let mut buf = [0.0f32; 4];
    outlet.push_sample_f(&[9.0; 4], 0.0, true);
    a.poll_now();
    outlet.push_sample_f(&[8.0; 4], 0.0, true);
    plain.pull_sample_f(&mut buf, 2.0).unwrap();
    assert_eq!(buf, [9.0; 4]);
    plain.pull_sample_f(&mut buf, 2.0).unwrap();
    assert_eq!(buf, [8.0; 4]);
    assert!(plain.pull_clock_records().is_empty());
}

#[test]
fn in_band_plain_feed() {
    in_band(false);
}

#[test]
fn in_band_compressed_feed() {
    in_band(true);
}

#[test]
fn reconnecting_inlet_gets_nothing_twice() {
    let (a, _b) = mesh();
    let info = StreamInfo::new(
        "ClockAgain",
        "EEG",
        4,
        100.0,
        ChannelFormat::Float32,
        "again",
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    outlet.attach_clock_mesh(&a, ClockTransport::IN_BAND);
    let mut got = Vec::new();
    for _ in 0..2 {
        // a second connection is sent the history again; the inlet keeps one of each
        let inlet = StreamInlet::new(outlet.info(), 360, 0, true);
        inlet.set_clock_records(true);
        inlet.open_stream(5.0).unwrap();
        records_through(&outlet, &inlet, 2, &mut got);
        assert_eq!(got.len(), 2);
        got.clear();
    }
}

#[test]
fn companion_stream() {
    let (a, _b) = mesh();
    let info = StreamInfo::new("ClockData", "EEG", 4, 100.0, ChannelFormat::Float32, "amp7");
    let outlet = StreamOutlet::new(&info, 0, 360);
    outlet.attach_clock_mesh(&a, ClockTransport::COMPANION);
    assert_eq!(outlet.clock_records_held(), 0, "not in-band");

    let companion = outlet.clock_companion_info().expect("a companion stream");
    assert_eq!(companion.type_(), CLOCK_STREAM_TYPE);
    assert_eq!(companion.source_id(), clock_source_id("amp7"));
    assert_eq!(companion.name(), "ClockData (clock)");
    assert_eq!(
        companion.channel_count() as usize,
        ClockRecord::FIELDS.len()
    );

    // a recorder that comes later asks for the past and gets the measurements made before
    let inlet = StreamInlet::new(&companion, 360, 0, true);
    inlet.set_replay(3600.0);
    inlet.open_stream(5.0).unwrap();
    let mut got = Vec::new();
    let t0 = Instant::now();
    while got.len() < 4 && t0.elapsed() < Duration::from_secs(5) {
        if got.len() == 2 {
            a.poll_now();
        }
        if let Ok((fields, ts)) = inlet.pull_sample_str(1.0) {
            if ts > 0.0 {
                got.push(ClockRecord::from_fields(&fields).expect("a clock record"));
            }
        }
    }
    assert_eq!(got, a.records());
    let full = inlet.get_fullinfo(5.0);
    let sync = full.desc().child("clock_sync");
    assert_eq!(sync.child_value("node"), "a");
    assert_eq!(sync.child_value("stream_source_id"), "amp7");
    assert_eq!(sync.child_value("stream_uid"), outlet.info().uid());
}

#[test]
fn outlet_gone_mesh_lets_go() {
    let (a, _b) = mesh();
    {
        let info = StreamInfo::new("ClockGone", "EEG", 1, 10.0, ChannelFormat::Float32, "gone");
        let outlet = StreamOutlet::new(&info, 0, 360);
        outlet.attach_clock_mesh(&a, ClockTransport::BOTH);
    }
    // the next measurement finds nobody to give it to, and the mesh carries on
    a.poll_now();
    a.poll_now();
    assert_eq!(a.records().len(), 6);
}
