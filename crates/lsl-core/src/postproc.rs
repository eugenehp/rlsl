//! Timestamp post-processing filters.
//!
//! Implements the three standard LSL timestamp corrections:
//! - **Clocksync**: adds the estimated clock offset (time_correction)
//! - **Dejitter**: smooths timestamps using an exponential moving average
//! - **Monotonize**: ensures timestamps are strictly increasing

use crate::types::*;

/// Timestamp post-processing pipeline.
pub struct TimestampPostProcessor {
    flags: u32,
    clock_offset: f64,
    // Dejitter state
    _smoothing_halftime: f64,
    srate: f64,
    samples_seen: u64,
    expected_next: f64,
    alpha: f64,
    // Monotonize state
    last_output: f64,
}

impl TimestampPostProcessor {
    pub fn new(flags: u32, srate: f64, smoothing_halftime: f32) -> Self {
        let alpha = if srate > 0.0 && smoothing_halftime > 0.0 {
            // Exponential smoothing: alpha = 1 - exp(-1 / (halftime * srate))
            1.0 - (-1.0 / (smoothing_halftime as f64 * srate)).exp()
        } else {
            1.0
        };

        TimestampPostProcessor {
            flags,
            clock_offset: 0.0,
            _smoothing_halftime: smoothing_halftime as f64,
            srate,
            samples_seen: 0,
            expected_next: 0.0,
            alpha,
            last_output: 0.0,
        }
    }

    /// Update the clock offset (called periodically from time_correction probes).
    pub fn set_clock_offset(&mut self, offset: f64) {
        self.clock_offset = offset;
    }

    /// Process a single timestamp through the enabled filters.
    pub fn process(&mut self, ts: f64) -> f64 {
        let mut t = ts;

        // 1. Clock sync: add remote-to-local offset
        if self.flags & PROC_CLOCKSYNC != 0 {
            t += self.clock_offset;
        }

        // 2. Dejitter: exponential smoothing against expected timestamps
        if self.flags & PROC_DEJITTER != 0 && self.srate > 0.0 {
            if self.samples_seen == 0 {
                // First sample: initialize
                self.expected_next = t;
            } else {
                // Expected timestamp based on nominal rate
                self.expected_next += 1.0 / self.srate;
                // Blend observed with expected
                let error = t - self.expected_next;
                self.expected_next += self.alpha * error;
            }
            self.samples_seen += 1;
            t = self.expected_next;
        }

        // 3. Monotonize: ensure strictly increasing
        if self.flags & PROC_MONOTONIZE != 0 && t <= self.last_output {
            t = self.last_output + 1e-12; // tiny epsilon
        }

        self.last_output = t;
        t
    }

    /// Reset state (e.g. after clock reset).
    pub fn reset(&mut self) {
        self.samples_seen = 0;
        self.expected_next = 0.0;
        self.last_output = 0.0;
    }
}
