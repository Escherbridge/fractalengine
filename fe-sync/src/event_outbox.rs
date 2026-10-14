//! Non-blocking `SyncEvent` emission for the sync thread: lossless for
//! state-bearing events, coalescing for advisory ones — see `src/AGENTS.md`
//! §event-outbox.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crossbeam::channel::TrySendError;

use crate::messages::{SyncEvent, SyncEventSender};

/// Wait before retrying a backlog the main thread has not drained yet.
pub const BACKLOG_RETRY: Duration = Duration::from_millis(5);
/// Most advisory events held while the channel is full; more are dropped and counted.
pub const MAX_ADVISORY_BACKLOG: usize = 1024;
/// How long shutdown waits for the main thread to take the remaining backlog.
pub const SHUTDOWN_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
/// A state-bearing backlog this long (and every 4x past it) is warned about.
const BACKLOG_WARN_LEN: usize = 256;

/// Where sync-thread code sends a [`SyncEvent`]; never blocks the caller.
pub trait EventSink: Send + Sync {
    /// Emit `event` (queued when the channel is full, never blocking).
    fn emit(&self, event: SyncEvent);
}

/// What an advisory event is coalesced by.
#[derive(Clone, PartialEq, Eq, Hash)]
enum AdvisoryKey {
    /// Only the newest relay health matters (a queued one is superseded).
    RelayHealth,
    /// An identical advertisement already queued is a repeat.
    TilesetAd {
        peer_id: String,
        advertisements_json: String,
    },
}

/// `Some` for events that may be coalesced or dropped under backpressure.
fn advisory_key(event: &SyncEvent) -> Option<AdvisoryKey> {
    match event {
        SyncEvent::RelayHealthChanged { .. } => Some(AdvisoryKey::RelayHealth),
        SyncEvent::PeerTilesetAdvertisement {
            peer_id,
            advertisements_json,
        } => Some(AdvisoryKey::TilesetAd {
            peer_id: peer_id.clone(),
            advertisements_json: advertisements_json.clone(),
        }),
        _ => None,
    }
}

/// Events waiting for channel capacity, in emission order.
#[derive(Default)]
struct Backlog {
    events: VecDeque<SyncEvent>,
    /// Advisory keys currently queued (coalescing + the advisory cap).
    advisory: HashSet<AdvisoryKey>,
}

struct Inner {
    tx: SyncEventSender,
    backlog: Mutex<Backlog>,
    wake: tokio::sync::Notify,
    dropped_advisory: AtomicU64,
    disconnected: AtomicBool,
}

/// The sync thread's event emitter: `try_send` first; on a full channel the
/// event joins a loop-drained backlog (the self-drain rule — nothing ever
/// awaits or blocks on a send the loop itself must keep servicing).
#[derive(Clone)]
pub struct EventOutbox {
    inner: Arc<Inner>,
}

