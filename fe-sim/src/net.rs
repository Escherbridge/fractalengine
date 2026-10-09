//! `SimNet` — the deterministic in-process network every simulated peer's
//! replica meets on (F8/A19). One hub per scenario; peers register by DID;
//! writes land in the hub's per-namespace virtual doc (latest-value-per-key,
//! iroh-docs semantics) and fan out to the other subscribers as scheduled
//! deliveries with scripted latency, membership, partitions, and churn.
//!
//! Everything is driven by the shared [`SimClock`]: delivery due-times are
//! simulated milliseconds, and [`SimNet::step`] drains what is due. With
//! zero latency a delivery lands on the very next drain after its write —
//! determinism comes from the (due_ms, seq) heap order, never from thread
//! scheduling.

use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fe_sync::replicator::{row_is_tombstone, ReplicatorFuture, RowChange};
use fe_sync::virtual_transport::{VirtualReplica, VirtualTransportFactory};
use fe_sync::VerseReplicator;

use crate::clock::SimClock;

/// Per-subscriber channel capacity (mirrors `MockVerseReplicator`'s 1024);
/// a full channel drops with a warn — §replication-backpressure, never a
/// blocking send on the hub.
const SUBSCRIBER_CAPACITY: usize = 1024;

/// One latest-per-key entry in a virtual doc (the doc's own state).
#[derive(Clone)]
struct DocEntry {
    table: String,
    record_id: String,
    data: Vec<u8>,
    author: String,
    /// Simulated epoch milliseconds at write time.
    written_ms: u64,
}

/// A live subscriber: one `subscribe()` registration (the sync thread's
/// per-replica inbound pump holds the receiver half).
struct Subscriber {
    peer: String,
    token: u64,
    tx: tokio::sync::mpsc::Sender<RowChange>,
}

/// One virtual document (per namespace).
struct VirtualDoc {
    entries: HashMap<String, DocEntry>,
    subscribers: Vec<Subscriber>,
}

/// An active partition: peers in different groups cannot exchange
/// deliveries. Multiple partitions may be active; any one separating a
/// pair blocks it.
struct Partition {
    groups: Vec<Vec<String>>,
}

/// Hub state under one lock.
struct NetState {
    docs: HashMap<String, VirtualDoc>,
    /// Every known peer's online flag (membership/churn).
    peers: HashMap<String, bool>,
    partitions: Vec<Partition>,
    /// Default per-link one-way latency (simulated ms).
    latency_ms: u64,
    /// In-flight deliveries, min-heap by (due_ms, seq).
    inflight: BinaryHeap<Delivery>,
    seq: u64,
}

/// A scheduled delivery to one subscriber.
struct Delivery {
    due_ms: u64,
    seq: u64,
    peer: String,
    token: u64,
    namespace_id: String,
    change: RowChange,
}

// BinaryHeap is a max-heap: order so the EARLIEST (due_ms, seq) pops first.
impl PartialEq for Delivery {
    fn eq(&self, other: &Self) -> bool {
        self.due_ms == other.due_ms && self.seq == other.seq
    }
}
impl Eq for Delivery {}
impl PartialOrd for Delivery {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Delivery {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse: greater Delivery = lower priority.
        other
            .due_ms
            .cmp(&self.due_ms)
            .then(other.seq.cmp(&self.seq))
    }
}

/// The deterministic virtual network hub.
pub struct SimNet {
    clock: Arc<SimClock>,
    state: Mutex<NetState>,
    /// Deliveries dropped at drain time (link down / channel full).
    dropped_deliveries: AtomicU64,
}

impl SimNet {
    /// New hub sharing `clock`.
    pub fn new(clock: Arc<SimClock>) -> Arc<Self> {
        Arc::new(Self {
            clock,
            state: Mutex::new(NetState {
                docs: HashMap::new(),
                peers: HashMap::new(),
                partitions: Vec::new(),
                latency_ms: 0,
                inflight: BinaryHeap::new(),
                seq: 0,
            }),
            dropped_deliveries: AtomicU64::new(0),
        })
    }

    /// The shared clock (scenario runners advance it, then [`Self::step`]).
    pub fn clock(&self) -> Arc<SimClock> {
        self.clock.clone()
    }

