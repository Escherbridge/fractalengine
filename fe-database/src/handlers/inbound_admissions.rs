//! The DB thread's in-memory deferred-admission queue for inbound CONTENT rows (role
//! propagation plan §2.4): a `Deferred` row waits here and is retried by a worklist loop when
//! the fact it awaits lands. Role statements never enter it — they are held durably in
//! `role_statement_pending`. See src/AGENTS.md §inbound-admissions.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use fe_runtime::messages::{DeferralReason, ReplicatedRowOutcome};

use crate::handlers::replicated_row::{
    apply_replicated_row_reporting, ApplyReport, SceneChangeSender,
};
use crate::repo::Db;
use crate::role_grants::ROLE_DOC_TABLE;

/// Deferred content rows held per verse before the oldest is dropped (A4).
pub const DEFERRED_PER_VERSE: usize = 4096;
/// Deferred content rows held per (verse, author) before that author's oldest is dropped (A4).
pub const DEFERRED_PER_AUTHOR: usize = 512;
/// Bug guard on one call's retry worklist (it terminates anyway — see §inbound-admissions).
const MAX_RETRIES_PER_CALL: usize = 4 * DEFERRED_PER_VERSE;

static DEFERRED_ADMISSION_DROPS: AtomicU64 = AtomicU64::new(0);

/// Deferred content rows dropped by a queue cap, process-wide (the snapshot replay on the
/// next replica open re-feeds them).
pub fn deferred_admission_drop_count() -> u64 {
    DEFERRED_ADMISSION_DROPS.load(Ordering::Relaxed)
}

/// How one inbound row (the delivered one, or a queued row a retry settled) was reconciled —
/// what the dispatch loop echoes as `DbResult::ReplicatedRowApplied`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowReport {
    pub verse_id: String,
    pub table: String,
    pub record_id: String,
    pub outcome: ReplicatedRowOutcome,
}

/// A queued row's identity: the latest bytes per (verse, table, record id, author).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct RowKey {
    verse_id: String,
    table: String,
    record_id: String,
    author_did: String,
}

struct HeldRow {
    key: RowKey,
    bytes: Vec<u8>,
    reason: DeferralReason,
}

/// A retry trigger: every row of `verse` waiting on `reason` is re-applied.
type Trigger = (String, DeferralReason);

/// The deferred-admission queue (§2.4 "Content rows"). Insertion order (`seq`) is age, so
/// drop-oldest is the first entry of the per-verse or per-author index.
pub struct InboundAdmissions {
    per_verse_cap: usize,
    per_author_cap: usize,
    next_seq: u64,
    rows: BTreeMap<u64, HeldRow>,
    seq_of: HashMap<RowKey, u64>,
    by_wait: HashMap<Trigger, BTreeSet<u64>>,
    by_verse: HashMap<String, BTreeSet<u64>>,
    by_author: HashMap<(String, String), BTreeSet<u64>>,
}

impl Default for InboundAdmissions {
    fn default() -> Self {
        Self::new()
    }
}

impl InboundAdmissions {
    /// A queue with the production caps ([`DEFERRED_PER_VERSE`], [`DEFERRED_PER_AUTHOR`]).
    pub fn new() -> Self {
        Self::with_caps(DEFERRED_PER_VERSE, DEFERRED_PER_AUTHOR)
    }

    /// A queue with explicit caps (each clamped to at least 1).
    pub fn with_caps(per_verse_cap: usize, per_author_cap: usize) -> Self {
        Self {
            per_verse_cap: per_verse_cap.max(1),
            per_author_cap: per_author_cap.max(1),
            next_seq: 0,
            rows: BTreeMap::new(),
            seq_of: HashMap::new(),
            by_wait: HashMap::new(),
            by_verse: HashMap::new(),
            by_author: HashMap::new(),
        }
    }

    /// Queued rows across every verse.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether nothing is queued.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Queued rows of one verse.
    pub fn len_for_verse(&self, verse_id: &str) -> usize {
        self.by_verse.get(verse_id).map_or(0, BTreeSet::len)
    }

