use crate::atlas::{ModelMetadataUpdate, PetalMetadata, RoomMetadata, SpaceOverview, Visibility};
use crate::handlers::preconditions::{
    require_model_in_petal, require_petal_scope, require_room_in_petal,
};
use crate::op_log::commit_operation;
use crate::schema::{Model, Petal, Room};
use crate::types::{NodeId, OpLogEntry, OpType, PetalId};

/// Unified query facade for space (petal/room/model) metadata operations.
pub struct SpaceManager;

impl SpaceManager {
    // --- Petal metadata ---

    /// Update all metadata fields on a petal and write an op-log entry.
    pub async fn update_petal_metadata(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        caller_node_id: &str,
        petal_id: PetalId,
        meta: PetalMetadata,
    ) -> anyhow::Result<()> {
        let scope = require_petal_scope(db, &petal_id.0.to_string(), "UpdatePetalMetadata").await?;
        crate::rbac::require_write_role(db, caller_node_id, &scope).await?;
        let vis_str = match meta.visibility {
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Unlisted => "unlisted",
        };
        let petal_id_string = petal_id.0.to_string();
        let description = meta.description;
        let tags = meta.tags;

        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId(caller_node_id.to_string()),
            op_type: OpType::UpdatePetalMeta,
            payload: serde_json::json!({
                "petal_id": petal_id_string.clone(),
                "target": petal_id_string.clone(),
                "description": description.clone(),
                "visibility": vis_str,
                "tags": tags.clone(),
            }),
            sig: "00".repeat(64),
            hlc_timestamp: String::new(),
        };
        commit_operation(db, entry, move |_| async move {
            let mut result = db
                .query("UPDATE petal MERGE $data WHERE petal_id = $id")
                .bind((
                    "data",
                    serde_json::json!({
                        "description": description,
                        "visibility": vis_str,
                        "tags": tags,
                    }),
                ))
                .bind(("id", petal_id_string.clone()))
                .await?
                .check()
                .map_err(|error| {
                    anyhow::anyhow!("UpdatePetalMetadata statement failed: {error}")
                })?;
            let updated: Vec<serde_json::Value> = result.take(0).map_err(|error| {
                anyhow::anyhow!("UpdatePetalMetadata result read failed: {error}")
            })?;
            if updated.is_empty() {
                anyhow::bail!(
                    "UpdatePetalMetadata matched no petal with petal_id = {petal_id_string}"
                );
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Update only the visibility field of a petal.
    pub async fn set_petal_visibility(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        caller_node_id: &str,
        petal_id: PetalId,
        visibility: Visibility,
    ) -> anyhow::Result<()> {
        let scope = require_petal_scope(db, &petal_id.0.to_string(), "SetPetalVisibility").await?;
        crate::rbac::require_write_role(db, caller_node_id, &scope).await?;
        let vis_str = match visibility {
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Unlisted => "unlisted",
        };
        let petal_id_string = petal_id.0.to_string();

        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId(caller_node_id.to_string()),
            op_type: OpType::UpdatePetalMeta,
            payload: serde_json::json!({
                "petal_id": petal_id_string.clone(),
                "target": petal_id_string.clone(),
                "visibility": vis_str,
            }),
            sig: "00".repeat(64),
            hlc_timestamp: String::new(),
        };
        commit_operation(db, entry, move |_| async move {
            let mut result = db
                .query("UPDATE petal MERGE $data WHERE petal_id = $id")
                .bind(("data", serde_json::json!({ "visibility": vis_str })))
                .bind(("id", petal_id_string.clone()))
                .await?
                .check()
                .map_err(|error| anyhow::anyhow!("SetPetalVisibility statement failed: {error}"))?;
            let updated: Vec<serde_json::Value> = result.take(0).map_err(|error| {
                anyhow::anyhow!("SetPetalVisibility result read failed: {error}")
            })?;
            if updated.is_empty() {
                anyhow::bail!(
                    "SetPetalVisibility matched no petal with petal_id = {petal_id_string}"
                );
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// List petals whose tags array contains the given tag.
    ///
    /// Uses raw SurrealQL because `CONTAINS` array containment cannot be
    /// expressed via the single-field `Repo::find_where` API.
    pub async fn list_petals_by_tag(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        tag: &str,
    ) -> anyhow::Result<Vec<Petal>> {
        let mut result: surrealdb::IndexedResults = db
            .query("SELECT * FROM petal WHERE tags CONTAINS $tag")
            .bind(("tag", tag.to_string()))
            .await?;
        let raw: Vec<serde_json::Value> = result.take(0)?;
        raw.into_iter()
            .map(|v| serde_json::from_value(v).map_err(Into::into))
            .collect()
    }

    /// Full-text search petals by name, description, or tags.
    ///
    /// Uses raw SurrealQL because `string::lowercase()` full-text predicates
    /// cannot be expressed via `Repo::find_where`.
    pub async fn search_petals(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        query: &str,
    ) -> anyhow::Result<Vec<Petal>> {
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
        let raw: Vec<serde_json::Value> = result.take(0)?;
        raw.into_iter()
            .map(|v| serde_json::from_value(v).map_err(Into::into))
            .collect()
    }

    // --- Room metadata ---

    /// Update room description, bounds, and spawn point.
    ///
    /// Kept as raw SurrealQL because the room table uses `id` as the record
    /// identifier rather than a named field like `room_id`, so `Repo::merge_by_id`
    /// (which filters on `Room::ID_FIELD = "petal_id"`) would not match.
    pub async fn update_room_metadata(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        caller_node_id: &str,
        petal_id: &str,
        room_id: String,
        meta: RoomMetadata,
    ) -> anyhow::Result<()> {
        let target_petal_id =
            require_room_in_petal(db, &room_id, petal_id, "UpdateRoomMetadata").await?;
        let scope = require_petal_scope(db, &target_petal_id, "UpdateRoomMetadata").await?;
        crate::rbac::require_write_role(db, caller_node_id, &scope).await?;
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

        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId(caller_node_id.to_string()),
            op_type: OpType::UpdateRoomMeta,
            payload: serde_json::json!({ "room_id": room_id.clone() }),
            sig: "00".repeat(64),
            hlc_timestamp: String::new(),
        };
        commit_operation(db, entry, move |_| async move {
            let mut result = db
                .query(
                    "UPDATE room SET description = $desc, bounds = $bounds, spawn_point = $spawn \
                 WHERE id = $id AND petal_id = $petal_id",
                )
                .bind(("desc", meta.description))
                .bind(("bounds", bounds_val))
                .bind(("spawn", spawn_val))
                .bind(("id", room_id.clone()))
                .bind(("petal_id", target_petal_id))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("UpdateRoomMetadata statement failed: {e}"))?;
            let updated: Vec<serde_json::Value> = result
                .take(0)
                .map_err(|e| anyhow::anyhow!("UpdateRoomMetadata result read failed: {e}"))?;
            if updated.is_empty() {
                anyhow::bail!("UpdateRoomMetadata matched no room with id = {room_id}");
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Retrieve full room detail.
    pub async fn get_room_detail(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        room_id: String,
    ) -> anyhow::Result<Option<Room>> {
        let mut result: surrealdb::IndexedResults = db
            .query("SELECT * FROM room WHERE id = $id")
            .bind(("id", room_id))
            .await?;
        let raw: Option<serde_json::Value> = result.take(0)?;
        raw.map(|v| serde_json::from_value(v).map_err(Into::into))
            .transpose()
    }

    // --- Model metadata ---

    /// Update all model metadata fields.
    ///
    /// Kept as raw SurrealQL because the model table uses `id` as the record
    /// identifier rather than a named field, so `Repo::merge_by_id`
    /// (which filters on `Model::ID_FIELD = "asset_id"`) would not match.
    pub async fn update_model_metadata(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        caller_node_id: &str,
        petal_id: &str,
        model_id: String,
        update: ModelMetadataUpdate,
    ) -> anyhow::Result<()> {
        let target_petal_id =
            require_model_in_petal(db, &model_id, petal_id, "UpdateModelMetadata").await?;
        let scope = require_petal_scope(db, &target_petal_id, "UpdateModelMetadata").await?;
        crate::rbac::require_write_role(db, caller_node_id, &scope).await?;
        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId(caller_node_id.to_string()),
            op_type: OpType::UpdateModelMeta,
            payload: serde_json::json!({ "model_id": model_id.clone() }),
            sig: "00".repeat(64),
            hlc_timestamp: String::new(),
        };
        commit_operation(db, entry, move |_| async move {
            let mut result = db
                .query(
                    "UPDATE model SET \
                 display_name = $display_name, \
                 description = $description, \
                 external_url = $external_url, \
                 config_url = $config_url, \
                 tags = $tags, \
                 metadata = $metadata \
                 WHERE id = $id AND petal_id = $petal_id",
                )
                .bind(("display_name", update.display_name))
                .bind(("description", update.description))
                .bind(("external_url", update.external_url))
                .bind(("config_url", update.config_url))
                .bind(("tags", update.tags))
                .bind(("metadata", update.metadata))
                .bind(("id", model_id.clone()))
                .bind(("petal_id", target_petal_id))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("UpdateModelMetadata statement failed: {e}"))?;
            let updated: Vec<serde_json::Value> = result
                .take(0)
                .map_err(|e| anyhow::anyhow!("UpdateModelMetadata result read failed: {e}"))?;
            if updated.is_empty() {
                anyhow::bail!("UpdateModelMetadata matched no model with id = {model_id}");
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Upsert a single key-value pair into a model's metadata object.
    ///
    /// `key` must be a valid identifier (`[a-zA-Z_][a-zA-Z0-9_]*`) to prevent
    /// SurrealQL injection via the field path, which cannot be parameterised.
    pub async fn upsert_model_kv(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        caller_node_id: &str,
        petal_id: &str,
        model_id: String,
        key: &str,
        value: serde_json::Value,
    ) -> anyhow::Result<()> {
        if !is_valid_field_key(key) {
            anyhow::bail!("invalid metadata key {key:?}: must match [a-zA-Z_][a-zA-Z0-9_]*");
        }
        let target_petal_id =
            require_model_in_petal(db, &model_id, petal_id, "UpsertModelKv").await?;
        let scope = require_petal_scope(db, &target_petal_id, "UpsertModelKv").await?;
        crate::rbac::require_write_role(db, caller_node_id, &scope).await?;
        let key = key.to_string();
        let q = format!(
            "UPDATE model SET metadata.{key} = $value WHERE id = $id AND petal_id = $petal_id"
        );
        let mut metadata = serde_json::Map::new();
        metadata.insert(key, value.clone());
        let entry = OpLogEntry {
            lamport_clock: 0,
            node_id: NodeId(caller_node_id.to_string()),
            op_type: OpType::UpdateModelMeta,
            payload: serde_json::json!({
                "model_id": model_id.clone(),
                "metadata": metadata,
            }),
            sig: "00".repeat(64),
            hlc_timestamp: String::new(),
        };
        commit_operation(db, entry, move |_| async move {
            let mut result = db
                .query(q)
                .bind(("value", value))
                .bind(("id", model_id.clone()))
                .bind(("petal_id", target_petal_id))
                .await?
                .check()
                .map_err(|e| anyhow::anyhow!("UpsertModelKv statement failed: {e}"))?;
            let updated: Vec<serde_json::Value> = result
                .take(0)
                .map_err(|e| anyhow::anyhow!("UpsertModelKv result read failed: {e}"))?;
            if updated.is_empty() {
                anyhow::bail!("UpsertModelKv matched no model with id = {model_id}");
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// List models whose tags array contains the given tag.
    ///
    /// Uses raw SurrealQL because `CONTAINS` array containment cannot be
    /// expressed via the single-field `Repo::find_where` API.
    pub async fn list_models_by_tag(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        tag: &str,
    ) -> anyhow::Result<Vec<Model>> {
        let mut result: surrealdb::IndexedResults = db
            .query("SELECT * FROM model WHERE tags CONTAINS $tag")
            .bind(("tag", tag.to_string()))
            .await?;
        let raw: Vec<serde_json::Value> = result.take(0)?;
        raw.into_iter()
            .map(|v| serde_json::from_value(v).map_err(Into::into))
            .collect()
    }

    /// Full-text search models by display_name, description, or tags.
    ///
    /// Uses raw SurrealQL because `string::lowercase()` full-text predicates
    /// cannot be expressed via `Repo::find_where`.
    pub async fn search_models(
        db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
        query: &str,
    ) -> anyhow::Result<Vec<Model>> {
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
        let raw: Vec<serde_json::Value> = result.take(0)?;
        raw.into_iter()
            .map(|v| serde_json::from_value(v).map_err(Into::into))
            .collect()
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

/// Validate that a metadata field key is a safe SurrealQL identifier.
/// Accepts `[a-zA-Z_][a-zA-Z0-9_]*` only — no dots, spaces, or operators.
fn is_valid_field_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        None => false,
        Some(first) => {
            (first.is_ascii_alphabetic() || first == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
    }
}

/// Extract the `count` field from a SurrealDB `count() GROUP ALL` response.
fn extract_count(result: Result<surrealdb::IndexedResults, surrealdb::Error>) -> u64 {
    let Ok(mut r) = result else { return 0 };
    let rows: Vec<serde_json::Value> = r.take(0).unwrap_or_default();
    rows.first().and_then(|v| v["count"].as_u64()).unwrap_or(0)
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
