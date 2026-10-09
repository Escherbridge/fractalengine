//! Analytics hot-cache wiring shared by the host binaries — see
//! `fe-api/AGENTS.md` §analytics-query. Extracted verbatim from the GUI
//! binary's startup (F24) so the relay hydrates and mirrors the
//! `EntityStore` the same way; the GUI keeps its local pre-extraction copy
//! until its migration (mechanical, follow-up).

use fe_entity_store::EntityStore;

/// Hydrate the analytics cache from every live local node before API
/// startup. A malformed row is a hard error so callers can fail the surface
/// closed (the GUI's posture) rather than serve a partial snapshot.
///
/// 10-second bounded: a wedged store must not stall host startup forever.
pub async fn hydrate_entity_store(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    store: &EntityStore,
) -> anyhow::Result<usize> {
    let query = db.query(
        // `created_at` is selected only to satisfy SurrealDB 3.x, which rejects an ORDER BY
        // idiom that the projection does not carry. Rows land as `serde_json::Value` and
        // `snapshot_from_node_row` reads keys by name, so the extra column is inert.
        "SELECT node_id, petal_id, position, elevation, rotation, scale, properties, created_at \
         FROM node WHERE tombstone = NONE ORDER BY created_at ASC",
    );
    let mut response = tokio::time::timeout(std::time::Duration::from_secs(10), query)
        .await
        .map_err(|_| anyhow::anyhow!("local node hydration query timed out"))?
        .map_err(|error| anyhow::anyhow!("local node hydration query failed: {error}"))?;
    let rows: Vec<serde_json::Value> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("local node hydration response failed: {error}"))?;
    let hydrated_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    for row in &rows {
        store.upsert(snapshot_from_node_row(row, hydrated_at_ms)?);
    }
    Ok(rows.len())
}

/// Convert a validated local node row into the EntityStore's analytics shape.
pub fn snapshot_from_node_row(
    row: &serde_json::Value,
    updated_at_ms: u64,
) -> anyhow::Result<fe_entity_store::EntitySnapshot> {
    let node_id = required_row_string(row, "node_id")?;
    let petal_id = required_row_string(row, "petal_id")?;
    let coordinates = row
        .pointer("/position/coordinates")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("node {node_id} has no position.coordinates array"))?;
    let position = [
        required_row_number(coordinates, 0, "position.coordinates")?,
        row.get("elevation")
            .and_then(serde_json::Value::as_f64)
            .ok_or_else(|| anyhow::anyhow!("node {node_id} has no elevation"))? as f32,
        required_row_number(coordinates, 1, "position.coordinates")?,
    ];
    let rotation = required_row_array3(row, "rotation")?;
    let scale = required_row_array3(row, "scale")?;
    let properties = match row.get("properties") {
        Some(value) if value.is_null() => None,
        Some(value) if value.is_object() => Some(value.clone()),
        Some(_) => anyhow::bail!("node {node_id} has non-object properties"),
        None => None,
    };

    Ok(fe_entity_store::EntitySnapshot {
        node_id,
        petal_id,
        position,
        rotation,
        scale,
        properties,
        updated_at_ms,
        node_log: Vec::new(),
    })
}

/// Convert a DB-thread `SceneChange` (fe-runtime's cross-thread shape) into
/// the EntityStore's local mirror enum — fe-entity-store deliberately does
/// not depend on fe-runtime, so every host bridge does this conversion at
/// its own seam, dropping the runtime-only routing field (petal attribution
/// — the store owns the node→petal projection).
pub fn runtime_scene_change_to_store(
    change: fe_runtime::messages::SceneChange,
) -> fe_entity_store::SceneChange {
    use fe_entity_store::SceneChange as StoreChange;
    match change {
        fe_runtime::messages::SceneChange::NodeAdded { node } => StoreChange::NodeAdded {
            node: fe_entity_store::NodeSnapshot {
                node_id: node.node_id,
                petal_id: node.petal_id,
                name: node.name,
                position: node.position,
                rotation: node.rotation,
                scale: node.scale,
                has_asset: node.has_asset,
                asset_path: node.asset_path,
            },
        },
        fe_runtime::messages::SceneChange::NodeRemoved { node_id, .. } => {
            StoreChange::NodeRemoved { node_id }
        }
        fe_runtime::messages::SceneChange::NodeRenamed {
            node_id, new_name, ..
        } => StoreChange::NodeRenamed { node_id, new_name },
        fe_runtime::messages::SceneChange::NodeTransform {
            node_id,
            position,
            rotation,
            scale,
            ..
        } => StoreChange::NodeTransform {
            node_id,
            position,
            rotation,
            scale,
        },
        fe_runtime::messages::SceneChange::TransformFailed {
            node_id,
            position,
            rotation,
            scale,
            ..
        } => StoreChange::TransformFailed {
            node_id,
            position,
            rotation,
            scale,
        },
        fe_runtime::messages::SceneChange::PropertyChanged {
            node_id,
            key,
            value,
            ..
        } => StoreChange::PropertyChanged {
            node_id,
            key,
            value,
        },
    }
}

/// Read a nonempty string field from a direct local node row.
fn required_row_string(row: &serde_json::Value, field: &str) -> anyhow::Result<String> {
    let value = row
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("node row has no {field}"))?;
    Ok(value.to_string())
}

/// Read one finite f32 from a direct local node row array.
fn required_row_number(
    values: &[serde_json::Value],
    index: usize,
    field: &str,
) -> anyhow::Result<f32> {
    let value = values
        .get(index)
        .and_then(serde_json::Value::as_f64)
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow::anyhow!("node row has invalid {field}[{index}]"))?;
    let value = value as f32;
    if !value.is_finite() {
        anyhow::bail!("node row has invalid {field}[{index}]");
    }
    Ok(value)
}

