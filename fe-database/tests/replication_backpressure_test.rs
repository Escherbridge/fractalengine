// FR-1 (p2p_unblock_now_20260711): a full replication bridge must drop-and-count,
// never block the DB thread. See fe-database/src/AGENTS.md §replication-backpressure.
// Single test fn: the drop counter is process-global, so scenarios run serially here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use fe_database::{replicate_row_with_petal, replication_drop_count};
use fe_runtime::blob_store::mock::MockBlobStore;
use fe_runtime::blob_store::BlobStoreHandle;

#[test]
fn full_bridge_drops_instead_of_blocking() {
    let store: BlobStoreHandle = Arc::new(MockBlobStore::new());
    // Bounded(1) channel with a live receiver that never drains: fill it.
    let (tx, _rx) = crossbeam::channel::bounded(1);
    replicate_row_with_petal(Some(&tx), &store, "v1", "node", "r1", b"{}", None);
    assert_eq!(tx.len(), 1, "first event should occupy the channel");

    let before = replication_drop_count();
    let start = Instant::now();
    // Channel is full — with the old blocking send this would hang forever.
    replicate_row_with_petal(Some(&tx), &store, "v1", "node", "r2", b"{}", None);
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "handler must return promptly when the bridge is full"
    );
    assert_eq!(
        replication_drop_count(),
        before + 1,
        "drop counter must increment on Full"
    );
    assert_eq!(tx.len(), 1, "dropped event must not enter the channel");

    // Disconnect is shutdown, not backpressure — no drop counted.
    let (tx2, rx2) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(1);
    drop(rx2);
    let before_disconnect = replication_drop_count();
    replicate_row_with_petal(Some(&tx2), &store, "v1", "node", "r3", b"{}", None);
    assert_eq!(replication_drop_count(), before_disconnect);
}
