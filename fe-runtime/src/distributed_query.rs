//! Distributed timeseries query vocabulary (M2/F7 — A15/A16/A17).
//!
//! The lowest-common-denominator types shared by every layer of the
//! distributed query fabric: fe-api (the request seam + response surfaces),
//! fe-sync (the planner/transport/merge), and fe-database (local partial
//! execution on the DB thread). They live here — like `timeseries.rs` — so
//! one definition serves all crates with no cycles and no drift.
//!
//! The **wire spec is structured, never SQL**: peers render their own partial
//! SQL from [`TsQueryKind`] via the fe-query builders with bound parameters,
//! so no requester can make a peer execute arbitrary statements. See
//! `fe-sync/src/AGENTS.md` §distributed-query for the transport contract.

use serde::{Deserialize, Serialize};

/// Default per-host partial row cap (A15 transport bounding: a partial
/// response must stay small enough for one gossip message; a truncation is
/// reported honestly through `TsPartialRows::truncated`).
pub const PARTIAL_ROW_CAP: usize = 2048;

/// Upper bound on a caller-supplied partial row cap — the executing host
/// never trusts an unbounded request from the wire.
pub const MAX_PARTIAL_ROW_CAP: usize = 8192;

/// Lower bound (defensive floor so a malformed cap can never request an
/// unbounded partial).
pub const MIN_PARTIAL_ROW_CAP: usize = 256;

/// Clamp a caller-supplied cap into the sanctioned range.
pub fn clamp_partial_row_cap(cap: usize) -> usize {
    cap.clamp(MIN_PARTIAL_ROW_CAP, MAX_PARTIAL_ROW_CAP)
}

/// A distributed timeseries query shape (the fe-query builders' three
/// distributed surfaces). `petal_id` is mandatory on every variant: the
/// fabric shards by `(petal, anchor, bucket)` and the API scope guard
/// authorizes a concrete petal, so a distributed query is petal-scoped by
/// construction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TsQueryKind {
    /// avg/min/max/count of `metric` per anchor over `[start_ms, end_ms)`.
    WindowAggregate {
        metric: String,
        start_ms: i64,
        end_ms: i64,
        petal_id: String,
    },
    /// Latest reading per (anchor, metric), optionally metric-scoped.
    LatestPerAnchor {
        petal_id: String,
        metric: Option<String>,
    },
    /// Raw reading rows for `metric` in `[start_ms, end_ms)`, oldest first.
    ReadingsInWindow {
        metric: String,
        start_ms: i64,
        end_ms: i64,
        petal_id: String,
    },
    /// The whole merged readings view of a petal, every metric, all time
    /// (M2/F7 — the analytics endpoint's registered `iot_reading` table).
    /// Partial rows are capped per host like any other shape; the merge is
    /// the same union-dedupe-by-`reading_id` the raw window uses.
    AllReadings { petal_id: String },
}

impl TsQueryKind {
    /// The petal every shard of this query lives under.
    pub fn petal_id(&self) -> &str {
        match self {
            Self::WindowAggregate { petal_id, .. }
            | Self::LatestPerAnchor { petal_id, .. }
            | Self::ReadingsInWindow { petal_id, .. }
            | Self::AllReadings { petal_id } => petal_id,
        }
    }

    /// The window `[start_ms, end_ms)` a query is bounded to, or `None` for
    /// unwindowed shapes (latest-per-anchor and all-readings span all of
    /// time; the planner targets every shard of the petal).
    pub fn window(&self) -> Option<(i64, i64)> {
        match self {
            Self::WindowAggregate {
                start_ms, end_ms, ..
            }
            | Self::ReadingsInWindow {
                start_ms, end_ms, ..
            } => Some((*start_ms, *end_ms)),
            Self::LatestPerAnchor { .. } | Self::AllReadings { .. } => None,
        }
    }
}

/// One shard constraint for a local partial: the executing store runs its
/// partial SQL restricted to this shard's anchor and time range. Derived
/// from the shard key (`{petal}/{anchor}/{bucket}`) by the requester and
/// carried explicitly so a responder with a divergent bucket width cannot
/// misread the key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialShard {
    /// The ledger shard key (`{petal}/{anchor}/{bucket}`).
    pub shard: String,
    /// The shard's anchor node id (the `node_id` filter).
    pub anchor_node_id: String,
    /// Inclusive range start (epoch ms).
    pub range_start_ms: i64,
    /// Exclusive range end (epoch ms).
    pub range_end_ms: i64,
}