fn lock(mutex: &Mutex<Backlog>) -> MutexGuard<'_, Backlog> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl EventOutbox {
    /// Emit through `tx` (the bounded channel the main thread drains).
    pub fn new(tx: SyncEventSender) -> Self {
        Self {
            inner: Arc::new(Inner {
                tx,
                backlog: Mutex::new(Backlog::default()),
                wake: tokio::sync::Notify::new(),
                dropped_advisory: AtomicU64::new(0),
                disconnected: AtomicBool::new(false),
            }),
        }
    }

    /// The raw channel, for code that already `try_send`s with drop-and-count.
    pub fn sender(&self) -> SyncEventSender {
        self.inner.tx.clone()
    }

    /// Events waiting for channel capacity.
    pub fn backlog_len(&self) -> usize {
        lock(&self.inner.backlog).events.len()
    }

    /// Advisory events dropped (cap reached) or coalesced away so far.
    pub fn dropped_advisory(&self) -> u64 {
        self.inner.dropped_advisory.load(Ordering::Relaxed)
    }

    /// Move as much backlog as the channel takes now, in order; never blocks.
    pub fn flush(&self) {
        let mut backlog = lock(&self.inner.backlog);
        while let Some(event) = backlog.events.pop_front() {
            let key = advisory_key(&event);
            match self.inner.tx.try_send(event) {
                Ok(()) => {
                    if let Some(key) = key {
                        backlog.advisory.remove(&key);
                    }
                }
                Err(TrySendError::Full(event)) => {
                    backlog.events.push_front(event);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.mark_disconnected(&mut backlog);
                    break;
                }
            }
        }
    }

    /// Resolves when the loop should call [`Self::flush`]: after a short
    /// retry delay while a backlog waits, else when an emit queues one.
    pub async fn backlog_ready(&self) {
        if self.backlog_len() > 0 {
            tokio::time::sleep(BACKLOG_RETRY).await;
        } else {
            self.inner.wake.notified().await;
        }
    }

    /// Shutdown: keep flushing for up to `timeout`, then drop (and warn about) the rest.
    pub async fn flush_before_exit(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            self.flush();
            let left = self.backlog_len();
            if left == 0 {
                return;
            }
            if Instant::now() >= deadline {
                tracing::warn!(
                    left,
                    "sync thread exiting with undelivered events (main thread stopped draining)"
                );
                *lock(&self.inner.backlog) = Backlog::default();
                return;
            }
            tokio::time::sleep(BACKLOG_RETRY).await;
        }
    }

    fn mark_disconnected(&self, backlog: &mut Backlog) {
        if !self.inner.disconnected.swap(true, Ordering::Relaxed) {
            tracing::warn!("sync event receiver dropped — further events are discarded");
        }
        *backlog = Backlog::default();
    }

    fn note_dropped_advisory(&self) {
        let dropped = self.inner.dropped_advisory.fetch_add(1, Ordering::Relaxed) + 1;
        if dropped == 1 || dropped.is_multiple_of(1000) {
            tracing::warn!(
                dropped,
                "sync event channel full — advisory events coalesced or dropped"
            );
        }
    }

    /// Queue `event` behind the backlog (coalescing advisory repeats).
    fn enqueue(&self, backlog: &mut Backlog, event: SyncEvent) {
        let Some(key) = advisory_key(&event) else {
            backlog.events.push_back(event);
            let len = backlog.events.len();
            if len >= BACKLOG_WARN_LEN
                && len.is_power_of_two()
                && len.trailing_zeros().is_multiple_of(2)
            {
                tracing::warn!(
                    len,
                    "sync event backlog growing — the main thread is not draining events"
                );
            }
            return;
        };
        if backlog.advisory.contains(&key) {
            if key == AdvisoryKey::RelayHealth {
                if let Some(slot) = backlog
                    .events
                    .iter_mut()
                    .find(|queued| matches!(queued, SyncEvent::RelayHealthChanged { .. }))
                {
                    *slot = event;
                }
            }
            self.note_dropped_advisory();
            return;
        }
        if backlog.advisory.len() >= MAX_ADVISORY_BACKLOG {
            self.note_dropped_advisory();
            return;
        }
        backlog.advisory.insert(key);
        backlog.events.push_back(event);
    }
}

impl EventSink for EventOutbox {
    fn emit(&self, event: SyncEvent) {
        if self.inner.disconnected.load(Ordering::Relaxed) {
            return;
        }
        let mut backlog = lock(&self.inner.backlog);
        let event = if backlog.events.is_empty() {
            match self.inner.tx.try_send(event) {
                Ok(()) => return,
                Err(TrySendError::Full(event)) => event,
                Err(TrySendError::Disconnected(_)) => {
                    self.mark_disconnected(&mut backlog);
                    return;
                }
            }
        } else {
            event
        };
        self.enqueue(&mut backlog, event);
        drop(backlog);
        self.inner.wake.notify_one();
    }
}

