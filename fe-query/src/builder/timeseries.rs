//! Time-series builders for `iot_reading` (FR-3) — see `src/AGENTS.md` §timeseries.

use super::filter::Filter;
use super::render::{BuiltQuery, SortDir};
use super::value::QueryValue;
use super::QueryBuilder;

/// The IoT sensor-reading table (schema in fe-database `schema.rs`).
pub const READINGS_TABLE: &str = "iot_reading";

/// Correlated `$parent` predicate selecting the newest row per (anchor, metric).
const LATEST_PREDICATE: &str = "recorded_at_ms = math::max((SELECT VALUE recorded_at_ms \
     FROM iot_reading WHERE node_id = $parent.node_id AND metric = $parent.metric))";

/// Spatial anchor filter: readings whose anchor node lies within `radius_m`
/// of local-meters point (x, z) — geometry lives on the node, not the reading.
pub fn anchors_within(x: f64, z: f64, radius_m: f64) -> Filter {
    let sub = QueryBuilder::new()
        .select_value("node_id")
        .from("node")
        .filter(Filter::d_within(
            "position",
            QueryValue::Point { x, z },
            radius_m,
        ))
        .build();
    Filter::in_subquery("node_id", sub)
}

/// Latest reading per (anchor, metric), optionally scoped to a petal/metric.
pub fn latest_per_anchor(petal_id: Option<&str>, metric: Option<&str>) -> BuiltQuery {
    let mut q = QueryBuilder::new().select(&["*"]).from(READINGS_TABLE);
    if let Some(p) = petal_id {
        q = q.filter(Filter::eq("petal_id", p));
    }
    if let Some(m) = metric {
        q = q.filter(Filter::eq("metric", m));
    }
    q.filter(Filter::raw(LATEST_PREDICATE)).build()
}

/// avg/min/max/count of `metric` per anchor over the half-open window
/// `[start_ms, end_ms)` on `recorded_at_ms`.
pub fn window_aggregate(
    metric: &str,
    start_ms: i64,
    end_ms: i64,
    petal_id: Option<&str>,
) -> BuiltQuery {
    let mut q = QueryBuilder::new()
        .select(&[
            "node_id",
            "metric",
            "math::mean(value) AS avg_value",
            "math::min(value) AS min_value",
            "math::max(value) AS max_value",
            "count() AS sample_count",
        ])
        .from(READINGS_TABLE)
        .filter(Filter::eq("metric", metric))
        .and(Filter::gte("recorded_at_ms", start_ms))
        .and(Filter::lt("recorded_at_ms", end_ms));
    if let Some(p) = petal_id {
        q = q.and(Filter::eq("petal_id", p));
    }
    q.group_by(&["node_id", "metric"]).build()
}

/// The **partial** form of [`window_aggregate`] for the distributed fabric
/// (M2/F7): per-anchor `sum`/`count`/`min`/`max` over the window — the
/// commutative mean monoid's `sum`+`count` pair, so per-host partials merge
/// exactly (count adds, min/max fold; mean is derived at the merge as
/// `sum/count`). `anchor` restricts the partial to one shard's anchor; the
/// same shape serves both the per-shard partials (window aggregates) and
/// single-host fallbacks. Rendered SQL is checked against the DB-thread
/// `RawQuery` keyword filter in `fe-database`.
pub fn window_aggregate_partial(
    metric: &str,
    start_ms: i64,
    end_ms: i64,
    petal_id: &str,
    anchor: Option<&str>,
) -> BuiltQuery {
    let mut q = QueryBuilder::new()
        .select(&[
            "node_id",
            "metric",
            "math::sum(value) AS sum_value",
            "math::min(value) AS min_value",
            "math::max(value) AS max_value",
            "count() AS sample_count",
        ])
        .from(READINGS_TABLE)
        .filter(Filter::eq("petal_id", petal_id))
        .and(Filter::eq("metric", metric))
        .and(Filter::gte("recorded_at_ms", start_ms))
        .and(Filter::lt("recorded_at_ms", end_ms));
    if let Some(a) = anchor {
        q = q.and(Filter::eq("node_id", a));
    }
    q.group_by(&["node_id", "metric"]).build()
}