    /// What a queued row is waiting for, or `None` if it is not queued.
    pub fn waiting_reason(
        &self,
        verse_id: &str,
        table: &str,
        record_id: &str,
        author_did: &str,
    ) -> Option<&DeferralReason> {
        let key = RowKey {
            verse_id: verse_id.to_string(),
            table: table.to_string(),
            record_id: record_id.to_string(),
            author_did: author_did.to_string(),
        };
        self.seq_of
            .get(&key)
            .and_then(|seq| self.rows.get(seq))
            .map(|row| &row.reason)
    }

    /// THE inbound entry point both the production DB loop and the test harness call: apply
    /// one replicated row through the real handler, queue it if it is a deferred content
    /// row, then retry every queued row the apply unblocked. The first report is the
    /// delivered row's; the rest are queued rows a retry settled (still-deferred retries are
    /// not reported again). Never awaits a channel the DB loop drains (self-drain rule).
    #[allow(clippy::too_many_arguments)]
    pub async fn apply(
        &mut self,
        db: &Db,
        verse_id: &str,
        table: &str,
        record_id: &str,
        row_bytes: &[u8],
        author_did: &str,
        entity_change_tx: Option<&SceneChangeSender>,
    ) -> Vec<RowReport> {
        let key = RowKey {
            verse_id: verse_id.to_string(),
            table: table.to_string(),
            record_id: record_id.to_string(),
            author_did: author_did.to_string(),
        };
        let report = run(db, &key, row_bytes, entity_change_tx).await;
        let mut reports = vec![row_report(&key, &report.outcome)];
        let mut triggers = VecDeque::new();
        self.settle(key, row_bytes.to_vec(), None, &report, &mut triggers);
        self.drain(db, triggers, entity_change_tx, &mut reports)
            .await;
        reports
    }

    /// Retry every row of `verse_id` waiting on an author's role — called after a LOCAL role
    /// change (AssignRole / RevokeRole), which raises roles outside the inbound path.
    pub async fn retry_author_roles(
        &mut self,
        db: &Db,
        verse_id: &str,
        entity_change_tx: Option<&SceneChangeSender>,
    ) -> Vec<RowReport> {
        let triggers: VecDeque<Trigger> = self
            .by_wait
            .keys()
            .filter(|(verse, reason)| {
                verse == verse_id && matches!(reason, DeferralReason::AuthorRole { .. })
            })
            .cloned()
            .collect();
        let mut reports = Vec::new();
        self.drain(db, triggers, entity_change_tx, &mut reports)
            .await;
        reports
    }

    /// Worklist (not recursion): pop a trigger, re-apply every row waiting on it, settle each
    /// (which may fire further triggers), until no trigger is left.
    async fn drain(
        &mut self,
        db: &Db,
        mut triggers: VecDeque<Trigger>,
        entity_change_tx: Option<&SceneChangeSender>,
        reports: &mut Vec<RowReport>,
    ) {
        let mut retries = 0usize;
        while let Some(trigger) = triggers.pop_front() {
            let Some(waiting) = self.by_wait.get(&trigger) else {
                continue;
            };
            let waiting: Vec<u64> = waiting.iter().copied().collect();
            for seq in waiting {
                if retries >= MAX_RETRIES_PER_CALL {
                    tracing::warn!(
                        retries,
                        queued = self.rows.len(),
                        "deferred-admission retry bound hit — remaining rows stay queued (bug guard)"
                    );
                    return;
                }
                let Some(row) = self.remove(seq) else {
                    continue;
                };
                retries += 1;
                let report = run(db, &row.key, &row.bytes, entity_change_tx).await;
                if !matches!(report.outcome, ReplicatedRowOutcome::Deferred(_)) {
                    reports.push(row_report(&row.key, &report.outcome));
                }
                self.settle(row.key, row.bytes, Some(row.reason), &report, &mut triggers);
            }
        }
    }

