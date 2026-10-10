//! Shard model + per-verse fabric state (M2/F6 — A13/A14). See
//! `fe-sync/src/AGENTS.md` §sharding.
//!
//! Shard id = (petal, anchor node, time bucket): a reading maps to exactly
//! one shard by `(anchor, recorded_at_ms)` integer-divided by the verse's
//! bucket width (epoch-aligned). The **shard ledger** rides the verse's own
//! iroh-docs namespace under `__shards/{shard}` keys, peer capacity
//! declarations under `__peers/{did}` — both replicate like any row, so a
//! joining peer converges the whole placement picture from the doc snapshot
//! (`seed_reconciliation` replays it through the same fabric `note_*` seam
//! the live pump uses).
//!
//! The fabric is **sync-plane state, not SurrealDB state**: the DB thread
//! maps `__shards`/`__peers` rows to `NotApplicable` — they never touch the
//! local store. One writer per shard ledger key (the peer that first saw
//! the shard plans and publishes), conflicts resolved by the doc's
//! latest-per-entry semantics.

use std::collections::BTreeMap;

use fe_runtime::timeseries::{TimeseriesMode, VerseTimeseriesSettings};

use crate::placement::{
    plan_shard_hosts, retention_decision, transfer_route, PlacementPlan, Retention, TransferRoute,
};

/// Doc-entry table prefix for shard ledger rows (`__shards/{petal}/{anchor}/{bucket}`).
pub const SHARD_TABLE: &str = "__shards";
/// Doc-entry table prefix for peer capacity declarations (`__peers/{did}`).
pub const PEER_DECL_TABLE: &str = "__peers";

/// One shard's identity: (petal, anchor node, epoch-aligned time bucket).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardId {
    pub petal_id: String,
    pub anchor_node_id: String,
    pub bucket: i64,
}

impl ShardId {
    /// The doc-record key: `{petal}/{anchor}/{bucket}` (slashed, so the
    /// `{table}/{record_id}` split keeps the whole shard id in `record_id`).
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.petal_id, self.anchor_node_id, self.bucket)
    }

    /// Epoch-aligned bucket index for a reading's `recorded_at_ms`.
    /// `div_euclid` keeps pre-1970 timestamps floor-aligned.
    pub fn bucket_index(recorded_at_ms: i64, bucket_width_ms: u64) -> i64 {
        recorded_at_ms.div_euclid(bucket_width_ms as i64)
    }

    /// Inclusive-exclusive range covered by one bucket.
    pub fn bucket_range(bucket: i64, bucket_width_ms: u64) -> (i64, i64) {
        let start = bucket.saturating_mul(bucket_width_ms as i64);
        (start, start.saturating_add(bucket_width_ms as i64))
    }

    /// The shard a serialized `iot_reading` row belongs to (`None` when the
    /// row does not carry the fields the model needs).
    pub fn of_reading_row(row: &serde_json::Value, bucket_width_ms: u64) -> Option<Self> {
        let petal_id = row.get("petal_id")?.as_str()?.to_string();
        let anchor_node_id = row.get("node_id")?.as_str()?.to_string();
        let recorded_at_ms = row.get("recorded_at_ms")?.as_i64()?;
        Some(Self {
            petal_id,
            anchor_node_id,
            bucket: Self::bucket_index(recorded_at_ms, bucket_width_ms),
        })
    }
}

/// One peer's hosting declaration — the `__peers/{did}` doc row. `capacity_bytes:
/// None` means unlimited (the default; the local peer declares defaults until
/// the settings surface says otherwise).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerDeclaration {
    /// Declared shard-hosting capacity in bytes (a planning hint, not an
    /// enforcement wall — see `placement::plan_shard_hosts`).
    pub capacity_bytes: Option<u64>,
    /// Fallback / relay seeder: hosts shards the regular peers cannot fit
    /// (D2 #3/#4 — overflow target, never a coordinator).
    pub seeder: bool,
}

impl PeerDeclaration {
    /// Parse a `__peers` doc row payload.
    pub fn from_row(row: &serde_json::Value) -> Self {
        Self {
            capacity_bytes: row
                .get("capacity_bytes")
                .and_then(|v| v.as_u64())
                .filter(|v| *v > 0),
            seeder: row.get("seeder").and_then(|v| v.as_bool()).unwrap_or(false),
        }
    }
}

