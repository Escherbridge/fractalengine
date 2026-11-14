//! Viewer-authorized bounded object measurements; see AGENTS.md.

use fe_runtime::node_history::NodeAccessGrant;
use fe_runtime::node_measurements::{
    MeasurementPoint, MeasurementSeries, NodeMeasurements, POINT_LIMIT, SERIES_LIMIT,
};

use crate::repo::Db;

pub(crate) async fn read(
    db: &Db,
    node_id: &str,
    petal_id: &str,
    requested_series: Option<MeasurementSeries>,
    requested_limit: usize,
    grant: &NodeAccessGrant,
    local_did: &str,
) -> anyhow::Result<NodeMeasurements> {
    let scope = super::crud::resolve_node_scope_handler(db, node_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("not found or not permitted"))?;
    let parts = crate::parse_scope(&scope)?;
    if parts.petal_id.as_deref() != Some(petal_id) {
        anyhow::bail!("not found or not permitted");
    }
    super::node_history::authorize_scope(
        db,
        grant,
        local_did,
        &scope,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let ancestry = "node_id = $node AND petal_id = $petal AND node_id IN (SELECT VALUE node_id FROM node WHERE node_id = $node AND petal_id = $petal AND tombstone = NONE) AND petal_id IN (SELECT VALUE petal_id FROM petal WHERE petal_id = $petal AND fractal_id = $fractal AND fractal_id IN (SELECT VALUE fractal_id FROM fractal WHERE fractal_id = $fractal AND verse_id = $verse))";
    let fractal = parts
        .fractal_id
        .ok_or_else(|| anyhow::anyhow!("not found or not permitted"))?;
    let mut response = db.query(format!("SELECT metric, units FROM iot_reading WHERE {ancestry} GROUP BY metric, units ORDER BY metric, units LIMIT {}", SERIES_LIMIT + 1))
        .bind(("node", node_id.to_owned())).bind(("petal", petal_id.to_owned()))
        .bind(("fractal", fractal.clone())).bind(("verse", parts.verse_id.clone())).await?.check()?;
    let rows: Vec<serde_json::Value> = response.take(0)?;
    let mut available_series: Vec<MeasurementSeries> =
        serde_json::from_value(serde_json::Value::Array(rows))?;
    let series_truncated = available_series.len() > SERIES_LIMIT;
    available_series.truncate(SERIES_LIMIT);
    if available_series
        .iter()
        .any(|series| series.metric.len() > 256 || series.units.len() > 128)
    {
        anyhow::bail!("stored measurement series name exceeds display limit");
    }
    let series = requested_series.or_else(|| available_series.first().cloned());
    let limit = requested_limit.clamp(1, POINT_LIMIT);
    let mut points = Vec::new();
    if let Some(series) = &series {
        if series.metric.len() > 256 || series.units.len() > 128 {
            anyhow::bail!("measurement series name exceeds display limit");
        }
        let mut response = db.query(format!("SELECT reading_id, recorded_at, recorded_at_ms, value FROM iot_reading WHERE {ancestry} AND metric = $metric AND units = $units ORDER BY recorded_at_ms DESC, reading_id DESC LIMIT $cap"))
            .bind(("node", node_id.to_owned())).bind(("petal", petal_id.to_owned()))
            .bind(("fractal", fractal)).bind(("verse", parts.verse_id))
            .bind(("metric", series.metric.clone())).bind(("units", series.units.clone()))
            .bind(("cap", limit + 1)).await?.check()?;
        let rows: Vec<serde_json::Value> = response.take(0)?;
        points = serde_json::from_value::<Vec<MeasurementPoint>>(serde_json::Value::Array(rows))?;
        if points.iter().any(|point| !point.value.is_finite()) {
            anyhow::bail!("stored measurement contains a nonfinite value");
        }
    }
    let current_scope = super::crud::resolve_node_scope_handler(db, node_id).await?;
    if current_scope.as_deref() != Some(scope.as_str()) {
        anyhow::bail!("object ancestry changed; refresh measurements");
    }
    super::node_history::authorize_scope(
        db,
        grant,
        local_did,
        &scope,
        fe_policy::RoleLevel::Viewer,
    )
    .await?;
    let truncated = points.len() > limit;
    points.truncate(limit);
    points.reverse();
    Ok(NodeMeasurements {
        node_id: node_id.to_owned(),
        petal_id: petal_id.to_owned(),
        series,
        available_series,
        series_truncated,
        points,
        truncated,
        limit,
    })
}
