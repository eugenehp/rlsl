//! The clock and what is built on it. Everything behaves as it did by default — a per-process
//! clock, `time_correction()` with its historical sign, a clock-sync flag that adds what the
//! caller last estimated — and the liblsl behaviour is there for those who ask for it, one by
//! one at run time (the machine clock, `clock_offset()`, automatic clock synchronisation) or
//! all at once with the cargo feature `liblsl-defaults`, under which these tests expect
//! liblsl's defaults instead. Resolved streams can be reached.

use rlsl::prelude::*;
use rlsl::types::{ChannelFormat, PROC_CLOCKSYNC};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

/// Built with liblsl's defaults rather than rlsl's own.
const LIBLSL: bool = cfg!(feature = "liblsl-defaults");

/// Run by the clock tests in a child process.
#[test]
fn print_clock_for_parent() {
    if std::env::var_os("RLSL_TEST_PRINT_CLOCK").is_some() {
        println!(
            "CLOCKS={:.9} {:.9}",
            rlsl::clock::local_clock(),
            rlsl::clock::system_clock()
        );
    }
}

/// `(local_clock, system_clock)` as a freshly started process reads them.
fn child_clocks(lsl_clock: Option<&str>) -> (f64, f64) {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["print_clock_for_parent", "--exact", "--nocapture"])
        .env("RLSL_TEST_PRINT_CLOCK", "1")
        .env_remove("LSL_CLOCK");
    if let Some(v) = lsl_clock {
        cmd.env("LSL_CLOCK", v);
    }
    let out = cmd.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let values: Vec<f64> = text
        .lines()
        .find_map(|l| l.split("CLOCKS=").nth(1))
        .unwrap_or_else(|| panic!("no clocks in the child's output: {text}"))
        .split_whitespace()
        .map(|v| v.parse().unwrap())
        .collect();
    (values[0], values[1])
}

/// By default the clock is what it has always been: seconds since the process first asked
/// (the machine's with `liblsl-defaults`). Either can be asked for, whatever the default.
#[test]
fn default_clock_counts_from_the_start_of_the_process() {
    let expected = if LIBLSL {
        rlsl::clock::ClockSource::System
    } else {
        rlsl::clock::ClockSource::Process
    };
    assert_eq!(rlsl::clock::clock_source(), expected);
    for setting in [None, Some("process")] {
        if LIBLSL && setting.is_none() {
            continue;
        }
        let (local, _) = child_clocks(setting);
        // the child read its clock for the first time just before printing it
        assert!(
            (0.0..1.0).contains(&local),
            "{setting:?}: a new process's clock reads {local}"
        );
    }
    if LIBLSL {
        let (local, system) = child_clocks(None);
        assert!((local - system).abs() < 0.01, "{local} vs {system}");
    }
    let t1 = rlsl::clock::local_clock();
    std::thread::sleep(Duration::from_millis(20));
    assert!(rlsl::clock::local_clock() > t1);
}

/// The machine clock is the same in every process, and `LSL_CLOCK=system` makes it the local
/// clock: a child started now reports a time between two readings taken here around it.
#[test]
fn system_clock_is_shared_between_processes() {
    let before = rlsl::clock::system_clock();
    let (default_local, system) = child_clocks(Some("process"));
    let (opted_local, opted_system) = child_clocks(Some("system"));
    let after = rlsl::clock::system_clock();
    assert!(after > before);
    if cfg!(any(unix, windows)) {
        assert!(
            before <= system && system <= after,
            "{before} <= {system} <= {after}"
        );
        // with the setting, local_clock is that clock
        assert!(
            before <= opted_local && opted_local <= after,
            "{before} <= {opted_local} <= {after}"
        );
        assert!((opted_system - opted_local).abs() < 0.01);
        // with the other, it is not
        assert!((default_local - system).abs() > 1.0 || system < 2.0);
    }
}

/// A time service whose clock is `ahead` seconds ahead of this machine's: answers
/// `LSL:timedata` like an outlet. Returns its port.
fn fake_time_service(ahead: f64) -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            let mut lines = text.lines();
            if lines.next().map(str::trim) != Some("LSL:timedata") {
                continue;
            }
            let params: Vec<&str> = lines.next().unwrap_or("").split_whitespace().collect();
            let (wave, t0) = (
                params.first().unwrap_or(&"0"),
                params.get(1).unwrap_or(&"0"),
            );
            let remote_now = rlsl::clock::local_clock() + ahead;
            let reply = format!(" {wave} {t0} {remote_now} {remote_now}");
            let _ = socket.send_to(reply.as_bytes(), from);
        }
    });
    port
}

