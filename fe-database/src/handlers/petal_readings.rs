//! Viewer-authorized latest-per-(node, metric, units) readings for one petal; see AGENTS.md §handlers petal_readings.

use std::collections::BTreeMap;

use fe_runtime::node_history::NodeAccessGrant;
use fe_runtime::petal_readings::{LatestReading, PetalReadingsSummary, ROW_LIMIT};

use crate::repo::Db;

/// Source rows read per summary before folding.
const SOURCE_ROW_LIMIT: usize = 5000;
/// Longest node id or metric name shown; longer stored rows are skipped (matches node_measurements).
const NAME_LIMIT: usize = 256;
/// Longest units label shown.
const UNITS_LIMIT: usize = 128;

pub(crate) async fn read(
    db: &Db,
    petal_id: &str,
    since_ms: i64,
    grant: &NodeAccessGrant,
    local_did: &str,
) -> anyhow::Result<PetalReadingsSummary> {
    let scope = super::crud::resolve_petal_scope_handler(db, petal_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not found or not permitted"))?;
    authorize(db, grant, local_did, &scope).await?;
    let mut response = db
        .query(format!(
            "SELECT node_id, metric, units, value, recorded_at_ms FROM iot_reading \
             WHERE petal_id = $petal AND recorded_at_ms >= $since \
             AND node_id IN (SELECT VALUE node_id FROM node WHERE petal_id = $petal AND tombstone = NONE) \
             ORDER BY recorded_at_ms DESC, reading_id DESC LIMIT {SOURCE_ROW_LIMIT}"
        ))
        .bind(("petal", petal_id.to_owned()))
        .bind(("since", since_ms))
        .await?
        .check()?;
    let source: Vec<serde_json::Value> = response.take(0)?;
    let source_capped = source.len() >= SOURCE_ROW_LIMIT;
    // Rows arrive newest first, so the first sight of a key is its latest reading.
    // Keyed by units too, so a series in "ft" never counts toward or overwrites the "m" one.
    let mut latest: BTreeMap<(String, String, String), LatestReading> = BTreeMap::new();
    for row in &source {
        let (Some(node_id), Some(metric), Some(units), Some(value), Some(recorded_at_ms)) = (
            row["node_id"].as_str(),
            row["metric"].as_str(),
            row["units"].as_str(),
            row["value"].as_f64(),
            row["recorded_at_ms"].as_i64(),
        ) else {
            continue;
        };
        if !value.is_finite()
            || node_id.len() > NAME_LIMIT
            || metric.len() > NAME_LIMIT
            || units.len() > UNITS_LIMIT
        {
            continue;
        }
        latest
            .entry((node_id.to_owned(), metric.to_owned(), units.to_owned()))
            .and_modify(|entry| entry.count_window = entry.count_window.saturating_add(1))
            .or_insert_with(|| LatestReading {
                node_id: node_id.to_owned(),
                metric: metric.to_owned(),
                units: units.to_owned(),
                value,
                recorded_at_ms,
                count_window: 1,
            });
    }
    let row_capped = latest.len() > ROW_LIMIT;
    let rows: Vec<LatestReading> = latest.into_values().take(ROW_LIMIT).collect();
    // Re-check after the read so a role revoked mid-query cannot leak rows.
    authorize(db, grant, local_did, &scope).await?;
    Ok(PetalReadingsSummary {
        petal_id: petal_id.to_owned(),
        as_of_ms: chrono::Utc::now().timestamp_millis(),
        rows,
        truncated: source_capped || row_capped,
    })
}

async fn authorize(
    db: &Db,
    grant: &NodeAccessGrant,
    local_did: &str,
    scope: &str,
) -> anyhow::Result<()> {
    super::node_history::authorize_scope(db, grant, local_did, scope, fe_policy::RoleLevel::Viewer)
        .await
}