/// Read the three visible transform components used by the analytics snapshot.
fn required_row_array3(row: &serde_json::Value, field: &str) -> anyhow::Result<[f32; 3]> {
    let values = row
        .get(field)
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("node row has no {field} array"))?;
    Ok([
        required_row_number(values, 0, field)?,
        required_row_number(values, 1, field)?,
        required_row_number(values, 2, field)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_row_hydrates_the_entity_store_shape() {
        let row = serde_json::json!({
            "node_id": "node-1",
            "petal_id": "petal-1",
            "position": { "coordinates": [1.0, 3.0] },
            "elevation": 2.0,
            "rotation": [0.1, 0.2, 0.3],
            "scale": [1.0, 2.0, 3.0],
            "properties": { "kind": "marker" }
        });

        let snapshot = snapshot_from_node_row(&row, 42).expect("valid node row");

        assert_eq!(snapshot.node_id, "node-1");
        assert_eq!(snapshot.petal_id, "petal-1");
        assert_eq!(snapshot.position, [1.0, 2.0, 3.0]);
        assert_eq!(snapshot.rotation, [0.1, 0.2, 0.3]);
        assert_eq!(snapshot.scale, [1.0, 2.0, 3.0]);
        assert_eq!(
            snapshot.properties,
            Some(serde_json::json!({ "kind": "marker" }))
        );
        assert_eq!(snapshot.updated_at_ms, 42);
    }

    #[test]
    fn malformed_node_row_rejects_startup_hydration() {
        let row = serde_json::json!({
            "node_id": "node-1",
            "petal_id": "petal-1",
            "position": { "coordinates": [1.0] },
            "elevation": 2.0,
            "rotation": [0.0, 0.0, 0.0],
            "scale": [1.0, 1.0, 1.0]
        });

        assert!(snapshot_from_node_row(&row, 42).is_err());
    }

    #[tokio::test]
    async fn hydration_loads_each_live_node_before_api_startup() {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .expect("in-memory SurrealDB");
        db.use_ns("test").use_db("test").await.expect("ns/db");
        fe_database::schema::apply_all(&db)
            .await
            .expect("apply schema");
        db.query(
            "CREATE node CONTENT { \
             node_id: 'live-node', petal_id: 'petal-1', \
             position: <geometry<point>> [1.0, 3.0], elevation: 2.0, \
             rotation: [0.0, 0.0, 0.0], scale: [1.0, 1.0, 1.0], \
             interactive: true, created_at: '2026-08-08T00:00:00Z' }",
        )
        .await
        .expect("create live node")
        .check()
        .expect("live node query succeeded");
        db.query(
            "CREATE node CONTENT { \
             node_id: 'deleted-node', petal_id: 'petal-1', \
             position: <geometry<point>> [4.0, 6.0], elevation: 5.0, \
             rotation: [0.0, 0.0, 0.0], scale: [1.0, 1.0, 1.0], \
             interactive: true, created_at: '2026-08-08T00:00:00Z', \
             tombstone: { hlc: 1, source_did: 'did:key:test' } }",
        )
        .await
        .expect("create tombstoned node")
        .check()
        .expect("tombstoned node query succeeded");

        let store = fe_entity_store::EntityStore::new();
        let count = hydrate_entity_store(&db, &store)
            .await
            .expect("hydrate live nodes");

        assert_eq!(count, 1);
        assert_eq!(store.node_count(), 1);
        assert_eq!(store.get("live-node").unwrap().position, [1.0, 2.0, 3.0]);
        assert!(store.get("deleted-node").is_none());
    }

    #[test]
    fn runtime_scene_changes_convert_to_the_store_mirror_shape() {
        // NodeAdded keeps every snapshot field the store mirror needs.
        let added = runtime_scene_change_to_store(fe_runtime::messages::SceneChange::NodeAdded {
            node: fe_runtime::messages::NodeDto {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "anchor".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        });
        match &added {
            fe_entity_store::SceneChange::NodeAdded { node } => {
                assert_eq!(node.node_id, "n1");
                assert_eq!(node.petal_id, "p1");
                assert_eq!(node.name, "anchor");
            }
            other => panic!("unexpected conversion: {other:?}"),
        }

        // The runtime-only petal attribution is dropped (the store owns the
        // node→petal projection); identity fields survive.
        let removed =
            runtime_scene_change_to_store(fe_runtime::messages::SceneChange::NodeRemoved {
                node_id: "n1".into(),
                petal_id: "p1".into(),
            });
        assert!(matches!(
            removed,
            fe_entity_store::SceneChange::NodeRemoved { ref node_id } if node_id == "n1"
        ));

        let renamed =
            runtime_scene_change_to_store(fe_runtime::messages::SceneChange::NodeRenamed {
                node_id: "n1".into(),
                new_name: "renamed".into(),
                petal_id: "p1".into(),
            });
        assert!(matches!(
            renamed,
            fe_entity_store::SceneChange::NodeRenamed { ref node_id, ref new_name }
                if node_id == "n1" && new_name == "renamed"
        ));

        // Applying a converted change through the store is the live mirror
        // path the relay's drain thread rides.
        let store = fe_entity_store::EntityStore::new();
        store.apply_scene_change(&added, 7);
        assert_eq!(store.node_count(), 1);
        assert_eq!(store.get("n1").unwrap().petal_id, "p1");
    }
}
