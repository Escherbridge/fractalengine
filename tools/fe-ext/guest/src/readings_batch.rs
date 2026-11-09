//! Readings batching: collect many readings, stage as many as the window has room for each tick.

use crate::readings::{self, Reading, Timestamp};
use crate::{window, HostError};

/// Take up to `room` items from the front of `pending`, in order.
pub fn take_chunk<T>(pending: &mut Vec<T>, room: usize) -> Vec<T> {
    let count = room.min(pending.len());
    pending.drain(..count).collect()
}

/// Readings waiting for window room; whatever does not fit stays queued for the next tick.
#[derive(Default)]
pub struct ReadingBatcher {
    pending: Vec<Reading>,
    dropped_non_finite: u64,
}

impl ReadingBatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one reading stamped with the flushing tick's time; non-finite values are dropped and counted.
    pub fn push(&mut self, anchor: &str, metric: &str, value: f64, units: &str) {
        self.push_at(anchor, metric, value, units, None);
    }

    /// Queue one reading with an explicit sensor time (epoch ms).
    pub fn push_at(&mut self, anchor: &str, metric: &str, value: f64, units: &str, recorded_at_ms: Option<i64>) {
        if !value.is_finite() {
            self.dropped_non_finite += 1;
            return;
        }
        self.pending.push(Reading {
            anchor: anchor.to_string(),
            metric: metric.to_string(),
            value,
            units: units.to_string(),
            recorded_at: recorded_at_ms.map(Timestamp::EpochMs),
        });
    }

    /// Readings still waiting.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Readings dropped for a non-finite value so far.
    pub fn dropped_non_finite(&self) -> u64 {
        self.dropped_non_finite
    }

    /// Stage what fits in this tick's window; returns how many were staged. Call once from `on_tick`.
    /// On a host refusal the chunk is requeued at the front, so nothing is lost.
    pub fn flush(&mut self) -> Result<usize, HostError> {
        let room = window::room().readings as usize;
        let chunk = take_chunk(&mut self.pending, room);
        if chunk.is_empty() {
            return Ok(0);
        }
        let staged = chunk.len();
        match readings::stage(&chunk) {
            Ok(()) => Ok(staged),
            Err(error) => {
                self.pending.splice(0..0, chunk);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_keep_order_and_leave_the_rest() {
        let mut pending = vec![1, 2, 3, 4, 5];
        assert_eq!(take_chunk(&mut pending, 2), vec![1, 2]);
        assert_eq!(pending, vec![3, 4, 5]);
        assert_eq!(take_chunk(&mut pending, 10), vec![3, 4, 5]);
        assert!(pending.is_empty());
    }
}
