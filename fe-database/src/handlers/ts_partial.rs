//! Distributed-query local partial execution (M2/F7 — A15).
//!
//! `execute_ts_partial` runs THIS host's slice of a distributed timeseries
//! query on the DB thread (the single-writer connection — the same class of
//! read `RawQuery` performs). The input is the structured
//! `fe_runtime::distributed_query` spec: SQL is rendered here via the
//! fe-query builders with bound parameters, never accepted from the wire, so
//! a peer request can only ever run one of the three sanctioned shapes
//! restricted to a petal/window/shard.
//!
//! Partial semantics (the merge contract, see fe-sync `distributed_query.rs`):
//! - `WindowAggregate` runs ONE query per requested shard (anchor + range),
//!   clamping each shard's range to the query window — per-shard attribution
//!   is what lets the merge prevent double-counting when multiple mirrors of
//!   a shard answer, while the sum/count/min/max monoid still adds up
//!   correctly across shards hosted by different peers.
//! - `ReadingsInWindow` / `LatestPerAnchor` run ONE whole-window query: their
//!   merges are union-dedupe-by-`reading_id` and max-by-timestamp, both exact
//!   under duplicate answers, so no shard attribution is needed.

use fe_query::builder::timeseries::{
    latest_per_anchor, readings_for_petal, readings_in_window, window_aggregate_partial,
};
use fe_runtime::distributed_query::{
    clamp_partial_row_cap, PartialShard, TsPartialRows, TsQueryKind,
};

use crate::query_helpers::exec_query;
use crate::repo::Db;

