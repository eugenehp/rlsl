//! High-resolution local clock, matching liblsl's lsl_local_clock().
//!
//! [`local_clock`] has two possible sources:
//!
//! * [`ClockSource::Process`] (the default, and the only behaviour up to 0.0.7): seconds since
//!   this process first asked. Monotonic, but every process has its own zero, so time stamps of
//!   two programs on one machine differ by the difference of their start times.
//! * [`ClockSource::System`]: the machine's monotonic clock, which every process reads alike —
//!   what liblsl's `steady_clock` is. Time stamps of programs on one machine are then directly
//!   comparable and the clock offset between them is zero.
//!
//! The source is chosen once, before the clock is first used: with [`set_clock_source`], or with
//! `clock = system` in `lsl_api.cfg` / the `LSL_CLOCK=system` environment variable.
//! [`system_clock`] reads the machine clock whatever the source.

use std::sync::atomic::{AtomicU8, Ordering};

/// Where [`local_clock`] takes its time from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    /// Seconds since this process first read the clock (the default).
    Process,
    /// The machine's monotonic clock, shared by all processes.
    System,
}

const UNSET: u8 = 0;
const PROCESS: u8 = 1;
const SYSTEM: u8 = 2;
static SOURCE: AtomicU8 = AtomicU8::new(UNSET);

/// Choose the source of [`local_clock`] for this process. Call it before anything else in the
/// library: switching after time stamps have been taken makes the clock jump.
pub fn set_clock_source(source: ClockSource) {
    let v = match source {
        ClockSource::Process => PROCESS,
        ClockSource::System => SYSTEM,
    };
    SOURCE.store(v, Ordering::Relaxed);
}

/// The source [`local_clock`] uses.
pub fn clock_source() -> ClockSource {
    match SOURCE.load(Ordering::Relaxed) {
        PROCESS => ClockSource::Process,
        SYSTEM => ClockSource::System,
        _ => {
            // first use: the configuration decides, and the answer is kept
            let source = if crate::config::CONFIG.system_clock {
                ClockSource::System
            } else {
                ClockSource::Process
            };
            // somebody may have set it meanwhile: theirs wins
            let v = if source == ClockSource::System {
                SYSTEM
            } else {
                PROCESS
            };
            match SOURCE.compare_exchange(UNSET, v, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => source,
                Err(_) => clock_source(),
            }
        }
    }
}

/// Return the current local clock time in seconds (monotonic, high-resolution).
/// Equivalent to liblsl's lsl_local_clock().
pub fn local_clock() -> f64 {
    match clock_source() {
        ClockSource::Process => process::now(),
        ClockSource::System => imp::now(),
    }
}

/// The machine's monotonic clock in seconds — the same value in every process at the same
/// moment — whatever source [`local_clock`] uses. On platforms without one it is the process
/// clock.
pub fn system_clock() -> f64 {
    imp::now()
}

#[cfg(unix)]
mod imp {
    /// The clock `std::chrono::steady_clock` reads on this platform.
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC_RAW;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    const CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

    pub fn now() -> f64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `ts` is a valid, writable timespec.
        if unsafe { libc::clock_gettime(CLOCK, &mut ts) } != 0 {
            return super::process::now();
        }
        ts.tv_sec as f64 + ts.tv_nsec as f64 / 1_000_000_000.0
    }
}

#[cfg(windows)]
mod imp {
    extern "system" {
        fn QueryPerformanceCounter(count: *mut i64) -> i32;
        fn QueryPerformanceFrequency(frequency: *mut i64) -> i32;
    }

    pub fn now() -> f64 {
        let (mut count, mut frequency) = (0i64, 0i64);
        // SAFETY: both pointers are valid, writable i64s.
        let ok = unsafe {
            QueryPerformanceFrequency(&mut frequency) != 0
                && QueryPerformanceCounter(&mut count) != 0
        };
        if !ok || frequency <= 0 {
            return super::process::now();
        }
        // whole seconds and remainder separately, to keep the precision of large counts
        (count / frequency) as f64 + (count % frequency) as f64 / frequency as f64
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub fn now() -> f64 {
        super::process::now()
    }
}

/// Seconds since this process first asked: monotonic, but not shared with other processes.
mod process {
    use once_cell::sync::Lazy;
    use std::time::Instant;

    static EPOCH: Lazy<Instant> = Lazy::new(Instant::now);

    pub fn now() -> f64 {
        let elapsed = EPOCH.elapsed();
        elapsed.as_secs() as f64 + elapsed.subsec_nanos() as f64 / 1_000_000_000.0
    }
}
