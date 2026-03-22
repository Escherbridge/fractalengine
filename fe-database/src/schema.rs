pub const DEFINE_PETAL: &str = "
    DEFINE TABLE IF NOT EXISTS petal SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS petal_id ON petal TYPE string;
    DEFINE FIELD IF NOT EXISTS name ON petal TYPE string;
    DEFINE FIELD IF NOT EXISTS node_id ON petal TYPE string;
    DEFINE FIELD IF NOT EXISTS created_at ON petal TYPE string;
";

pub const DEFINE_ROOM: &str = "
    DEFINE TABLE IF NOT EXISTS room SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS petal_id ON room TYPE string;
    DEFINE FIELD IF NOT EXISTS name ON room TYPE string;
";

pub const DEFINE_MODEL: &str = "
    DEFINE TABLE IF NOT EXISTS model SCHEMAFULL;
    DEFINE FIELD IF NOT EXISTS petal_id ON model TYPE string;
    DEFINE FIELD IF NOT EXISTS asset_id ON model TYPE string;
    DEFINE FIELD IF NOT EXISTS transform ON model TYPE object;
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
    DEFINE FIELD IF NOT EXISTS payload ON op_log TYPE object;
    DEFINE FIELD IF NOT EXISTS sig ON op_log TYPE string;
";