/// Run the local partial for `spec` (see the module docs for the semantics).
///
/// `shards` is only consulted by `WindowAggregate` (per-shard attribution);
/// raw and latest partials ignore it and scan the local window. `row_cap`
/// is clamped into the sanctioned range — an executing host never trusts an
/// unbounded request.
pub async fn execute_ts_partial(
    db: &Db,
    spec: &TsQueryKind,
    shards: &[PartialShard],
    row_cap: usize,
) -> Result<TsPartialRows, String> {
    let row_cap = clamp_partial_row_cap(row_cap);
    match spec {
        TsQueryKind::WindowAggregate {
            metric,
            start_ms,
            end_ms,
            petal_id,
        } => {
            if metric.is_empty() {
                return Err("window aggregate requires a metric".to_string());
            }
            if start_ms >= end_ms {
                return Err("window aggregate requires start_ms < end_ms".to_string());
            }
            let mut per_shard = Vec::with_capacity(shards.len());
            let mut truncated = false;
            for shard in shards {
                // Clamp the shard's bucket range to the query window so a
                // partially-overlapping bucket cannot contribute rows outside
                // the window (adjacent windows tile without double-counting).
                let range_start = shard.range_start_ms.max(*start_ms);
                let range_end = shard.range_end_ms.min(*end_ms);
                if range_start >= range_end {
                    per_shard.push(fe_runtime::distributed_query::ShardRows {
                        shard: shard.shard.clone(),
                        rows: Vec::new(),
                    });
                    continue;
                }
                let q = window_aggregate_partial(
                    metric,
                    range_start,
                    range_end,
                    petal_id,
                    Some(&shard.anchor_node_id),
                );
                let mut res = exec_query(db, &q).await.map_err(|e| e.to_string())?;
                let mut rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
                if rows.len() > row_cap {
                    rows.truncate(row_cap);
                    truncated = true;
                }
                per_shard.push(fe_runtime::distributed_query::ShardRows {
                    shard: shard.shard.clone(),
                    rows,
                });
            }
            Ok(TsPartialRows {
                per_shard,
                rows: Vec::new(),
                truncated,
                failed: false,
            })
        }
        TsQueryKind::ReadingsInWindow {
            metric,
            start_ms,
            end_ms,
            petal_id,
        } => {
            if metric.is_empty() {
                return Err("readings-in-window requires a metric".to_string());
            }
            if start_ms >= end_ms {
                return Err("readings-in-window requires start_ms < end_ms".to_string());
            }
            let q = readings_in_window(metric, *start_ms, *end_ms, Some(petal_id), None);
            let mut res = exec_query(db, &q).await.map_err(|e| e.to_string())?;
            let mut rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
            let truncated = rows.len() > row_cap;
            rows.truncate(row_cap);
            Ok(TsPartialRows {
                per_shard: Vec::new(),
                rows,
                truncated,
                failed: false,
            })
        }
        TsQueryKind::LatestPerAnchor { petal_id, metric } => {
            let q = latest_per_anchor(Some(petal_id), metric.as_deref());
            let mut res = exec_query(db, &q).await.map_err(|e| e.to_string())?;
            let mut rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
            let truncated = rows.len() > row_cap;
            rows.truncate(row_cap);
            Ok(TsPartialRows {
                per_shard: Vec::new(),
                rows,
                truncated,
                failed: false,
            })
        }
        TsQueryKind::AllReadings { petal_id } => {
            let q = readings_for_petal(petal_id);
            let mut res = exec_query(db, &q).await.map_err(|e| e.to_string())?;
            let mut rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
            let truncated = rows.len() > row_cap;
            rows.truncate(row_cap);
            Ok(TsPartialRows {
                per_shard: Vec::new(),
                rows,
                truncated,
                failed: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{Db, Repo};
    use crate::schema::IotReading;
    use fe_runtime::distributed_query::PARTIAL_ROW_CAP;

    async fn schema_db() -> Db {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("mem db");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        crate::schema::apply_all(&db).await.expect("schema");
        db
    }

    fn reading(id: &str, node: &str, value: f64, ms: i64) -> IotReading {
        IotReading {
            reading_id: id.into(),
            node_id: node.into(),
            petal_id: "p1".into(),
            metric: "temperature_c".into(),
            value,
            units: "C".into(),
            recorded_at: "2026-07-15T10:00:00Z".into(),
            recorded_at_ms: ms,
            hlc_timestamp: ms,
            source_did: "did:key:z6MkSeed".into(),
        }
    }

    async fn seed(db: &Db, readings: &[IotReading]) {
        for r in readings {
            Repo::<IotReading>::create(db, r)
                .await
                .expect("seed reading");
        }
    }

    fn shard(key: &str, anchor: &str, start: i64, end: i64) -> PartialShard {
        PartialShard {
            shard: key.into(),
            anchor_node_id: anchor.into(),
            range_start_ms: start,
            range_end_ms: end,
        }
    }

    #[tokio::test]
    async fn window_aggregate_partial_sums_per_shard_and_clamps_to_window() {
        let db = schema_db().await;
        // node-a: two rows in bucket [0,60), one row in bucket [60,120).
        // node-b: one row in bucket [0,60).
        seed(
            &db,
            &[
                reading("r1", "node-a", 10.0, 0),
                reading("r2", "node-a", 20.0, 30),
                reading("r3", "node-b", 5.0, 10),
            ],
        )
        .await;

        // Window [0, 30) — the r2 row at t=30 and anything later is outside.
        let spec = TsQueryKind::WindowAggregate {
            metric: "temperature_c".into(),
            start_ms: 0,
            end_ms: 30,
            petal_id: "p1".into(),
        };
        let shards = vec![
            shard("p1/node-a/0", "node-a", 0, 60),
            shard("p1/node-b/0", "node-b", 0, 60),
        ];
        let partial = execute_ts_partial(&db, &spec, &shards, PARTIAL_ROW_CAP)
            .await
            .expect("partial");
        assert!(!partial.truncated);
        let a = partial.per_shard.iter().find(|s| s.shard == "p1/node-a/0");
        let b = partial.per_shard.iter().find(|s| s.shard == "p1/node-b/0");
        let a = a.expect("node-a shard present").rows[0].clone();
        let b = b.expect("node-b shard present").rows[0].clone();
        // Only r1 (t=0) is inside the window: sum 10, count 1.
        assert_eq!(a["node_id"], "node-a");
        assert_eq!(a["sum_value"].as_f64(), Some(10.0));
        assert_eq!(a["sample_count"].as_i64(), Some(1));
        assert_eq!(a["min_value"].as_f64(), Some(10.0));
        assert_eq!(b["node_id"], "node-b");
        assert_eq!(b["sample_count"].as_i64(), Some(1));
    }

    #[tokio::test]
    async fn window_aggregate_partial_multiple_rows_per_shard() {
        let db = schema_db().await;
        seed(
            &db,
            &[
                reading("r1", "node-a", 10.0, 0),
                reading("r2", "node-a", 20.0, 30),
                reading("r3", "node-a", 30.0, 59),
            ],
        )
        .await;
        let spec = TsQueryKind::WindowAggregate {
            metric: "temperature_c".into(),
            start_ms: 0,
            end_ms: 60,
            petal_id: "p1".into(),
        };
        let shards = vec![shard("p1/node-a/0", "node-a", 0, 60)];
        let partial = execute_ts_partial(&db, &spec, &shards, PARTIAL_ROW_CAP)
            .await
            .expect("partial");
        let row = &partial.per_shard[0].rows[0];
        assert_eq!(row["sum_value"].as_f64(), Some(60.0));
        assert_eq!(row["sample_count"].as_i64(), Some(3));
        assert_eq!(row["min_value"].as_f64(), Some(10.0));
        assert_eq!(row["max_value"].as_f64(), Some(30.0));
    }

    #[tokio::test]
    async fn readings_partial_returns_window_rows_only() {
        let db = schema_db().await;
        seed(
            &db,
            &[
                reading("r1", "node-a", 10.0, 0),
                reading("r2", "node-a", 20.0, 120),
            ],
        )
        .await;
        let spec = TsQueryKind::ReadingsInWindow {
            metric: "temperature_c".into(),
            start_ms: 0,
            end_ms: 60,
            petal_id: "p1".into(),
        };
        let partial = execute_ts_partial(&db, &spec, &[], PARTIAL_ROW_CAP)
            .await
            .expect("partial");
        assert_eq!(partial.rows.len(), 1);
        assert_eq!(partial.rows[0]["reading_id"], "r1");
        assert!(partial.per_shard.is_empty());
    }

    #[tokio::test]
    async fn latest_partial_keeps_newest_per_anchor_metric() {
        let db = schema_db().await;
        seed(
            &db,
            &[
                reading("r1", "node-a", 10.0, 0),
                reading("r2", "node-a", 99.0, 120),
                reading("r3", "node-b", 5.0, 60),
            ],
        )
        .await;
        let spec = TsQueryKind::LatestPerAnchor {
            petal_id: "p1".into(),
            metric: None,
        };
        let partial = execute_ts_partial(&db, &spec, &[], PARTIAL_ROW_CAP)
            .await
            .expect("partial");
        assert_eq!(partial.rows.len(), 2);
        for row in &partial.rows {
            match row["node_id"].as_str() {
                Some("node-a") => assert_eq!(row["value"].as_f64(), Some(99.0)),
                Some("node-b") => assert_eq!(row["value"].as_f64(), Some(5.0)),
                other => panic!("unexpected anchor {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn all_readings_partial_returns_every_metric_of_the_petal() {
        let db = schema_db().await;
        seed(
            &db,
            &[
                reading("r1", "node-a", 10.0, 0),
                reading("r2", "node-a", 20.0, 120),
                reading("r3", "node-b", 5.0, 60),
            ],
        )
        .await;
        // A reading of ANOTHER petal must stay out of the petal's view.
        let mut foreign = reading("rx", "node-a", 1.0, 10);
        foreign.petal_id = "p2".into();
        seed(&db, std::slice::from_ref(&foreign)).await;
        let spec = TsQueryKind::AllReadings {
            petal_id: "p1".into(),
        };
        let partial = execute_ts_partial(&db, &spec, &[], PARTIAL_ROW_CAP)
            .await
            .expect("partial");
        let ids: Vec<&str> = partial
            .rows
            .iter()
            .filter_map(|r| r["reading_id"].as_str())
            .collect();
        // Oldest first, all metrics of the petal, no foreign rows.
        assert_eq!(ids, vec!["r1", "r3", "r2"]);
        assert!(partial.per_shard.is_empty());
        assert!(!partial.truncated);
    }

    #[tokio::test]
    async fn row_cap_truncates_and_flags() {
        let db = schema_db().await;
        // 300 rows — more than the clamped cap floor (256), so the truncation
        // branch actually fires when a tiny requested cap clamps up to it.
        seed(
            &db,
            &(0..300)
                .map(|i| reading(&format!("r{i}"), "node-a", i as f64, i))
                .collect::<Vec<_>>(),
        )
        .await;
        let spec = TsQueryKind::ReadingsInWindow {
            metric: "temperature_c".into(),
            start_ms: 0,
            end_ms: 1_000,
            petal_id: "p1".into(),
        };
        // The executor clamps a requested cap of 1 up to MIN_PARTIAL_ROW_CAP
        // and truncates the 300-row window to exactly that floor, flagging
        // `truncated` so the merge reports incompleteness (A16).
        let partial = execute_ts_partial(&db, &spec, &[], 1)
            .await
            .expect("partial");
        assert_eq!(
            partial.rows.len(),
            fe_runtime::distributed_query::MIN_PARTIAL_ROW_CAP
        );
        assert!(partial.truncated, "a hit cap must flag truncated");
        assert!(partial.rows.len() <= fe_runtime::distributed_query::MAX_PARTIAL_ROW_CAP);
    }

    #[tokio::test]
    async fn invalid_specs_are_rejected() {
        let db = schema_db().await;
        let bad_window = TsQueryKind::WindowAggregate {
            metric: "".into(),
            start_ms: 0,
            end_ms: 60,
            petal_id: "p1".into(),
        };
        assert!(execute_ts_partial(&db, &bad_window, &[], PARTIAL_ROW_CAP)
            .await
            .is_err());
        let inverted = TsQueryKind::WindowAggregate {
            metric: "m".into(),
            start_ms: 60,
            end_ms: 0,
            petal_id: "p1".into(),
        };
        assert!(execute_ts_partial(&db, &inverted, &[], PARTIAL_ROW_CAP)
            .await
            .is_err());
        let empty_window = TsQueryKind::ReadingsInWindow {
            metric: "m".into(),
            start_ms: 0,
            end_ms: 0,
            petal_id: "p1".into(),
        };
        assert!(execute_ts_partial(&db, &empty_window, &[], PARTIAL_ROW_CAP)
            .await
            .is_err());
    }
}
