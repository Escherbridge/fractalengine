/// Definition of a SurrealDB table with its fields.
#[derive(Debug, Clone)]
pub struct TableDef {
    pub name: &'static str,
    pub fields: Vec<FieldSchema>,
}

/// A single field in a table definition.
#[derive(Debug, Clone)]
pub struct FieldSchema {
    pub name: &'static str,
    pub surreal_type: &'static str,
    pub nullable: bool,
    pub default: Option<&'static str>,
    /// If true, uses FLEXIBLE TYPE instead of strict TYPE.
    pub flexible: bool,
}

/// An index definition on a table.
#[derive(Debug, Clone)]
pub struct IndexDef {
    pub name: &'static str,
    pub table: &'static str,
    pub fields: Vec<&'static str>,
    pub unique: bool,
}

/// Trait for types that define their own SurrealDB schema.
///
/// Implementing this trait on a marker struct produces self-describing
/// DDL that can be executed to create or migrate a table.
pub trait SchemaObject {
    fn table_def() -> TableDef;
    fn indexes() -> Vec<IndexDef>;

    /// Generate the full DDL (DEFINE TABLE + DEFINE FIELD + DEFINE INDEX)
    /// for this schema object.
    fn ddl() -> String {
        let table = Self::table_def();
        let mut lines = Vec::new();

        lines.push(format!(
            "DEFINE TABLE IF NOT EXISTS {} SCHEMAFULL;",
            table.name
        ));

        for field in &table.fields {
            let type_keyword = if field.flexible {
                "FLEXIBLE TYPE"
            } else {
                "TYPE"
            };

            let type_clause = if field.nullable {
                format!("{} option<{}>", type_keyword, field.surreal_type)
            } else {
                format!("{} {}", type_keyword, field.surreal_type)
            };

            let default_clause = match field.default {
                Some(d) => format!(" DEFAULT {}", d),
                None => String::new(),
            };

            lines.push(format!(
                "DEFINE FIELD IF NOT EXISTS {} ON {} {}{};",
                field.name, table.name, type_clause, default_clause
            ));
        }

        for idx in &Self::indexes() {
            let unique = if idx.unique { " UNIQUE" } else { "" };
            lines.push(format!(
                "DEFINE INDEX IF NOT EXISTS {} ON {} FIELDS {}{};",
                idx.name,
                idx.table,
                idx.fields.join(", "),
                unique
            ));
        }

        lines.join("\n")
    }
}

// ── Built-in schema objects for the FractalEngine hierarchy ──

/// Schema for the `verse` table.
pub struct VerseTable;

impl SchemaObject for VerseTable {
    fn table_def() -> TableDef {
        TableDef {
            name: "verse",
            fields: vec![
                FieldSchema { name: "verse_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "display_name", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "created_at", surreal_type: "datetime", nullable: false, default: Some("time::now()"), flexible: false },
            ],
        }
    }

    fn indexes() -> Vec<IndexDef> {
        vec![IndexDef {
            name: "idx_verse_id",
            table: "verse",
            fields: vec!["verse_id"],
            unique: true,
        }]
    }
}

/// Schema for the `fractal` table.
pub struct FractalTable;

impl SchemaObject for FractalTable {
    fn table_def() -> TableDef {
        TableDef {
            name: "fractal",
            fields: vec![
                FieldSchema { name: "fractal_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "verse_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "display_name", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "created_at", surreal_type: "datetime", nullable: false, default: Some("time::now()"), flexible: false },
            ],
        }
    }

    fn indexes() -> Vec<IndexDef> {
        vec![
            IndexDef { name: "idx_fractal_id", table: "fractal", fields: vec!["fractal_id"], unique: true },
            IndexDef { name: "idx_fractal_verse", table: "fractal", fields: vec!["verse_id"], unique: false },
        ]
    }
}

/// Schema for the `petal` table.
pub struct PetalTable;

impl SchemaObject for PetalTable {
    fn table_def() -> TableDef {
        TableDef {
            name: "petal",
            fields: vec![
                FieldSchema { name: "petal_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "fractal_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "display_name", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "created_at", surreal_type: "datetime", nullable: false, default: Some("time::now()"), flexible: false },
            ],
        }
    }

    fn indexes() -> Vec<IndexDef> {
        vec![
            IndexDef { name: "idx_petal_id", table: "petal", fields: vec!["petal_id"], unique: true },
            IndexDef { name: "idx_petal_fractal", table: "petal", fields: vec!["fractal_id"], unique: false },
        ]
    }
}

/// Schema for the `node` table.
pub struct NodeTable;

impl SchemaObject for NodeTable {
    fn table_def() -> TableDef {
        TableDef {
            name: "node",
            fields: vec![
                FieldSchema { name: "node_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "petal_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "display_name", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "asset_id", surreal_type: "string", nullable: true, default: None, flexible: false },
                FieldSchema { name: "position", surreal_type: "geometry<point>", nullable: false, default: None, flexible: false },
                FieldSchema { name: "elevation", surreal_type: "float", nullable: false, default: Some("0.0"), flexible: false },
                FieldSchema { name: "rotation", surreal_type: "array<float, 3>", nullable: false, default: None, flexible: false },
                FieldSchema { name: "scale", surreal_type: "array<float, 3>", nullable: false, default: None, flexible: false },
                FieldSchema { name: "interactive", surreal_type: "bool", nullable: false, default: Some("true"), flexible: false },
                FieldSchema { name: "created_at", surreal_type: "datetime", nullable: false, default: Some("time::now()"), flexible: false },
                FieldSchema { name: "edit_seq", surreal_type: "int", nullable: false, default: Some("0"), flexible: false },
                FieldSchema { name: "properties", surreal_type: "object", nullable: true, default: None, flexible: true },
            ],
        }
    }