/// Raw reading rows for `metric` in `[start_ms, end_ms)`, oldest first, with
/// an optional extra filter (e.g. [`anchors_within`]) — the combined
/// spatial+temporal query shape.
pub fn readings_in_window(
    metric: &str,
    start_ms: i64,
    end_ms: i64,
    petal_id: Option<&str>,
    extra: Option<Filter>,
) -> BuiltQuery {
    let mut q = QueryBuilder::new()
        .select(&["*"])
        .from(READINGS_TABLE)
        .filter(Filter::eq("metric", metric))
        .and(Filter::gte("recorded_at_ms", start_ms))
        .and(Filter::lt("recorded_at_ms", end_ms));
    if let Some(p) = petal_id {
        q = q.and(Filter::eq("petal_id", p));
    }
    if let Some(f) = extra {
        q = q.and(f);
    }
    q.order_by("recorded_at_ms", SortDir::Asc).build()
}

/// The whole readings view of a petal — every metric, all time, oldest
/// first (M2/F7 — the `AllReadings` distributed partial; the analytics
/// endpoint's registered `iot_reading` table). The executing host's row cap
/// bounds the scan; the distributed merge dedupes by `reading_id`.
pub fn readings_for_petal(petal_id: &str) -> BuiltQuery {
    QueryBuilder::new()
        .select(&["*"])
        .from(READINGS_TABLE)
        .filter(Filter::eq("petal_id", petal_id))
        .order_by("recorded_at_ms", SortDir::Asc)
        .build()
}

