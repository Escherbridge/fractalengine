use std::any::Any;
use std::sync::Arc;

use datafusion::arrow::array::{Float32Array, RecordBatch, StringBuilder, UInt64Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::catalog::Session;
use datafusion::datasource::memory::MemTable;
use datafusion::datasource::{TableProvider, TableType};
use datafusion::error::DataFusionError;
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;

use fe_entity_store::EntitySnapshot;
use fe_entity_store::EntityStore;

/// DataFusion `TableProvider` backed by an `EntityStore`.
///
/// Optionally scoped to a single petal via `petal_id`.
pub struct EntityStoreTable {
    store: Arc<EntityStore>,
    petal_id: Option<String>,
    schema: SchemaRef,
}

impl std::fmt::Debug for EntityStoreTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntityStoreTable")
            .field("petal_id", &self.petal_id)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl EntityStoreTable {
    pub fn new(store: Arc<EntityStore>, petal_id: Option<String>) -> Self {
        Self {
            store,
            petal_id,
            schema: entity_schema(),
        }
    }

    fn collect_snapshots(&self) -> Vec<EntitySnapshot> {
        match &self.petal_id {
            Some(pid) => {
                let ids = self.store.get_by_petal(pid);
                ids.iter().filter_map(|id| self.store.get(id)).collect()
            }
            None => self.store.all_snapshots(),
        }
    }
}

/// Arrow schema for the entity table.
pub fn entity_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("node_id", DataType::Utf8, false),
        Field::new("petal_id", DataType::Utf8, false),
        Field::new("pos_x", DataType::Float32, false),
        Field::new("pos_y", DataType::Float32, false),
        Field::new("pos_z", DataType::Float32, false),
        Field::new("rot_x", DataType::Float32, false),
        Field::new("rot_y", DataType::Float32, false),
        Field::new("rot_z", DataType::Float32, false),
        Field::new("scale_x", DataType::Float32, false),
        Field::new("scale_y", DataType::Float32, false),
        Field::new("scale_z", DataType::Float32, false),
        Field::new("properties", DataType::Utf8, true),
        Field::new("updated_at_ms", DataType::UInt64, false),
    ]))
}

/// Convert a slice of snapshots into an Arrow `RecordBatch`.
pub fn snapshots_to_batch(
    snapshots: &[EntitySnapshot],
    schema: &SchemaRef,
) -> Result<RecordBatch, DataFusionError> {
    let len = snapshots.len();

    let mut node_ids = StringBuilder::with_capacity(len, len * 16);
    let mut petal_ids = StringBuilder::with_capacity(len, len * 16);
    let mut pos_x = Vec::with_capacity(len);
    let mut pos_y = Vec::with_capacity(len);
    let mut pos_z = Vec::with_capacity(len);
    let mut rot_x = Vec::with_capacity(len);
    let mut rot_y = Vec::with_capacity(len);
    let mut rot_z = Vec::with_capacity(len);
    let mut sc_x = Vec::with_capacity(len);
    let mut sc_y = Vec::with_capacity(len);
    let mut sc_z = Vec::with_capacity(len);
    let mut props = StringBuilder::with_capacity(len, len * 32);
    let mut updated = Vec::with_capacity(len);

    for snap in snapshots {
        node_ids.append_value(&snap.node_id);
        petal_ids.append_value(&snap.petal_id);
        pos_x.push(snap.position[0]);
        pos_y.push(snap.position[1]);
        pos_z.push(snap.position[2]);
        rot_x.push(snap.rotation[0]);
        rot_y.push(snap.rotation[1]);
        rot_z.push(snap.rotation[2]);
        sc_x.push(snap.scale[0]);
        sc_y.push(snap.scale[1]);
        sc_z.push(snap.scale[2]);
        match &snap.properties {
            Some(v) => props.append_value(v.to_string()),
            None => props.append_null(),
        }
        updated.push(snap.updated_at_ms);
    }

    RecordBatch::try_new(
        Arc::clone(schema),
        vec![
            Arc::new(node_ids.finish()),
            Arc::new(petal_ids.finish()),
            Arc::new(Float32Array::from(pos_x)),
            Arc::new(Float32Array::from(pos_y)),
            Arc::new(Float32Array::from(pos_z)),
            Arc::new(Float32Array::from(rot_x)),
            Arc::new(Float32Array::from(rot_y)),
            Arc::new(Float32Array::from(rot_z)),
            Arc::new(Float32Array::from(sc_x)),
            Arc::new(Float32Array::from(sc_y)),
            Arc::new(Float32Array::from(sc_z)),
            Arc::new(props.finish()),
            Arc::new(UInt64Array::from(updated)),
        ],
    )
    .map_err(|e| DataFusionError::ArrowError(e, None))
}

