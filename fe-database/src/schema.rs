//! Table definitions for every SurrealDB table — one [`define_table!`] call
//! per table (see fe-database/src/AGENTS.md §schema-macro).

/// Define a SurrealDB table as a Rust struct with auto-generated idempotent
/// DDL (syntax guide: fe-database/src/AGENTS.md §schema-macro).
///
/// Each field's `=>` right-hand side is the SurrealQL type clause after
/// `ON TABLE <name>` (may include `ASSERT`, `VALUE`, `DEFAULT`, `FLEXIBLE`).
macro_rules! define_table {
    (
        $(#[$struct_meta:meta])*
        table $table_name:literal => $struct_name:ident (id: $id_field:ident) {
            $(
                $(#[$field_meta:meta])*
                $field:ident : $rust_ty:ty => $surreal_def:literal
            ),* $(,)?
        }
    ) => {
        $(#[$struct_meta])*
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
        pub struct $struct_name {
            $(
                $(#[$field_meta])*
                pub $field: $rust_ty,
            )*
        }

        impl $crate::repo::Table for $struct_name {
            const TABLE_NAME: &'static str = $table_name;
            const ID_FIELD: &'static str = stringify!($id_field);

            fn schema() -> String {
                let mut s = format!(
                    "DEFINE TABLE IF NOT EXISTS {} SCHEMAFULL;\n",
                    $table_name,
                );
                $(
                    s.push_str(&format!(
                        "DEFINE FIELD IF NOT EXISTS {} ON TABLE {} {};\n",
                        stringify!($field),
                        $table_name,
                        $surreal_def,
                    ));
                )*
                s
            }

            fn id_value(&self) -> String {
                serde_json::to_value(&self.$id_field)
                    .ok()
                    .and_then(|v| match v {
                        serde_json::Value::String(s) => Some(s),
                        other => Some(other.to_string()),
                    })
                    .unwrap_or_default()
            }
        }
    };
}

// define_table! is used within this module only; no re-export needed.

// ---------------------------------------------------------------------------
// Table definitions
// ---------------------------------------------------------------------------

define_table! {
    /// A petal (space) in the fractal hierarchy.
    table "petal" => Petal (id: petal_id) {
        petal_id:    String         => "TYPE string",
        name:        String         => "TYPE string",
        node_id:     String         => "TYPE string",
        created_at:  String         => "TYPE string",
        description: Option<String> => "TYPE option<string>",
        visibility:  String         => "TYPE string ASSERT $value IN ['public', 'private', 'unlisted'] VALUE $value OR 'private'",
        tags:        Vec<String>    => "TYPE array<string> VALUE $value OR []",
        fractal_id:  Option<String> => "TYPE option<string>",
        #[serde(skip_serializing_if = "Option::is_none")]
        bounds: Option<serde_json::Value> => "TYPE option<geometry<polygon>>",
        #[serde(skip_serializing_if = "Option::is_none")]
        hexon_manifest: Option<serde_json::Value> => "TYPE option<object> FLEXIBLE",
        #[serde(skip_serializing_if = "Option::is_none")]
        terrain: Option<serde_json::Value> => "TYPE option<object> FLEXIBLE"
    }
}

define_table! {
    /// A room within a petal.
    table "room" => Room (id: petal_id) {
        petal_id:    String                       => "TYPE string",
        name:        String                       => "TYPE string",
        description: Option<String>               => "TYPE option<string>",
        bounds:      Option<serde_json::Value>    => "TYPE option<object> FLEXIBLE",
        spawn_point: Option<serde_json::Value>    => "TYPE option<object> FLEXIBLE"
    }
}

define_table! {
    /// A 3-D model placed inside a petal.
    table "model" => Model (id: asset_id) {
        petal_id:     String                    => "TYPE string",
        asset_id:     String                    => "TYPE string",
        transform:    serde_json::Value         => "TYPE object FLEXIBLE",
        display_name: Option<String>            => "TYPE option<string>",
        description:  Option<String>            => "TYPE option<string>",
        external_url: Option<String>            => "TYPE option<string>",
        config_url:   Option<String>            => "TYPE option<string>",
        tags:         Vec<String>               => "TYPE array<string> VALUE $value OR []",
        metadata:     Option<serde_json::Value> => "TYPE option<object> FLEXIBLE"
    }
}

define_table! {
    /// RBAC role assignment with hierarchical scope.
    /// Scope uses the Resource Descriptor format: VERSE#id-FRACTAL#id-PETAL#id
    table "role" => Role (id: peer_did) {
        peer_did: String => "TYPE string",
        scope:    String => "TYPE string",
        role:     String => "TYPE string"
    }
}

define_table! {
    /// Append-only operation log for CRDT convergence.
    table "op_log" => OpLog (id: lamport_clock) {
        lamport_clock:  i64              => "TYPE int",
        hlc_timestamp:  String           => "TYPE string DEFAULT ''",
        node_id:        String           => "TYPE string",
        op_type:        String           => "TYPE string",
        payload:        serde_json::Value => "TYPE object FLEXIBLE",
        sig:            String           => "TYPE string"
    }
}

define_table! {
    /// A verse -- the top-level container in the hierarchy.
    table "verse" => Verse (id: verse_id) {
        verse_id:       String         => "TYPE string",
        name:           String         => "TYPE string",
        created_by:     String         => "TYPE string",
        created_at:     String         => "TYPE string",
        namespace_id:   Option<String> => "TYPE option<string>",
        default_access: String         => "TYPE string DEFAULT 'viewer'"
    }
}

define_table! {
    /// Membership record linking a peer DID to a verse.
    table "verse_member" => VerseMemberRow (id: member_id) {
        member_id:        String         => "TYPE string",
        verse_id:         String         => "TYPE string",
        peer_did:         String         => "TYPE string",
        status:           String         => "TYPE string ASSERT $value IN ['active', 'revoked']",
        invited_by:       String         => "TYPE string",
        invite_sig:       String         => "TYPE string",
        invite_timestamp: String         => "TYPE string",
        revoked_at:       Option<String> => "TYPE option<string>",
        revoked_by:       Option<String> => "TYPE option<string>"
    }
}

define_table! {
    /// A fractal -- groups petals under a verse.
    table "fractal" => Fractal (id: fractal_id) {
        fractal_id:  String         => "TYPE string",
        verse_id:    String         => "TYPE string",
        owner_did:   String         => "TYPE string",
        name:        String         => "TYPE string",
        description: Option<String> => "TYPE option<string>",
        created_at:  String         => "TYPE string"
    }
}

define_table! {
    /// An interactive object placed within a petal.
    ///
    /// `position` is stored as a GeoJSON Point for 2-D spatial queries on the
    /// XZ plane (X = longitude, Z = latitude).  `elevation` stores the Y axis
    /// height separately.
    table "node" => Node (id: node_id) {
        node_id:      String                    => "TYPE string",
        petal_id:     String                    => "TYPE string",
        display_name: Option<String>            => "TYPE option<string>",
        asset_id:     Option<String>            => "TYPE option<string>",
        /// GeoJSON Point: `{"type":"Point","coordinates":[x,z]}`.
        position:     serde_json::Value         => "TYPE geometry<point>",
        elevation:    f64                       => "TYPE float DEFAULT 0.0",
        rotation:     Vec<f64>                  => "TYPE array",
        scale:        Vec<f64>                  => "TYPE array",
        interactive:  bool                      => "TYPE bool DEFAULT false",
        created_at:   String                    => "TYPE string",
        /// Monotonic edit counter for optimistic concurrency on node mutations.
        edit_seq:     i64                       => "TYPE int DEFAULT 0",
        #[serde(default, skip_serializing_if = "Option::is_none")]
        properties:   Option<serde_json::Value> => "TYPE option<object> FLEXIBLE",
        /// FR-1 durable tombstone: `NONE` = live; an object `{ hlc, source_did,
        /// tombstoned_at }` = sync-safe soft delete. The row persists so the
        /// delete survives reload/P2P merge (N-4); reads filter `tombstone = NONE`.
        /// See fe-database/src/AGENTS.md §lifecycle.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tombstone:    Option<serde_json::Value> => "TYPE option<object> FLEXIBLE"
    }
}

define_table! {
    /// Binary asset metadata (GLTF/GLB models and other media).
    ///
    /// `data` is the legacy base64-encoded content -- being phased out in
    /// favour of `content_hash` which references the blob store.
    table "asset" => Asset (id: asset_id) {
        asset_id:     String         => "TYPE string",
        name:         String         => "TYPE string",
        content_type: String         => "TYPE string",
        size_bytes:   i64            => "TYPE int",
        data:         Option<String> => "TYPE option<string> VALUE $value OR NONE",
        created_at:   String         => "TYPE string",
        content_hash: Option<String> => "TYPE option<string>"
    }
}

define_table! {
    /// Append-only per-node operation log — INSERT-only, immutable rows
    /// (row_version/HLC semantics: fe-database/src/AGENTS.md §node-log).
    table "node_log" => NodeLog (id: log_id) {
        log_id:        String           => "TYPE string",
        node_id:       String           => "TYPE string",
        hlc_timestamp: i64              => "TYPE int",
        op:            String           => "TYPE string",
        source_did:    String           => "TYPE string DEFAULT ''",
        payload:       serde_json::Value => "TYPE object FLEXIBLE",
        row_version:   i64              => "TYPE int",
        created_at:    String           => "TYPE string"
    }
}

define_table! {
    /// One IoT sensor reading anchored to a node — no geometry on the row;
    /// position joins through the anchor node (see AGENTS.md §iot-readings).
    table "iot_reading" => IotReading (id: reading_id) {
        reading_id:     String => "TYPE string",
        node_id:        String => "TYPE string",
        petal_id:       String => "TYPE string",
        metric:         String => "TYPE string",
        value:          f64    => "TYPE float",
        units:          String => "TYPE string DEFAULT ''",
        recorded_at:    String => "TYPE string",
        recorded_at_ms: i64    => "TYPE int",
        hlc_timestamp:  i64    => "TYPE int",
        source_did:     String => "TYPE string DEFAULT ''"
    }
}

define_table! {
    /// Schema definition for user-defined custom properties on entities.
    table "field_def" => FieldDef (id: field_def_id) {
        field_def_id: String                    => "TYPE string",
        scope:        String                    => "TYPE string",
        entity_type:  String                    => "TYPE string",
        key:          String                    => "TYPE string",
        value_type:   String                    => "TYPE string",
        default_val:  Option<serde_json::Value> => "TYPE option<object> FLEXIBLE",
        created_by:   String                    => "TYPE string",
        created_at:   String                    => "TYPE string"
    }
}

define_table! {
    /// Hexon crate registry — tracks locally installed .fecrate packages.
    table "crate_registry" => CrateRegistry (id: hexon_uri) {
        hexon_uri:       String => "TYPE string",
        manifest_hash:   String => "TYPE string",
        publisher_did:   String => "TYPE string",
        hexon_type:      String => "TYPE string",
        version:         String => "TYPE string",
        name:            String => "TYPE string",
        tags:            String => "TYPE string VALUE $value OR '[]'",
        petal_id:        String => "TYPE string",
        size_bytes:      i64    => "TYPE int",
        installed_at:    String => "TYPE string",
        signature_valid: bool   => "TYPE bool",
    }
}

define_table! {
    /// Individual asset entries within an installed hexon crate.
    table "crate_entry" => CrateEntry (id: entry_id) {
        entry_id:   String                    => "TYPE string",
        hexon_uri:  String                    => "TYPE string",
        kind:       String                    => "TYPE string",
        asset_hash: String                    => "TYPE string",
        format:     String                    => "TYPE string",
        label:      String                    => "TYPE string",
        metadata:   Option<serde_json::Value> => "TYPE option<object> FLEXIBLE",
    }
}

// ---------------------------------------------------------------------------
// Canonical Fractal Data Log tables (SPEC-4 / SPEC-3 §5, Workstream G)
//
// Appended, never interleaved with the legacy tables above. Nothing in the
// existing dispatch loop reads or writes them — see
// `fe-database/src/canon_log/AGENTS.md` §parallel-and-dormant.
// ---------------------------------------------------------------------------

define_table! {
    /// SPEC-4 §3 verified log: the durable, content-addressed, append-only set of
    /// admitted operation bytes (rationale: `canon_log/AGENTS.md` §verified-log).
    ///
    /// `envelope_bytes` is the authority; every other column is a durable index
    /// derived from it so parents and ordering keys are findable without decoding.
    table "verified_op_log" => VerifiedOpLog (id: op_id_hex) {
        op_id_hex:             String      => "TYPE string",
        /// Standard base64 of the exact complete-envelope bytes.
        envelope_bytes:        String      => "TYPE string",
        operation_kind:        i64         => "TYPE int",
        branch_id:             String      => "TYPE string",
        parent_op_ids:         Vec<String> => "TYPE array<string> VALUE $value OR []",
        author_public_key_hex: String      => "TYPE string",
        wall_ms:               i64         => "TYPE int",
        hlc_counter:           i64         => "TYPE int",
        appended_at_hlc:       String      => "TYPE string DEFAULT ''",
    }
}

define_table! {
    /// SPEC-4 §3.4 apply marker: one row per (materializer, version, branch, operation)
    /// recording that the operation was processed for that exact projection identity.
    table "materializer_apply_marker" => MaterializerApplyMarker (id: marker_key) {
        /// Colon-joined `{materializer_id}:{version}:{branch_id}:{op_id_hex}`.
        marker_key:           String => "TYPE string",
        materializer_id:      String => "TYPE string",
        materializer_version: i64    => "TYPE int",
        branch_id:            String => "TYPE string",
        op_id_hex:            String => "TYPE string",
        /// `applied`, or `excluded:{reason}` for a §4.6 recorded exclusion.
        disposition:          String => "TYPE string ASSERT $value = 'applied' OR string::starts_with($value, 'excluded:')",
        applied_at_hlc:       String => "TYPE string DEFAULT ''",
    }
}

define_table! {
    /// SPEC-3 §5.1 persistent epoch state, keyed by the hex of the canonical scope encoding.
    table "scope_epoch_state" => ScopeEpochState (id: scope_key) {
        scope_key:            String      => "TYPE string",
        current_epoch:        i64         => "TYPE int",
        admitted_bump_op_ids: Vec<String> => "TYPE array<string> VALUE $value OR []",
    }
}

define_table! {
    /// SPEC-8 shadow ledger for the legacy-to-canonical migration run; written and read by
    /// `fe-database/src/migration/`, defined here so every table lives in one module.
    table "migration_shadow_ledger" => MigrationShadowLedger (id: entry_id) {
        entry_id:                   String => "TYPE string",
        run_id:                     String => "TYPE string",
        intent_digest_hex:          String => "TYPE string",
        mutation_kind:              String => "TYPE string",
        run_local_correlation_id:   String => "TYPE string",
        member_op_ids_json:         String => "TYPE string DEFAULT '[]'",
        candidate_byte_hashes_json: String => "TYPE string DEFAULT '[]'",
        disposition:                String => "TYPE string",
        created_at:                 String => "TYPE string",
    }
}

// ---------------------------------------------------------------------------
// Convenience helpers
// ---------------------------------------------------------------------------

use crate::repo::Table;

/// Run every table's DDL against `db` (idempotent).
pub async fn apply_all(db: &crate::repo::Db) -> anyhow::Result<()> {
    use crate::repo::Repo;
    Repo::<Verse>::apply_schema(db).await?;
    Repo::<VerseMemberRow>::apply_schema(db).await?;
    Repo::<Fractal>::apply_schema(db).await?;
    Repo::<Petal>::apply_schema(db).await?;
    Repo::<Room>::apply_schema(db).await?;
    Repo::<Model>::apply_schema(db).await?;
    Repo::<Role>::apply_schema(db).await?;
    Repo::<OpLog>::apply_schema(db).await?;
    Repo::<Node>::apply_schema(db).await?;
    // Backfill nodes created before `edit_seq` existed: DEFAULT only fires
    // when a field is absent from a write, not when it's stored as NONE, so
    // any pre-existing node with edit_seq = NONE fails schema coercion on
    // every future UPDATE (including SetNodeProperty) until backfilled once.
    db.query("UPDATE node SET edit_seq = 0 WHERE edit_seq = NONE")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("edit_seq backfill: {e}"))?;
    Repo::<Asset>::apply_schema(db).await?;
    Repo::<FieldDef>::apply_schema(db).await?;
    Repo::<NodeLog>::apply_schema(db).await?;
    Repo::<IotReading>::apply_schema(db).await?;
    Repo::<CrateRegistry>::apply_schema(db).await?;
    Repo::<CrateEntry>::apply_schema(db).await?;
    Repo::<VerifiedOpLog>::apply_schema(db).await?;
    Repo::<MaterializerApplyMarker>::apply_schema(db).await?;
    Repo::<ScopeEpochState>::apply_schema(db).await?;
    Repo::<MigrationShadowLedger>::apply_schema(db).await?;

    // Critical indexes for query performance
    db.query("DEFINE INDEX IF NOT EXISTS idx_node_petal ON TABLE node FIELDS petal_id")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("idx_node_petal: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_petal_fractal ON TABLE petal FIELDS fractal_id")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("idx_petal_fractal: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_fractal_verse ON TABLE fractal FIELDS verse_id")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("idx_fractal_verse: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_role_scope ON TABLE role FIELDS scope")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("idx_role_scope: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_node_log_node ON TABLE node_log FIELDS node_id")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("idx_node_log_node: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_node_log_hlc ON TABLE node_log FIELDS node_id, hlc_timestamp")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_node_log_hlc: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_iot_reading_node ON TABLE iot_reading FIELDS node_id")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("idx_iot_reading_node: {e}"))?;
    db.query(
        "DEFINE INDEX IF NOT EXISTS idx_iot_reading_petal ON TABLE iot_reading FIELDS petal_id",
    )
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("idx_iot_reading_petal: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_iot_reading_series ON TABLE iot_reading FIELDS node_id, metric, recorded_at_ms")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_iot_reading_series: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_crate_registry_hexon_uri ON TABLE crate_registry FIELDS hexon_uri")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_crate_registry_hexon_uri: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_crate_entry_hexon_uri ON TABLE crate_entry FIELDS hexon_uri")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_crate_entry_hexon_uri: {e}"))?;

    // Canonical-log indexes. The two UNIQUE ones are load-bearing, not tuning:
    // exactly-once append (SPEC-4 §3.1) and one apply marker per projection
    // position (§3.4) are enforced by the storage layer, not only by read-then-write.
    db.query("DEFINE INDEX IF NOT EXISTS idx_verified_op_log_op_id ON TABLE verified_op_log FIELDS op_id_hex UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_verified_op_log_op_id: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_verified_op_log_branch ON TABLE verified_op_log FIELDS branch_id")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_verified_op_log_branch: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_verified_op_log_equivocation ON TABLE verified_op_log FIELDS author_public_key_hex, wall_ms, hlc_counter")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_verified_op_log_equivocation: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_apply_marker_key ON TABLE materializer_apply_marker FIELDS marker_key UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_apply_marker_key: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_apply_marker_projection ON TABLE materializer_apply_marker FIELDS materializer_id, materializer_version, branch_id")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_apply_marker_projection: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_scope_epoch_state_key ON TABLE scope_epoch_state FIELDS scope_key UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_scope_epoch_state_key: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_migration_shadow_ledger_run ON TABLE migration_shadow_ledger FIELDS run_id")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_migration_shadow_ledger_run: {e}"))?;

    Ok(())
}

