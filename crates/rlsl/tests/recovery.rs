//! A stream that goes away and comes back — the program that sends it was restarted — is the
//! same stream to an inlet when it has a `source_id`, as in liblsl: the inlet finds the new
//! outlet and goes on. A stream without one can only be the very outlet it was.

use rlsl::prelude::*;
use rlsl::types::ChannelFormat;
use std::time::{Duration, Instant};

fn outlet(name: &str, source_id: &str) -> StreamOutlet {
    let info = StreamInfo::new(name, "EEG", 2, 100.0, ChannelFormat::Double64, source_id);
    StreamOutlet::new(&info, 0, 360)
}

/// Push `value` until the inlet has pulled it (or `secs` are over); what was pulled meanwhile.
fn feed_until_seen(
    out: &StreamOutlet,
    inlet: &StreamInlet,
    value: f64,
    secs: u64,
) -> Vec<(f64, f64)> {
    let mut got = Vec::new();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        out.push_sample_d(&[value, -value], 0.0, true);
        let (stamps, data) = inlet.pull_chunk_d(64, 0.05).unwrap_or_default();
        got.extend(stamps.into_iter().zip(data.chunks(2).map(|row| row[0])));
        if got.iter().any(|&(_, v)| v == value) {
            break;
        }
    }
    got
}

#[test]
fn a_restarted_stream_with_a_source_id_is_recovered() {
    let name = "rlsl-test-recovery";
    let first = outlet(name, "recovery-device-1");
    let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
    let inlet = StreamInlet::new(&found[0], 360, 0, true);
    inlet.set_clock_sync(true);
    inlet.set_postprocessing(rlsl::types::PROC_CLOCKSYNC | rlsl::types::PROC_MONOTONIZE);
    inlet.open_stream(10.0).expect("data connection");
    let before = feed_until_seen(&first, &inlet, 1.0, 5);
    assert!(
        before.iter().any(|&(_, v)| v == 1.0),
        "samples of the first outlet"
    );

    // the sender goes, and another takes its place: same stream, new outlet
    let first_uid = found[0].uid();
    drop(first);
    std::thread::sleep(Duration::from_millis(300));
    let second = outlet(name, "recovery-device-1");
    assert_ne!(second.info().uid(), first_uid);
    let after = feed_until_seen(&second, &inlet, 2.0, 20);
    assert!(
        after.iter().any(|&(_, v)| v == 2.0),
        "the inlet goes on with the outlet that took the first one's place"
    );
    // on this machine's clock, like the ones before, and not behind them
    let now = rlsl::clock::local_clock();
    let last_before = before.last().unwrap().0;
    assert!(after
        .iter()
        .all(|&(t, _)| t >= last_before && (t - now).abs() < 30.0));
    // and the time service asked from now on is the new outlet's
    assert!(inlet.clock_offset(2.0).is_some());
}

#[test]
fn a_stream_without_a_source_id_is_not_taken_for_another() {
    let name = "rlsl-test-recovery-anonymous";
    let first = outlet(name, "");
    let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
    let inlet = StreamInlet::new(&found[0], 360, 0, true);
    inlet.open_stream(10.0).expect("data connection");
    assert!(feed_until_seen(&first, &inlet, 1.0, 5)
        .iter()
        .any(|&(_, v)| v == 1.0));

    drop(first);
    std::thread::sleep(Duration::from_millis(300));
    let second = outlet(name, "");
    let after = feed_until_seen(&second, &inlet, 2.0, 6);
    assert!(
        after.iter().all(|&(_, v)| v != 2.0),
        "nothing says this outlet is the stream that was lost"
    );
}
