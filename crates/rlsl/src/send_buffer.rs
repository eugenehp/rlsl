//! Send buffer: single-producer, multiple-consumer broadcast buffer for samples.

use crate::sample::Sample;
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A broadcast-style send buffer. The outlet pushes samples in, and each consumer
/// (TCP client session) gets its own queue.
pub struct SendBuffer {
    consumers: Mutex<Vec<ConsumerEntry>>,
    has_consumers: AtomicBool,
    /// Samples dropped from full consumer queues, over all consumers.
    dropped: std::sync::atomic::AtomicU64,
    /// The outlet's own `max_buffered` (seconds, or hundreds of samples): the most any consumer
    /// may ask it to hold. 0 = no limit set.
    max_buffered: std::sync::atomic::AtomicI32,
    /// The newest samples, kept for consumers that connect late and ask for them.
    history: Mutex<History>,
}

/// What an outlet keeps of what it has sent ([`SendBuffer::set_history`]).
#[derive(Default)]
struct History {
    /// How long a sample is kept; zero: nothing is.
    keep: std::time::Duration,
    /// And how many at most.
    max_samples: usize,
    /// The stream's sample interval, to give a sample without a time stamp of its own
    /// (a later sample of a chunk) the one it stands for.
    interval: f64,
    last_timestamp: f64,
    samples: std::collections::VecDeque<(std::time::Instant, Sample)>,
}

impl History {
    fn push(&mut self, sample: &Sample, now: std::time::Instant) {
        if self.keep.is_zero() {
            return;
        }
        let mut kept = sample.clone();
        // replayed on its own, a sample cannot lean on the one before it for its time
        if kept.timestamp == crate::types::DEDUCED_TIMESTAMP {
            kept.timestamp = self.last_timestamp + self.interval;
        }
        self.last_timestamp = kept.timestamp;
        self.samples.push_back((now, kept));
        while self.samples.len() > self.max_samples.max(1)
            || self
                .samples
                .front()
                .is_some_and(|(at, _)| now.saturating_duration_since(*at) > self.keep)
        {
            self.samples.pop_front();
        }
    }
}

struct ConsumerEntry {
    sender: Sender<Option<Sample>>,
    max_buffered: usize,
    /// For a consumer registered with [`SendBuffer::new_buffered_consumer`]: the other end of
    /// its queue, to drop its oldest sample when it is full, and the token that says the
    /// consumer is still there.
    overflow: Option<(Receiver<Option<Sample>>, std::sync::Weak<()>)>,
    /// The first drop from this consumer's queue has been logged.
    warned: AtomicBool,
}

/// A consumer's queue, as returned by [`SendBuffer::new_buffered_consumer`]. The consumer is
/// registered for as long as this value lives.
pub struct BufferedConsumer {
    pub receiver: Receiver<Option<Sample>>,
    _alive: Arc<()>,
}