    /// Total deliveries dropped at drain time (fault effects + backpressure).
    pub fn dropped_deliveries(&self) -> u64 {
        self.dropped_deliveries.load(Ordering::SeqCst)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, NetState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register a peer (first registration is online). Re-registration keeps
    /// the current online flag — churn is a scripted decision
    /// ([`Self::set_peer_online`]), not an artifact of re-opening.
    pub fn register_peer(&self, peer: &str) {
        let mut state = self.lock();
        state.peers.entry(peer.to_string()).or_insert(true);
    }

    /// Scripted churn: take a peer offline (its writes stop reaching others,
    /// nothing is delivered to it — entries it writes while offline stay in
    /// the doc, like a real offline node recording locally) or bring it
    /// back. Coming back online converges the doc state to every linkable
    /// subscriber — the same latest-per-key convergence a rejoining
    /// iroh-docs swarm performs.
    pub fn set_peer_online(&self, peer: &str, online: bool) {
        let mut state = self.lock();
        let was_online = state.peers.get(peer).copied().unwrap_or(false);
        state.peers.insert(peer.to_string(), online);
        if online && !was_online {
            let docs: Vec<(String, Vec<DocEntry>)> = state
                .docs
                .iter()
                .map(|(ns, doc)| (ns.clone(), doc.entries.values().cloned().collect()))
                .collect();
            let now = self.clock.now_ms();
            for (ns, entries) in docs {
                for entry in &entries {
                    schedule_to_subscribers(
                        &mut state,
                        &ns,
                        &entry.author,
                        change_from_entry(entry, &entry.author),
                        now,
                    );
                }
            }
            tracing::info!(peer, "sim net: peer back online — converging doc state");
        }
    }

    /// Scripted fault: partition the network into `groups` (any pair split
    /// across groups of an active partition cannot exchange deliveries).
    pub fn partition(&self, groups: Vec<Vec<String>>) {
        tracing::info!(groups = ?groups, "sim net: partition activated");
        self.lock().partitions.push(Partition { groups });
    }

    /// Scripted fault: heal every active partition. A heal converges the
    /// doc state across the healed links (latest-per-key re-delivery is
    /// idempotent under union/merge semantics — own-author rows are
    /// filtered at each subscriber's inbound handler).
    pub fn heal(&self) {
        let mut state = self.lock();
        state.partitions.clear();
        let docs: Vec<(String, Vec<DocEntry>)> = state
            .docs
            .iter()
            .map(|(ns, doc)| (ns.clone(), doc.entries.values().cloned().collect()))
            .collect();
        let now = self.clock.now_ms();
        for (ns, entries) in docs {
            for entry in &entries {
                schedule_to_subscribers(
                    &mut state,
                    &ns,
                    &entry.author,
                    change_from_entry(entry, &entry.author),
                    now,
                );
            }
        }
        tracing::info!("sim net: partitions healed — doc state converging");
    }

    /// Scripted fault: set the default per-link latency (simulated ms).
    pub fn set_latency_ms(&self, latency_ms: u64) {
        self.lock().latency_ms = latency_ms;
    }

    /// Whether two peers are both online and not separated by an active
    /// partition. A peer absent from a partition's groups is unaffected by
    /// that partition.
    fn pair_linked(state: &NetState, a: &str, b: &str) -> bool {
        let a_online = state.peers.get(a).copied().unwrap_or(false);
        let b_online = state.peers.get(b).copied().unwrap_or(false);
        if !a_online || !b_online {
            return false;
        }
        for partition in &state.partitions {
            let a_group = partition
                .groups
                .iter()
                .position(|g| g.iter().any(|p| p == a));
            let b_group = partition
                .groups
                .iter()
                .position(|g| g.iter().any(|p| p == b));
            if let (Some(ga), Some(gb)) = (a_group, b_group) {
                if ga != gb {
                    return false;
                }
            }
        }
        true
    }

    /// Record a write (latest-per-key) and schedule fan-out deliveries.
    /// Called by `SimVerseReplicator::write_row`. An offline writer's entry
    /// still lands in the doc (the durable local write — a real offline
    /// node records locally too) but nothing is scheduled: no link is up.
    fn write_entry(
        &self,
        namespace_id: &str,
        peer: &str,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) {
        let mut state = self.lock();
        let now = self.clock.now_ms();
        let doc = state
            .docs
            .entry(namespace_id.to_string())
            .or_insert_with(|| VirtualDoc {
                entries: HashMap::new(),
                subscribers: Vec::new(),
            });
        doc.entries.insert(
            format!("{table}/{record_id}"),
            DocEntry {
                table: table.to_string(),
                record_id: record_id.to_string(),
                data: data.to_vec(),
                author: peer.to_string(),
                written_ms: now,
            },
        );
        let online = state.peers.get(peer).copied().unwrap_or(false);
        if !online {
            return;
        }
        let change = RowChange {
            table: table.to_string(),
            record_id: record_id.to_string(),
            content_hash: *blake3::hash(data).as_bytes(),
            author_id: peer.to_string(),
            timestamp: now.saturating_mul(1000), // µs, iroh entry parity
            is_tombstone: row_is_tombstone(data),
            data: data.to_vec(),
        };
        schedule_to_subscribers(&mut state, namespace_id, peer, change, now);
    }

    /// Register a subscriber for `namespace_id` (one per `subscribe()`),
    /// returning the receiver and its registration token (the replicator
    /// presents the token at `close()`).
    fn subscribe(
        &self,
        namespace_id: &str,
        peer: &str,
    ) -> (tokio::sync::mpsc::Receiver<RowChange>, u64) {
        let mut state = self.lock();
        let (tx, rx) = tokio::sync::mpsc::channel(SUBSCRIBER_CAPACITY);
        let token = state.seq;
        state.seq += 1;
        state.peers.entry(peer.to_string()).or_insert(true);
        let doc = state
            .docs
            .entry(namespace_id.to_string())
            .or_insert_with(|| VirtualDoc {
                entries: HashMap::new(),
                subscribers: Vec::new(),
            });
        doc.subscribers.push(Subscriber {
            peer: peer.to_string(),
            token,
            tx,
        });
        (rx, token)
    }

    /// Remove one subscriber registration (its `close()`).
    fn unsubscribe(&self, namespace_id: &str, peer: &str, token: u64) {
        let mut state = self.lock();
        if let Some(doc) = state.docs.get_mut(namespace_id) {
            doc.subscribers
                .retain(|s| !(s.peer == peer && s.token == token));
        }
    }

    /// The doc's current latest-per-key entries as `RowChange`s — the
    /// startup-reconciliation snapshot (a rejoining peer converges through
    /// the same inbound apply path the live pump uses).
    fn snapshot(&self, namespace_id: &str) -> Vec<RowChange> {
        let state = self.lock();
        state
            .docs
            .get(namespace_id)
            .map(|doc| {
                doc.entries
                    .values()
                    .map(|e| change_from_entry(e, &e.author))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drain every delivery due at or before the clock's current time.
    /// The scenario runner calls this after each clock advance; deliveries
    /// are processed in (due_ms, seq) order. A delivery whose link broke
    /// while in flight is dropped (counted) — the honest loss a partition
    /// causes. A full subscriber channel drops with a warn
    /// (§replication-backpressure), never blocks the hub.
    pub fn step(&self) -> usize {
        let mut delivered = 0usize;
        loop {
            let next = {
                let now = self.clock.now_ms();
                let mut state = self.lock();
                match state.inflight.peek() {
                    Some(d) if d.due_ms <= now => {
                        state.inflight.pop().expect("peeked delivery present")
                    }
                    _ => return delivered,
                }
            };
            // Link state is re-checked at delivery time — a partition that
            // hit while the message was in flight loses it, like real nets.
            let linked = {
                let state = self.lock();
                Self::pair_linked(&state, &next.change.author_id, &next.peer)
            };
            if !linked {
                self.dropped_deliveries.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            // `try_send`'s error carries the whole RowChange back; fold it
            // into a small outcome here so the closure's Result stays thin
            // (clippy result_large_err) — the payload is already cloned.
            enum SendOutcome {
                Delivered,
                Backpressure,
                SubscriberGone,
            }
            let send_outcome = {
                let state = self.lock();
                state
                    .docs
                    .get(&next.namespace_id)
                    .and_then(|doc| {
                        doc.subscribers
                            .iter()
                            .find(|s| s.peer == next.peer && s.token == next.token)
                            .map(|s| match s.tx.try_send(next.change.clone()) {
                                Ok(()) => SendOutcome::Delivered,
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                    SendOutcome::Backpressure
                                }
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    SendOutcome::SubscriberGone
                                }
                            })
                    })
                    // No matching subscriber: its replica closed between
                    // scheduling and delivery — the same shape as a closed
                    // channel (expected, counted, never fatal).
                    .unwrap_or(SendOutcome::SubscriberGone)
            };
            match send_outcome {
                SendOutcome::Delivered => delivered += 1,
                SendOutcome::Backpressure => {
                    self.dropped_deliveries.fetch_add(1, Ordering::SeqCst);
                    tracing::warn!(
                        peer = %next.peer,
                        "sim net: subscriber channel full — delivery dropped (drop-and-count)"
                    );
                }
                SendOutcome::SubscriberGone => {
                    // The pump shut down (replica closed) — expected, counted.
                    self.dropped_deliveries.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }

    /// Number of deliveries still in flight (diagnostics / settle checks).
    pub fn inflight_count(&self) -> usize {
        self.lock().inflight.len()
    }
}

/// Schedule one `change` (authored by `author`) to every subscriber of
/// `namespace_id` whose link to `author` is up — the fan-out half of a
/// live write and of the heal/return convergence passes.
fn schedule_to_subscribers(
    state: &mut NetState,
    namespace_id: &str,
    author: &str,
    change: RowChange,
    now_ms: u64,
) {
    let Some(doc) = state.docs.get(namespace_id) else {
        return;
    };
    let subscribers: Vec<(String, u64)> = doc
        .subscribers
        .iter()
        .map(|s| (s.peer.clone(), s.token))
        .collect();
    for (peer, token) in subscribers {
        // iroh parity: own writes never echo back to the author — the
        // inbound handler filters own-author rows anyway, so re-delivering
        // them would only burn subscriber channel budget.
        if peer == author {
            continue;
        }
        if !SimNet::pair_linked(state, author, &peer) {
            continue;
        }
        state.seq += 1;
        state.inflight.push(Delivery {
            due_ms: now_ms.saturating_add(state.latency_ms),
            seq: state.seq,
            peer,
            token,
            namespace_id: namespace_id.to_string(),
            change: change.clone(),
        });
    }
}

/// Render a doc entry as the `RowChange` an inbound pump delivers.
fn change_from_entry(entry: &DocEntry, author: &str) -> RowChange {
    RowChange {
        table: entry.table.clone(),
        record_id: entry.record_id.clone(),
        content_hash: *blake3::hash(&entry.data).as_bytes(),
        author_id: author.to_string(),
        timestamp: entry.written_ms.saturating_mul(1000),
        is_tombstone: row_is_tombstone(&entry.data),
        data: entry.data.clone(),
    }
}

// ---------------------------------------------------------------------------
// SimVerseReplicator — the per-verse virtual replica
// ---------------------------------------------------------------------------

/// The per-verse virtual replica the sync thread drives: implements the
/// SAME `VerseReplicator` + open-lifecycle contract as `IrohDocsReplicator`
/// (fe-sync `virtual_transport.rs`, decision D3) over the shared [`SimNet`]
/// hub. Open-phase calls are near-trivial — the hub is always available —
/// but the lifecycle contract (loud non-replication after a failed open,
/// closed-replica write rejection) is preserved for parity.
pub struct SimVerseReplicator {
    net: Arc<SimNet>,
    #[allow(dead_code)]
    verse_id: String,
    namespace_id: String,
    peer: String,
    /// This instance's subscriber registration tokens (one per subscribe).
    tokens: Mutex<Vec<u64>>,
    closed: AtomicU64,
    open_failed: Mutex<Option<String>>,
}

impl SimVerseReplicator {
    /// Built by [`SimTransportFactory`] at each replica open.
    pub fn new(net: Arc<SimNet>, verse_id: String, namespace_id: String, peer: String) -> Self {
        net.register_peer(&peer);
        Self {
            net,
            verse_id,
            namespace_id,
            peer,
            tokens: Mutex::new(Vec::new()),
            closed: AtomicU64::new(0),
            open_failed: Mutex::new(None),
        }
    }

    fn check_live(&self) -> anyhow::Result<()> {
        if let Some(reason) = self
            .open_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            anyhow::bail!("replica is not replicating: document open failed: {reason}");
        }
        if self.closed.load(Ordering::SeqCst) != 0 {
            anyhow::bail!("SimVerseReplicator is closed");
        }
        Ok(())
    }
}

impl VerseReplicator for SimVerseReplicator {
    fn write_row(
        &self,
        table: &str,
        record_id: &str,
        data: &[u8],
    ) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        let table = table.to_string();
        let record_id = record_id.to_string();
        let data = data.to_vec();
        Box::pin(async move {
            self.check_live()?;
            self.net
                .write_entry(&self.namespace_id, &self.peer, &table, &record_id, &data);
            Ok(())
        })
    }

    fn subscribe(
        &self,
    ) -> ReplicatorFuture<'_, anyhow::Result<tokio::sync::mpsc::Receiver<RowChange>>> {
        Box::pin(async move {
            self.check_live()?;
            let (rx, token) = self.net.subscribe(&self.namespace_id, &self.peer);
            self.tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(token);
            Ok(rx)
        })
    }

    fn snapshot(&self) -> ReplicatorFuture<'_, anyhow::Result<Vec<RowChange>>> {
        Box::pin(async move {
            self.check_live()?;
            Ok(self.net.snapshot(&self.namespace_id))
        })
    }

    fn close(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            self.closed.store(1, Ordering::SeqCst);
            let tokens: Vec<u64> =
                std::mem::take(&mut *self.tokens.lock().unwrap_or_else(|e| e.into_inner()));
            for token in tokens {
                self.net.unsubscribe(&self.namespace_id, &self.peer, token);
            }
            Ok(())
        })
    }
}

