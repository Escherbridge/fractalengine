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
}
