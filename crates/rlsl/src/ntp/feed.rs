//! Clock records inside a data stream's own connection.
//!
//! An rlsl inlet that wants them says `Rlsl-Clock-Records: 1` when it asks for the feed; an
//! outlet that can send them answers the same. (liblsl does neither, and its feed is untouched.)
//! From then on records are mixed in with the samples:
//!
//! * plain feed (protocol 1.10): `[0xC5][u16 LE length][record line]` between samples — a
//!   sample starts with tag 1 or 2, never 0xC5;
//! * compressed feed: a frame `[u16 LE length][u16 LE 0][record line]` — a sample is never
//!   0 bytes — which does not pass through the decoder.
//!
//! On connecting, the inlet is first sent the records the outlet still holds, so a recorder
//! that starts late has the measurements from before it started too.

use super::ClockRecord;
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// The header both ends send.
pub const HEADER: &str = "Rlsl-Clock-Records";
/// The tag of a clock record in a plain feed.
pub const CLOCK_TAG: u8 = 0xC5;
/// Records an outlet holds for consumers that connect later.
pub const HISTORY: usize = 20_000;

/// The records of one outlet: kept for later consumers, and handed to the connected ones.
#[derive(Default)]
pub struct ClockHub {
    history: Mutex<VecDeque<Arc<ClockRecord>>>,
    consumers: Mutex<Vec<crossbeam_channel::Sender<Arc<ClockRecord>>>>,
}

impl ClockHub {
    pub fn push(&self, record: ClockRecord) {
        let record = Arc::new(record);
        {
            let mut history = self.history.lock();
            if history.len() == HISTORY {
                history.pop_front();
            }
            history.push_back(record.clone());
        }
        self.consumers
            .lock()
            .retain(|tx| tx.send(record.clone()).is_ok());
    }

    /// A new consumer: the records held so far, then each new one.
    pub fn subscribe(&self) -> crossbeam_channel::Receiver<Arc<ClockRecord>> {
        let (tx, rx) = crossbeam_channel::unbounded();
        // the history and the subscription under one lock: nothing missed, nothing twice
        let history = self.history.lock();
        for r in history.iter() {
            let _ = tx.send(r.clone());
        }
        self.consumers.lock().push(tx);
        rx
    }

    pub fn len(&self) -> usize {
        self.history.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Append a record to a plain feed.
pub fn encode_plain(record: &ClockRecord, out: &mut Vec<u8>) {
    let line = record.to_line();
    let line = &line.as_bytes()[..line.len().min(u16::MAX as usize)];
    out.push(CLOCK_TAG);
    out.extend_from_slice(&(line.len() as u16).to_le_bytes());
    out.extend_from_slice(line);
}

/// Append a record to a compressed feed.
pub fn encode_compressed(record: &ClockRecord, out: &mut Vec<u8>) {
    let line = record.to_line();
    let line = &line.as_bytes()[..line.len().min(u16::MAX as usize)];
    out.extend_from_slice(&(line.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(line);
}

/// What an inlet has received: the records not yet pulled, and the latest measurement time of
/// each series so that a reconnection's repeat of the history is not taken twice.
#[derive(Default)]
pub struct ClockInbox {
    pending: Mutex<Vec<ClockRecord>>,
    latest: Mutex<HashMap<(String, String, super::ClockKind), f64>>,
}

impl ClockInbox {
    /// Take in one record as it came off the wire.
    pub fn accept(&self, bytes: &[u8]) {
        let Some(record) = std::str::from_utf8(bytes)
            .ok()
            .and_then(ClockRecord::from_line)
        else {
            return;
        };
        let key = (record.from.clone(), record.to.clone(), record.clock);
        {
            let mut latest = self.latest.lock();
            if latest.get(&key).is_some_and(|&at| record.at <= at) {
                return;
            }
            latest.insert(key, record.at);
        }
        self.pending.lock().push(record);
    }

    /// The records received since the last call, in the order they came.
    pub fn take(&self) -> Vec<ClockRecord> {
        std::mem::take(&mut *self.pending.lock())
    }
}

#[cfg(test)]
mod tests {
    use super::super::ClockKind;
    use super::*;

    fn rec(at: f64) -> ClockRecord {
        ClockRecord {
            from: "a".into(),
            to: "b".into(),
            clock: ClockKind::Mono,
            at,
            offset: 0.5,
            delay: 0.001,
            at_unix: 1.0e9 + at,
        }
    }

    #[test]
    fn late_consumers_get_the_history_once() {
        let hub = ClockHub::default();
        hub.push(rec(1.0));
        hub.push(rec(2.0));
        let rx = hub.subscribe();
        hub.push(rec(3.0));
        let got: Vec<f64> = rx.try_iter().map(|r| r.at).collect();
        assert_eq!(got, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn inbox_drops_repeats() {
        let inbox = ClockInbox::default();
        for at in [1.0, 2.0, 1.0, 2.0, 3.0] {
            inbox.accept(rec(at).to_line().as_bytes());
        }
        inbox.accept(b"garbage");
        let got: Vec<f64> = inbox.take().iter().map(|r| r.at).collect();
        assert_eq!(got, [1.0, 2.0, 3.0]);
        assert!(inbox.take().is_empty());
    }

    #[test]
    fn frames() {
        let r = rec(1.0);
        let line = r.to_line();
        let mut plain = Vec::new();
        encode_plain(&r, &mut plain);
        assert_eq!(plain[0], CLOCK_TAG);
        assert_eq!(
            u16::from_le_bytes([plain[1], plain[2]]) as usize,
            line.len()
        );
        assert_eq!(&plain[3..], line.as_bytes());
        let mut framed = Vec::new();
        encode_compressed(&r, &mut framed);
        assert_eq!(&framed[2..4], &[0, 0]);
        assert_eq!(&framed[4..], line.as_bytes());
    }
}