/// One shard ledger row — the `__shards/{key}` doc payload. The host set is
/// the placement plan at first sight; `row_count`/`size_bytes` are
/// best-effort estimates (written once at first sight, tracked locally
/// afterwards — re-publishing per reading would double doc writes for
/// metadata that is not authoritative).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ShardLedgerEntry {
    /// The shard key (`{petal}/{anchor}/{bucket}`).
    pub shard: String,
    /// Hosting peers' DIDs (the placement plan).
    pub hosts: Vec<String>,
    /// Mode recorded at placement (the ledger is honest history).
    pub mode: TimeseriesMode,
    /// R requested at placement.
    pub replication_factor: u32,
    /// Bucket width in force at placement (the bucket index depends on it).
    pub bucket_width_ms: u64,
    /// Inclusive bucket start (epoch ms).
    pub range_start_ms: i64,
    /// Exclusive bucket end (epoch ms).
    pub range_end_ms: i64,
    /// Best-effort row count estimate.
    pub row_count: u64,
    /// Best-effort size estimate (bytes).
    pub size_bytes: u64,
}

impl ShardLedgerEntry {
    fn from_row(row: &serde_json::Value) -> Option<Self> {
        Some(Self {
            shard: row.get("shard")?.as_str()?.to_string(),
            hosts: row
                .get("hosts")?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            mode: row
                .get("mode")
                .and_then(|v| v.as_str())
                .and_then(TimeseriesMode::parse)
                .unwrap_or(TimeseriesMode::Mirror),
            replication_factor: row
                .get("replication_factor")
                .and_then(|v| v.as_u64())
                .map(|v| v.max(1) as u32)
                .unwrap_or(1),
            bucket_width_ms: row
                .get("bucket_width_ms")
                .and_then(|v| v.as_u64())
                .filter(|v| *v > 0)
                .unwrap_or(fe_runtime::timeseries::DEFAULT_BUCKET_WIDTH_MS),
            range_start_ms: row
                .get("range_start_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            range_end_ms: row
                .get("range_end_ms")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            row_count: row.get("row_count").and_then(|v| v.as_u64()).unwrap_or(0),
            size_bytes: row.get("size_bytes").and_then(|v| v.as_u64()).unwrap_or(0),
        })
    }
}

/// Per-verse timeseries fabric state: settings (from the verse manifest),
/// peer declarations, and the shard ledger. Owned by the sync thread's
/// command loop — one entry per open verse, learned from BOTH directions of
/// verse-doc traffic (outbound writes parse the blob before publishing;
/// inbound rows and the reconciliation snapshot go through `note_entry`).
#[derive(Debug, Default)]
pub struct VerseFabric {
    pub settings: VerseTimeseriesSettings,
    /// Every declared peer by DID, the local one included.
    pub peers: BTreeMap<String, PeerDeclaration>,
    /// Shard ledger by shard key.
    pub shards: BTreeMap<String, ShardLedgerEntry>,
}

impl VerseFabric {
    /// Record the local peer's declaration (replica open / SetShardCapacity).
    pub fn note_local_declaration(&mut self, local_did: &str, decl: PeerDeclaration) {
        self.peers.insert(local_did.to_string(), decl);
    }

    /// Record a remote peer's `__peers` declaration row. Returns whether the
    /// declaration changed (callers may re-plan on membership churn later —
    /// F6 does not re-plan existing shards).
    pub fn note_peer_declaration(&mut self, peer_did: &str, decl: PeerDeclaration) -> bool {
        match self.peers.get(peer_did) {
            Some(old) if *old == decl => false,
            _ => {
                self.peers.insert(peer_did.to_string(), decl);
                true
            }
        }
    }

    /// Parse the `ts_*` settings off a verse manifest row. Returns whether
    /// the settings changed.
    pub fn note_verse_row(&mut self, row: &serde_json::Value) -> bool {
        let parsed = VerseTimeseriesSettings::from_verse_row(row);
        if parsed == self.settings {
            return false;
        }
        tracing::info!(
            mode = parsed.mode.as_str(),
            replication_factor = parsed.replication_factor,
            bucket_width_ms = parsed.bucket_width_ms,
            "Verse fabric: timeseries settings updated from the verse manifest"
        );
        self.settings = parsed;
        true
    }

    /// Record an inbound `__shards` ledger row.
    pub fn note_shard_row(&mut self, row: &serde_json::Value) {
        let Some(entry) = ShardLedgerEntry::from_row(row) else {
            tracing::debug!("malformed shard ledger row — ignored");
            return;
        };
        self.shards.insert(entry.shard.clone(), entry);
    }