    /// Queue or forget the row per its outcome, and enqueue the triggers the apply fired.
    fn settle(
        &mut self,
        key: RowKey,
        bytes: Vec<u8>,
        previous_reason: Option<DeferralReason>,
        report: &ApplyReport,
        triggers: &mut VecDeque<Trigger>,
    ) {
        let verse_id = key.verse_id.clone();
        let is_role_statement = key.table == ROLE_DOC_TABLE;
        let applied = matches!(
            report.outcome,
            ReplicatedRowOutcome::Applied | ReplicatedRowOutcome::AppliedTombstone
        );
        let parent_trigger = (applied && !is_role_statement).then(|| {
            (
                verse_id.clone(),
                DeferralReason::Parent {
                    table: key.table.clone(),
                    id: key.record_id.clone(),
                },
            )
        });
        match &report.outcome {
            // Role statements are held durably by the DB, never in this queue (§2.4).
            ReplicatedRowOutcome::Deferred(_) if is_role_statement => {}
            ReplicatedRowOutcome::Deferred(reason) => self.hold(key, bytes, reason.clone()),
            // A transient failure keeps a retried row queued under its old reason.
            ReplicatedRowOutcome::Failed => {
                if let Some(reason) = previous_reason {
                    self.hold(key, bytes, reason);
                }
            }
            // Applied, denied, not applicable: settled for good — drop any queued copy.
            _ => self.forget(&key),
        }
        if report.manifest_landed {
            triggers.push_back((verse_id.clone(), DeferralReason::Manifest));
        }
        for did in &report.risen_dids {
            triggers.push_back((
                verse_id.clone(),
                DeferralReason::AuthorRole { did: did.clone() },
            ));
        }
        if let Some(trigger) = parent_trigger {
            triggers.push_back(trigger);
        }
    }

    /// Queue `bytes` as the key's latest version, then enforce both caps (drop-oldest).
    fn hold(&mut self, key: RowKey, bytes: Vec<u8>, reason: DeferralReason) {
        self.forget(&key);
        let seq = self.next_seq;
        self.next_seq += 1;
        let verse_id = key.verse_id.clone();
        let author = (key.verse_id.clone(), key.author_did.clone());
        self.by_wait
            .entry((verse_id.clone(), reason.clone()))
            .or_default()
            .insert(seq);
        self.by_verse
            .entry(verse_id.clone())
            .or_default()
            .insert(seq);
        self.by_author
            .entry(author.clone())
            .or_default()
            .insert(seq);
        self.seq_of.insert(key.clone(), seq);
        self.rows.insert(seq, HeldRow { key, bytes, reason });

        while self.by_author.get(&author).map_or(0, BTreeSet::len) > self.per_author_cap {
            let oldest = self.by_author.get(&author).and_then(|s| s.first().copied());
            self.drop_oldest(oldest, "per-author");
        }
        while self.by_verse.get(&verse_id).map_or(0, BTreeSet::len) > self.per_verse_cap {
            let oldest = self
                .by_verse
                .get(&verse_id)
                .and_then(|s| s.first().copied());
            self.drop_oldest(oldest, "per-verse");
        }
    }

    fn drop_oldest(&mut self, seq: Option<u64>, cap: &'static str) {
        let Some(row) = seq.and_then(|seq| self.remove(seq)) else {
            return;
        };
        let total = DEFERRED_ADMISSION_DROPS.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(
            verse_id = %row.key.verse_id,
            table = %row.key.table,
            record_id = %row.key.record_id,
            author = %row.key.author_did,
            cap,
            dropped_total = total,
            "deferred-admission queue full — oldest deferred row dropped (the next replica open replays it)"
        );
    }

    fn forget(&mut self, key: &RowKey) {
        if let Some(seq) = self.seq_of.get(key).copied() {
            self.remove(seq);
        }
    }

    /// Remove one queued row from every index.
    fn remove(&mut self, seq: u64) -> Option<HeldRow> {
        let row = self.rows.remove(&seq)?;
        self.seq_of.remove(&row.key);
        let wait = (row.key.verse_id.clone(), row.reason.clone());
        remove_from(&mut self.by_wait, &wait, seq);
        remove_from(&mut self.by_verse, &row.key.verse_id, seq);
        let author = (row.key.verse_id.clone(), row.key.author_did.clone());
        remove_from(&mut self.by_author, &author, seq);
        Some(row)
    }
}

