//! A consumer that connects late can have what it missed: an outlet that keeps a history sends
//! it to an inlet that asks, ahead of the live samples — and nothing changes for anybody else.

use rlsl::prelude::*;
use rlsl::types::ChannelFormat;
use std::time::{Duration, Instant};

fn open(name: &str, replay: f64) -> StreamInlet {
    let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
    let inlet = StreamInlet::new(&found[0], 360, 0, true);
    inlet.set_replay(replay);
    inlet.open_stream(10.0).expect("data connection");
    inlet
}

fn pull_strings(inlet: &StreamInlet, n: usize) -> Vec<(f64, String)> {
    let mut out = Vec::new();
    let t0 = Instant::now();
    while out.len() < n && t0.elapsed() < Duration::from_secs(5) {
        let (stamps, texts) = inlet.pull_chunk_str(64, 0.2).unwrap();
        out.extend(stamps.into_iter().zip(texts));
    }
    out
}

/// A marker stream starts sending before anybody listens. The recorder that joins a moment
/// later and asks gets the markers sent meanwhile — in order, with their own time stamps,
/// each once — and then the live ones.
#[test]
fn a_late_consumer_gets_the_markers_it_missed() {
    let name = "rlsl-test-replay-markers";
    let info = StreamInfo::new(name, "Markers", 1, 0.0, ChannelFormat::String, name);
    let outlet = StreamOutlet::new(&info, 0, 360);
    assert_eq!(outlet.history(), rlsl::outlet::DEFAULT_MARKER_HISTORY_SECS);
    for k in 0..5 {
        outlet.push_sample_str(&[format!("early-{k}")], 100.0 + f64::from(k), true);
    }
    assert!(!outlet.have_consumers());

    let asking = open(name, 5.0);
    assert_eq!(asking.replay_granted(), 5.0);
    let plain = open(name, 0.0);
    assert_eq!(plain.replay_granted(), 0.0);
    while !outlet.have_consumers() {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(200));
    for k in 0..3 {
        outlet.push_sample_str(&[format!("live-{k}")], 200.0 + f64::from(k), true);
    }

    let got = pull_strings(&asking, 8);
    let want: Vec<(f64, String)> = (0..5)
        .map(|k| (100.0 + f64::from(k), format!("early-{k}")))
        .chain((0..3).map(|k| (200.0 + f64::from(k), format!("live-{k}"))))
        .collect();
    assert_eq!(got, want);
    // the one that did not ask gets what liblsl's would: what was sent after it connected
    let got = pull_strings(&plain, 3);
    assert_eq!(
        got.iter().map(|g| g.1.as_str()).collect::<Vec<_>>(),
        ["live-0", "live-1", "live-2"]
    );
}

/// The history holds what it was told to and no more: older samples are gone, an outlet told
/// to keep nothing grants nothing, and a regular stream keeps none unless asked.
#[test]
fn the_history_is_bounded_and_can_be_turned_off() {
    let name = "rlsl-test-replay-window";
    let info = StreamInfo::new(name, "Markers", 1, 0.0, ChannelFormat::String, name);
    let outlet = StreamOutlet::new(&info, 0, 360);
    outlet.set_history(0.5);
    outlet.push_sample_str(&["too old".into()], 1.0, true);
    std::thread::sleep(Duration::from_millis(800));
    outlet.push_sample_str(&["recent".into()], 2.0, true);
    let inlet = open(name, 60.0);
    // granted: no more than the outlet keeps
    assert_eq!(inlet.replay_granted(), 0.5);
    let got = pull_strings(&inlet, 1);
    assert_eq!(got, [(2.0, "recent".to_string())]);

    outlet.set_history(0.0);
    outlet.push_sample_str(&["unkept".into()], 3.0, true);
    let inlet = open(name, 60.0);
    assert_eq!(inlet.replay_granted(), 0.0);
    std::thread::sleep(Duration::from_millis(300));
    assert!(inlet.pull_chunk_str(8, 0.2).unwrap().0.is_empty());

    let name = "rlsl-test-replay-regular";
    let info = StreamInfo::new(name, "EEG", 2, 100.0, ChannelFormat::Float32, name);
    let outlet = StreamOutlet::new(&info, 0, 360);
    assert_eq!(outlet.history(), 0.0);
}

/// A regular stream asked to keep a history: a chunk's samples, which travel without time
/// stamps of their own, are replayed with the ones they stand for.
#[test]
fn a_regular_stream_replays_with_the_right_time_stamps() {
    let name = "rlsl-test-replay-eeg";
    let info = StreamInfo::new(name, "EEG", 2, 100.0, ChannelFormat::Float32, name);
    let outlet = StreamOutlet::new(&info, 0, 360);
    outlet.set_history(2.0);
    // 50 samples as one chunk ending at t = 10.49
    let data: Vec<f32> = (0..50).flat_map(|k| [k as f32, -(k as f32)]).collect();
    outlet.push_chunk_f(&data, 10.49, true);

    let inlet = open(name, 2.0);
    let (mut stamps, mut values) = (Vec::new(), Vec::new());
    let t0 = Instant::now();
    while stamps.len() < 50 && t0.elapsed() < Duration::from_secs(5) {
        let (t, v) = inlet.pull_chunk_f(64, 0.2).unwrap();
        stamps.extend(t);
        values.extend(v);
    }
    assert_eq!(values, data);
    for (k, t) in stamps.iter().enumerate() {
        assert!(
            (t - (10.0 + k as f64 * 0.01)).abs() < 1e-6,
            "sample {k} stamped {t}"
        );
    }
}