    /// Bump the local estimates for a reading seen for a known shard.
    pub fn note_reading_seen(&mut self, shard_key: &str, size_bytes: u64) {
        if let Some(entry) = self.shards.get_mut(shard_key) {
            entry.row_count += 1;
            entry.size_bytes = entry.size_bytes.saturating_add(size_bytes);
        }
    }

    /// The current per-host assigned totals (the planner's load input).
    pub fn assigned_bytes(&self) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for entry in self.shards.values() {
            for host in &entry.hosts {
                *out.entry(host.clone()).or_insert(0) += entry.size_bytes;
            }
        }
        out
    }

    /// Ensure the shard exists in the ledger. When it is new, plan its host
    /// set (pure placement over the fabric's current view) and return the
    /// ledger row to publish to the doc; when it is known, just bump the
    /// local estimates and return `None` (the host set never re-plans in
    /// F6 — membership churn re-planning is deferred).
    pub fn ensure_shard(
        &mut self,
        shard: &ShardId,
        first_row_bytes: usize,
        local_did: &str,
    ) -> Option<Vec<u8>> {
        let key = shard.key();
        if let Some(entry) = self.shards.get_mut(&key) {
            entry.row_count += 1;
            entry.size_bytes = entry.size_bytes.saturating_add(first_row_bytes as u64);
            return None;
        }
        // A fabric with no declared peers at all (local declaration raced the
        // first ingest) plans for the local peer alone rather than leaving
        // the shard homeless.
        if self.peers.is_empty() {
            self.note_local_declaration(local_did, PeerDeclaration::default());
        }
        let (start, end) = ShardId::bucket_range(shard.bucket, self.settings.bucket_width_ms);
        let PlacementPlan { hosts, overflowed } = plan_shard_hosts(
            &key,
            first_row_bytes as u64,
            &self.peers,
            &self.assigned_bytes(),
            &self.settings,
        );
        if overflowed {
            tracing::warn!(
                shard = %key,
                hosts = ?hosts,
                "Shard placement exceeded every peer's declared capacity — placed on the least-utilized host (a shard must live somewhere)"
            );
        }
        let entry = ShardLedgerEntry {
            shard: key,
            hosts,
            mode: self.settings.mode,
            replication_factor: self.settings.replication_factor,
            bucket_width_ms: self.settings.bucket_width_ms,
            range_start_ms: start,
            range_end_ms: end,
            row_count: 1,
            size_bytes: first_row_bytes as u64,
        };
        tracing::debug!(
            shard = %entry.shard,
            hosts = ?entry.hosts,
            mode = entry.mode.as_str(),
            "Shard first seen — planned placement, publishing ledger row"
        );
        self.shards.insert(entry.shard.clone(), entry.clone());
        serde_json::to_vec(&entry).ok()
    }

    /// The host set for a shard, when its ledger row has converged locally.
    pub fn shard_hosts(&self, shard_key: &str) -> Option<&[String]> {
        self.shards.get(shard_key).map(|e| e.hosts.as_slice())
    }

    /// The `__peers/{local_did}` doc payload for the local peer's declaration
    /// (published on replica open and on every capacity change).
    pub fn local_peer_row(&self, local_did: &str) -> Option<Vec<u8>> {
        let decl = self.peers.get(local_did)?;
        serde_json::to_vec(decl).ok()
    }

    /// Route the publish of a row for this shard per mode (A13).
    pub fn transfer_route_for(&self, hosts: &[String]) -> TransferRoute {
        transfer_route(&self.settings, hosts)
    }

    /// Whether the local peer retains a reading for this shard (A13,
    /// receive side). Unknown shards retain — see `retention_decision`.
    pub fn retention_for(&self, shard_key: &str, local_did: &str) -> Retention {
        retention_decision(&self.settings, self.shard_hosts(shard_key), local_did)
    }

