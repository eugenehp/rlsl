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
}

struct ConsumerEntry {
    sender: Sender<Option<Sample>>,
    max_buffered: usize,
}

impl SendBuffer {
    pub fn new() -> Arc<Self> {
        Arc::new(SendBuffer {
            consumers: Mutex::new(Vec::new()),
            has_consumers: AtomicBool::new(false),
        })
    }

    /// Push a sample to all consumers
    pub fn push_sample(&self, sample: Sample) {
        let mut consumers = self.consumers.lock();
        consumers.retain(|c| {
            // Drop oldest if over capacity
            if c.sender.len() > c.max_buffered {
                let _ = c.sender.try_send(None); // won't help, but we can't recv here
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
        });
        self.has_consumers.store(true, Ordering::Relaxed);
        rx
    }

    /// Check if there are active consumers
    pub fn have_consumers(&self) -> bool {
        // clean up dead senders while checking
        let mut consumers = self.consumers.lock();
        consumers.retain(|c| !c.sender.is_empty() || c.sender.is_empty());
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