impl VirtualReplica for SimVerseReplicator {
    /// The virtual doc is always openable — the hub is in-process. (The
    /// namespace capability is deliberately not enforced here: admission
    /// control on the sim plane is the A3 role gate on each peer's DB
    /// thread, exactly as in prod.)
    fn open_document(&self) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn is_doc_backed(&self) -> bool {
        true
    }

    /// Membership is scripted in the hub (`register_peer`/churn faults), so
    /// there is nothing to dial — the iroh `start_sync` equivalent is the
    /// subscription the sync thread's pump already holds.
    fn start_sync(&self, _peers: Vec<iroh::NodeAddr>) -> ReplicatorFuture<'_, anyhow::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn mark_open_failed(&self, reason: String) {
        *self.open_failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(reason);
    }

    fn open_error(&self) -> Option<String> {
        self.open_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

// ---------------------------------------------------------------------------
// SimTransportFactory — the per-peer injection point
// ---------------------------------------------------------------------------

/// Builds this peer's replicas on the shared hub; handed to
/// `TestPeer::spawn_with_transport` → `spawn_sync_thread_with_transport`.
///
/// Carries NO peer identity of its own: the replica's author identity is
/// the sync thread's `local_did` (handed to [`Self::open_replica`] at every
/// open), because that DID is what the A3 inbound role gate resolves —
/// a factory-supplied label would desynchronize row authorship from the
/// verse's `created_by` and every row would deny.
pub struct SimTransportFactory {
    net: Arc<SimNet>,
}

impl SimTransportFactory {
    /// One factory per simulated peer's sync thread.
    pub fn new(net: Arc<SimNet>) -> Arc<Self> {
        Arc::new(Self { net })
    }
}

impl VirtualTransportFactory for SimTransportFactory {
    fn open_replica(
        &self,
        verse_id: &str,
        namespace_id: &str,
        _namespace_secret: Option<String>,
        local_did: &str,
    ) -> Box<dyn VirtualReplica> {
        Box::new(SimVerseReplicator::new(
            self.net.clone(),
            verse_id.to_string(),
            namespace_id.to_string(),
            local_did.to_string(),
        ))
    }

    fn is_available(&self) -> bool {
        true // in-process hub
    }

    fn describe(&self) -> &'static str {
        "virtual (sim)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one `async` block on a minimal current-thread runtime (the
    /// replicator futures are boxed and runtime-agnostic).
    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(fut)
    }

    struct Fixture {
        net: Arc<SimNet>,
        alpha: SimVerseReplicator,
        beta: SimVerseReplicator,
        beta_rx: tokio::sync::mpsc::Receiver<RowChange>,
    }

    /// Two subscribed replicas on one hub doc, clock at t=1_000_000.
    fn fixture() -> Fixture {
        let clock = SimClock::new(1_000_000);
        let net = SimNet::new(clock);
        let alpha =
            SimVerseReplicator::new(net.clone(), "v1".into(), "ns".into(), "did:alpha".into());
        let beta =
            SimVerseReplicator::new(net.clone(), "v1".into(), "ns".into(), "did:beta".into());
        let beta_rx = block_on(beta.subscribe()).expect("beta subscribes");
        let _alpha_rx = block_on(alpha.subscribe()).expect("alpha subscribes");
        Fixture {
            net,
            alpha,
            beta,
            beta_rx,
        }
    }

    fn row(id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({ "node_id": id, "v": 1 })).unwrap()
    }

    #[test]
    fn writes_cross_in_order_and_never_echo_to_the_author() {
        let mut f = fixture();
        block_on(f.alpha.write_row("node", "n1", &row("n1"))).unwrap();
        block_on(f.alpha.write_row("node", "n2", &row("n2"))).unwrap();
        assert_eq!(
            f.net.inflight_count(),
            2,
            "zero-latency fan-out schedules immediately"
        );
        let delivered = f.net.step();
        assert_eq!(delivered, 2);

        let first = f.beta_rx.try_recv().expect("n1 crosses");
        assert_eq!(first.record_id, "n1");
        assert_eq!(first.author_id, "did:alpha");
        assert!(!first.is_tombstone);
        let second = f.beta_rx.try_recv().expect("n2 crosses in write order");
        assert_eq!(second.record_id, "n2");
        assert!(f.beta_rx.try_recv().is_err(), "no further deliveries");
    }

    #[test]
    fn latest_per_key_snapshot_and_overwrite() {
        let f = fixture();
        block_on(f.alpha.write_row("node", "n1", &row("n1"))).unwrap();
        let newer = serde_json::to_vec(&serde_json::json!({ "node_id": "n1", "v": 2 })).unwrap();
        block_on(f.alpha.write_row("node", "n1", &newer)).unwrap();
        f.net.step();
        let snap = block_on(f.beta.snapshot()).unwrap();
        assert_eq!(snap.len(), 1, "latest-per-key: one entry for the key");
        assert_eq!(snap[0].data, newer, "the doc holds the latest write");

        // A fresh replica converges from the snapshot at open time.
        let joiner =
            SimVerseReplicator::new(f.net.clone(), "v1".into(), "ns".into(), "did:gamma".into());
        let joiner_snap = block_on(joiner.snapshot()).unwrap();
        assert_eq!(joiner_snap.len(), 1);
    }

    #[test]
    fn latency_delays_the_delivery_until_the_simulated_due_time() {
        let mut f = fixture();
        f.net.set_latency_ms(500);
        block_on(f.alpha.write_row("node", "n1", &row("n1"))).unwrap();
        assert_eq!(f.net.step(), 0, "not due yet at t=1_000_000");
        assert!(f.beta_rx.try_recv().is_err());
        f.net.clock().advance_ms(499);
        assert_eq!(f.net.step(), 0, "still not due at t+499");
        f.net.clock().advance_ms(1);
        assert_eq!(f.net.step(), 1, "due exactly at t+500");
        assert_eq!(
            f.beta_rx.try_recv().expect("delivery lands").record_id,
            "n1"
        );
    }

    #[test]
    fn partitions_drop_inflight_and_heals_converge() {
        let mut f = fixture();
        // Cut alpha|beta, then write — nothing is even scheduled.
        f.net
            .partition(vec![vec!["did:alpha".into()], vec!["did:beta".into()]]);
        block_on(f.alpha.write_row("node", "n1", &row("n1"))).unwrap();
        f.net.step();
        assert!(f.beta_rx.try_recv().is_err(), "partitioned: no crossing");
        assert_eq!(
            f.net.dropped_deliveries(),
            0,
            "never scheduled, not dropped"
        );

        // Heal: doc state converges across the healed link.
        f.net.heal();
        f.net.step();
        let change = f.beta_rx.try_recv().expect("heal converges the doc state");
        assert_eq!(change.record_id, "n1");
        assert_eq!(change.author_id, "did:alpha");

        // A partition that hits WHILE in flight loses the message (the
        // link is re-checked at delivery time).
        f.net.set_latency_ms(100);
        block_on(f.alpha.write_row("node", "n2", &row("n2"))).unwrap();
        f.net
            .partition(vec![vec!["did:alpha".into()], vec!["did:beta".into()]]);
        f.net.clock().advance_ms(100);
        f.net.step();
        assert!(
            f.beta_rx.try_recv().is_err(),
            "in-flight message lost to the cut"
        );
        assert_eq!(f.net.dropped_deliveries(), 1, "the loss is counted");
    }

    #[test]
    fn offline_writer_records_locally_and_converges_on_return() {
        let mut f = fixture();
        f.net.set_peer_online("did:beta", false);
        block_on(f.alpha.write_row("node", "n1", &row("n1"))).unwrap();
        // Wait — with beta offline, nothing is scheduled to beta…
        assert_eq!(f.net.inflight_count(), 0);
        f.net.step();
        assert!(f.beta_rx.try_recv().is_err());

        // But the OFFLINE writer still records locally: its entry is in the
        // doc (the durable local write a real offline node performs).
        f.net.set_peer_online("did:alpha", false);
        block_on(f.alpha.write_row("node", "n2", &row("n2"))).unwrap();
        let snap = block_on(f.alpha.snapshot()).unwrap();
        assert_eq!(snap.len(), 2, "offline writes persist in the doc");

        // On return, convergence replays the doc to every linkable peer.
        f.net.set_peer_online("did:beta", true);
        f.net.set_peer_online("did:alpha", true);
        f.net.step();
        let mut ids: Vec<String> = (0..2)
            .filter_map(|_| f.beta_rx.try_recv().ok().map(|c| c.record_id))
            .collect();
        ids.sort();
        assert_eq!(
            ids,
            vec!["n1".to_string(), "n2".to_string()],
            "both rows converge on return"
        );
    }

    #[test]
    fn closed_replicas_reject_writes_and_unsubscribe() {
        let mut f = fixture();
        block_on(f.beta.close()).unwrap();
        assert!(block_on(f.beta.write_row("node", "n9", &row("n9"))).is_err());
        // Alpha can still write; the closed subscription is gone, so
        // fan-out finds nobody for beta (no drop, no delivery).
        block_on(f.alpha.write_row("node", "n1", &row("n1"))).unwrap();
        f.net.step();
        assert!(f.beta_rx.try_recv().is_err());
        assert_eq!(f.net.dropped_deliveries(), 0);
    }

    #[test]
    fn failed_open_leaves_the_replica_loudly_non_replicating() {
        let f = fixture();
        f.beta.mark_open_failed("sim injected open failure".into());
        assert_eq!(
            f.beta.open_error().as_deref(),
            Some("sim injected open failure")
        );
        let err = block_on(f.beta.write_row("node", "n1", &row("n1")))
            .expect_err("failed-open replicas must not silently succeed");
        assert!(
            err.to_string().contains("not replicating"),
            "the rejection must be loud: {err}"
        );
        // Parity shape: the replica is doc-backed and openable (the hub is
        // in-process) — the injected failure is the only non-replicating bit.
        assert!(f.beta.is_doc_backed());
        block_on(f.beta.open_document()).unwrap();
    }

    #[test]
    fn factory_builds_replicas_identity_from_the_local_did() {
        let mut f = fixture();
        block_on(f.alpha.write_row("node", "n1", &row("n1"))).unwrap();
        f.net.step();
        // Consume n1 first so the next delivery is unambiguous (the hub
        // drains in (due_ms, seq) order — n1 precedes n3 by seq).
        let n1 = f.beta_rx.try_recv().expect("n1 crosses");
        assert_eq!(n1.author_id, "did:alpha");

        let factory = SimTransportFactory::new(f.net.clone());
        let replica = factory.open_replica("v1", "ns", None, "did:some-peer");
        let snap = block_on(replica.snapshot()).unwrap();
        assert_eq!(snap.len(), 1, "the factory's doc is the shared hub doc");
        assert!(factory.is_available());
        assert_eq!(factory.describe(), "virtual (sim)");
        // The author identity of a write through the factory-built replica
        // is the sync thread's DID — verified by what the other peer sees.
        block_on(replica.write_row("node", "n3", &row("n3"))).unwrap();
        f.net.step();
        let seen = f.beta_rx.try_recv().expect("n3 crosses to beta");
        assert_eq!(seen.record_id, "n3");
        assert_eq!(seen.author_id, "did:some-peer");
    }
}
