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
        terrain: Option<serde_json::Value> => "TYPE option<object> FLEXIBLE",
        /// Visibility-Group registry `[{id, name, visible, order}]` (VIS-PH4, D-3).
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility_groups: Option<serde_json::Value> => "TYPE option<array<object>> FLEXIBLE",
        /// Persisted petal eye (VIS-PH4); `NONE` = visible.
        #[serde(skip_serializing_if = "Option::is_none")]
        view_hidden: Option<bool> => "TYPE option<bool>",
        /// Durable per-petal residency horizon (p2p_asset_streaming S6);
        /// `NONE` = inherit the global `AppSettings.render_distance`. The
        /// `50000` ceiling mirrors `fe-ui`'s `RENDER_DISTANCE_MAX` — see
        /// `src/AGENTS.md` §app-settings.
        #[serde(skip_serializing_if = "Option::is_none")]
        render_distance: Option<f64> => "TYPE option<float> ASSERT $value = NONE OR ($value >= 0 AND $value <= 50000)"
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
    /// Materialized; written only by `role_grants::rematerialize` (src/AGENTS.md §role-propagation).
    table "role" => Role (id: peer_did) {
        peer_did: String => "TYPE string",
        scope:    String => "TYPE string",
        role:     String => "TYPE string"
    }
}

define_table! {
    /// An ADMITTED role statement: one live row per (scope, peer_did, grantor_did)
    /// across this table and `role_statement_pending` (src/AGENTS.md §role-propagation).
    table "role_grant" => RoleGrantRow (id: statement_key) {
        /// The doc record id `{scope}/{peer_did}/{grantor_did}`.
        statement_key: String => "TYPE string",
        verse_id:      String => "TYPE string",
        scope:         String => "TYPE string",
        peer_did:      String => "TYPE string",
        grantor_did:   String => "TYPE string",
        role:          String => "TYPE string ASSERT $value IN ['manager', 'editor', 'viewer', 'none']",
        revoked:       bool   => "TYPE bool",
        /// Grantor HLC as a decimal u64 string (the wire range exceeds SurrealDB's i64).
        hlc:           String => "TYPE string",
        updated_at:    String => "TYPE string"
    }
}

define_table! {
    /// A DEFERRED (held) role statement whose grantor lacks authority yet (R7, D-RP-1).
    table "role_statement_pending" => RoleStatementPendingRow (id: statement_key) {
        statement_key: String => "TYPE string",
        verse_id:      String => "TYPE string",
        scope:         String => "TYPE string",
        peer_did:      String => "TYPE string",
        grantor_did:   String => "TYPE string",
        role:          String => "TYPE string ASSERT $value IN ['manager', 'editor', 'viewer', 'none']",
        revoked:       bool   => "TYPE bool",
        hlc:           String => "TYPE string",
        updated_at:    String => "TYPE string",
        received_at:   String => "TYPE string"
    }
}

define_table! {
    /// One-shot local data-migration markers (the RP-2 legacy role migration).
    table "local_migration_marker" => LocalMigrationMarker (id: marker_key) {
        marker_key: String => "TYPE string",
        applied_at: String => "TYPE string"
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
        default_access: String         => "TYPE string DEFAULT 'viewer'",
        /// M2/F6 (A13): timeseries replication mode — mirror|sharded|balanced.
        ts_mode:                 String => "TYPE string DEFAULT 'mirror'",
        /// M2/F6: durability slider R (1..N); hosts per shard in balanced mode.
        ts_replication_factor:   i64    => "TYPE int DEFAULT 1",
        /// M2/F6: shard bucket width (ms), epoch-aligned; default 1 day.
        ts_bucket_width_ms:       i64    => "TYPE int DEFAULT 86400000",
        /// Persisted verse eye (VIS-PH4, D-1); `NONE` = visible.
        #[serde(skip_serializing_if = "Option::is_none")]
        view_hidden: Option<bool> => "TYPE option<bool>"
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
        created_at:  String         => "TYPE string",
        /// Persisted fractal eye (VIS-PH4, D-1); `NONE` = visible.
        #[serde(skip_serializing_if = "Option::is_none")]
        view_hidden: Option<bool> => "TYPE option<bool>"
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
        content_hash: Option<String> => "TYPE option<string>",
        /// Uploader DID (API token `sub`); `NONE` on legacy / non-API rows.
        /// Reach + quota rules: fe-api/AGENTS.md §assets, §asset-ingest.
        owner_did:    Option<String> => "TYPE option<string>",
        /// Anchor petal the upload was authorized against; `NONE` on legacy rows.
        petal_id:     Option<String> => "TYPE option<string>"
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
        source_did:     String => "TYPE string DEFAULT ''",
        /// R5 "decoded from / sent with this payload" link (CDL-SPEC10 A3):
        /// the `device_payload.payload_id` this reading travelled with, if
        /// any. Additive (`option<string>`, no DEFAULT) so an old peer's
        /// SCHEMAFULL row coercion is unaffected by its absence.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload_id: Option<String> => "TYPE option<string>"
    }
}

