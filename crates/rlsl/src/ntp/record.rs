//! Clock measurements as records that travel with the data, and what to do with them later.
//!
//! A [`ClockRecord`] is one measurement one machine made of another's clock. Records reach a
//! consumer in one of three ways (see [`ClockTransport`](super::ClockTransport)): inside a data
//! stream's own connection, as a companion stream next to it, or measured by the recorder
//! itself. Whichever way they came, [`ClockFit`] turns a series of them into a line — offset and
//! drift — to correct time stamps with after the recording.

use super::OffsetSample;
use crate::stream_info::StreamInfo;
use crate::types::ChannelFormat;

/// Which clocks a measurement compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ClockKind {
    /// The machines' monotonic clocks ([`system_clock`](crate::clock::system_clock)): what
    /// LSL time stamps are taken on by liblsl, and by rlsl with `LSL_CLOCK=system`. Use these
    /// to correct LSL time stamps.
    Mono,
    /// The machines' wall clocks (Unix time), as NTP compares them.
    Wall,
}

impl ClockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClockKind::Mono => "mono",
            ClockKind::Wall => "wall",
        }
    }

    pub fn parse(s: &str) -> Option<ClockKind> {
        match s {
            "mono" => Some(ClockKind::Mono),
            "wall" => Some(ClockKind::Wall),
            _ => None,
        }
    }
}

/// One measurement of node `to`'s clock by node `from`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClockRecord {
    /// The measuring node.
    pub from: String,
    /// The measured node.
    pub to: String,
    pub clock: ClockKind,
    /// When, on `from`'s monotonic clock ([`system_clock`](crate::clock::system_clock)).
    pub at: f64,
    /// Seconds `to`'s clock is ahead of `from`'s.
    pub offset: f64,
    /// Round-trip delay of the measurement; the offset is off by at most half of it.
    pub delay: f64,
    /// When, as Unix time on `from`'s wall clock: to line records up with wall-clock times.
    pub at_unix: f64,
}

impl ClockRecord {
    /// The names of the fields, in the order of [`to_fields`](Self::to_fields) — the channel
    /// labels of a clock stream.
    pub const FIELDS: [&'static str; 7] =
        ["from", "to", "clock", "at", "offset", "delay", "at_unix"];

    pub fn to_fields(&self) -> Vec<String> {
        vec![
            self.from.clone(),
            self.to.clone(),
            self.clock.as_str().to_string(),
            self.at.to_string(),
            self.offset.to_string(),
            self.delay.to_string(),
            self.at_unix.to_string(),
        ]
    }

    /// A record from its fields (a sample of a clock stream), or `None` if they are not one.
    pub fn from_fields<S: AsRef<str>>(f: &[S]) -> Option<ClockRecord> {
        if f.len() != Self::FIELDS.len() {
            return None;
        }
        let num = |i: usize| f[i].as_ref().parse::<f64>().ok().filter(|v| v.is_finite());
        let (from, to) = (f[0].as_ref(), f[1].as_ref());
        if from.is_empty() || to.is_empty() {
            return None;
        }
        Some(ClockRecord {
            from: from.to_string(),
            to: to.to_string(),
            clock: ClockKind::parse(f[2].as_ref())?,
            at: num(3)?,
            offset: num(4)?,
            delay: num(5)?,
            at_unix: num(6)?,
        })
    }

    /// The fields joined by tabs (names never contain tabs: they are replaced when made).
    pub fn to_line(&self) -> String {
        self.to_fields().join("\t")
    }

    pub fn from_line(line: &str) -> Option<ClockRecord> {
        Self::from_fields(&line.split('\t').collect::<Vec<_>>())
    }

    /// The measurement alone, for a [`ClockTracker`](super::ClockTracker) or [`ClockFit`].
    pub fn sample(&self) -> OffsetSample {
        OffsetSample {
            at: self.at,
            offset: self.offset,
            delay: self.delay,
        }
    }
}

/// A straight line through offset measurements, each weighted by how precise it is (one over
/// its delay squared): offset at time `t` = `offset0 + drift · (t − at0)`.
///
/// ```
/// use rlsl::ntp::{ClockFit, OffsetSample};
///
/// let samples: Vec<OffsetSample> = (0..20)
///     .map(|i| OffsetSample { at: i as f64, offset: 0.010 + 5e-6 * i as f64, delay: 0.001 })
///     .collect();
/// let fit = ClockFit::fit(&samples).unwrap();
/// assert!((fit.offset_at(100.0) - (0.010 + 5e-6 * 100.0)).abs() < 1e-9);
/// assert!((fit.drift_ppm() - 5.0).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockFit {
    /// The (weighted) mean measurement time the line is centred on.
    pub at0: f64,
    /// The offset at `at0`.
    pub offset0: f64,
    /// Seconds of offset gained per second (multiply by 10⁶ for ppm); 0 from one measurement.
    pub drift: f64,
    /// Root-mean-square distance of the measurements from the line.
    pub residual: f64,
    /// Measurements behind it.
    pub count: usize,
}

