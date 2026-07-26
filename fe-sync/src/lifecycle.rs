//! Node lifecycle-event forwarding onto the sync/replication seam (FR-6).
//! See fe-sync/src/AGENTS.md §lifecycle-forwarding.

use fe_runtime::messages::LifecycleEvent;

/// Sender half for lifecycle events forwarded to the sync path.
pub type LifecycleEventSender = crossbeam::channel::Sender<LifecycleEvent>;
/// Receiver half for lifecycle events on the sync path.
pub type LifecycleEventReceiver = crossbeam::channel::Receiver<LifecycleEvent>;

/// Bounded lifecycle-event channel (matches the crate-wide `bounded(256)` seam
/// convention).
pub fn lifecycle_channel(bound: usize) -> (LifecycleEventSender, LifecycleEventReceiver) {
    crossbeam::channel::bounded(bound)
}

/// Bridges node lifecycle events (create / promote / delete-tombstone / reflow)
/// from the DB thread onto the sync/replication seam so peers and reporting
/// (T5) observe the same truth (FR-6).
///
/// Forwarding is non-blocking (`try_send`): a full channel drops the event with
/// a warn rather than stalling the emitting Bevy/DB system (N-5 — no blocking on
/// the channel seam). The op-log remains the durable source of truth, so a
/// dropped in-process forward is a lost notification, never lost data.
#[derive(Clone)]
pub struct LifecycleForwarder {
    tx: LifecycleEventSender,
}

impl LifecycleForwarder {
    /// Wrap a sender half.
    pub fn new(tx: LifecycleEventSender) -> Self {
        Self { tx }
    }

    /// Forward one lifecycle event to the sync path. Returns `false` if the
    /// channel is full or disconnected (the event is dropped, loud-logged).
    pub fn forward(&self, event: LifecycleEvent) -> bool {
        match self.tx.try_send(event) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("lifecycle event forward dropped: {e}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_event_reaches_the_sync_path() {
        let (tx, rx) = lifecycle_channel(8);
        let fwd = LifecycleForwarder::new(tx);

        let event = LifecycleEvent::NodeDeleted {
            address: "fe://v1/f1/p1/n1".into(),
            node_id: "n1".into(),
        };
        assert!(fwd.forward(event));

        let received = rx.recv().expect("event must reach the sync path");
        assert_eq!(received.address(), Some("fe://v1/f1/p1/n1"));
        match received {
            LifecycleEvent::NodeDeleted { node_id, .. } => assert_eq!(node_id, "n1"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn all_four_variants_forward() {
        let (tx, rx) = lifecycle_channel(8);
        let fwd = LifecycleForwarder::new(tx);
        let events = [
            LifecycleEvent::NodeCreated {
                address: "fe://v/f/p/n".into(),
                node_id: "n".into(),
            },
            LifecycleEvent::NodePromoted {
                address: "fe://v/f/p/path%23inst-1".into(),
                node_id: "path#inst-1".into(),
                path_id: "path".into(),
                instance_index: 1,
            },
            LifecycleEvent::NodeDeleted {
                address: "fe://v/f/p/n".into(),
                node_id: "n".into(),
            },
            LifecycleEvent::PathReflow {
                path_id: "path".into(),
            },
        ];
        for ev in events.iter().cloned() {
            assert!(fwd.forward(ev));
        }
        assert_eq!(rx.len(), 4, "every lifecycle op reaches the sync path");
    }

    #[test]
    fn full_channel_drops_without_blocking() {
        let (tx, _rx) = lifecycle_channel(1);
        let fwd = LifecycleForwarder::new(tx);
        // First send fills the single slot.
        assert!(fwd.forward(LifecycleEvent::PathReflow {
            path_id: "a".into()
        }));
        // Second must not block — it drops and returns false.
        assert!(!fwd.forward(LifecycleEvent::PathReflow {
            path_id: "b".into()
        }));
    }
}
