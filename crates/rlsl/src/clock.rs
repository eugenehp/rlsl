//! High-resolution local clock, matching liblsl's lsl_local_clock().
//!
//! [`local_clock`] has two possible sources:
//!
//! * [`ClockSource::Process`] (the default, as in every release): seconds since this process
//!   first asked. Monotonic, but every process has its own zero, so time stamps of two programs
//!   on one machine differ by the difference of their start times.
//! * [`ClockSource::System`]: the machine's monotonic clock, which every process reads alike —
//!   liblsl's `steady_clock`. Time stamps of programs on one machine, rlsl or liblsl, are then
//!   directly comparable and the clock offset between them is zero.
//!
//! The source is chosen once, before the clock is first used: with [`set_clock_source`], with
//! `clock = system` (or `process`) in `lsl_api.cfg` / the `LSL_CLOCK` environment variable, or —
//! for a program that wants liblsl's behaviour throughout — by building with the cargo feature
//! `liblsl-defaults`, which makes the machine clock the default. [`system_clock`] reads the
//! machine clock whatever the source.

use std::sync::atomic::{AtomicU8, Ordering};

/// Where [`local_clock`] takes its time from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSource {
    /// Seconds since this process first read the clock (the default).
    Process,
    /// The machine's monotonic clock, shared by all processes, as in liblsl (the default with
    /// the `liblsl-defaults` feature).
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

/// What to add to a [`local_clock`] time to get Unix time (seconds since 1970), as the two
/// clocks stand right now. The two are read as close together as possible, several times, and
/// the tightest reading is kept.
///
/// This is not a constant. `local_clock()` is monotonic and Unix time is the wall clock, which
/// the system keeps in step with a time server by running it slightly fast or slow: over an hour
/// the two drift apart by tens of milliseconds (54 ms on one measured machine). A program that
/// records LSL time stamps as wall-clock time should therefore not take this once and keep it;
/// see [`UnixOffset`].
pub fn unix_offset_now() -> f64 {
    let unix = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    };
    (0..5)
        .map(|_| {
            let (a, lsl, b) = (unix(), local_clock(), unix());
            (b - a, (a + b) / 2.0 - lsl)
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map_or(0.0, |(_, offset)| offset)
}

/// [`unix_offset_now`], kept current: measured again every few seconds and followed at a
/// limited rate, so that time stamps mapped with it stay evenly spaced while they keep to the
/// wall clock. A wall clock that is *set* (a change of more than a second) is followed at once.
///
/// ```
/// let mut to_unix = rlsl::clock::UnixOffset::new();
/// let stamp = rlsl::clock::local_clock(); // or a sample's time stamp on the local clock
/// let unix_seconds = stamp + to_unix.offset();
/// # assert!(unix_seconds > 1.0e9);
/// ```
#[derive(Debug, Clone)]
pub struct UnixOffset {
    offset: f64,
    target: f64,
    measured: std::time::Instant,
    updated: std::time::Instant,
}

impl UnixOffset {
    /// How often the two clocks are compared.
    pub const REMEASURE: std::time::Duration = std::time::Duration::from_secs(5);
    /// The fastest the offset follows a change: 500 ppm, the most a disciplined wall clock is
    /// slewed.
    pub const SLEW_PER_S: f64 = 500e-6;
    /// A change this large (seconds) is the wall clock being set, not drifting.
    pub const STEP_S: f64 = 1.0;

    pub fn new() -> Self {
        Self::starting_at(unix_offset_now(), std::time::Instant::now())
    }

    /// With a given first offset and starting moment (for tests and replays).
    pub fn starting_at(offset: f64, now: std::time::Instant) -> Self {
        Self {
            offset,
            target: offset,
            measured: now,
            updated: now,
        }
    }

    /// What to add to a time on this process's [`local_clock`] to get Unix time.
    pub fn offset(&mut self) -> f64 {
        self.offset_at(std::time::Instant::now(), unix_offset_now)
    }

    /// [`offset`](Self::offset) at `now`, with `measure` comparing the clocks.
    pub fn offset_at(&mut self, now: std::time::Instant, measure: impl FnOnce() -> f64) -> f64 {
        if now.saturating_duration_since(self.measured) >= Self::REMEASURE {
            self.measured = now;
            self.target = measure();
        }
        let dt = now.saturating_duration_since(self.updated).as_secs_f64();
        self.updated = now;
        let behind = self.target - self.offset;
        if behind.abs() > Self::STEP_S {
            self.offset = self.target;
        } else {
            let step = Self::SLEW_PER_S * dt;
            self.offset += behind.clamp(-step, step);
        }
        self.offset
    }

    /// A [`local_clock`] time as Unix time.
    pub fn unix_time(&mut self, local_time: f64) -> f64 {
        local_time + self.offset()
    }
}

impl Default for UnixOffset {
    fn default() -> Self {
        Self::new()
    }
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
