//! IoT sensor-reading write path (append-only) — see `src/AGENTS.md` §iot-readings.

use fe_query::{Filter, InsertBuilder, QueryBuilder, QueryValue};

use crate::op_log::next_hlc_timestamp;
use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::{BlobStoreHandle, ReplicationSender};

/// One incoming sensor reading, pre-validation. Canonical definition lives in
/// `fe_runtime::messages` (M2/F7 — `DbCommand::InsertIotReadings` carries it
/// cross-thread); re-exported so every existing import path is unchanged.
pub use fe_runtime::messages::IotReadingInput;

/// Typed ingestion failures so the API layer can map real HTTP statuses.
#[derive(Debug, thiserror::Error)]
pub enum IotIngestError {
    #[error("unknown anchor node '{0}' in this petal")]
    UnknownAnchor(String),
    #[error("invalid recorded_at '{0}' (expected RFC-3339)")]
    InvalidTimestamp(String),
    #[error("metric name must be non-empty")]
    EmptyMetric,
    #[error("iot_reading write failed: {0}")]
    Db(String),
}

impl IotIngestError {
    /// The typed 422-class detail when this is a validation failure, `None`
    /// for DB failures (F24) — the DB-thread `InsertIotReadings` arm packs
    /// the rejection into `DbResult::IotReadingsRejected` so the API
    /// fallback never string-sniffs `DbResult::Error` for status mapping.
    pub fn validation_rejection(&self) -> Option<fe_runtime::messages::IotIngestRejection> {
        match self {
            Self::UnknownAnchor(node_id) => {
                Some(fe_runtime::messages::IotIngestRejection::UnknownAnchor {
                    node_id: node_id.clone(),
                })
            }
            Self::InvalidTimestamp(raw) => {
                Some(fe_runtime::messages::IotIngestRejection::InvalidTimestamp {
                    raw: raw.clone(),
                })
            }
            Self::EmptyMetric => Some(fe_runtime::messages::IotIngestRejection::EmptyMetric),
            Self::Db(_) => None,
        }
    }
}

/// Insert a batch of readings anchored to nodes of `petal_id` (all-or-nothing:
/// every anchor is validated against the petal before any row is written).
///
/// Delegating shim: replicates nothing. Callers on a replication seam use
/// [`insert_readings_with_replication`] (F5/A11).
pub async fn insert_readings(
    db: &Db,
    petal_id: &str,
    source_did: &str,
    readings: &[IotReadingInput],
) -> Result<usize, IotIngestError> {
    insert_readings_with_replication(db, petal_id, None, source_did, readings, None, None).await
}