define_table! {
    /// R5 declared device payload format descriptor (CDL-SPEC10 A3,
    /// SPEC-10 §2.2) — immutable per `(verse_id, format_id)`; re-declaring
    /// with a different `mime` is a conflict (handlers/device_payload.rs).
    table "device_format" => DeviceFormat (id: format_id) {
        format_id:   String                    => "TYPE string",
        verse_id:    String                    => "TYPE string",
        mime:        String                    => "TYPE string",
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<serde_json::Value>      => "TYPE option<object> FLEXIBLE",
        declared_by: String                    => "TYPE string",
        created_at:  String                    => "TYPE string"
    }
}

define_table! {
    /// R5 device payload: an anchor node, a declared format and an ORDERED
    /// list of already-stored `asset` ids (R4 D-5: "allow for multiple
    /// records (a list of records)"). Replicates as Timeseries (union,
    /// `payload_id` key) — see `replication_mode.rs`.
    table "device_payload" => DevicePayload (id: payload_id) {
        payload_id:     String      => "TYPE string",
        node_id:        String      => "TYPE string",
        petal_id:       String      => "TYPE string",
        /// `NONE` for a row created with no verse context (durable-only,
        /// mirrors `iot_reading`'s `verse_id: Option<&str>` emit gate).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verse_id:       Option<String> => "TYPE option<string>",
        format_id:      String      => "TYPE string",
        recorded_at:    String      => "TYPE string",
        recorded_at_ms: i64         => "TYPE int",
        hlc_timestamp:  i64         => "TYPE int",
        /// Ordered, deduplicated-by-caller asset ids; order is preserved
        /// (R5: the list IS the payload, not a set).
        asset_ids:      Vec<String> => "TYPE array<string> VALUE $value OR []",
        source_did:     String      => "TYPE string DEFAULT ''"
    }
}

