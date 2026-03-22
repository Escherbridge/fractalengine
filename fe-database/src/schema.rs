pub const DEFINE_PETAL: &str = "
    DEFINE TABLE petal SCHEMAFULL PERMISSIONS
      FOR select WHERE $auth.role = 'public' OR $auth.petal_id = id
      FOR create, update, delete WHERE $auth.role = 'admin';
    DEFINE FIELD petal_id ON petal TYPE string;
    DEFINE FIELD name ON petal TYPE string;
    DEFINE FIELD node_id ON petal TYPE string;
    DEFINE FIELD created_at ON petal TYPE datetime;
";

pub const DEFINE_ROOM: &str = "/* DEFINE TABLE room SCHEMAFULL ... */";

pub const DEFINE_MODEL: &str = "/* DEFINE TABLE model SCHEMAFULL ... */";

pub const DEFINE_ROLE: &str = "/* DEFINE TABLE role SCHEMAFULL ... */";

pub const DEFINE_OP_LOG: &str = "
    DEFINE TABLE op_log SCHEMAFULL;
    DEFINE FIELD lamport_clock ON op_log TYPE int;
    DEFINE FIELD node_id ON op_log TYPE string;
    DEFINE FIELD op_type ON op_log TYPE string;
    DEFINE FIELD payload ON op_log TYPE object;
    DEFINE FIELD sig ON op_log TYPE bytes;
";