impl ClockFit {
    /// No delay counts as less than this, so that one lucky measurement cannot outweigh the
    /// rest.
    pub const MIN_DELAY: f64 = 100e-6;

    /// The line through `samples`, or `None` if there are none.
    pub fn fit<'a>(samples: impl IntoIterator<Item = &'a OffsetSample>) -> Option<ClockFit> {
        let samples: Vec<&OffsetSample> = samples.into_iter().collect();
        if samples.is_empty() {
            return None;
        }
        let w = |s: &OffsetSample| 1.0 / s.delay.max(Self::MIN_DELAY).powi(2);
        let sw: f64 = samples.iter().map(|s| w(s)).sum();
        let at0 = samples.iter().map(|s| w(s) * s.at).sum::<f64>() / sw;
        let y0 = samples.iter().map(|s| w(s) * s.offset).sum::<f64>() / sw;
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for s in &samples {
            let dx = s.at - at0;
            sxy += w(s) * dx * (s.offset - y0);
            sxx += w(s) * dx * dx;
        }
        let drift = if sxx > 0.0 { sxy / sxx } else { 0.0 };
        let residual = (samples
            .iter()
            .map(|s| (s.offset - (y0 + drift * (s.at - at0))).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt();
        Some(ClockFit {
            at0,
            offset0: y0,
            drift,
            residual,
            count: samples.len(),
        })
    }

    /// The line through those of `records` that measured `to` from `from` on clock `clock`.
    pub fn from_records<'a>(
        records: impl IntoIterator<Item = &'a ClockRecord>,
        from: &str,
        to: &str,
        clock: ClockKind,
    ) -> Option<ClockFit> {
        let samples: Vec<OffsetSample> = records
            .into_iter()
            .filter(|r| r.from == from && r.to == to && r.clock == clock)
            .map(ClockRecord::sample)
            .collect();
        Self::fit(&samples)
    }

    /// The offset the line gives at `t` (on the measuring node's clock).
    pub fn offset_at(&self, t: f64) -> f64 {
        self.offset0 + self.drift * (t - self.at0)
    }

    pub fn drift_ppm(&self) -> f64 {
        self.drift * 1e6
    }
}

/// The description of a clock stream: the [`ClockRecord`]s of `node` that go with the stream
/// `data` — a companion stream of an outlet, or the records a recorder received in-band.
///
/// Its samples are strings, one channel per field of [`ClockRecord::FIELDS`]; its source id is
/// the data stream's with `#clock` appended, and `<desc><clock_sync>` names the data stream.
pub fn clock_stream_info(data: &StreamInfo, node: &str) -> StreamInfo {
    let source_id = if data.source_id().is_empty() {
        format!("{}#clock", data.uid())
    } else {
        format!("{}#clock", data.source_id())
    };
    let info = StreamInfo::new(
        &format!("{} (clock)", data.name()),
        CLOCK_STREAM_TYPE,
        ClockRecord::FIELDS.len() as u32,
        0.0,
        ChannelFormat::String,
        &source_id,
    );
    let desc = info.desc();
    let channels = desc.append_child("channels");
    for label in ClockRecord::FIELDS {
        let ch = channels.append_child("channel");
        ch.append_child_value("label", label);
        let unit = match label {
            "at" | "offset" | "delay" | "at_unix" => "seconds",
            _ => "",
        };
        if !unit.is_empty() {
            ch.append_child_value("unit", unit);
        }
    }
    let sync = desc.append_child("clock_sync");
    sync.append_child_value("node", node);
    sync.append_child_value("stream_name", &data.name());
    sync.append_child_value("stream_source_id", &data.source_id());
    sync.append_child_value("stream_uid", &data.uid());
    sync.append_child_value(
        "offset_meaning",
        "seconds the clock of `to` is ahead of the clock of `from`, at `at` on `from`'s monotonic clock",
    );
    info
}

