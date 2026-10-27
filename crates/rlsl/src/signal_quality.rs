//! Signal quality metrics — SNR, dropout rate, jitter statistics.
//!
//! Attach a `SignalQuality` monitor to a stream to track real-time quality.

use std::collections::VecDeque;

/// Rolling signal-quality statistics for a stream.
#[derive(Clone, Debug)]
pub struct SignalQuality {
    /// Nominal sample rate (Hz), 0 for irregular
    srate: f64,
    /// Window of recent inter-sample intervals
    intervals: VecDeque<f64>,
    /// Max window size
    window: usize,
    /// Last timestamp seen
    last_ts: f64,
    /// Total samples received
    pub total_samples: u64,
    /// Total dropouts detected (gap > 1.5× expected interval)
    pub total_dropouts: u64,
    /// Running sum/sum² for channel amplitude (for SNR)
    ch_sum: Vec<f64>,
    ch_sum2: Vec<f64>,
    ch_count: u64,
}

/// Snapshot of quality metrics.
#[derive(Clone, Debug, Default)]
pub struct QualitySnapshot {
    /// Actual effective sample rate
    pub effective_srate: f64,
    /// Mean jitter (std dev of inter-sample intervals) in seconds
    pub jitter_sec: f64,
    /// Dropout rate (fraction of expected samples that were missing)
    pub dropout_rate: f64,
    /// Per-channel SNR in dB (signal = mean, noise = std dev)
    pub snr_db: Vec<f64>,
    /// Total samples received
    pub total_samples: u64,
    /// Total dropouts
    pub total_dropouts: u64,
}

impl SignalQuality {
    pub fn new(srate: f64, n_channels: usize) -> Self {
        SignalQuality {
            srate,
            intervals: VecDeque::with_capacity(2048),
            window: 2000,
            last_ts: 0.0,
            total_samples: 0,
            total_dropouts: 0,
            ch_sum: vec![0.0; n_channels],
            ch_sum2: vec![0.0; n_channels],
            ch_count: 0,
        }
    }

    /// Feed a sample timestamp + channel values.
    pub fn update(&mut self, timestamp: f64, channel_values: &[f64]) {
        self.total_samples += 1;

        // Track inter-sample intervals
        if self.last_ts > 0.0 && timestamp > self.last_ts {
            let dt = timestamp - self.last_ts;
            if self.intervals.len() >= self.window {
                self.intervals.pop_front();
            }
            self.intervals.push_back(dt);

            // Detect dropout
            if self.srate > 0.0 {
                let expected = 1.0 / self.srate;
                if dt > expected * 1.5 {
                    let missed = (dt / expected).round() as u64;
                    self.total_dropouts += missed.saturating_sub(1);
                }
            }
        }
        self.last_ts = timestamp;

        // Track channel statistics (for SNR)
        for (i, &v) in channel_values.iter().enumerate() {
            if i < self.ch_sum.len() {
                self.ch_sum[i] += v;
                self.ch_sum2[i] += v * v;
            }
        }
        self.ch_count += 1;
    }

    /// Compute a snapshot of current quality metrics.
    pub fn snapshot(&self) -> QualitySnapshot {
        let n = self.intervals.len();
        if n < 2 {
            return QualitySnapshot {
                total_samples: self.total_samples,
                total_dropouts: self.total_dropouts,
                ..Default::default()
            };
        }

        // Mean interval → effective sample rate
        let sum: f64 = self.intervals.iter().sum();
        let mean_dt = sum / n as f64;
        let effective_srate = if mean_dt > 0.0 { 1.0 / mean_dt } else { 0.0 };

        // Jitter = std dev of intervals
        let var: f64 = self
            .intervals
            .iter()
            .map(|&dt| (dt - mean_dt).powi(2))
            .sum::<f64>()
            / n as f64;
        let jitter_sec = var.sqrt();

        // Dropout rate
        let dropout_rate = if self.total_samples > 0 {
            self.total_dropouts as f64 / (self.total_samples + self.total_dropouts) as f64
        } else {
            0.0
        };

        // Per-channel SNR
        let snr_db = if self.ch_count > 1 {
            self.ch_sum
                .iter()
                .zip(self.ch_sum2.iter())
                .map(|(&s, &s2)| {
                    let mean = s / self.ch_count as f64;
                    let var = (s2 / self.ch_count as f64) - mean * mean;
                    let std = var.abs().sqrt();
                    if std > 1e-15 {
                        20.0 * (mean.abs() / std).log10()
                    } else {
                        f64::INFINITY
                    }
                })
                .collect()
        } else {
            vec![]
        };

        QualitySnapshot {
            effective_srate,
            jitter_sec,
            dropout_rate,
            snr_db,
            total_samples: self.total_samples,
            total_dropouts: self.total_dropouts,
        }
    }

    /// Reset all statistics.
    pub fn reset(&mut self) {
        self.intervals.clear();
        self.last_ts = 0.0;
        self.total_samples = 0;
        self.total_dropouts = 0;
        self.ch_sum.fill(0.0);
        self.ch_sum2.fill(0.0);
        self.ch_count = 0;
    }
}