/// One shard's partial rows inside a [`TsPartialRows`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShardRows {
    pub shard: String,
    pub rows: Vec<serde_json::Value>,
}

/// The result of a local partial execution ([`crate::messages::DbCommand::ExecuteTsPartial`]).
///
/// Aggregate partials carry per-shard rows (`math::sum`/`count`/`min`/`max`
/// per anchor — the mean monoid's `sum`+`count` pair); raw and
/// latest-per-anchor partials carry whole-window rows in `rows` (their
/// merges are dedupe/max — no per-shard attribution needed).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TsPartialRows {
    /// Per-shard aggregate partials (window aggregates only).
    pub per_shard: Vec<ShardRows>,
    /// Whole-window rows (raw readings / latest-per-anchor).
    pub rows: Vec<serde_json::Value>,
    /// The partial hit its row cap and is incomplete (honesty flag).
    pub truncated: bool,
    /// The executing host FAILED to render this partial (DB error) — the
    /// rows are empty because they could not be read, not because they do
    /// not exist (F23: a failed partial must never masquerade as an
    /// authoritative covered-empty). The merge excludes failed partials
    /// from rows, coverage, and `answered_hosts`, so a formal host that
    /// failed lands in `missing_hosts` and its shards in `missing_shards`.
    /// Serde-defaulted so a pre-F23 peer's envelope (no field) parses as
    /// a normal partial.
    #[serde(default)]
    pub failed: bool,
}

impl TsPartialRows {
    /// An empty partial (a responder that hosts nothing requested).
    pub fn empty() -> Self {
        Self {
            per_shard: Vec::new(),
            rows: Vec::new(),
            truncated: false,
            failed: false,
        }
    }

    /// A FAILED partial (the executing host's DB query errored): empty rows
    /// because they could not be read, flagged so the merge never treats it
    /// as an authoritative covered-empty answer.
    pub fn failed() -> Self {
        Self {
            failed: true,
            ..Self::empty()
        }
    }

    /// Whether the partial carries any data at all.
    pub fn is_empty(&self) -> bool {
        self.per_shard.iter().all(|s| s.rows.is_empty()) && self.rows.is_empty()
    }
}

/// The honesty metadata every distributed result carries (A16): which shards
/// contributed, which were invisible (all their hosts offline), which hosts
/// never answered, and the fabric settings in force.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistributedQueryMeta {
    /// Shard keys whose data contributed to the merged result.
    pub covered_shards: Vec<String>,
    /// Shard keys whose hosts did not answer — their data is invisible in
    /// this result (at R=1 an offline host makes the shard invisible; at
    /// R≥2 a surviving mirror covers it, and it is NOT missing).
    pub missing_shards: Vec<String>,
    /// Peer DIDs that answered (including the local peer).
    pub answered_hosts: Vec<String>,
    /// Peer DIDs expected to answer but silent until the deadline (or whose
    /// partial arrived with `failed: true` — data-silent, F23: a host that
    /// could not execute contributes nothing and is reported as missing, so
    /// the metadata never reads "covered" where the truth is "host failed").
    pub missing_hosts: Vec<String>,
    /// Mode recorded at placement time for the targeted shards.
    pub mode: String,
    /// Replication factor recorded at placement time.
    pub replication_factor: u32,
    /// Some host's partial hit its row cap (the merge is incomplete).
    pub truncated: bool,
}

/// The merged outcome of one distributed query, replied to the requester
/// (API surface, harness peer, or future sim host).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistributedQueryOutcome {
    /// The merged rows (same shape the single-host builders produce).
    pub rows: Vec<serde_json::Value>,
    /// Honesty metadata (A16) — always present, even on failure paths where
    /// the row set is empty.
    pub meta: DistributedQueryMeta,
    /// `None` on success; a human-readable reason when the query could not
    /// run at all (no fabric, transport offline, window too wide, queue
    /// full). The API surfaces map this to their error responses.
    pub error: Option<String>,
}

/// One distributed query request (the structured spec + transport knobs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistributedQueryRequest {
    /// Correlation id (ULID) — matches responses to this request.
    pub request_id: String,
    /// The verse whose fabric + gossip topic the query fans out over.
    pub verse_id: String,
    /// The structured query spec (never raw SQL).
    pub spec: TsQueryKind,
    /// Per-host answer deadline in ms (bounded by the transport's cap).
    pub timeout_ms: u64,
    /// Row cap per participating host's partial.
    pub row_cap: usize,
}