define_table! {
    /// SPEC-10 §3.4 rule 5 window-membership record (CDL-SPEC10 A3, A1
    /// hand-back item 3): written ONLY by the two local ingest doors
    /// (`insert_readings_with_replication`, `insert_device_payloads`),
    /// immediately after their durable insert — NEVER by the inbound
    /// replication path (`handlers/replicated_row.rs`), even for a
    /// replicated row that names this node's own DID as `source_did`. That
    /// asymmetry is the whole point: a peer that could get a replicated row
    /// into this table would be able to get this node to sign a batch
    /// containing data it never locally admitted (a signing oracle). B3's
    /// sealer reads ONLY this table to decide batch membership. Local-only
    /// by construction: never added to `ALL_TABLE_NAMES`'s replicated set or
    /// to `replication_mode.rs` (`fe-database/src/AGENTS.md`
    /// §window-membership).
    table "observation_window_member" => ObservationWindowMember (id: id) {
        id:            String => "TYPE string",
        /// The hot table the member row lives in (`"iot_reading"` |
        /// `"device_payload"`).
        table:         String => "TYPE string ASSERT $value IN ['iot_reading', 'device_payload']",
        /// That row's own id (`reading_id` | `payload_id`).
        row_id:        String => "TYPE string",
        petal_id:      String => "TYPE string",
        /// Caller-declared source class at ingest (SPEC-10 §7.1 rule 6) —
        /// recorded here, never inferred, so B3's sealer can build the G6
        /// `ObservationSource` set without re-deriving it from `source_did`.
        source_class:  String => "TYPE string",
        /// Ingestion-HLC wall-time bucket start (ms) of the sealing window
        /// (SPEC-10 §3.4 rule 2; `canon_log/AGENTS.md` §observation).
        window_bucket: i64    => "TYPE int",
        hlc_wall_ms:   i64    => "TYPE int",
        hlc_counter:   i64    => "TYPE int",
        /// Hex `op_id` of the kind-5 batch that sealed this row (CDL-SPEC10
        /// B3); `NONE` while pending. Claimed before the append, released if
        /// the append never lands (`canon_log/AGENTS.md` §observation).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sealed_op_id: Option<String> => "TYPE option<string>",
        /// Why this accepted row can never be sealed (SPEC-10 §8 rule 3,
        /// `legacy_succeeded_without_candidate`); `NONE` when coverable.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uncovered: Option<String> => "TYPE option<string>",
        /// Hex BLAKE3 commitment to the exact observation the door accepted
        /// (`canon_log/AGENTS.md` §observation, door digest): the sealer signs
        /// only a hot row that still matches it, so a row deleted and then
        /// re-supplied by replication is never signed. `NONE` = unbound.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        door_digest: Option<String> => "TYPE option<string>"
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
        default_val:  Option<serde_json::Value> => "TYPE any FLEXIBLE",
        #[serde(default, skip_serializing_if = "Option::is_none")]
        enforced_schema: Option<serde_json::Value> => "TYPE option<object> FLEXIBLE",
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema_revisions: Option<serde_json::Value> => "TYPE option<array> FLEXIBLE",
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

define_table! {
    /// SPEC-3 §2.4 capability chains this process holds, by `chain_id` (W4-7). Local-only and
    /// never replicated; `chain_bytes` is the authority, the other columns are lookup aids
    /// re-derived on load (`canon_log/AGENTS.md` §durable-authority-sources).
    table "canon_capability_chain" => CanonCapabilityChain (id: chain_id_hex) {
        chain_id_hex:      String => "TYPE string",
        leaf_audience_did: String => "TYPE string",
        epoch_scope_key:   String => "TYPE string",
        /// Standard base64 of the exact canonical chain bytes.
        chain_bytes:       String => "TYPE string",
    }
}

define_table! {
    /// Local-only content-addressed bytes: sealed SPEC-6 artifacts and admitted payload
    /// ciphertexts, keyed by `BLAKE3(bytes)` (W4-7). Never the iroh blob store.
    table "canon_sealed_artifact" => CanonSealedArtifact (id: artifact_id_hex) {
        artifact_id_hex: String => "TYPE string",
        content_class:   String => "TYPE string ASSERT $value IN ['sealed_artifact', 'payload_ciphertext']",
        /// Standard base64 of the exact bytes.
        content_bytes:   String => "TYPE string",
    }
}

define_table! {
    /// The NON-SECRET scope-key custody index (W4-7): which `(scope, epoch)` keys the OS
    /// keystore holds, by committed `key_id`. Key material never enters this table.
    table "canon_scope_key_index" => CanonScopeKeyIndex (id: index_key) {
        /// `{scope_key}:{epoch}`.
        index_key:        String => "TYPE string",
        scope_key:        String => "TYPE string",
        epoch:            i64    => "TYPE int",
        key_id_hex:       String => "TYPE string",
        state:            String => "TYPE string ASSERT $value IN ['held', 'issuance_stopped', 'destroyed']",
        destroyed_at_hlc: String => "TYPE string DEFAULT ''",
        note:             String => "TYPE string DEFAULT ''",
    }
}

define_table! {
    /// SPEC-3 §10.2.1 self-signed device enrolments and revocations (owner R2, W4-8), one row
    /// per (principal, device) record id. Replicated as an ordinary row; `artifact_cbor` is the
    /// authority and every other column is re-derived from it (`canon_log/AGENTS.md`
    /// §device-enrolment).
    table "canon_device_enrolment" => CanonDeviceEnrolment (id: record_id) {
        record_id:      String => "TYPE string",
        principal_did:  String => "TYPE string",
        device_key_hex: String => "TYPE string",
        /// Standard base64 of the exact canonical artifact bytes.
        artifact_cbor:  String => "TYPE string",
        revoked:        bool   => "TYPE bool",
    }
}

define_table! {
    /// SPEC-10 §4.4 kind-5 reference row, written ONLY by
    /// `ObservationBatchMaterializer` through the apply-marker commit; the field set is exactly
    /// `ObservationBatchReference::to_row` after `cbor_to_json` (byte fields as lowercase hex).
    /// Derived from `verified_op_log`, so not admin-clearable (`canon_log/AGENTS.md`
    /// §observation).
    table "observation_batch" => ObservationBatchRow (id: op_id) {
        op_id:                String      => "TYPE string",
        verse_id:             String      => "TYPE string",
        petal_id:             String      => "TYPE string",
        author_public_key:    String      => "TYPE string",
        hlc_wall_ms:          i64         => "TYPE int",
        hlc_counter:          i64         => "TYPE int",
        hexon_artifact_id:    String      => "TYPE string",
        hexon_stored_length:  i64         => "TYPE int",
        hlc_min_wall_ms:      i64         => "TYPE int",
        hlc_min_counter:      i64         => "TYPE int",
        hlc_max_wall_ms:      i64         => "TYPE int",
        hlc_max_counter:      i64         => "TYPE int",
        sensor_time_min_ms:   i64         => "TYPE int",
        sensor_time_max_ms:   i64         => "TYPE int",
        stats_hash:           String      => "TYPE string",
        count:                i64         => "TYPE int",
        anchors:              Vec<String> => "TYPE array<string>",
        asset_refs:           Vec<String> => "TYPE array<string>",
        /// Each source's canonical signed bytes, lowercase hex, in signed order.
        sources:              Vec<String> => "TYPE array<string>",
        hexon_format_version: i64         => "TYPE int",
        stats_stored_length:  i64         => "TYPE int",
    }
}

define_table! {
    /// SPEC-10 §5 hydrated catalog: one scalar reading per `reading_id`, the union of every
    /// hydrated batch (`op_ids` names each batch that carried an identical copy). Derived and
    /// rebuildable; local-only, never REST/MCP/BI-queryable (§5.1 rule 3).
    table "observation_catalog_reading" => ObservationCatalogReading (id: reading_id) {
        reading_id:     String      => "TYPE string",
        op_ids:         Vec<String> => "TYPE array<string>",
        verse_id:       String      => "TYPE string",
        petal_id:       String      => "TYPE string",
        anchor:         String      => "TYPE string",
        anchor_node_id: String      => "TYPE string",
        metric:         String      => "TYPE string",
        value:          f64         => "TYPE float",
        /// `f64::to_bits` as 16 lowercase hex digits: the exact IEEE-754 pattern (§8 rule 4).
        value_bits:     String      => "TYPE string",
        units:          String      => "TYPE string",
        recorded_at:    String      => "TYPE string",
        recorded_at_ms: i64         => "TYPE int",
        hlc_wall_ms:    i64         => "TYPE int",
        hlc_counter:    i64         => "TYPE int",
        #[serde(default, skip_serializing_if = "Option::is_none")]
        payload_id:     Option<String> => "TYPE option<string>",
        /// The row's `ObservationSource`, canonical bytes as lowercase hex.
        source:         String      => "TYPE string",
    }
}

define_table! {
    /// SPEC-10 §5 hydrated catalog: one device-payload reference per `payload_id`.
    table "observation_catalog_payload" => ObservationCatalogPayload (id: payload_id) {
        payload_id:           String      => "TYPE string",
        op_ids:               Vec<String> => "TYPE array<string>",
        verse_id:             String      => "TYPE string",
        petal_id:             String      => "TYPE string",
        anchor:               String      => "TYPE string",
        anchor_node_id:       String      => "TYPE string",
        format_id:            String      => "TYPE string",
        recorded_at:          String      => "TYPE string",
        recorded_at_ms:       i64         => "TYPE int",
        hlc_wall_ms:          i64         => "TYPE int",
        hlc_counter:          i64         => "TYPE int",
        asset_ids:            Vec<String> => "TYPE array<string>",
        asset_content_hashes: Vec<String> => "TYPE array<string>",
        source:               String      => "TYPE string",
    }
}

define_table! {
    /// SPEC-10 §5.3 rule 4 conflict evidence: an identifier two hydrated batches disagree on.
    /// Neither copy enters the catalog; this row names every batch involved.
    table "observation_catalog_conflict" => ObservationCatalogConflict (id: conflict_key) {
        /// `{observation_table}:{row_id}`.
        conflict_key:      String      => "TYPE string",
        observation_table: String      => "TYPE string ASSERT $value IN ['iot_reading', 'device_payload']",
        row_id:            String      => "TYPE string",
        op_ids:            Vec<String> => "TYPE array<string>",
    }
}

define_table! {
    /// SPEC-10 §5.3 rule 1: the one hydration outcome of each admitted kind-5 batch.
    table "observation_hydration" => ObservationHydration (id: op_id) {
        op_id:   String => "TYPE string",
        outcome: String => "TYPE string ASSERT $value IN ['hydrated', 'referenced_artifact_unavailable', 'observation_batch_invalid']",
        detail:  String => "TYPE string DEFAULT ''",
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
    // VIS-PH4: see AGENTS.md §schema-macro "array<object> FLEXIBLE" — the
    // auto-created element field must be re-declared FLEXIBLE (OVERWRITE is
    // idempotent; IF NOT EXISTS would silently skip the auto-created one).
    db.query("DEFINE FIELD OVERWRITE visibility_groups[*] ON TABLE petal TYPE object FLEXIBLE")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("petal.visibility_groups element: {e}"))?;
    Repo::<Room>::apply_schema(db).await?;
    Repo::<Model>::apply_schema(db).await?;
    Repo::<Role>::apply_schema(db).await?;
    Repo::<RoleGrantRow>::apply_schema(db).await?;
    Repo::<RoleStatementPendingRow>::apply_schema(db).await?;
    Repo::<LocalMigrationMarker>::apply_schema(db).await?;
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
    db.query("DEFINE FIELD OVERWRITE default_val ON TABLE field_def TYPE any FLEXIBLE")
        .await?
        .check()?;
    Repo::<NodeLog>::apply_schema(db).await?;
    Repo::<IotReading>::apply_schema(db).await?;
    Repo::<DeviceFormat>::apply_schema(db).await?;
    Repo::<DevicePayload>::apply_schema(db).await?;
    Repo::<ObservationWindowMember>::apply_schema(db).await?;
    Repo::<CrateRegistry>::apply_schema(db).await?;
    Repo::<CrateEntry>::apply_schema(db).await?;
    Repo::<VerifiedOpLog>::apply_schema(db).await?;
    Repo::<MaterializerApplyMarker>::apply_schema(db).await?;
    Repo::<ScopeEpochState>::apply_schema(db).await?;
    Repo::<MigrationShadowLedger>::apply_schema(db).await?;
    Repo::<CanonCapabilityChain>::apply_schema(db).await?;
    Repo::<CanonSealedArtifact>::apply_schema(db).await?;
    Repo::<CanonScopeKeyIndex>::apply_schema(db).await?;
    Repo::<CanonDeviceEnrolment>::apply_schema(db).await?;
    Repo::<ObservationBatchRow>::apply_schema(db).await?;
    Repo::<ObservationCatalogReading>::apply_schema(db).await?;
    Repo::<ObservationCatalogPayload>::apply_schema(db).await?;
    Repo::<ObservationCatalogConflict>::apply_schema(db).await?;
    Repo::<ObservationHydration>::apply_schema(db).await?;

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
    // Own-id point lookups (scope resolvers, existence oracle). Plain, not UNIQUE:
    // see src/AGENTS.md §schema-macro "Id lookup indexes".
    for (name, table, field) in [
        ("idx_petal_id", "petal", "petal_id"),
        ("idx_fractal_id", "fractal", "fractal_id"),
        ("idx_node_id", "node", "node_id"),
        ("idx_verse_id", "verse", "verse_id"),
        // Asset ownership (quota sums) + content-hash reads (`GET /assets/{hash}`, GC).
        ("idx_asset_owner", "asset", "owner_did"),
        ("idx_asset_content_hash", "asset", "content_hash"),
    ] {
        db.query(format!(
            "DEFINE INDEX IF NOT EXISTS {name} ON TABLE {table} FIELDS {field}"
        ))
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
    }
    db.query("DEFINE INDEX IF NOT EXISTS idx_role_scope ON TABLE role FIELDS scope")
        .await?
        .check()
        .map_err(|e| anyhow::anyhow!("idx_role_scope: {e}"))?;
    // RP-2 §2.1: the statement key is unique per table, and every held-row
    // lookup goes through (verse_id, grantor_did) — src/AGENTS.md §role-propagation.
    for table in ["role_grant", "role_statement_pending"] {
        for (suffix, fields, unique) in [
            ("key", "scope, peer_did, grantor_did", " UNIQUE"),
            ("verse_grantor", "verse_id, grantor_did", ""),
            ("verse", "verse_id", ""),
            // Every live-version lookup/delete is `WHERE statement_key = $k`; unindexed it
            // full-scans the held table per inbound statement (quadratic under a flood).
            ("statement", "statement_key", ""),
        ] {
            db.query(format!(
                "DEFINE INDEX IF NOT EXISTS idx_{table}_{suffix} ON TABLE {table} FIELDS {fields}{unique}"
            ))
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("idx_{table}_{suffix}: {e}"))?;
        }
    }
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
    db.query(
        "DEFINE INDEX IF NOT EXISTS idx_iot_reading_payload ON TABLE iot_reading FIELDS payload_id",
    )
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("idx_iot_reading_payload: {e}"))?;
    db.query(
        "DEFINE INDEX IF NOT EXISTS idx_device_format_verse ON TABLE device_format FIELDS verse_id",
    )
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("idx_device_format_verse: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_device_payload_petal ON TABLE device_payload FIELDS petal_id")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_device_payload_petal: {e}"))?;
    db.query(
        "DEFINE INDEX IF NOT EXISTS idx_device_payload_node ON TABLE device_payload FIELDS node_id",
    )
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("idx_device_payload_node: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_device_payload_format ON TABLE device_payload FIELDS format_id")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_device_payload_format: {e}"))?;
    // F23 precedent: idempotent-retry union key, storage-enforced (see the
    // iot_reading.reading_id UNIQUE rationale above).
    let unique_payload_idx = db
        .query("DEFINE INDEX IF NOT EXISTS idx_device_payload_payload_id ON TABLE device_payload FIELDS payload_id UNIQUE")
        .await
        .and_then(|r| r.check());
    if let Err(e) = unique_payload_idx {
        tracing::error!(
            "device_payload.payload_id UNIQUE index rejected ({e}) — falling back to the \
             non-unique index (the read-then-write applier remains the enforcement)"
        );
        db.query("DEFINE INDEX IF NOT EXISTS idx_device_payload_payload_id ON TABLE device_payload FIELDS payload_id")
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("idx_device_payload_payload_id: {e}"))?;
    }
    db.query("DEFINE INDEX IF NOT EXISTS idx_observation_window_member_bucket ON TABLE observation_window_member FIELDS petal_id, window_bucket")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_observation_window_member_bucket: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_observation_window_member_row ON TABLE observation_window_member FIELDS table, row_id")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_observation_window_member_row: {e}"))?;
    // F23 (M2 scrutiny F5 minor): the A12 union key, storage-enforced. The
    // inbound applier probes `reading_id` per replicated row and F20
    // reconciliation replays the full doc snapshot on every open — the
    // UNIQUE index backs that probe with an actual index and enforces the
    // union key at the storage layer as defense in depth under the
    // read-then-write applier (which itself cannot race: the DB thread is
    // the single writer, and all sanctioned writers only ever mint fresh
    // ULID reading_ids). UNIQUE validates EXISTING rows eagerly: a store
    // that somehow predates this index with duplicate reading_ids fails
    // the definition — fall back to the plain index with a LOUD error so
    // the node stays up (the read-then-write applier remains the
    // enforcement, and the operator learns) rather than failing startup.
    let unique_idx = db
        .query("DEFINE INDEX IF NOT EXISTS idx_iot_reading_reading_id ON TABLE iot_reading FIELDS reading_id UNIQUE")
        .await
        .and_then(|r| r.check());
    if let Err(e) = unique_idx {
        tracing::error!(
            "iot_reading.reading_id UNIQUE index rejected ({e}) — the store holds duplicate \
             reading_ids, so storage-level union-key enforcement is unavailable; falling back \
             to the non-unique index (the read-then-write applier remains the enforcement)"
        );
        db.query("DEFINE INDEX IF NOT EXISTS idx_iot_reading_reading_id ON TABLE iot_reading FIELDS reading_id")
            .await?
            .check()
            .map_err(|e| anyhow::anyhow!("idx_iot_reading_reading_id: {e}"))?;
    }
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
    db.query("DEFINE INDEX IF NOT EXISTS idx_canon_capability_chain_id ON TABLE canon_capability_chain FIELDS chain_id_hex UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_canon_capability_chain_id: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_canon_sealed_artifact_id ON TABLE canon_sealed_artifact FIELDS artifact_id_hex UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_canon_sealed_artifact_id: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_canon_scope_key_index_key ON TABLE canon_scope_key_index FIELDS index_key UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_canon_scope_key_index_key: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_canon_device_enrolment_record ON TABLE canon_device_enrolment FIELDS record_id UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_canon_device_enrolment_record: {e}"))?;
    // SPEC-10 §4.4 rule 1: one reference row per kind-5 operation (CDL-SPEC10 B3).
    db.query("DEFINE INDEX IF NOT EXISTS idx_observation_batch_op_id ON TABLE observation_batch FIELDS op_id UNIQUE")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_observation_batch_op_id: {e}"))?;
    db.query("DEFINE INDEX IF NOT EXISTS idx_observation_window_member_sealed ON TABLE observation_window_member FIELDS sealed_op_id")
        .await?.check().map_err(|e| anyhow::anyhow!("idx_observation_window_member_sealed: {e}"))?;

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
    DeviceFormat::TABLE_NAME,
    DevicePayload::TABLE_NAME,
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
        assert_eq!(ALL_TABLE_NAMES.len(), 17);
        assert!(ALL_TABLE_NAMES.contains(&"iot_reading"));
        assert!(ALL_TABLE_NAMES.contains(&"petal"));
        assert!(ALL_TABLE_NAMES.contains(&"verse_member"));
        assert!(ALL_TABLE_NAMES.contains(&"asset"));
        assert!(ALL_TABLE_NAMES.contains(&"device_format"));
        assert!(ALL_TABLE_NAMES.contains(&"device_payload"));
        assert!(ALL_TABLE_NAMES.contains(&CrateRegistry::TABLE_NAME));
        assert!(ALL_TABLE_NAMES.contains(&CrateEntry::TABLE_NAME));
    }

    #[test]
    fn observation_window_member_is_never_in_the_admin_clearable_or_replicated_set() {
        // SPEC-10 §3.4 rule 5 (A1 hand-back item 3): the window-membership
        // table is local-only by construction — it must never be reachable
        // from the admin dump/clear surface or the replicated-row path.
        assert!(!ALL_TABLE_NAMES.contains(&ObservationWindowMember::TABLE_NAME));
    }

    #[test]
    fn observation_plane_tables_are_never_admin_clearable() {
        // SPEC-10 §5.1 rule 3 / SPEC-4 §1.4 (CDL-SPEC10 B3): the reference projection and the
        // hydrated catalog are local-only and never on the admin dump/clear surface.
        for table in [
            ObservationBatchRow::TABLE_NAME,
            ObservationCatalogReading::TABLE_NAME,
            ObservationCatalogPayload::TABLE_NAME,
            ObservationCatalogConflict::TABLE_NAME,
            ObservationHydration::TABLE_NAME,
        ] {
            assert!(!ALL_TABLE_NAMES.contains(&table), "{table}");
        }
    }

    // --- DeviceFormat / DevicePayload schema ---

    #[test]
    fn device_format_schema_is_keyed_by_format_id_and_carries_verse_and_mime() {
        let s = DeviceFormat::schema();
        assert!(s.contains("DEFINE TABLE IF NOT EXISTS device_format SCHEMAFULL"));
        assert!(s.contains("format_id ON TABLE device_format TYPE string"));
        assert!(s.contains("verse_id ON TABLE device_format TYPE string"));
        assert!(s.contains("mime ON TABLE device_format TYPE string"));
        assert!(s.contains("schema ON TABLE device_format TYPE option<object>"));
    }

    #[test]
    fn device_payload_schema_carries_an_ordered_asset_id_array() {
        let s = DevicePayload::schema();
        assert!(s.contains("DEFINE TABLE IF NOT EXISTS device_payload SCHEMAFULL"));
        assert!(s.contains("asset_ids ON TABLE device_payload TYPE array<string>"));
        assert!(s.contains("node_id ON TABLE device_payload TYPE string"));
        assert!(s.contains("petal_id ON TABLE device_payload TYPE string"));
        assert!(s.contains("format_id ON TABLE device_payload TYPE string"));
    }

    #[test]
    fn observation_window_member_schema_constrains_table_to_the_two_hot_tables() {
        let s = ObservationWindowMember::schema();
        assert!(s.contains("DEFINE TABLE IF NOT EXISTS observation_window_member SCHEMAFULL"));
        assert!(s.contains("ASSERT $value IN ['iot_reading', 'device_payload']"));
        assert!(s.contains("source_class ON TABLE observation_window_member TYPE string"));
        assert!(s.contains("door_digest ON TABLE observation_window_member TYPE option<string>"));
    }

    #[test]
    fn iot_reading_schema_carries_an_optional_payload_id() {
        let s = IotReading::schema();
        assert!(s.contains("payload_id ON TABLE iot_reading TYPE option<string>"));
    }
}
