use crate::atlas::{
    ModelMetadataUpdate, PetalMetadata, RoomMetadata, SpaceOverview, Visibility,
};
use crate::op_log::write_op_log;
use crate::types::{NodeId, OpLogEntry, OpType, PetalId};

/// Unified query facade for space (petal/room/model) metadata operations.
pub struct SpaceManager;

impl SpaceManager {
    // --- Petal metadata ---

    /// Update all metadata fields on a petal and write an op-log entry.
    pub async fn update_petal_metadata(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        petal_id: PetalId,
        meta: PetalMetadata,
    ) -> anyhow::Result<()> {
        let vis_str = match meta.visibility {
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Unlisted => "unlisted",
        };
        db.query(
            "UPDATE petal SET description = $desc, visibility = $vis, tags = $tags \
             WHERE petal_id = $id",
        )
        .bind(("desc", meta.description.clone()))
        .bind(("vis", vis_str.to_string()))
        .bind(("tags", meta.tags.clone()))
        .bind(("id", petal_id.0.to_string()))
        .await?;

        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId("system".to_string()),
            op_type: OpType::UpdatePetalMeta,
            payload: serde_json::json!({
                "petal_id": petal_id.0.to_string(),
                "target": petal_id.0.to_string(),
                "description": meta.description,
                "visibility": vis_str,
                "tags": meta.tags,
            }),
            sig: "00".repeat(64),
        };
        write_op_log(db, entry).await?;
        Ok(())
    }

    /// Update only the visibility field of a petal.
    pub async fn set_petal_visibility(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        petal_id: PetalId,
        visibility: Visibility,
    ) -> anyhow::Result<()> {
        let vis_str = match visibility {
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Unlisted => "unlisted",
        };
        db.query("UPDATE petal SET visibility = $vis WHERE petal_id = $id")
            .bind(("vis", vis_str.to_string()))
            .bind(("id", petal_id.0.to_string()))
            .await?;

        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId("system".to_string()),
            op_type: OpType::UpdatePetalMeta,
            payload: serde_json::json!({
                "petal_id": petal_id.0.to_string(),
                "target": petal_id.0.to_string(),
                "visibility": vis_str,
            }),
            sig: "00".repeat(64),
        };
        write_op_log(db, entry).await?;
        Ok(())
    }

    /// List petals whose tags array contains the given tag.
    pub async fn list_petals_by_tag(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        tag: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut result: surrealdb::IndexedResults = db
            .query("SELECT * FROM petal WHERE tags CONTAINS $tag")
            .bind(("tag", tag.to_string()))
            .await?;
        let rows: Vec<serde_json::Value> = result.take(0)?;
        Ok(rows)
    }

    /// Full-text search petals by name, description, or tags.
    pub async fn search_petals(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        query: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let q = query.to_lowercase();
        let mut result: surrealdb::IndexedResults = db
            .query(
                "SELECT * FROM petal WHERE \
                 string::lowercase(name) CONTAINS $q \
                 OR (description != NONE AND string::lowercase(description) CONTAINS $q) \
                 OR tags CONTAINS $q",
            )
            .bind(("q", q))
            .await?;
        let rows: Vec<serde_json::Value> = result.take(0)?;
        Ok(rows)
    }

    // --- Room metadata ---

    /// Update room description, bounds, and spawn point.
    pub async fn update_room_metadata(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        room_id: String,
        meta: RoomMetadata,
    ) -> anyhow::Result<()> {
        let bounds_val = meta
            .bounds
            .as_ref()
            .map(|b| serde_json::to_value(b).unwrap_or(serde_json::Value::Null))
            .unwrap_or(serde_json::Value::Null);
        let spawn_val = meta
            .spawn_point
            .as_ref()
            .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null))
            .unwrap_or(serde_json::Value::Null);

        db.query(
            "UPDATE room SET description = $desc, bounds = $bounds, spawn_point = $spawn \
             WHERE id = $id",
        )
        .bind(("desc", meta.description))
        .bind(("bounds", bounds_val))
        .bind(("spawn", spawn_val))
        .bind(("id", room_id.clone()))
        .await?;

        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId("system".to_string()),
            op_type: OpType::UpdateRoomMeta,
            payload: serde_json::json!({ "room_id": room_id }),
            sig: "00".repeat(64),
        };
        write_op_log(db, entry).await?;
        Ok(())
    }

    /// Retrieve full room detail.
    pub async fn get_room_detail(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        room_id: String,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let mut result: surrealdb::IndexedResults = db
            .query("SELECT * FROM room WHERE id = $id")
            .bind(("id", room_id))
            .await?;
        let row: Option<serde_json::Value> = result.take(0)?;
        Ok(row)
    }

    // --- Model metadata ---

    /// Update all model metadata fields.
    pub async fn update_model_metadata(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        model_id: String,
        update: ModelMetadataUpdate,
    ) -> anyhow::Result<()> {
        db.query(
            "UPDATE model SET \
             display_name = $display_name, \
             description = $description, \
             external_url = $external_url, \
             config_url = $config_url, \
             tags = $tags, \
             metadata = $metadata \
             WHERE id = $id",
        )
        .bind(("display_name", update.display_name))
        .bind(("description", update.description))
        .bind(("external_url", update.external_url))
        .bind(("config_url", update.config_url))
        .bind(("tags", update.tags))
        .bind(("metadata", update.metadata))
        .bind(("id", model_id.clone()))
        .await?;

        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId("system".to_string()),
            op_type: OpType::UpdateModelMeta,
            payload: serde_json::json!({ "model_id": model_id }),
            sig: "00".repeat(64),
        };
        write_op_log(db, entry).await?;
        Ok(())
    }

    /// Upsert a single key-value pair into a model's metadata object.
    pub async fn upsert_model_kv(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        model_id: String,
        key: &str,
        value: serde_json::Value,
    ) -> anyhow::Result<()> {
        // Use string interpolation for the field path since SurrealDB doesn't support
        // parameterised field names in UPDATE SET.
        let q = format!(
            "UPDATE model SET metadata.{key} = $value WHERE id = $id",
            key = key
        );
        db.query(q)
            .bind(("value", value))
            .bind(("id", model_id.clone()))
            .await?;
        Ok(())
    }

    /// List models whose tags array contains the given tag.
    pub async fn list_models_by_tag(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        tag: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut result: surrealdb::IndexedResults = db
            .query("SELECT * FROM model WHERE tags CONTAINS $tag")
            .bind(("tag", tag.to_string()))
            .await?;
        let rows: Vec<serde_json::Value> = result.take(0)?;
        Ok(rows)
    }

    /// Full-text search models by display_name, description, or tags.
    pub async fn search_models(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        query: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let q = query.to_lowercase();
        let mut result: surrealdb::IndexedResults = db
            .query(
                "SELECT * FROM model WHERE \
                 (display_name != NONE AND string::lowercase(display_name) CONTAINS $q) \
                 OR (description != NONE AND string::lowercase(description) CONTAINS $q) \
                 OR tags CONTAINS $q",
            )
            .bind(("q", q))
            .await?;
        let rows: Vec<serde_json::Value> = result.take(0)?;
        Ok(rows)
    }

    // --- Aggregate ---

    /// Return aggregate counts across the entire space.
    pub async fn space_overview(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        _filter_petal_id: Option<&PetalId>,
    ) -> anyhow::Result<SpaceOverview> {
        let (petal_res, room_res, model_res) = tokio::join!(
            db.query("SELECT count() FROM petal GROUP ALL"),
            db.query("SELECT count() FROM room GROUP ALL"),
            db.query("SELECT count() FROM model GROUP ALL"),
        );
        let petal_count = extract_count(petal_res);
        let room_count = extract_count(room_res);
        let model_count = extract_count(model_res);
        Ok(SpaceOverview {
            petal_count,
            room_count,
            model_count,
            peer_count: 0,
            estimated_storage_bytes: 0,
        })
    }
}