    fn indexes() -> Vec<IndexDef> {
        vec![
            IndexDef { name: "idx_node_id", table: "node", fields: vec!["node_id"], unique: true },
            IndexDef { name: "idx_node_petal", table: "node", fields: vec!["petal_id"], unique: false },
        ]
    }
}

/// Schema for the `node_log` table.
pub struct NodeLogTable;

impl SchemaObject for NodeLogTable {
    fn table_def() -> TableDef {
        TableDef {
            name: "node_log",
            fields: vec![
                FieldSchema { name: "log_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "node_id", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "hlc_timestamp", surreal_type: "int", nullable: false, default: None, flexible: false },
                FieldSchema { name: "op", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "source_did", surreal_type: "string", nullable: false, default: None, flexible: false },
                FieldSchema { name: "payload", surreal_type: "object", nullable: true, default: None, flexible: true },
                FieldSchema { name: "row_version", surreal_type: "int", nullable: false, default: Some("0"), flexible: false },
                FieldSchema { name: "created_at", surreal_type: "datetime", nullable: false, default: Some("time::now()"), flexible: false },
            ],
        }
    }

    fn indexes() -> Vec<IndexDef> {
        vec![
            IndexDef { name: "idx_node_log_id", table: "node_log", fields: vec!["log_id"], unique: true },
            IndexDef { name: "idx_node_log_node", table: "node_log", fields: vec!["node_id"], unique: false },
            IndexDef { name: "idx_node_log_hlc", table: "node_log", fields: vec!["node_id", "hlc_timestamp"], unique: false },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_table_ddl() {
        let ddl = NodeTable::ddl();
        assert!(ddl.contains("DEFINE TABLE IF NOT EXISTS node SCHEMAFULL;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS node_id ON node TYPE string;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS petal_id ON node TYPE string;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS asset_id ON node TYPE option<string>;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS position ON node TYPE geometry<point>;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS elevation ON node TYPE float DEFAULT 0.0;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS interactive ON node TYPE bool DEFAULT true;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS properties ON node FLEXIBLE TYPE option<object>;"));
        assert!(ddl.contains("DEFINE INDEX IF NOT EXISTS idx_node_id ON node FIELDS node_id UNIQUE;"));
        assert!(ddl.contains("DEFINE INDEX IF NOT EXISTS idx_node_petal ON node FIELDS petal_id;"));
    }

    #[test]
    fn node_log_table_ddl() {
        let ddl = NodeLogTable::ddl();
        assert!(ddl.contains("DEFINE TABLE IF NOT EXISTS node_log SCHEMAFULL;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS payload ON node_log FLEXIBLE TYPE option<object>;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS row_version ON node_log TYPE int DEFAULT 0;"));
        assert!(ddl.contains("DEFINE INDEX IF NOT EXISTS idx_node_log_hlc ON node_log FIELDS node_id, hlc_timestamp;"));
    }

    #[test]
    fn verse_table_ddl() {
        let ddl = VerseTable::ddl();
        assert!(ddl.contains("DEFINE TABLE IF NOT EXISTS verse SCHEMAFULL;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS verse_id ON verse TYPE string;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS created_at ON verse TYPE datetime DEFAULT time::now();"));
        assert!(ddl.contains("DEFINE INDEX IF NOT EXISTS idx_verse_id ON verse FIELDS verse_id UNIQUE;"));
    }

    #[test]
    fn fractal_table_ddl() {
        let ddl = FractalTable::ddl();
        assert!(ddl.contains("DEFINE TABLE IF NOT EXISTS fractal SCHEMAFULL;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS verse_id ON fractal TYPE string;"));
        assert!(ddl.contains("DEFINE INDEX IF NOT EXISTS idx_fractal_verse ON fractal FIELDS verse_id;"));
    }

    #[test]
    fn petal_table_ddl() {
        let ddl = PetalTable::ddl();
        assert!(ddl.contains("DEFINE TABLE IF NOT EXISTS petal SCHEMAFULL;"));
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS fractal_id ON petal TYPE string;"));
        assert!(ddl.contains("DEFINE INDEX IF NOT EXISTS idx_petal_fractal ON petal FIELDS fractal_id;"));
    }

    #[test]
    fn nullable_field_wraps_option() {
        let ddl = NodeTable::ddl();
        // asset_id is nullable -> option<string>
        assert!(ddl.contains("TYPE option<string>"));
        // node_id is not nullable -> just TYPE string
        assert!(ddl.contains("DEFINE FIELD IF NOT EXISTS node_id ON node TYPE string;"));
    }

    #[test]
    fn flexible_field_keyword() {
        let ddl = NodeTable::ddl();
        // properties is flexible and nullable -> FLEXIBLE TYPE option<object>
        assert!(ddl.contains("FLEXIBLE TYPE option<object>"));
    }

    #[test]
    fn ddl_line_count_sanity() {
        let ddl = NodeTable::ddl();
        let lines: Vec<&str> = ddl.lines().collect();
        // 1 table + 12 fields + 2 indexes = 15 lines
        assert_eq!(lines.len(), 15);
    }
}
