pub const DEFINE_PETAL: &str = "
    DEFINE TABLE IF NOT EXISTS petal SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS petal_id ON petal TYPE string;
    DEFINE FIELD IF NOT EXISTS name ON petal TYPE string;
    DEFINE FIELD IF NOT EXISTS node_id ON petal TYPE string;
    DEFINE FIELD IF NOT EXISTS created_at ON petal TYPE string;
    DEFINE FIELD IF NOT EXISTS description ON TABLE petal TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS visibility ON TABLE petal TYPE string ASSERT $value IN ['public', 'private', 'unlisted'] VALUE $value OR 'private';
    DEFINE FIELD IF NOT EXISTS tags ON TABLE petal TYPE array<string> VALUE $value OR [];
";

pub const DEFINE_ROOM: &str = "
    DEFINE TABLE IF NOT EXISTS room SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS petal_id ON room TYPE string;
    DEFINE FIELD IF NOT EXISTS name ON room TYPE string;
    DEFINE FIELD IF NOT EXISTS description ON TABLE room TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS bounds ON TABLE room TYPE option<object> FLEXIBLE;
    DEFINE FIELD IF NOT EXISTS spawn_point ON TABLE room TYPE option<object> FLEXIBLE;
";

pub const DEFINE_MODEL: &str = "
    DEFINE TABLE IF NOT EXISTS model SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS petal_id ON model TYPE string;
    DEFINE FIELD IF NOT EXISTS asset_id ON model TYPE string;
    DEFINE FIELD IF NOT EXISTS transform ON model TYPE object FLEXIBLE;
    DEFINE FIELD IF NOT EXISTS display_name ON TABLE model TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS description ON TABLE model TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS external_url ON TABLE model TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS config_url ON TABLE model TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS tags ON TABLE model TYPE array<string> VALUE $value OR [];
    DEFINE FIELD IF NOT EXISTS metadata ON TABLE model TYPE option<object> FLEXIBLE;
";

pub const DEFINE_ROLE: &str = "
    DEFINE TABLE IF NOT EXISTS role SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS node_id ON role TYPE string;
    DEFINE FIELD IF NOT EXISTS petal_id ON role TYPE string;
    DEFINE FIELD IF NOT EXISTS role ON role TYPE string;
";

pub const DEFINE_OP_LOG: &str = "
    DEFINE TABLE IF NOT EXISTS op_log SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS lamport_clock ON op_log TYPE int;
    DEFINE FIELD IF NOT EXISTS node_id ON op_log TYPE string;
    DEFINE FIELD IF NOT EXISTS op_type ON op_log TYPE string;
    DEFINE FIELD IF NOT EXISTS payload ON op_log TYPE object FLEXIBLE;
    DEFINE FIELD IF NOT EXISTS sig ON op_log TYPE string;
";

// --- Verse / Fractal / Node schema (Verse → Fractal → Petal → Node hierarchy) ---

pub const DEFINE_VERSE: &str = "
    DEFINE TABLE IF NOT EXISTS verse SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS verse_id ON verse TYPE string;
    DEFINE FIELD IF NOT EXISTS name ON verse TYPE string;
    DEFINE FIELD IF NOT EXISTS created_by ON verse TYPE string;
    DEFINE FIELD IF NOT EXISTS created_at ON verse TYPE string;
";

pub const DEFINE_VERSE_MEMBER: &str = "
    DEFINE TABLE IF NOT EXISTS verse_member SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS member_id ON verse_member TYPE string;
    DEFINE FIELD IF NOT EXISTS verse_id ON verse_member TYPE string;
    DEFINE FIELD IF NOT EXISTS peer_did ON verse_member TYPE string;
    DEFINE FIELD IF NOT EXISTS status ON verse_member TYPE string
        ASSERT $value IN ['active', 'revoked'];
    DEFINE FIELD IF NOT EXISTS invited_by ON verse_member TYPE string;
    DEFINE FIELD IF NOT EXISTS invite_sig ON verse_member TYPE string;
    DEFINE FIELD IF NOT EXISTS invite_timestamp ON verse_member TYPE string;
    DEFINE FIELD IF NOT EXISTS revoked_at ON verse_member TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS revoked_by ON verse_member TYPE option<string>;
";

pub const DEFINE_FRACTAL: &str = "
    DEFINE TABLE IF NOT EXISTS fractal SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS fractal_id ON fractal TYPE string;
    DEFINE FIELD IF NOT EXISTS verse_id ON fractal TYPE string;
    DEFINE FIELD IF NOT EXISTS owner_did ON fractal TYPE string;
    DEFINE FIELD IF NOT EXISTS name ON fractal TYPE string;
    DEFINE FIELD IF NOT EXISTS description ON fractal TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS created_at ON fractal TYPE string;
";

/// Node: interactive object within a Petal.
/// position is a GeoJSON Point used for 2D spatial queries on the XZ plane (X=lng, Z=lat).
/// elevation stores the Y-axis height separately.
/// SurrealDB geometry<point> fields support INSIDE/INTERSECTS natively without a special index.
pub const DEFINE_NODE: &str = "
    DEFINE TABLE IF NOT EXISTS node SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS node_id      ON TABLE node TYPE string;
    DEFINE FIELD IF NOT EXISTS petal_id     ON TABLE node TYPE string;
    DEFINE FIELD IF NOT EXISTS display_name ON TABLE node TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS asset_id     ON TABLE node TYPE option<string>;
    DEFINE FIELD IF NOT EXISTS position     ON TABLE node TYPE geometry<point>;
    DEFINE FIELD IF NOT EXISTS elevation    ON TABLE node TYPE float DEFAULT 0.0;
    DEFINE FIELD IF NOT EXISTS rotation     ON TABLE node TYPE array;
    DEFINE FIELD IF NOT EXISTS scale        ON TABLE node TYPE array;
    DEFINE FIELD IF NOT EXISTS interactive  ON TABLE node TYPE bool DEFAULT false;
    DEFINE FIELD IF NOT EXISTS created_at   ON TABLE node TYPE string;
