use std::sync::Arc;

use datafusion::arrow::array::RecordBatch;
use datafusion::arrow::json::ArrayWriter;
use datafusion::prelude::SessionContext;
use fe_entity_store::EntityStore;

use super::provider::EntityStoreTable;
use super::udf::register_spatial_udfs;

/// High-level analytics context wrapping a DataFusion `SessionContext`.
///
/// Register entity tables from an `EntityStore`, then execute SQL queries
/// that are evaluated entirely in-process against columnar Arrow data.
pub struct AnalyticsContext {
    ctx: SessionContext,
    store: Arc<EntityStore>,
}

impl AnalyticsContext {
    /// Create a new context with spatial UDFs pre-registered.
    pub fn new(store: Arc<EntityStore>) -> Self {
        let ctx = SessionContext::new();
        register_spatial_udfs(&ctx);
        Self { ctx, store }
    }

    /// Register nodes from the entity store as a named table.
    ///
    /// When `petal_id` is `Some`, only nodes belonging to that petal are
    /// visible in the table.
    pub fn register_node_table(
        &self,
        table_name: &str,
        petal_id: Option<&str>,
    ) -> anyhow::Result<()> {
        let table = EntityStoreTable::new(
            Arc::clone(&self.store),
            petal_id.map(|s| s.to_string()),
        );
        self.ctx
            .register_table(table_name, Arc::new(table))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(())
    }

    /// Execute a SQL query and collect the results as `RecordBatch`es.
    pub async fn execute(&self, sql: &str) -> anyhow::Result<Vec<RecordBatch>> {
        let df = self.ctx.sql(sql).await?;
        let batches = df.collect().await?;
        Ok(batches)
    }

    /// Execute a SQL query and return results as JSON values.
    pub async fn execute_to_json(
        &self,
        sql: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let batches = self.execute(sql).await?;
        batches_to_json(&batches)
    }
}

/// Convert record batches to a vector of JSON objects.
fn batches_to_json(batches: &[RecordBatch]) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut buf = Vec::new();
    let mut writer = ArrayWriter::new(&mut buf);
    for batch in batches {
        writer.write(batch)?;
    }
    writer.finish()?;
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&buf)?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_entity_store::EntitySnapshot;

    fn make_snap(node_id: &str, petal_id: &str, x: f32) -> EntitySnapshot {
        EntitySnapshot {
            node_id: node_id.into(),
            petal_id: petal_id.into(),
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            properties: None,
            updated_at_ms: 100,
            node_log: vec![],
        }
    }

    #[tokio::test]
    async fn select_all() {
        let store = Arc::new(EntityStore::new());
        store.upsert(make_snap("n1", "p1", 1.0));
        store.upsert(make_snap("n2", "p1", 2.0));

        let ctx = AnalyticsContext::new(Arc::clone(&store));
        ctx.register_node_table("nodes", None).unwrap();

        let batches = ctx.execute("SELECT node_id, pos_x FROM nodes ORDER BY pos_x").await.unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 2);
    }

    #[tokio::test]
    async fn select_with_petal_filter() {
        let store = Arc::new(EntityStore::new());
        store.upsert(make_snap("n1", "p1", 1.0));
        store.upsert(make_snap("n2", "p2", 2.0));
        store.upsert(make_snap("n3", "p1", 3.0));

        let ctx = AnalyticsContext::new(Arc::clone(&store));
        ctx.register_node_table("nodes", Some("p1")).unwrap();

        let rows = ctx.execute_to_json("SELECT COUNT(*) AS cnt FROM nodes").await.unwrap();
        let cnt = rows[0]["cnt"].as_i64().unwrap();
        assert_eq!(cnt, 2);
    }

    #[tokio::test]
    async fn execute_to_json_works() {
        let store = Arc::new(EntityStore::new());
        store.upsert(make_snap("n1", "p1", 5.0));

        let ctx = AnalyticsContext::new(Arc::clone(&store));
        ctx.register_node_table("nodes", None).unwrap();

        let rows = ctx.execute_to_json("SELECT node_id, pos_x FROM nodes").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["node_id"], "n1");
    }

    #[tokio::test]
    async fn spatial_udf_in_query() {
        let store = Arc::new(EntityStore::new());
        store.upsert(make_snap("n1", "p1", 0.0));

        let ctx = AnalyticsContext::new(Arc::clone(&store));
        ctx.register_node_table("nodes", None).unwrap();

        let rows = ctx
            .execute_to_json(
                "SELECT st_distance(CAST(0.0 AS DOUBLE), CAST(0.0 AS DOUBLE), CAST(1.0 AS DOUBLE), CAST(1.0 AS DOUBLE)) AS dist FROM nodes",
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let dist = rows[0]["dist"].as_f64().unwrap();
        assert!(dist > 100_000.0, "expected >100km, got {dist}");
    }
}