/// All table names, for admin operations like dump / clear.
///
/// Deliberately excludes the canonical-log tables: SPEC-4 §1.4 makes the verified log the
/// authority and the SurrealDB projection derivative, so an admin "clear everything" that
/// erased `verified_op_log` would destroy history a rebuild cannot recover. The projection
/// tables listed here are rebuildable from the log; the log is not rebuildable from them.
pub const ALL_TABLE_NAMES: &[&str] = &[
    Petal::TABLE_NAME,
    Room::TABLE_NAME,
    Model::TABLE_NAME,
    Role::TABLE_NAME,
    OpLog::TABLE_NAME,
    Verse::TABLE_NAME,
    VerseMemberRow::TABLE_NAME,
    Fractal::TABLE_NAME,
    Node::TABLE_NAME,
    Asset::TABLE_NAME,
    FieldDef::TABLE_NAME,
    NodeLog::TABLE_NAME,
    IotReading::TABLE_NAME,
    CrateRegistry::TABLE_NAME,
    CrateEntry::TABLE_NAME,
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Petal schema ---

    #[test]
    fn petal_schema_contains_description_field() {
        assert!(Petal::schema().contains("DEFINE FIELD IF NOT EXISTS description ON TABLE petal"));
    }

    #[test]
    fn petal_schema_contains_visibility_field_with_assert() {
        let s = Petal::schema();
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS visibility ON TABLE petal"));
        assert!(s.contains("ASSERT"));
        assert!(s.contains("public"));
        assert!(s.contains("private"));
        assert!(s.contains("unlisted"));
    }

    #[test]
    fn petal_schema_contains_tags_array_field() {
        let s = Petal::schema();
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS tags ON TABLE petal"));
        assert!(s.contains("array<string>"));
    }

    #[test]
    fn petal_schema_contains_fractal_id_field() {
        assert!(Petal::schema().contains("fractal_id ON TABLE petal TYPE option<string>"));
    }

    #[test]
    fn petal_schema_contains_bounds_field() {
        assert!(Petal::schema().contains("bounds ON TABLE petal TYPE option<geometry<polygon>>"));
    }

    #[test]
    fn petal_schema_contains_hexon_manifest_field() {
        let s = Petal::schema();
        assert!(s.contains("hexon_manifest ON TABLE petal"));
        assert!(s.contains("option<object>"));
    }

    #[test]
    fn petal_schema_contains_terrain_field() {
        let s = Petal::schema();
        assert!(s.contains("terrain ON TABLE petal"));
        assert!(s.contains("option<object>"));
    }

    // --- Room schema ---

    #[test]
    fn room_schema_contains_description_field() {
        assert!(Room::schema().contains("DEFINE FIELD IF NOT EXISTS description ON TABLE room"));
    }

    #[test]
    fn room_schema_contains_bounds_object_field() {
        let s = Room::schema();
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS bounds ON TABLE room"));
        assert!(s.contains("option<object>"));
    }

    #[test]
    fn room_schema_contains_spawn_point_field() {
        assert!(Room::schema().contains("DEFINE FIELD IF NOT EXISTS spawn_point ON TABLE room"));
    }

    // --- Model schema ---

    #[test]
    fn model_schema_contains_display_name() {
        assert!(Model::schema().contains("DEFINE FIELD IF NOT EXISTS display_name ON TABLE model"));
    }

    #[test]
    fn model_schema_contains_metadata_flexible() {
        let s = Model::schema();
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS metadata ON TABLE model"));
        assert!(s.contains("FLEXIBLE"));
    }

    #[test]
    fn model_schema_contains_tags_and_urls() {
        let s = Model::schema();
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS tags ON TABLE model"));
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS external_url ON TABLE model"));
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS config_url ON TABLE model"));
    }

    // --- Verse / VerseMember schema ---

    #[test]
    fn verse_member_schema_contains_status_assert() {
        let s = VerseMemberRow::schema();
        assert!(s.contains("status ON TABLE verse_member TYPE string"));
        assert!(s.contains("ASSERT"));
        assert!(s.contains("active"));
        assert!(s.contains("revoked"));
    }

    #[test]
    fn fractal_schema_contains_verse_id_field() {
        assert!(Fractal::schema()
            .contains("DEFINE FIELD IF NOT EXISTS verse_id ON TABLE fractal TYPE string"));
    }

    // --- Node schema ---

    #[test]
    fn node_schema_contains_petal_id_field() {
        let s = Node::schema();
        assert!(s.contains("petal_id") && s.contains("ON TABLE node") && s.contains("TYPE string"));
    }

    #[test]
    fn node_schema_contains_geometry_point() {
        assert!(Node::schema().contains("geometry<point>"));
    }

    #[test]
    fn node_schema_contains_durable_tombstone_field() {
        // FR-1: the soft-delete marker must be a real (optional) column so the
        // tombstone persists across reload rather than being a raw row drop.
        let s = Node::schema();
        assert!(s.contains("tombstone ON TABLE node"));
        assert!(s.contains("option<object>"));
    }

    // --- Asset schema ---

    #[test]
    fn asset_schema_contains_content_hash() {
        let s = Asset::schema();
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS content_hash"));
        assert!(s.contains("ON TABLE asset"));
        assert!(s.contains("TYPE option<string>"));
    }

    #[test]
    fn asset_data_is_optional() {
        assert!(Asset::schema().contains("data ON TABLE asset TYPE option<string>"));
    }

    // --- Verse namespace_id ---

    #[test]
    fn verse_schema_contains_namespace_id() {
        let s = Verse::schema();
        assert!(s.contains("DEFINE FIELD IF NOT EXISTS namespace_id"));
        assert!(s.contains("ON TABLE verse"));
        assert!(s.contains("TYPE option<string>"));
    }

    // --- Table trait conformance ---

    // --- IotReading schema ---

    #[test]
    fn iot_reading_schema_core_fields() {
        let s = IotReading::schema();
        assert!(s.contains("DEFINE TABLE IF NOT EXISTS iot_reading SCHEMAFULL"));
        assert!(s.contains("node_id ON TABLE iot_reading TYPE string"));
        assert!(s.contains("petal_id ON TABLE iot_reading TYPE string"));
        assert!(s.contains("metric ON TABLE iot_reading TYPE string"));
        assert!(s.contains("value ON TABLE iot_reading TYPE float"));
        assert!(s.contains("units ON TABLE iot_reading TYPE string DEFAULT ''"));
    }

    #[test]
    fn iot_reading_schema_timestamp_fields() {
        let s = IotReading::schema();
        assert!(s.contains("recorded_at ON TABLE iot_reading TYPE string"));
        assert!(s.contains("recorded_at_ms ON TABLE iot_reading TYPE int"));
        assert!(s.contains("hlc_timestamp ON TABLE iot_reading TYPE int"));
    }

    #[test]
    fn iot_reading_has_no_geometry_column() {
        // Position joins through the anchor node — see AGENTS.md §iot-readings.
        assert!(!IotReading::schema().contains("geometry"));
    }

    // --- Canonical-log tables ---

    #[test]
    fn verified_op_log_schema_carries_the_bytes_and_the_parent_index() {
        let s = VerifiedOpLog::schema();
        assert!(s.contains("DEFINE TABLE IF NOT EXISTS verified_op_log SCHEMAFULL"));
        assert!(s.contains("op_id_hex ON TABLE verified_op_log TYPE string"));
        assert!(s.contains("envelope_bytes ON TABLE verified_op_log TYPE string"));
        assert!(s.contains("parent_op_ids ON TABLE verified_op_log TYPE array<string>"));
        assert!(s.contains("author_public_key_hex ON TABLE verified_op_log TYPE string"));
        assert!(s.contains("wall_ms ON TABLE verified_op_log TYPE int"));
        assert!(s.contains("hlc_counter ON TABLE verified_op_log TYPE int"));
        assert!(s.contains("appended_at_hlc ON TABLE verified_op_log TYPE string DEFAULT ''"));
    }

    #[test]
    fn apply_marker_disposition_is_constrained_to_applied_or_excluded() {
        let s = MaterializerApplyMarker::schema();
        assert!(s.contains("marker_key ON TABLE materializer_apply_marker TYPE string"));
        assert!(s.contains("materializer_version ON TABLE materializer_apply_marker TYPE int"));
        assert!(s.contains("ASSERT $value = 'applied'"));
        assert!(s.contains("string::starts_with($value, 'excluded:')"));
    }

    #[test]
    fn scope_epoch_state_keeps_the_epoch_and_its_admitted_bump_evidence() {
        let s = ScopeEpochState::schema();
        assert!(s.contains("scope_key ON TABLE scope_epoch_state TYPE string"));
        assert!(s.contains("current_epoch ON TABLE scope_epoch_state TYPE int"));
        assert!(s.contains("admitted_bump_op_ids ON TABLE scope_epoch_state TYPE array<string>"));
    }

    #[test]
    fn migration_shadow_ledger_columns_match_the_contract_the_migration_module_codes_against() {
        // These names are a cross-module contract; renaming one silently breaks
        // `fe-database/src/migration/`, which writes these rows blind.
        let s = MigrationShadowLedger::schema();
        for column in [
            "entry_id",
            "run_id",
            "intent_digest_hex",
            "mutation_kind",
            "run_local_correlation_id",
            "member_op_ids_json",
            "candidate_byte_hashes_json",
            "disposition",
            "created_at",
        ] {
            assert!(
                s.contains(&format!("{column} ON TABLE migration_shadow_ledger")),
                "migration_shadow_ledger lost its {column} column"
            );
        }
        assert!(s.contains(
            "member_op_ids_json ON TABLE migration_shadow_ledger TYPE string DEFAULT '[]'"
        ));
        assert!(s.contains(
            "candidate_byte_hashes_json ON TABLE migration_shadow_ledger TYPE string DEFAULT '[]'"
        ));
    }

    #[test]
    fn the_verified_log_is_not_an_admin_clearable_table() {
        // SPEC-4 §1.4: the log is authority, the projection is derivative.
        assert!(!ALL_TABLE_NAMES.contains(&VerifiedOpLog::TABLE_NAME));
        assert!(!ALL_TABLE_NAMES.contains(&MaterializerApplyMarker::TABLE_NAME));
        assert!(!ALL_TABLE_NAMES.contains(&ScopeEpochState::TABLE_NAME));
    }

    #[test]
    fn all_table_names_are_present() {
        assert_eq!(ALL_TABLE_NAMES.len(), 15);
        assert!(ALL_TABLE_NAMES.contains(&"iot_reading"));
        assert!(ALL_TABLE_NAMES.contains(&"petal"));
        assert!(ALL_TABLE_NAMES.contains(&"verse_member"));
        assert!(ALL_TABLE_NAMES.contains(&"asset"));
        assert!(ALL_TABLE_NAMES.contains(&CrateRegistry::TABLE_NAME));
        assert!(ALL_TABLE_NAMES.contains(&CrateEntry::TABLE_NAME));
    }
}