/// The type of clock streams.
pub const CLOCK_STREAM_TYPE: &str = "ClockSync";

/// The source id of the companion clock stream of a stream with this source id.
pub fn clock_source_id(source_id: &str) -> String {
    format!("{source_id}#clock")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ClockRecord {
        ClockRecord {
            from: "a".into(),
            to: "b".into(),
            clock: ClockKind::Mono,
            at: 12_345.678_901_234,
            offset: -0.038_196_5,
            delay: 0.000_7,
            at_unix: 1_794_797_633.841_073,
        }
    }

    #[test]
    fn line_round_trip() {
        let r = record();
        assert_eq!(ClockRecord::from_line(&r.to_line()), Some(r.clone()));
        assert_eq!(ClockRecord::from_fields(&r.to_fields()), Some(r));
        assert_eq!(ClockRecord::from_line("a\tb\tfoo\t1\t2\t3\t4"), None);
        assert_eq!(ClockRecord::from_line("a\tb\tmono\t1\t2"), None);
        assert_eq!(ClockRecord::from_line("a\tb\tmono\tNaN\t2\t3\t4"), None);
    }

    #[test]
    fn fit_prefers_precise_measurements() {
        let mut samples: Vec<OffsetSample> = (0..10)
            .map(|i| OffsetSample {
                at: i as f64,
                offset: 0.001,
                delay: 0.0005,
            })
            .collect();
        // one far-off measurement over a slow path
        samples.push(OffsetSample {
            at: 5.0,
            offset: 0.050,
            delay: 0.100,
        });
        let fit = ClockFit::fit(&samples).unwrap();
        assert!((fit.offset_at(5.0) - 0.001).abs() < 1e-5, "{fit:?}");
        assert_eq!(fit.count, 11);

        let one = ClockFit::fit(&samples[..1]).unwrap();
        assert_eq!((one.drift, one.offset_at(99.0)), (0.0, 0.001));
        assert!(ClockFit::fit(&[]).is_none());
    }

    #[test]
    fn fit_from_records_picks_the_pair() {
        let mut records = Vec::new();
        for i in 0..5 {
            let mut r = record();
            r.at = i as f64;
            r.offset = 0.002 * i as f64;
            records.push(r.clone());
            r.clock = ClockKind::Wall;
            r.offset = 1.0;
            records.push(r);
        }
        let fit = ClockFit::from_records(&records, "a", "b", ClockKind::Mono).unwrap();
        assert!((fit.drift - 0.002).abs() < 1e-12);
        assert!(ClockFit::from_records(&records, "b", "a", ClockKind::Mono).is_none());
    }

    #[test]
    fn clock_stream_describes_its_data_stream() {
        let data = StreamInfo::new("EEG", "EEG", 8, 250.0, ChannelFormat::Float32, "amp1");
        let info = clock_stream_info(&data, "mm4");
        assert_eq!(info.name(), "EEG (clock)");
        assert_eq!(info.type_(), CLOCK_STREAM_TYPE);
        assert_eq!(info.source_id(), clock_source_id("amp1"));
        assert_eq!(info.channel_count(), 7);
        assert_eq!(info.channel_format(), ChannelFormat::String);
        assert_eq!(info.desc().child("clock_sync").child_value("node"), "mm4");
    }
}