fn remove_from<K, Q>(index: &mut HashMap<K, BTreeSet<u64>>, key: &Q, seq: u64)
where
    K: std::borrow::Borrow<Q> + std::hash::Hash + Eq,
    Q: std::hash::Hash + Eq + ?Sized,
{
    if let Some(set) = index.get_mut(key) {
        set.remove(&seq);
        if set.is_empty() {
            index.remove(key);
        }
    }
}

fn row_report(key: &RowKey, outcome: &ReplicatedRowOutcome) -> RowReport {
    RowReport {
        verse_id: key.verse_id.clone(),
        table: key.table.clone(),
        record_id: key.record_id.clone(),
        outcome: outcome.clone(),
    }
}

/// One apply through the production handler; an error is logged and reported `Failed`.
async fn run(
    db: &Db,
    key: &RowKey,
    bytes: &[u8],
    entity_change_tx: Option<&SceneChangeSender>,
) -> ApplyReport {
    match apply_replicated_row_reporting(
        db,
        &key.verse_id,
        &key.table,
        &key.record_id,
        bytes,
        &key.author_did,
        entity_change_tx,
    )
    .await
    {
        Ok(report) => report,
        Err(e) => {
            tracing::warn!(
                verse_id = %key.verse_id,
                table = %key.table,
                record_id = %key.record_id,
                "Applying replicated row failed: {e:#}"
            );
            ApplyReport::of(ReplicatedRowOutcome::Failed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn schema_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema apply");
        db.query(
            "CREATE verse CONTENT { verse_id: 'v-cap', name: 'V', created_by: 'did:key:owner', \
             created_at: '2026-10-15T00:00:00Z', default_access: 'viewer' }",
        )
        .await
        .expect("seed verse")
        .check()
        .expect("seed verse check");
        db
    }

    fn node_row(node_id: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "node_id": node_id,
            "petal_id": "petal-cap",
            "display_name": "N",
            "position": { "type": "Point", "coordinates": [0.0, 0.0] },
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
            "created_at": "2026-10-15T00:00:00Z",
        }))
        .expect("node json")
    }

    /// §2.4 caps (content rows): one author can hold at most its per-author cap, the verse at
    /// most its per-verse cap; overflow drops the OLDEST and counts every drop.
    #[tokio::test]
    async fn the_deferred_queue_caps_per_author_and_counts_drops() {
        let db = schema_db().await;
        let mut queue = InboundAdmissions::with_caps(6, 4);
        let before = deferred_admission_drop_count();

        // One viewer-level author sends 5 rows: the per-author cap (4) drops the first.
        for i in 0..5 {
            let id = format!("node-a{i}");
            let reports = queue
                .apply(&db, "v-cap", "node", &id, &node_row(&id), "did:key:a", None)
                .await;
            assert!(matches!(
                reports[0].outcome,
                ReplicatedRowOutcome::Deferred(DeferralReason::AuthorRole { .. })
            ));
        }
        assert_eq!(queue.len(), 4);
        assert!(
            queue
                .waiting_reason("v-cap", "node", "node-a0", "did:key:a")
                .is_none(),
            "the oldest row of the over-cap author was dropped"
        );
        assert!(queue
            .waiting_reason("v-cap", "node", "node-a4", "did:key:a")
            .is_some());

        // A second author pushes the verse past its cap (6): the verse's oldest go first.
        for i in 0..3 {
            let id = format!("node-b{i}");
            queue
                .apply(&db, "v-cap", "node", &id, &node_row(&id), "did:key:b", None)
                .await;
        }
        assert_eq!(queue.len_for_verse("v-cap"), 6);
        assert!(queue
            .waiting_reason("v-cap", "node", "node-a1", "did:key:a")
            .is_none());
        assert!(queue
            .waiting_reason("v-cap", "node", "node-b2", "did:key:b")
            .is_some());
        assert!(
            deferred_admission_drop_count() - before >= 2,
            "every dropped row is counted"
        );

        // Re-delivery of a queued key replaces it (latest bytes), it does not add a row.
        queue
            .apply(
                &db,
                "v-cap",
                "node",
                "node-b2",
                &node_row("node-b2"),
                "did:key:b",
                None,
            )
            .await;
        assert_eq!(queue.len_for_verse("v-cap"), 6);
    }
}