/// For a remote clock that is 100 s ahead, `time_correction` reports +100 as it always has
/// (remote minus local) and `clock_offset` reports -100: what to add to the remote stamps, as
/// liblsl defines it. With `liblsl-defaults`, `time_correction` is liblsl's too.
#[test]
fn time_correction_keeps_its_sign_and_clock_offset_has_liblsl_s() {
    for ahead in [100.0, -250.0] {
        let port = fake_time_service(ahead);
        let tc = rlsl::time_receiver::time_correction("127.0.0.1", port, 2.0);
        let expected = if LIBLSL { -ahead } else { ahead };
        assert!(
            (tc - expected).abs() < 0.05,
            "remote ahead by {ahead}: time_correction {tc}"
        );
        let offset = rlsl::time_receiver::clock_offset("127.0.0.1", port, 2.0).expect("an answer");
        assert!(
            (offset + ahead).abs() < 0.05,
            "remote ahead by {ahead}: clock_offset {offset}"
        );
        let remote_stamp = rlsl::clock::local_clock() + ahead;
        assert!((remote_stamp + offset - rlsl::clock::local_clock()).abs() < 0.05);
    }
    // nobody there: 0 from the one, `None` from the other
    let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    assert_eq!(
        rlsl::time_receiver::time_correction("127.0.0.1", port, 0.3),
        0.0
    );
    assert_eq!(
        rlsl::time_receiver::clock_offset("127.0.0.1", port, 0.3),
        None
    );
}

/// An inlet on a stream whose outlet's clock is 100 s ahead (the time service says so).
fn inlet_with_remote_clock(name: &str, ahead: f64) -> (StreamOutlet, StreamInlet) {
    let info = StreamInfo::new(
        name,
        "Test",
        1,
        100.0,
        ChannelFormat::Float32,
        &format!("{name}-src"),
    );
    let outlet = StreamOutlet::new(&info, 0, 360);
    let found = rlsl::resolver::resolve_by_property("name", name, 1, 10.0);
    let remote = found.first().expect("stream resolves").clone();
    remote.set_v4address("127.0.0.1");
    remote.set_v4service_port(fake_time_service(ahead));
    let inlet = StreamInlet::new(&remote, 360, 0, true);
    inlet.set_postprocessing(PROC_CLOCKSYNC);
    inlet.open_stream(10.0).expect("data connection");
    let t0 = Instant::now();
    while !outlet.have_consumers() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    (outlet, inlet)
}

/// Unchanged: the clock-sync flag adds what the last `time_correction()` call found, and
/// nothing when there was no such call. (With `liblsl-defaults` the inlet synchronises by
/// itself unless told not to, as here.)
#[test]
fn clocksync_flag_alone_behaves_as_before() {
    const AHEAD: f64 = 100.0;
    let (outlet, inlet) = inlet_with_remote_clock("rlsl-test-clocksync-manual", AHEAD);
    if LIBLSL {
        inlet.set_clock_sync(false);
    }
    let mut buf = [0f32; 1];

    let stamp = rlsl::clock::local_clock() + AHEAD;
    outlet.push_sample_f(&[1.0], stamp, true);
    let ts = inlet.pull_sample_f(&mut buf, 5.0).unwrap();
    assert!(
        (ts - stamp).abs() < 1e-6,
        "no call, no correction: {ts} vs {stamp}"
    );

    let tc = inlet.time_correction(2.0);
    let expected = if LIBLSL { -AHEAD } else { AHEAD };
    assert!((tc - expected).abs() < 0.05, "{tc}");
    let stamp = rlsl::clock::local_clock() + AHEAD;
    outlet.push_sample_f(&[2.0], stamp, true);
    let ts = inlet.pull_sample_f(&mut buf, 5.0).unwrap();
    assert!(
        (ts - (stamp + tc)).abs() < 1e-6,
        "the stored value is added: {ts} vs {stamp} + {tc}"
    );
}

