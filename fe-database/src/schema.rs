//! Table definitions for every SurrealDB table in the fractal-engine schema.
//!
//! Each table is defined exactly once via the [`define_table!`] macro, which
//! generates:
//!
//! 1. A `pub struct` with `Debug, Clone, Serialize, Deserialize`.
//! 2. An `impl Table` (see [`crate::repo::Table`]) providing `TABLE_NAME`,
//!    `ID_FIELD`, `schema()` (the SurrealQL DDL), and `id_value()`.
//!
//! The generated `schema()` string uses `DEFINE TABLE/FIELD IF NOT EXISTS`
//! so it is fully idempotent and safe to run on every startup.

/// Define a SurrealDB table as a Rust struct with auto-generated DDL.
///
/// # Syntax
///
/// ```ignore
/// define_table! {
///     /// Doc comment on the struct.
///     table "surreal_table_name" => RustStructName (id: id_field_name) {
///         field_a: String        => "TYPE string",
///         field_b: Option<String> => "TYPE option<string>",
///     }
/// }
/// ```
///
/// The right-hand side of `=>` for each field is the SurrealQL type clause
/// (everything after `ON TABLE <name>`).  It can include `ASSERT`, `VALUE`,
/// `DEFAULT`, and `FLEXIBLE` modifiers.
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
        bounds: Option<serde_json::Value> => "TYPE option<geometry<polygon>>"
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
        lamport_clock: i64              => "TYPE int",
        node_id:       String           => "TYPE string",
        op_type:       String           => "TYPE string",
        payload:       serde_json::Value => "TYPE object FLEXIBLE",
        sig:           String           => "TYPE string"
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
        created_at:   String                    => "TYPE string"
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
    Repo::<Asset>::apply_schema(db).await?;
    Ok(())
}

/// All table names, for admin operations like dump / clear.
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
        assert!(Fractal::schema().contains("DEFINE FIELD IF NOT EXISTS verse_id ON TABLE fractal TYPE string"));
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

    #[test]
    fn all_table_names_are_present() {
        assert_eq!(ALL_TABLE_NAMES.len(), 10);
        assert!(ALL_TABLE_NAMES.contains(&"petal"));
        assert!(ALL_TABLE_NAMES.contains(&"verse_member"));
        assert!(ALL_TABLE_NAMES.contains(&"asset"));
    }
}