/// Tests may hand handlers a raw channel; production code cannot (cfg(test) only).
#[cfg(test)]
impl EventSink for SyncEventSender {
    fn emit(&self, event: SyncEvent) {
        if let Err(e) = self.send(event) {
            tracing::warn!("sync event send failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay_config::RelayHealth;

    fn failed(seq: u32) -> SyncEvent {
        SyncEvent::ChunkFailed {
            tileset_id: "t".into(),
            chunk_seq: seq,
            reason: "x".into(),
        }
    }

    fn seq_of(event: &SyncEvent) -> Option<u32> {
        match event {
            SyncEvent::ChunkFailed { chunk_seq, .. } => Some(*chunk_seq),
            _ => None,
        }
    }

    /// W1 residual: 300 state-bearing events from spawned tasks over a
    /// full bounded(256) channel never block the loop task — it keeps
    /// serving its shutdown channel — and every event arrives, in order,
    /// once the receiver drains.
    #[test]
    fn a_burst_from_spawned_tasks_never_blocks_the_loop_and_loses_nothing() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (tx, rx) = crossbeam::channel::bounded(256);
        let outbox = EventOutbox::new(tx);
        let served_shutdown = rt.block_on(async {
            for seq in 0..300u32 {
                let outbox = outbox.clone();
                tokio::spawn(async move { outbox.emit(failed(seq)) });
            }
            let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);
            tokio::spawn(async move {
                tokio::task::yield_now().await;
                shutdown_tx.send(()).await.unwrap();
            });
            // The sync loop's shape: flush, then select over its inputs.
            loop {
                outbox.flush();
                tokio::select! {
                    _ = outbox.backlog_ready() => {}
                    _ = shutdown_rx.recv() => break true,
                    _ = tokio::time::sleep(Duration::from_secs(10)) => break false,
                }
            }
        });
        assert!(
            served_shutdown,
            "the loop served Shutdown with the channel full"
        );
        assert_eq!(rx.len(), 256, "the channel is full");
        assert_eq!(outbox.backlog_len(), 44, "the rest wait in the backlog");

        let mut seen: Vec<u32> = rx.try_iter().filter_map(|e| seq_of(&e)).collect();
        outbox.flush();
        seen.extend(rx.try_iter().filter_map(|e| seq_of(&e)));
        assert_eq!(seen, (0..300).collect::<Vec<_>>());
        assert_eq!(outbox.dropped_advisory(), 0);
    }

    /// Advisory events coalesce under backpressure: a newer relay health
    /// supersedes the queued one in place, and a repeated ad is dropped.
    #[test]
    fn advisory_events_coalesce_while_state_events_queue() {
        let (tx, rx) = crossbeam::channel::bounded(1);
        let outbox = EventOutbox::new(tx);
        outbox.emit(failed(0));
        let health = |health| SyncEvent::RelayHealthChanged { health };
        let ad = || SyncEvent::PeerTilesetAdvertisement {
            peer_id: "p".into(),
            advertisements_json: "[]".into(),
        };
        outbox.emit(health(RelayHealth::Degraded));
        outbox.emit(ad());
        outbox.emit(failed(1));
        outbox.emit(health(RelayHealth::Healthy));
        outbox.emit(ad());
        assert_eq!(outbox.backlog_len(), 3);
        assert_eq!(outbox.dropped_advisory(), 2);

        let mut delivered = Vec::new();
        for _ in 0..4 {
            delivered.push(rx.try_recv().unwrap());
            outbox.flush();
        }
        assert!(matches!(
            delivered[0],
            SyncEvent::ChunkFailed { chunk_seq: 0, .. }
        ));
        assert!(matches!(
            delivered[1],
            SyncEvent::RelayHealthChanged {
                health: RelayHealth::Healthy
            }
        ));
        assert!(matches!(
            delivered[2],
            SyncEvent::PeerTilesetAdvertisement { .. }
        ));
        assert!(matches!(
            delivered[3],
            SyncEvent::ChunkFailed { chunk_seq: 1, .. }
        ));
    }
}