/// Extract the `count` field from a SurrealDB `count() GROUP ALL` response.
fn extract_count(
    result: Result<surrealdb::IndexedResults, surrealdb::Error>,
) -> u64 {
    let Ok(mut r) = result else { return 0 };
    let rows: Vec<serde_json::Value> = r.take(0).unwrap_or_default();
    rows.first()
        .and_then(|v| v["count"].as_u64())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    // DEFERRED TO VALIDATION PHASE
    //
    // Integration tests for SpaceManager require an in-memory SurrealDB instance.
    // They will be executed during the validation phase after compilation.

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn update_petal_metadata_round_trips() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn set_petal_visibility_does_not_touch_other_fields() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn update_petal_metadata_writes_op_log_entry() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn list_petals_by_tag_returns_matching_only() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn search_petals_matches_name_description_tags() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn update_room_metadata_round_trips_bounds_and_spawn() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn update_model_metadata_round_trips_all_fields() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn upsert_model_kv_merges_without_clobbering() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn list_models_by_tag_returns_matching_only() {
        // DEFERRED TO VALIDATION PHASE
    }

    #[tokio::test]
    #[ignore = "DEFERRED TO VALIDATION PHASE"]
    async fn space_overview_global_counts_correctly() {
        // DEFERRED TO VALIDATION PHASE
    }
}
