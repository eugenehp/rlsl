//! Following one clock over time: a window of offset measurements and what they add up to.

use super::NtpSample;
use std::collections::VecDeque;

/// One measurement of another clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OffsetSample {
    /// When it was taken, on this machine's [`local_clock`](crate::clock::local_clock).
    pub at: f64,
    /// Seconds the other clock is ahead of this one.
    pub offset: f64,
    /// The round-trip delay of the measurement, in seconds.
    pub delay: f64,
}

impl From<&NtpSample> for OffsetSample {
    fn from(s: &NtpSample) -> Self {
        OffsetSample {
            at: s.local_time,
            offset: s.offset,
            delay: s.delay,
        }
    }
}

/// What a window of measurements of one clock says. Times in seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OffsetStats {
    /// Measurements in the window.
    pub count: usize,
    /// The time the window covers, first measurement to last.
    pub span: f64,
    /// The latest offset.
    pub last_offset: f64,
    pub mean_offset: f64,
    pub min_offset: f64,
    pub max_offset: f64,
    /// The standard deviation of the offsets: how much one measurement can be trusted.
    pub jitter: f64,
    /// How fast the other clock gains on this one, in parts per million (µs per second),
    /// from a straight line fitted to the offsets; `None` with fewer than two measurements
    /// at different times.
    pub drift_ppm: Option<f64>,
    pub mean_delay: f64,
    pub min_delay: f64,
    pub max_delay: f64,
}

impl OffsetStats {
    /// The statistics of `samples`, or `None` if there are none.
    pub fn of<'a>(samples: impl IntoIterator<Item = &'a OffsetSample>) -> Option<OffsetStats> {
        let samples: Vec<&OffsetSample> = samples.into_iter().collect();
        let last = *samples.last()?;
        let n = samples.len() as f64;
        let mean = |f: fn(&OffsetSample) -> f64| samples.iter().map(|s| f(s)).sum::<f64>() / n;
        let min =
            |f: fn(&OffsetSample) -> f64| samples.iter().map(|s| f(s)).fold(f64::MAX, f64::min);
        let max =
            |f: fn(&OffsetSample) -> f64| samples.iter().map(|s| f(s)).fold(f64::MIN, f64::max);

        let mean_offset = mean(|s| s.offset);
        let mean_at = mean(|s| s.at);
        let jitter = if samples.len() > 1 {
            let ss: f64 = samples
                .iter()
                .map(|s| (s.offset - mean_offset).powi(2))
                .sum();
            (ss / (n - 1.0)).sqrt()
        } else {
            0.0
        };
        // least squares, about the means to keep the precision of large clock values
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for s in &samples {
            let dx = s.at - mean_at;
            sxy += dx * (s.offset - mean_offset);
            sxx += dx * dx;
        }
        let drift_ppm = (sxx > 0.0).then(|| sxy / sxx * 1e6);

        Some(OffsetStats {
            count: samples.len(),
            span: max(|s| s.at) - min(|s| s.at),
            last_offset: last.offset,
            mean_offset,
            min_offset: min(|s| s.offset),
            max_offset: max(|s| s.offset),
            jitter,
            drift_ppm,
            mean_delay: mean(|s| s.delay),
            min_delay: min(|s| s.delay),
            max_delay: max(|s| s.delay),
        })
    }
}

/// The last measurements of one clock, up to a fixed number, and their statistics.
///
/// ```
/// use rlsl::ntp::{ClockTracker, OffsetSample};
///
/// let mut tracker = ClockTracker::new(100);
/// for i in 0..10 {
///     let t = i as f64 * 10.0;
///     // a clock 2 ms ahead that gains 20 µs per second
///     tracker.push(OffsetSample { at: t, offset: 0.002 + 20e-6 * t, delay: 0.001 });
/// }
/// let stats = tracker.stats().unwrap();
/// assert!((stats.drift_ppm.unwrap() - 20.0).abs() < 1e-6);
/// ```
#[derive(Debug, Clone)]
pub struct ClockTracker {
    window: VecDeque<OffsetSample>,
    capacity: usize,
    total: u64,
}