/// Insert a batch of readings and publish each accepted row to the verse's
/// replication bridge (A11) — see `src/AGENTS.md` §iot-readings.
///
/// The emission seam is here, not in fe-api, so **both** ingestion paths speak
/// it: the API thread's `db_reader` write (fe-api `iot.rs` — the §iot-readings
/// append-only exception) and any DB-thread caller pass their own blob handle +
/// `ReplicationSender`.
///
/// Ordering: each row is durably inserted *before* its `ReplicationEvent` is
/// emitted. Replication is best-effort (the bridge drops-and-counts on a full
/// channel), so publishing first would risk losing a row that the local store
/// never accepted. A row whose blob write or channel send fails is simply not
/// replicated; it remains durable locally, and the union CRDT makes a later
/// re-delivery harmless.
///
/// `verse_id` names the replica the row belongs in; `None` disables emission
/// (the shim path, or a caller with no verse context).
pub async fn insert_readings_with_replication(
    db: &Db,
    petal_id: &str,
    verse_id: Option<&str>,
    source_did: &str,
    readings: &[IotReadingInput],
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
) -> Result<usize, IotIngestError> {
    if readings.is_empty() {
        return Ok(0);
    }

    // Validate shape before touching the DB.
    let mut anchor_ids: Vec<String> = Vec::new();
    for r in readings {
        if r.metric.trim().is_empty() {
            return Err(IotIngestError::EmptyMetric);
        }
        if !anchor_ids.contains(&r.node_id) {
            anchor_ids.push(r.node_id.clone());
        }
    }

    // Every anchor must be an existing node of this petal (scope integrity).
    let known = fetch_known_anchors(db, petal_id, &anchor_ids).await?;
    for id in &anchor_ids {
        if !known.contains(id) {
            return Err(IotIngestError::UnknownAnchor(id.clone()));
        }
    }

    // Warn once (not per row) when an emit seam was wired but the batch cannot
    // actually be published — a silently non-replicating ingestion path is the
    // failure mode this seam exists to prevent. `repl_tx: None` is a legitimate
    // "replication disabled" configuration and stays silent.
    let can_publish = verse_id.is_some() && blob_store.is_some();
    if repl_tx.is_some() && !can_publish {
        tracing::warn!(
            petal_id,
            verse_id = ?verse_id,
            "iot ingest: replication sender wired but verse_id or blob store missing — readings will not be published"
        );
    }

    let mut written = 0usize;
    // Parse every timestamp up-front so a bad row aborts before any insert
    // (the all-or-nothing validation contract in §iot-readings). HLC + id are
    // also assigned up-front, exactly as the pre-F5 loop did.
    let mut prepared: Vec<(String, serde_json::Value)> = Vec::with_capacity(readings.len());
    for r in readings {
        let recorded = match &r.recorded_at {
            Some(raw) => chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| IotIngestError::InvalidTimestamp(raw.clone()))?
                .with_timezone(&chrono::Utc),
            None => chrono::Utc::now(),
        };
        let (hlc_packed, _hlc_str) = next_hlc_timestamp();
        let reading_id = ulid::Ulid::new().to_string();
        let row = serde_json::json!({
            "reading_id": reading_id.clone(),
            "node_id": r.node_id,
            "petal_id": petal_id,
            "metric": r.metric,
            "value": r.value,
            "units": r.units,
            "recorded_at": recorded.to_rfc3339(),
            "recorded_at_ms": recorded.timestamp_millis(),
            "hlc_timestamp": hlc_packed as i64,
            "source_did": source_did,
        });
        prepared.push((reading_id, row));
    }

    for (reading_id, row) in prepared {
        // No geometry columns on the row, so InsertBuilder is legal here
        // (AGENTS.md §geometry-inserts applies only to geometry-typed columns).
        let q = InsertBuilder::insert_into("iot_reading")
            .values(row.clone())
            .build();
        exec_query(db, &q)
            .await
            .map_err(|e| IotIngestError::Db(e.to_string()))?;
        written += 1;

        // Durable first, then publish (see the ordering note above).
        if let (Some(verse_id), Some(store)) = (verse_id, blob_store) {
            match serde_json::to_vec(&row) {
                Ok(bytes) => crate::replicate_row_with_petal(
                    repl_tx,
                    store,
                    verse_id,
                    "iot_reading",
                    &reading_id,
                    &bytes,
                    Some(petal_id.to_string()),
                ),
                Err(e) => tracing::warn!(
                    reading_id,
                    petal_id,
                    "iot reading accepted but not serialised for replication: {e}"
                ),
            }
        }
    }

    Ok(written)
}

/// Return the subset of `anchor_ids` that exist as nodes of `petal_id`.
async fn fetch_known_anchors(
    db: &Db,
    petal_id: &str,
    anchor_ids: &[String],
) -> Result<Vec<String>, IotIngestError> {
    let ids = QueryValue::Array(
        anchor_ids
            .iter()
            .map(|s| QueryValue::from(s.clone()))
            .collect(),
    );
    let q = QueryBuilder::new()
        .select(&["node_id"])
        .from("node")
        .filter(Filter::eq("petal_id", petal_id))
        .and(Filter::is_in("node_id", ids))
        .build();
    let mut res = exec_query(db, &q)
        .await
        .map_err(|e| IotIngestError::Db(e.to_string()))?;
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    Ok(rows
        .iter()
        .filter_map(|r| r["node_id"].as_str().map(str::to_string))
        .collect())
}