    /// Feed one inbound (or snapshot) row change into the fabric. Runs
    /// BEFORE the own-author filter: our own snapshot rows are exactly how
    /// the fabric re-learns settings/peers/ledger after a restart.
    ///
    /// **Admission control (F23, the sync-plane hardening pass):** a
    /// `__peers` declaration is SELF-declaration only — the row's key
    /// (`__peers/{did}`, i.e. `record_id`) must name its own author, whose
    /// identity comes from the doc entry (`author_id`), never from the
    /// row bytes. A row declaring ANOTHER peer's capacity (e.g. a member
    /// publishing `__peers/{honest_did}` with `capacity_bytes: 1` to
    /// starve that peer out of future placement) is dropped with a warn.
    /// The honest path is unaffected: every sanctioned publisher (replica
    /// open, `SetShardDeclaration`, the restart snapshot) writes its OWN
    /// key under its own author identity.
    pub fn note_entry(&mut self, change: &crate::replicator::RowChange) {
        match change.table.as_str() {
            "verse" => {
                if !change.data.is_empty() {
                    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&change.data) {
                        self.note_verse_row(&row);
                    }
                }
            }
            PEER_DECL_TABLE => {
                if change.record_id != change.author_id {
                    tracing::warn!(
                        record_id = %change.record_id,
                        author = %change.author_id,
                        "Forged peer declaration dropped — __peers rows must be self-declared \
                         (the key must name the author's own DID)"
                    );
                    return;
                }
                if !change.data.is_empty() {
                    if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&change.data) {
                        self.note_peer_declaration(
                            &change.record_id,
                            PeerDeclaration::from_row(&row),
                        );
                    }
                }
            }
            SHARD_TABLE if !change.data.is_empty() => {
                if let Ok(row) = serde_json::from_slice::<serde_json::Value>(&change.data) {
                    self.note_shard_row(&row);
                }
            }
            _ => {}
        }
    }

    /// Serialize the fabric for `SyncCommand::GetShardLedger` (diagnostics,
    /// harness assertions — the fabric is otherwise sync-thread-internal).
    pub fn to_dump_json(&self) -> serde_json::Value {
        serde_json::json!({
            "settings": self.settings,
            "peers": self.peers,
            "shards": self.shards,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::timeseries::{TimeseriesMode, DEFAULT_BUCKET_WIDTH_MS};

    fn reading_row(petal: &str, anchor: &str, recorded_at_ms: i64) -> serde_json::Value {
        serde_json::json!({
            "reading_id": "r1",
            "node_id": anchor,
            "petal_id": petal,
            "metric": "temperature_c",
            "value": 21.5,
            "recorded_at": "2026-07-15T10:00:00Z",
            "recorded_at_ms": recorded_at_ms,
        })
    }

    #[test]
    fn buckets_align_to_epoch_width_boundaries() {
        assert_eq!(ShardId::bucket_index(0, 1_000), 0);
        assert_eq!(ShardId::bucket_index(999, 1_000), 0);
        assert_eq!(ShardId::bucket_index(1_000, 1_000), 1);
        assert_eq!(
            ShardId::bucket_index(-1, 1_000),
            -1,
            "pre-epoch floors down"
        );
        let (start, end) = ShardId::bucket_range(1, 1_000);
        assert_eq!((start, end), (1_000, 2_000));
    }

    #[test]
    fn a_reading_maps_to_exactly_one_shard_by_anchor_and_bucket() {
        let row = reading_row("p1", "a1", 1_752_580_800_000);
        let shard = ShardId::of_reading_row(&row, DEFAULT_BUCKET_WIDTH_MS).expect("shard");
        assert_eq!(shard.key(), "p1/a1/20284");
        // Same day, same shard.
        let same_day = ShardId::of_reading_row(
            &reading_row("p1", "a1", 1_752_600_000_000),
            DEFAULT_BUCKET_WIDTH_MS,
        )
        .expect("shard");
        assert_eq!(same_day.key(), shard.key());
        // Different anchor or day, different shard.
        let other_anchor = ShardId::of_reading_row(
            &reading_row("p1", "a2", 1_752_580_800_000),
            DEFAULT_BUCKET_WIDTH_MS,
        )
        .expect("shard");
        assert_ne!(other_anchor.key(), shard.key());
        let other_day = ShardId::of_reading_row(
            &reading_row("p1", "a1", 1_752_667_200_000),
            DEFAULT_BUCKET_WIDTH_MS,
        )
        .expect("shard");
        assert_ne!(other_day.key(), shard.key());
    }

    #[test]
    fn missing_fields_do_not_shard() {
        assert!(ShardId::of_reading_row(&serde_json::json!({"reading_id": "r"}), 1_000).is_none());
    }

    #[test]
    fn peer_declaration_round_trips_through_the_doc_row() {
        let decl = PeerDeclaration {
            capacity_bytes: Some(5000),
            seeder: true,
        };
        let row = serde_json::to_value(decl).expect("serialize");
        assert_eq!(PeerDeclaration::from_row(&row), decl);
        // Zero/absent capacity reads back as unlimited.
        let none = serde_json::json!({"capacity_bytes": 0, "seeder": false});
        assert_eq!(PeerDeclaration::from_row(&none), PeerDeclaration::default());
    }

    #[test]
    fn verse_manifest_updates_fabric_settings() {
        let mut fabric = VerseFabric::default();
        assert_eq!(fabric.settings.mode, TimeseriesMode::Mirror);
        let changed = fabric.note_verse_row(&serde_json::json!({
            "ts_mode": "balanced", "ts_replication_factor": 2, "ts_bucket_width_ms": 3_600_000
        }));
        assert!(changed);
        assert_eq!(fabric.settings.mode, TimeseriesMode::Balanced);
        assert_eq!(fabric.settings.replication_factor, 2);
        // Same row again: no change reported.
        assert!(!fabric.note_verse_row(&serde_json::json!({
            "ts_mode": "balanced", "ts_replication_factor": 2, "ts_bucket_width_ms": 3_600_000
        })));
        // Pre-F6 row: back to mirror defaults.
        assert!(fabric.note_verse_row(&serde_json::json!({"verse_id": "v"})));
        assert_eq!(fabric.settings, VerseTimeseriesSettings::default());
    }

    #[test]
    fn ensure_shard_plans_publishes_once_then_estimates() {
        let mut fabric = VerseFabric::default();
        fabric.note_verse_row(&serde_json::json!({
            "ts_mode": "balanced", "ts_replication_factor": 2, "ts_bucket_width_ms": DEFAULT_BUCKET_WIDTH_MS
        }));
        fabric.note_local_declaration("local", PeerDeclaration::default());
        fabric.note_peer_declaration("peer-b", PeerDeclaration::default());

        let shard = ShardId {
            petal_id: "p1".into(),
            anchor_node_id: "a1".into(),
            bucket: 1,
        };
        let row = fabric
            .ensure_shard(&shard, 200, "local")
            .expect("ledger row");
        let entry: ShardLedgerEntry = serde_json::from_slice(&row).expect("ledger json");
        assert_eq!(entry.hosts.len(), 2, "R=2 over two peers");
        assert_eq!(entry.row_count, 1);
        assert_eq!(entry.size_bytes, 200);

        // Second sight of the same shard: no re-publish, estimates grow.
        assert!(fabric.ensure_shard(&shard, 100, "local").is_none());
        let entry = fabric.shards.get(&shard.key()).expect("entry");
        assert_eq!(entry.row_count, 2);
        assert_eq!(entry.size_bytes, 300);
    }

    #[test]
    fn inbound_shard_and_peer_rows_feed_the_fabric() {
        let mut fabric = VerseFabric::default();
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p1/a1/1", "hosts": ["local", "peer-b"],
            "mode": "sharded", "replication_factor": 1, "bucket_width_ms": 1000,
            "range_start_ms": 1000, "range_end_ms": 2000, "row_count": 3, "size_bytes": 900
        }));
        assert_eq!(
            fabric.shard_hosts("p1/a1/1"),
            Some(&["local".to_string(), "peer-b".to_string()][..])
        );

        fabric.note_peer_declaration(
            "peer-b",
            PeerDeclaration {
                capacity_bytes: Some(1),
                seeder: true,
            },
        );
        assert_eq!(fabric.peers["peer-b"].capacity_bytes, Some(1));

        // assigned_bytes folds over the ledger.
        let assigned = fabric.assigned_bytes();
        assert_eq!(assigned.get("local"), Some(&900));
        assert_eq!(assigned.get("peer-b"), Some(&900));
    }

    #[test]
    fn retention_follows_mode_and_host_set() {
        let mut fabric = VerseFabric::default();
        // Mirror default: unknown shard retains.
        assert_eq!(fabric.retention_for("p/a/1", "local"), Retention::Retain);
        fabric.note_verse_row(&serde_json::json!({"ts_mode": "sharded"}));
        fabric.note_shard_row(&serde_json::json!({
            "shard": "p/a/1", "hosts": ["peer-b"], "row_count": 1, "size_bytes": 1
        }));
        assert_eq!(fabric.retention_for("p/a/1", "local"), Retention::Skip);
        assert_eq!(fabric.retention_for("p/a/1", "peer-b"), Retention::Retain);
        // Unknown shard under sharded mode: retain (the safe default).
        assert_eq!(fabric.retention_for("p/a/2", "local"), Retention::Retain);
    }

    #[test]
    fn transfer_route_reflects_the_mode() {
        let mut fabric = VerseFabric::default();
        assert_eq!(
            fabric.transfer_route_for(&["a".to_string()]),
            TransferRoute::Broadcast
        );
        fabric.note_verse_row(&serde_json::json!({"ts_mode": "balanced"}));
        assert_eq!(
            fabric.transfer_route_for(&["a".to_string(), "b".to_string()]),
            TransferRoute::Targeted(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn note_entry_routed_by_table() {
        let mut fabric = VerseFabric::default();
        let verse_row = serde_json::to_vec(&serde_json::json!({
            "verse_id": "v", "ts_mode": "sharded", "ts_replication_factor": 1,
            "ts_bucket_width_ms": DEFAULT_BUCKET_WIDTH_MS
        }))
        .expect("json");
        fabric.note_entry(&crate::replicator::RowChange {
            table: "verse".into(),
            record_id: "v".into(),
            content_hash: [0; 32],
            author_id: "did:key:peer".into(),
            timestamp: 0,
            is_tombstone: false,
            data: verse_row,
        });
        assert_eq!(fabric.settings.mode, TimeseriesMode::Sharded);

        fabric.note_entry(&crate::replicator::RowChange {
            table: "iot_reading".into(),
            record_id: "r".into(),
            content_hash: [0; 32],
            author_id: "did:key:peer".into(),
            timestamp: 0,
            is_tombstone: false,
            data: vec![],
        });
        // Readings do not touch the ledger through note_entry (retention and
        // estimate bookkeeping happen on the apply path, where the shard is
        // known).
        assert!(fabric.shards.is_empty());
    }

    #[test]
    fn ledger_row_serde_round_trips() {
        let entry = ShardLedgerEntry {
            shard: "p/a/1".into(),
            hosts: vec!["did:key:z6MkA".into()],
            mode: TimeseriesMode::Balanced,
            replication_factor: 2,
            bucket_width_ms: 3_600_000,
            range_start_ms: 3_600_000,
            range_end_ms: 7_200_000,
            row_count: 7,
            size_bytes: 1_400,
        };
        let json = serde_json::to_vec(&entry).expect("serialize");
        let back: ShardLedgerEntry = serde_json::from_slice(&json).expect("deserialize");
        assert_eq!(back, entry);
    }

    // ── F23: ledger admission control (__peers is self-declaration only) ──

    fn peer_decl_change(record_id: &str, author_id: &str) -> crate::replicator::RowChange {
        crate::replicator::RowChange {
            table: PEER_DECL_TABLE.into(),
            record_id: record_id.into(),
            content_hash: [0; 32],
            author_id: author_id.into(),
            timestamp: 0,
            is_tombstone: false,
            data: br#"{"capacity_bytes":1,"seeder":false}"#.to_vec(),
        }
    }

    #[test]
    fn forged_peer_declaration_is_dropped() {
        // The M2 scrutiny F6 major shape: a member publishes
        // `__peers/{honest_did}` (starving the honest peer's placement)
        // under its OWN author identity — the record key does not name the
        // author, so the declaration is refused.
        let mut fabric = VerseFabric::default();
        fabric.note_entry(&peer_decl_change("did:key:honest", "did:key:attacker"));
        assert!(
            !fabric.peers.contains_key("did:key:honest"),
            "a forged foreign-DID declaration must never enter the fabric"
        );
        assert!(
            !fabric.peers.contains_key("did:key:attacker"),
            "the declaration must not be re-keyed to the attacker either"
        );
    }

    #[test]
    fn honest_self_declaration_converges() {
        let mut fabric = VerseFabric::default();
        fabric.note_entry(&peer_decl_change("did:key:bob", "did:key:bob"));
        let decl = fabric
            .peers
            .get("did:key:bob")
            .expect("self-declaration converges into the fabric");
        assert_eq!(decl.capacity_bytes, Some(1));
        assert!(!decl.seeder);
        // A later honest re-declaration still updates in place.
        fabric.note_entry(&crate::replicator::RowChange {
            table: PEER_DECL_TABLE.into(),
            record_id: "did:key:bob".into(),
            content_hash: [0; 32],
            author_id: "did:key:bob".into(),
            timestamp: 1,
            is_tombstone: false,
            data: br#"{"capacity_bytes":900,"seeder":true}"#.to_vec(),
        });
        let decl = fabric.peers.get("did:key:bob").expect("still declared");
        assert_eq!(decl.capacity_bytes, Some(900));
        assert!(decl.seeder);
    }
}