/// Opted in (or by default with `liblsl-defaults`), an inlet delivers local-clock time stamps
/// without the caller ever estimating anything: samples stamped by an outlet whose clock is
/// 100 s ahead come out at about "now".
#[test]
fn automatic_clock_sync_gives_local_time_stamps() {
    const AHEAD: f64 = 100.0;
    let (outlet, inlet) = inlet_with_remote_clock("rlsl-test-clocksync-auto", AHEAD);
    if !LIBLSL {
        inlet.set_clock_sync(true);
    }

    let mut buf = [0f32; 1];
    for k in 0..3 {
        // stamped on the "remote" clock; compared with the local time of the push, not of the
        // pull, which for the first sample waits for the offset estimate (~0.5 s of probes)
        let sent = rlsl::clock::local_clock();
        outlet.push_sample_f(&[k as f32], sent + AHEAD, true);
        let ts = inlet.pull_sample_f(&mut buf, 5.0).unwrap();
        assert!(
            (ts - sent).abs() < 0.05,
            "sample {k} stamped {ts}, sent at local {sent}"
        );
        assert_eq!(buf[0], k as f32);
    }
    // and the estimate is there for whoever asks
    let offset = inlet.clock_offset(1.0).expect("an answer");
    assert!((offset + AHEAD).abs() < 0.05, "{offset}");
}

/// A resolved stream is known by an address it can be reached at, every time: the IPv4 address
/// its replies came from, whichever of its IPv4 / IPv6 replies arrived first.
#[test]
fn resolved_streams_carry_a_reachable_address() {
    let info = StreamInfo::new(
        "rlsl-test-reachable",
        "Test",
        1,
        0.0,
        ChannelFormat::Float32,
        "reachable-src",
    );
    let _outlet = StreamOutlet::new(&info, 0, 360);
    for round in 0..8 {
        let found = rlsl::resolver::resolve_by_property("name", "rlsl-test-reachable", 1, 5.0);
        let s = found
            .first()
            .unwrap_or_else(|| panic!("round {round}: not resolved"));
        assert!(
            !s.v4address().is_empty(),
            "round {round}: no IPv4 address (v6 {:?})",
            s.v6address()
        );
        let inlet = StreamInlet::new(s, 360, 0, false);
        inlet
            .open_stream(5.0)
            .unwrap_or_else(|e| panic!("round {round}: {e}"));
    }
}

/// The local clock on the Unix timeline: right now, and kept current as the two drift apart.
#[test]
fn unix_offset_follows_the_wall_clock_smoothly() {
    use rlsl::clock::{local_clock, unix_offset_now, UnixOffset};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    // now: local clock plus the offset is the wall clock
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    assert!((local_clock() + unix_offset_now() - unix).abs() < 0.05);
    let mut live = UnixOffset::new();
    assert!((live.unix_time(local_clock()) - unix).abs() < 0.05);

    // an hour of 15 ppm between the clocks, asked 50 times a second
    let t0 = Instant::now();
    let mut kept = UnixOffset::starting_at(1000.0, t0);
    let truth = |s: f64| 1000.0 + 15e-6 * s;
    let (mut prev, mut step, mut worst) = (1000.0, 0.0f64, 0.0f64);
    for k in 1..=3600 * 50 {
        let s = f64::from(k) / 50.0;
        let got = kept.offset_at(t0 + Duration::from_secs_f64(s), || truth(s));
        step = step.max((got - prev).abs());
        worst = worst.max((got - truth(s)).abs());
        prev = got;
    }
    assert!(worst < 0.0002, "off by {} ms", worst * 1e3);
    assert!(
        step <= 0.02 * UnixOffset::SLEW_PER_S + 1e-12,
        "a step of {} us",
        step * 1e6
    );
    assert!((prev - 1000.054).abs() < 0.0002);

    // a bad reading moves it by no more than the slew allows before the next one
    let glitch = kept.offset_at(t0 + Duration::from_secs(3605), || truth(3600.0) + 0.05);
    assert!((glitch - prev).abs() < 0.003);
    // the wall clock set an hour ahead: followed at once
    let set = kept.offset_at(t0 + Duration::from_secs(3610), || truth(3600.0) + 3600.0);
    assert!((set - truth(3600.0) - 3600.0).abs() < 1e-6);
}