#[async_trait::async_trait]
impl TableProvider for EntityStoreTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>, DataFusionError> {
        let snapshots = self.collect_snapshots();
        let batch = snapshots_to_batch(&snapshots, &self.schema)?;
        let mem = MemTable::try_new(Arc::clone(&self.schema), vec![vec![batch]])?;
        mem.scan(state, projection, filters, limit).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::arrow::array::{Array, StringArray};

    fn make_snap(node_id: &str, petal_id: &str, x: f32) -> EntitySnapshot {
        EntitySnapshot {
            node_id: node_id.into(),
            petal_id: petal_id.into(),
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            properties: Some(serde_json::json!({"color": "red"})),
            updated_at_ms: 42,
            node_log: vec![],
        }
    }

    #[test]
    fn schema_field_count() {
        let s = entity_schema();
        assert_eq!(s.fields().len(), 13);
    }

    #[test]
    fn snapshots_to_batch_empty() {
        let schema = entity_schema();
        let batch = snapshots_to_batch(&[], &schema).unwrap();
        assert_eq!(batch.num_rows(), 0);
        assert_eq!(batch.num_columns(), 13);
    }

    #[test]
    fn snapshots_to_batch_roundtrip() {
        let schema = entity_schema();
        let snaps = vec![make_snap("n1", "p1", 1.0), make_snap("n2", "p1", 2.0)];
        let batch = snapshots_to_batch(&snaps, &schema).unwrap();
        assert_eq!(batch.num_rows(), 2);

        let col = batch
            .column_by_name("pos_x")
            .unwrap()
            .as_any()
            .downcast_ref::<Float32Array>()
            .unwrap();
        assert_eq!(col.value(0), 1.0);
        assert_eq!(col.value(1), 2.0);
    }

    #[test]
    fn null_properties() {
        let schema = entity_schema();
        let mut snap = make_snap("n1", "p1", 0.0);
        snap.properties = None;
        let batch = snapshots_to_batch(&[snap], &schema).unwrap();
        let col = batch
            .column_by_name("properties")
            .unwrap()
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert!(col.is_null(0));
    }

    #[test]
    fn petal_filter_collects_subset() {
        let store = Arc::new(EntityStore::new());
        store.upsert(make_snap("n1", "p1", 1.0));
        store.upsert(make_snap("n2", "p2", 2.0));
        store.upsert(make_snap("n3", "p1", 3.0));

        let table = EntityStoreTable::new(Arc::clone(&store), Some("p1".into()));
        let snaps = table.collect_snapshots();
        assert_eq!(snaps.len(), 2);
        assert!(snaps.iter().all(|s| s.petal_id == "p1"));
    }

    #[test]
    fn no_petal_filter_collects_all() {
        let store = Arc::new(EntityStore::new());
        store.upsert(make_snap("n1", "p1", 1.0));
        store.upsert(make_snap("n2", "p2", 2.0));

        let table = EntityStoreTable::new(Arc::clone(&store), None);
        let snaps = table.collect_snapshots();
        assert_eq!(snaps.len(), 2);
    }
}