// ── Tests (rendered SQL, per test policy) ───────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_per_anchor_renders_correlated_max() {
        let q = latest_per_anchor(Some("p1"), Some("temperature_c"));
        assert_eq!(
            q.sql,
            "SELECT * FROM iot_reading WHERE petal_id = $p0 AND metric = $p1 AND \
             recorded_at_ms = math::max((SELECT VALUE recorded_at_ms FROM iot_reading \
             WHERE node_id = $parent.node_id AND metric = $parent.metric))"
        );
        assert_eq!(q.params.len(), 2);
        assert_eq!(q.params[0].1, serde_json::json!("p1"));
        assert_eq!(q.params[1].1, serde_json::json!("temperature_c"));
    }

    #[test]
    fn latest_per_anchor_unscoped_has_no_params() {
        let q = latest_per_anchor(None, None);
        assert!(q
            .sql
            .starts_with("SELECT * FROM iot_reading WHERE recorded_at_ms = math::max("));
        assert!(q.params.is_empty());
    }

    #[test]
    fn window_aggregate_renders_aggs_and_half_open_window() {
        let q = window_aggregate("temperature_c", 1_000, 2_000, Some("p1"));
        assert_eq!(
            q.sql,
            "SELECT node_id, metric, math::mean(value) AS avg_value, \
             math::min(value) AS min_value, math::max(value) AS max_value, \
             count() AS sample_count FROM iot_reading \
             WHERE metric = $p0 AND recorded_at_ms >= $p1 AND recorded_at_ms < $p2 \
             AND petal_id = $p3 GROUP BY node_id, metric"
        );
        assert_eq!(q.params[0].1, serde_json::json!("temperature_c"));
        assert_eq!(q.params[1].1, serde_json::json!(1_000));
        assert_eq!(q.params[2].1, serde_json::json!(2_000));
        assert_eq!(q.params[3].1, serde_json::json!("p1"));
    }

    #[test]
    fn window_aggregate_without_petal_omits_petal_filter() {
        let q = window_aggregate("humidity_pct", 0, 10, None);
        assert!(!q.sql.contains("petal_id"));
        assert_eq!(q.params.len(), 3);
    }

    #[test]
    fn window_aggregate_partial_renders_monoid_aggs() {
        let q = window_aggregate_partial("temperature_c", 1_000, 2_000, "p1", Some("node-a"));
        assert_eq!(
            q.sql,
            "SELECT node_id, metric, math::sum(value) AS sum_value, \
             math::min(value) AS min_value, math::max(value) AS max_value, \
             count() AS sample_count FROM iot_reading \
             WHERE petal_id = $p0 AND metric = $p1 AND recorded_at_ms >= $p2 \
             AND recorded_at_ms < $p3 AND node_id = $p4 GROUP BY node_id, metric"
        );
        assert_eq!(q.params[0].1, serde_json::json!("p1"));
        assert_eq!(q.params[1].1, serde_json::json!("temperature_c"));
        assert_eq!(q.params[2].1, serde_json::json!(1_000));
        assert_eq!(q.params[3].1, serde_json::json!(2_000));
        assert_eq!(q.params[4].1, serde_json::json!("node-a"));
    }

    #[test]
    fn window_aggregate_partial_without_anchor_keeps_shard_free_shape() {
        let q = window_aggregate_partial("co2_ppm", 0, 60_000, "p2", None);
        assert!(!q.sql.contains("node_id = $"));
        assert!(q.sql.contains("math::sum(value) AS sum_value"));
        assert!(q.sql.contains("count() AS sample_count"));
        assert_eq!(q.params.len(), 4);
    }

    #[test]
    fn window_aggregate_partial_passes_rawquery_keyword_filter() {
        // The DB-thread ExecuteTsPartial arm rejects the same bare-word
        // keywords RawQuery does; the partial builder must never render one.
        let blocked = [
            "CREATE", "UPDATE", "DELETE", "DEFINE", "REMOVE", "RELATE", "INSERT", "LET", "RETURN",
            "INFO", "FOR", "THROW", "SLEEP", "BREAK", "LIVE", "KILL", "IF", "BEGIN", "COMMIT",
            "CANCEL",
        ];
        for (metric, anchor) in [("temperature_c", Some("node-a")), ("m2", None)] {
            let q = window_aggregate_partial(metric, 0, 1, "p1", anchor);
            let sql_upper = q.sql.to_uppercase();
            for kw in blocked {
                assert!(
                    !sql_upper.contains(kw),
                    "partial SQL must not contain blocked keyword {kw}: {}",
                    q.sql
                );
            }
        }
    }

    #[test]
    fn readings_for_petal_renders_whole_view_oldest_first() {
        let q = readings_for_petal("p1");
        assert_eq!(
            q.sql,
            "SELECT * FROM iot_reading WHERE petal_id = $p0 ORDER BY recorded_at_ms ASC"
        );
        assert_eq!(q.params.len(), 1);
        assert_eq!(q.params[0].1, serde_json::json!("p1"));
        // The AllReadings partial rides the same DB-thread keyword screen
        // as every other partial shape.
        let sql_upper = q.sql.to_uppercase();
        for kw in ["CREATE", "UPDATE", "DELETE", "INSERT", "LET", "INFO"] {
            assert!(
                !sql_upper.contains(kw),
                "partial SQL must not contain blocked keyword {kw}: {}",
                q.sql
            );
        }
    }

    #[test]
    fn anchors_within_renders_select_value_subquery() {
        let q = QueryBuilder::new()
            .select(&["*"])
            .from(READINGS_TABLE)
            .filter(anchors_within(10.0, 20.0, 500.0))
            .build();
        assert_eq!(
            q.sql,
            "SELECT * FROM iot_reading WHERE node_id IN \
             (SELECT VALUE node_id FROM node WHERE geo::distance(position, $p0) <= $p1)"
        );
        assert_eq!(q.params[0].1["type"], "Point");
        assert_eq!(q.params[1].1, serde_json::json!(500.0));
    }

    #[test]
    fn readings_in_window_combines_spatial_and_temporal() {
        let q = readings_in_window(
            "temperature_c",
            1_000,
            2_000,
            Some("p1"),
            Some(anchors_within(1.5, 2.5, 100.0)),
        );
        assert_eq!(
            q.sql,
            "SELECT * FROM iot_reading \
             WHERE metric = $p0 AND recorded_at_ms >= $p1 AND recorded_at_ms < $p2 \
             AND petal_id = $p3 AND node_id IN \
             (SELECT VALUE node_id FROM node WHERE geo::distance(position, $p4) <= $p5) \
             ORDER BY recorded_at_ms ASC"
        );
        // Subquery params remapped after the outer filters.
        assert_eq!(q.params.len(), 6);
        assert_eq!(q.params[3].1, serde_json::json!("p1"));
        assert_eq!(q.params[4].1["coordinates"], serde_json::json!([1.5, 2.5]));
        assert_eq!(q.params[5].1, serde_json::json!(100.0));
    }

    #[test]
    fn readings_in_window_temporal_only() {
        let q = readings_in_window("co2_ppm", 5, 6, None, None);
        assert_eq!(
            q.sql,
            "SELECT * FROM iot_reading WHERE metric = $p0 AND recorded_at_ms >= $p1 \
             AND recorded_at_ms < $p2 ORDER BY recorded_at_ms ASC"
        );
        assert_eq!(q.params.len(), 3);
    }
}