";

/// Binary asset store for GLTF/GLB models and other media.
/// data is the base64-encoded file content.
pub const DEFINE_ASSET: &str = "
    DEFINE TABLE IF NOT EXISTS asset SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS asset_id      ON TABLE asset TYPE string;
    DEFINE FIELD IF NOT EXISTS name          ON TABLE asset TYPE string;
    DEFINE FIELD IF NOT EXISTS content_type  ON TABLE asset TYPE string;
    DEFINE FIELD IF NOT EXISTS size_bytes    ON TABLE asset TYPE int;
    DEFINE FIELD IF NOT EXISTS data          ON TABLE asset TYPE string;
    DEFINE FIELD IF NOT EXISTS created_at    ON TABLE asset TYPE string;
";

/// Add fractal_id FK to petal (optional so existing petals don't break).
pub const DEFINE_PETAL_FRACTAL_ID: &str = "
    DEFINE FIELD IF NOT EXISTS fractal_id ON petal TYPE option<string>;
";

/// Add geometry<polygon> bounds to petal for spatial containment queries.
/// Enables: SELECT * FROM node WHERE position INSIDE (SELECT bounds FROM petal WHERE petal_id = $id)
pub const DEFINE_PETAL_BOUNDS: &str = "
    DEFINE FIELD IF NOT EXISTS bounds ON TABLE petal TYPE option<geometry<polygon>>;
";

#[cfg(test)]
mod tests {
    use super::*;

    // --- Petal schema tests (Task 1.2) ---

    #[test]
    fn petal_schema_contains_description_field() {
        assert!(DEFINE_PETAL.contains("DEFINE FIELD IF NOT EXISTS description ON TABLE petal"));
    }

    #[test]
    fn petal_schema_contains_visibility_field_with_assert() {
        assert!(DEFINE_PETAL.contains("DEFINE FIELD IF NOT EXISTS visibility ON TABLE petal"));
        assert!(DEFINE_PETAL.contains("ASSERT"));
        assert!(DEFINE_PETAL.contains("public"));
        assert!(DEFINE_PETAL.contains("private"));
        assert!(DEFINE_PETAL.contains("unlisted"));
    }

    #[test]
    fn petal_schema_contains_tags_array_field() {
        assert!(DEFINE_PETAL.contains("DEFINE FIELD IF NOT EXISTS tags ON TABLE petal"));
        assert!(DEFINE_PETAL.contains("array<string>"));
    }

    // --- Room schema tests (Task 1.3) ---

    #[test]
    fn room_schema_contains_description_field() {
        assert!(DEFINE_ROOM.contains("DEFINE FIELD IF NOT EXISTS description ON TABLE room"));
    }

    #[test]
    fn room_schema_contains_bounds_object_field() {
        assert!(DEFINE_ROOM.contains("DEFINE FIELD IF NOT EXISTS bounds ON TABLE room"));
        assert!(DEFINE_ROOM.contains("option<object>"));
    }

    #[test]
    fn room_schema_contains_spawn_point_field() {
        assert!(DEFINE_ROOM.contains("DEFINE FIELD IF NOT EXISTS spawn_point ON TABLE room"));
    }

    // --- Model schema tests (Task 1.4) ---

    #[test]
    fn model_schema_contains_display_name() {
        assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS display_name ON TABLE model"));
    }

    #[test]
    fn model_schema_contains_metadata_flexible() {
        assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS metadata ON TABLE model"));
        assert!(DEFINE_MODEL.contains("FLEXIBLE"));
    }

    #[test]
    fn model_schema_contains_tags_and_urls() {
        assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS tags ON TABLE model"));
        assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS external_url ON TABLE model"));
        assert!(DEFINE_MODEL.contains("DEFINE FIELD IF NOT EXISTS config_url ON TABLE model"));
    }

    // --- Verse schema tests ---

    #[test]
    fn verse_schema_contains_status_assert() {
        assert!(DEFINE_VERSE_MEMBER.contains("status ON verse_member TYPE string"));
        assert!(DEFINE_VERSE_MEMBER.contains("ASSERT"));
        assert!(DEFINE_VERSE_MEMBER.contains("active"));
        assert!(DEFINE_VERSE_MEMBER.contains("revoked"));
    }

    #[test]
    fn fractal_schema_contains_verse_id_field() {
        assert!(DEFINE_FRACTAL.contains("DEFINE FIELD IF NOT EXISTS verse_id ON fractal TYPE string"));
    }

    #[test]
    fn node_schema_contains_petal_id_field() {
        assert!(DEFINE_NODE.contains("petal_id") && DEFINE_NODE.contains("ON TABLE node") && DEFINE_NODE.contains("TYPE string"));
    }

    #[test]
    fn petal_schema_contains_fractal_id_field() {
        assert!(DEFINE_PETAL_FRACTAL_ID.contains("fractal_id ON petal TYPE option<string>"));
    }
}