impl SendBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(SendBuffer {
            consumers: Mutex::new(Vec::new()),
            has_consumers: AtomicBool::new(false),
            dropped: std::sync::atomic::AtomicU64::new(0),
            max_buffered: std::sync::atomic::AtomicI32::new(0),
            history: Mutex::new(History::default()),
        })
    }

    /// Push a sample to all consumers
    pub fn push_sample(&self, sample: Sample) {
        let mut consumers = self.consumers.lock();
        // (under the consumers' lock: a consumer that registers gets every sample exactly
        // once, from the history or from here)
        self.history.lock().push(&sample, std::time::Instant::now());
        consumers.retain(|c| {
            match &c.overflow {
                Some((queue, alive)) => {
                    if alive.strong_count() == 0 {
                        return false; // the consumer is gone
                    }
                    // Full: the oldest sample makes room, as in liblsl. The consumer stays
                    // connected and keeps receiving; it has lost the start of its backlog.
                    while c.sender.len() >= c.max_buffered.max(1) {
                        match queue.try_recv() {
                            // a shutdown sentinel is not a sample: put it back and stop
                            Ok(None) => {
                                let _ = c.sender.send(None);
                                break;
                            }
                            Ok(Some(_)) => {
                                // say so once per consumer, then count
                                if self.dropped.fetch_add(1, Ordering::Relaxed) == 0 || !c.warned.swap(true, Ordering::Relaxed) {
                                    log::warn!(
                                        "a consumer is {} samples behind: its oldest samples are being dropped",
                                        c.max_buffered
                                    );
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
                None => {
                    // Drop oldest if over capacity
                    if c.sender.len() > c.max_buffered {
                        let _ = c.sender.try_send(None); // won't help, but we can't recv here
                    }
                }
            }
            c.sender.send(Some(sample.clone())).is_ok()
        });
        self.has_consumers
            .store(!consumers.is_empty(), Ordering::Relaxed);
    }

    /// Wake up consumers (e.g., during shutdown)
    pub fn push_sentinel(&self) {
        let consumers = self.consumers.lock();
        for c in consumers.iter() {
            let _ = c.sender.send(None);
        }
    }

    /// Register a new consumer and return its receiver
    pub fn new_consumer(&self, max_buffered: usize) -> Receiver<Option<Sample>> {
        let (tx, rx) = unbounded();
        let mut consumers = self.consumers.lock();
        consumers.push(ConsumerEntry {
            sender: tx,
            max_buffered,
            overflow: None,
            warned: AtomicBool::new(false),
        });
        self.has_consumers.store(true, Ordering::Relaxed);
        rx
    }

    /// Register a consumer whose queue holds up to `max_samples` samples: when the consumer
    /// falls further behind than that, its oldest samples are dropped and it goes on from
    /// there. ([`new_consumer`](Self::new_consumer) ends the consumer's feed instead.)
    pub fn new_buffered_consumer(&self, max_samples: usize) -> BufferedConsumer {
        self.new_replaying_consumer(max_samples, 0.0)
    }

    /// Keep the samples of the last `seconds` (at most `max_samples` of them) for consumers
    /// that connect late and ask for what they missed
    /// ([`new_replaying_consumer`](Self::new_replaying_consumer)). Zero keeps nothing.
    /// `nominal_srate` is the stream's rate (0 for an irregular one).
    pub fn set_history(&self, seconds: f64, max_samples: usize, nominal_srate: f64) {
        let mut history = self.history.lock();
        history.keep = std::time::Duration::from_secs_f64(seconds.clamp(0.0, 86_400.0));
        history.max_samples = max_samples;
        history.interval = if nominal_srate > 0.0 {
            1.0 / nominal_srate
        } else {
            0.0
        };
        if history.keep.is_zero() {
            history.samples.clear();
        }
    }

    /// The seconds of history kept ([`set_history`](Self::set_history)).
    pub fn history_seconds(&self) -> f64 {
        self.history.lock().keep.as_secs_f64()
    }

    /// [`new_buffered_consumer`](Self::new_buffered_consumer) for a consumer that also wants
    /// what was sent in the `replay_seconds` before it connected: those samples, as far as the
    /// history holds them, are in its queue ahead of everything sent from now on.
    pub fn new_replaying_consumer(
        &self,
        max_samples: usize,
        replay_seconds: f64,
    ) -> BufferedConsumer {
        let (tx, rx) = unbounded();
        let alive = Arc::new(());
        let mut consumers = self.consumers.lock();
        if replay_seconds > 0.0 {
            let now = std::time::Instant::now();
            let wanted = std::time::Duration::from_secs_f64(replay_seconds.min(86_400.0));
            for (at, sample) in self.history.lock().samples.iter() {
                if now.saturating_duration_since(*at) <= wanted {
                    let _ = tx.send(Some(sample.clone()));
                }
            }
        }
        consumers.push(ConsumerEntry {
            sender: tx,
            max_buffered: max_samples,
            overflow: Some((rx.clone(), Arc::downgrade(&alive))),
            warned: AtomicBool::new(false),
        });
        self.has_consumers.store(true, Ordering::Relaxed);
        BufferedConsumer {
            receiver: rx,
            _alive: alive,
        }
    }

    /// Set the most (in the units of `max_buffered`: seconds, or hundreds of samples) this
    /// buffer holds for any one consumer, whatever the consumer asks for.
    pub fn set_max_buffered(&self, max_buffered: i32) {
        self.max_buffered
            .store(max_buffered.max(0), Ordering::Relaxed);
    }

    /// A consumer's request for `requested` of buffer, capped by
    /// [`set_max_buffered`](Self::set_max_buffered).
    pub fn granted_buffer(&self, requested: i32) -> i32 {
        match self.max_buffered.load(Ordering::Relaxed) {
            0 => requested,
            limit => requested.min(limit),
        }
    }

    /// Samples dropped so far because a consumer's queue was full (it had fallen further behind
    /// than its buffer holds), over all consumers. Zero means every consumer has been sent
    /// everything.
    pub fn dropped_samples(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Check if there are active consumers
    pub fn have_consumers(&self) -> bool {
        // forget consumers that have gone (those registered with a liveness token; the others
        // are found out at the next push)
        let mut consumers = self.consumers.lock();
        consumers.retain(|c| {
            c.overflow
                .as_ref()
                .is_none_or(|(_, alive)| alive.strong_count() > 0)
        });
        let has = !consumers.is_empty();
        self.has_consumers.store(has, Ordering::Relaxed);
        has
    }

    /// Wait until at least one consumer is registered
    pub fn wait_for_consumers(&self, timeout: f64) -> bool {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs_f64(timeout.max(0.0));
        loop {
            if !self.consumers.lock().is_empty() {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
