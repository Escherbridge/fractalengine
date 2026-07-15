//! IoT sensor-reading write path (append-only) — see `src/AGENTS.md` §iot-readings.

use fe_query::{Filter, InsertBuilder, QueryBuilder, QueryValue};

use crate::op_log::next_hlc_timestamp;
use crate::query_helpers::exec_query;
use crate::repo::Db;

/// One incoming sensor reading, pre-validation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IotReadingInput {
    /// Anchor node the reading is spatially attached to.
    pub node_id: String,
    /// Metric name, e.g. `temperature_c`.
    pub metric: String,
    pub value: f64,
    #[serde(default)]
    pub units: String,
    /// RFC-3339 sensor timestamp; server ingest time when absent.
    #[serde(default)]
    pub recorded_at: Option<String>,
}

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

/// Insert a batch of readings anchored to nodes of `petal_id` (all-or-nothing:
/// every anchor is validated against the petal before any row is written).
pub async fn insert_readings(
    db: &Db,
    petal_id: &str,
    source_did: &str,
    readings: &[IotReadingInput],
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

    // Parse all timestamps up-front so a bad row aborts before any insert.
    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(readings.len());
    for r in readings {
        let recorded = match &r.recorded_at {
            Some(raw) => chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| IotIngestError::InvalidTimestamp(raw.clone()))?
                .with_timezone(&chrono::Utc),
            None => chrono::Utc::now(),
        };
        let (hlc_packed, _hlc_str) = next_hlc_timestamp();
        rows.push(serde_json::json!({
            "reading_id": ulid::Ulid::new().to_string(),
            "node_id": r.node_id,
            "petal_id": petal_id,
            "metric": r.metric,
            "value": r.value,
            "units": r.units,
            "recorded_at": recorded.to_rfc3339(),
            "recorded_at_ms": recorded.timestamp_millis(),
            "hlc_timestamp": hlc_packed as i64,
            "source_did": source_did,
        }));
    }

    // No geometry columns on the row, so InsertBuilder is legal here
    // (AGENTS.md §geometry-inserts applies only to geometry-typed columns).
    for row in rows {
        let q = InsertBuilder::insert_into("iot_reading")
            .values(row)
            .build();
        exec_query(db, &q)
            .await
            .map_err(|e| IotIngestError::Db(e.to_string()))?;
    }

    Ok(readings.len())
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