impl ClockTracker {
    /// A tracker keeping the last `capacity` measurements (at least one).
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            window: VecDeque::with_capacity(capacity.min(4096)),
            capacity,
            total: 0,
        }
    }

    /// Add a measurement, dropping the oldest if the window is full.
    pub fn push(&mut self, sample: OffsetSample) {
        if self.window.len() == self.capacity {
            self.window.pop_front();
        }
        self.window.push_back(sample);
        self.total += 1;
    }

    /// Add the result of an NTP exchange.
    pub fn push_ntp(&mut self, sample: &NtpSample) {
        self.push(sample.into());
    }

    /// The statistics of the window, or `None` before the first measurement.
    pub fn stats(&self) -> Option<OffsetStats> {
        OffsetStats::of(&self.window)
    }

    /// The measurements in the window, oldest first.
    pub fn samples(&self) -> impl Iterator<Item = &OffsetSample> {
        self.window.iter()
    }

    /// The latest measurement.
    pub fn last(&self) -> Option<&OffsetSample> {
        self.window.back()
    }

    /// How many measurements were ever added, including those no longer in the window.
    pub fn total_samples(&self) -> u64 {
        self.total
    }

    /// How many measurements the window holds at most.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn clear(&mut self) {
        self.window.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: f64, offset: f64, delay: f64) -> OffsetSample {
        OffsetSample { at, offset, delay }
    }

    #[test]
    fn summary() {
        let mut t = ClockTracker::new(10);
        assert!(t.stats().is_none());
        for (i, (o, d)) in [(1.0, 0.1), (3.0, 0.3), (2.0, 0.2)].into_iter().enumerate() {
            t.push(sample(i as f64, o, d));
        }
        let s = t.stats().unwrap();
        assert_eq!(s.count, 3);
        assert_eq!(s.span, 2.0);
        assert_eq!(s.last_offset, 2.0);
        assert_eq!((s.min_offset, s.max_offset), (1.0, 3.0));
        assert!((s.mean_offset - 2.0).abs() < 1e-12);
        assert!((s.jitter - 1.0).abs() < 1e-12); // sample standard deviation of 1, 3, 2
        assert!((s.drift_ppm.unwrap() - 0.5e6).abs() < 1e-6);
        assert!((s.mean_delay - 0.2).abs() < 1e-12);
        assert_eq!((s.min_delay, s.max_delay), (0.1, 0.3));
    }

    #[test]
    fn one_sample_has_no_drift() {
        let mut t = ClockTracker::new(10);
        t.push(sample(5.0, 0.001, 0.0));
        let s = t.stats().unwrap();
        assert_eq!(s.jitter, 0.0);
        assert_eq!(s.drift_ppm, None);
        assert_eq!(s.span, 0.0);
    }

    #[test]
    fn drift_on_a_large_clock() {
        // a machine up for weeks: local clock ~ 2e6 s; 35 ms offset gaining 12.5 ppm, with noise
        let mut t = ClockTracker::new(1000);
        for i in 0..600 {
            let at = 2.0e6 + i as f64;
            let noise = if i % 2 == 0 { 50e-6 } else { -50e-6 };
            t.push(sample(at, 0.035 + 12.5e-6 * i as f64 + noise, 0.01));
        }
        let s = t.stats().unwrap();
        assert!((s.drift_ppm.unwrap() - 12.5).abs() < 0.01, "{s:?}");
    }

    #[test]
    fn window_keeps_the_latest() {
        let mut t = ClockTracker::new(3);
        for i in 0..5 {
            t.push(sample(i as f64, i as f64, 0.0));
        }
        assert_eq!(t.total_samples(), 5);
        let kept: Vec<f64> = t.samples().map(|s| s.offset).collect();
        assert_eq!(kept, [2.0, 3.0, 4.0]);
        assert_eq!(t.stats().unwrap().min_offset, 2.0);
    }
}