/// A request plus its reply seam — the payload of the API→sync channel and
/// of [`fe_sync::messages::SyncCommand::SubmitComputeTask`]. The binary (or
/// harness) owns the channel pair and bridges the API side into the sync
/// thread, exactly like the replication bridge. The reply is a crossbeam
/// sender (Clone + Debug) so the command enum keeps its derives.
#[derive(Debug, Clone)]
pub struct DistributedQueryCall {
    pub request: DistributedQueryRequest,
    pub reply: crossbeam::channel::Sender<DistributedQueryOutcome>,
}

/// Channel halves for the seam (type aliases, mirroring `replication.rs`).
pub type DistributedQueryCallSender = crossbeam::channel::Sender<DistributedQueryCall>;
pub type DistributedQueryCallReceiver = crossbeam::channel::Receiver<DistributedQueryCall>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_query_kind_serde_roundtrip() {
        let specs = vec![
            TsQueryKind::WindowAggregate {
                metric: "temperature_c".into(),
                start_ms: 0,
                end_ms: 60_000,
                petal_id: "p1".into(),
            },
            TsQueryKind::LatestPerAnchor {
                petal_id: "p1".into(),
                metric: Some("humidity_pct".into()),
            },
            TsQueryKind::LatestPerAnchor {
                petal_id: "p1".into(),
                metric: None,
            },
            TsQueryKind::ReadingsInWindow {
                metric: "co2_ppm".into(),
                start_ms: -1,
                end_ms: 1,
                petal_id: "p1".into(),
            },
            TsQueryKind::AllReadings {
                petal_id: "p1".into(),
            },
        ];
        for spec in &specs {
            let json = serde_json::to_string(spec).unwrap();
            let back: TsQueryKind = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, spec, "roundtrip failed for {json}");
        }
    }

    #[test]
    fn ts_query_kind_petal_and_window_accessors() {
        let agg = TsQueryKind::WindowAggregate {
            metric: "m".into(),
            start_ms: 5,
            end_ms: 9,
            petal_id: "p9".into(),
        };
        assert_eq!(agg.petal_id(), "p9");
        assert_eq!(agg.window(), Some((5, 9)));

        let latest = TsQueryKind::LatestPerAnchor {
            petal_id: "p9".into(),
            metric: None,
        };
        assert_eq!(latest.petal_id(), "p9");
        assert_eq!(latest.window(), None);

        let all = TsQueryKind::AllReadings {
            petal_id: "p9".into(),
        };
        assert_eq!(all.petal_id(), "p9");
        assert_eq!(all.window(), None);
    }

    #[test]
    fn partial_rows_serde_roundtrip() {
        let partial = TsPartialRows {
            per_shard: vec![ShardRows {
                shard: "p/a/1".into(),
                rows: vec![serde_json::json!({"node_id": "a", "sum_value": 4.0})],
            }],
            rows: vec![serde_json::json!({"reading_id": "r1"})],
            truncated: true,
            failed: false,
        };
        let json = serde_json::to_string(&partial).unwrap();
        let back: TsPartialRows = serde_json::from_str(&json).unwrap();
        assert_eq!(back, partial);
        assert!(back.truncated);
        assert!(!back.is_empty());
        assert!(TsPartialRows::empty().is_empty());
        assert!(!TsPartialRows::empty().failed);
        // Wire compat: a pre-F23 peer's envelope carries no `failed` field —
        // serde-default parses it as a normal (non-failed) partial.
        let legacy = serde_json::from_str::<TsPartialRows>(
            r#"{"per_shard":[],"rows":[],"truncated":false}"#,
        )
        .unwrap();
        assert!(!legacy.failed && !legacy.truncated && legacy.is_empty());
        // The failed constructor is empty-but-flagged.
        let failed = TsPartialRows::failed();
        assert!(failed.failed && failed.is_empty());
        let failed_json = serde_json::to_value(&failed).unwrap();
        assert_eq!(failed_json["failed"], true);
    }

    #[test]
    fn outcome_meta_shape_is_stable() {
        let outcome = DistributedQueryOutcome {
            rows: vec![serde_json::json!({"node_id": "a"})],
            meta: DistributedQueryMeta {
                covered_shards: vec!["p/a/1".into()],
                missing_shards: vec![],
                answered_hosts: vec!["did:local".into()],
                missing_hosts: vec!["did:offline".into()],
                mode: "sharded".into(),
                replication_factor: 1,
                truncated: false,
            },
            error: None,
        };
        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["meta"]["missing_hosts"][0], "did:offline");
        assert_eq!(json["meta"]["replication_factor"], 1);
    }
}
