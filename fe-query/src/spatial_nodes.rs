//! Generic spatial-node querying by type tag (T5 FR-6, N-10).
//!
//! Promoted stamp nodes (T2) and earthwork region nodes (T3) are ordinary
//! `node` rows distinguished by a `properties.node_kind` tag — so this crate
//! serves them through one generic abstraction without depending on the T2/T3
//! producers. The tag vocabulary + type-specific property keys form the JSON
//! contract those tracks write and this one reads. See `fe-query`'s notes and
//! `fe-api/AGENTS.md` §endpoint-surface.

use std::fmt;

/// Property key carrying a node's type tag (absent = plain node).
pub const KIND_KEY: &str = "node_kind";
/// Stamp property: owning path node id whose curve the stamp follows (D-A5).
pub const STAMP_PATH_ID_KEY: &str = "path_id";
/// Stamp property: zero-based instance index within the stamp group.
pub const STAMP_INSTANCE_INDEX_KEY: &str = "instance_index";
/// Earthwork property: cut volume in real cubic metres (D-A8, N-1).
pub const CUT_VOLUME_KEY: &str = "cut_volume_m3";
/// Earthwork property: fill volume in real cubic metres (D-A8, N-1).
pub const FILL_VOLUME_KEY: &str = "fill_volume_m3";
/// Earthwork property: material tag.
pub const MATERIAL_KEY: &str = "material";

/// A node's type tag (the value of `properties.node_kind`). Kept an enum so the
/// tag vocabulary is a single source of truth shared by read + egress paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A promoted stamped-asset instance (T2, D-A5).
    Stamp,
    /// A baked earthwork modification region (T3, D-A8).
    EarthworkRegion,
}

impl NodeKind {
    /// The `properties.node_kind` tag value.
    pub fn as_tag(self) -> &'static str {
        match self {
            NodeKind::Stamp => "stamp",
            NodeKind::EarthworkRegion => "earthwork_region",
        }
    }

    /// Parse a tag value back into a kind.
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "stamp" => Some(NodeKind::Stamp),
            "earthwork_region" => Some(NodeKind::EarthworkRegion),
            _ => None,
        }
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_tag())
    }
}

/// A parameterized generic-node query: SurrealQL text carrying `$petal_id` /
/// `$path_id` placeholders, plus the `(name, value)` id binds the caller must
/// supply to the driver. Ids are ALWAYS bound, never interpolated, so a hostile
/// id cannot break out of the statement (no SQL-injection surface). Tag values
/// (`node_kind`, `NodeKind::as_tag`) stay inline because they are compile-time
/// constants. Callers bind exactly like `fe-api`'s `load_node` (`.bind(...)`) —
/// see `fe-query`'s AGENTS.md §spatial-nodes.
pub type SpatialNodeQuery = (String, Vec<(String, String)>);

/// Every live node of a given kind in a petal (N-10, tombstone-filtered). The
/// petal id rides in the binds (`$petal_id`), never interpolated into the text.
pub fn nodes_of_kind_sql(petal_id: &str, kind: NodeKind) -> SpatialNodeQuery {
    (
        format!(
            "SELECT * FROM node WHERE petal_id = $petal_id AND properties.{KIND_KEY} = '{}' \
             AND tombstone = NONE ORDER BY created_at ASC",
            kind.as_tag(),
        ),
        vec![("petal_id".to_string(), petal_id.to_string())],
    )
}

/// All live stamp instances following a given path, in instance order
/// ("all stamps on path X" — FR-6 acceptance). The path id rides in the binds
/// (`$path_id`), never interpolated into the text.
pub fn stamps_on_path_sql(path_id: &str) -> SpatialNodeQuery {
    (
        format!(
            "SELECT * FROM node WHERE properties.{KIND_KEY} = '{}' AND properties.{STAMP_PATH_ID_KEY} = $path_id \
             AND tombstone = NONE ORDER BY properties.{STAMP_INSTANCE_INDEX_KEY} ASC",
            NodeKind::Stamp.as_tag(),
        ),
        vec![("path_id".to_string(), path_id.to_string())],
    )
}

/// Aggregate real-unit cut/fill across a petal's earthwork regions
/// ("total cut/fill in petal P" — FR-6 acceptance). Volumes are already in
/// real m³ (computed through the terrain scale authority, N-1). The petal id
/// rides in the binds (`$petal_id`), never interpolated into the text.
pub fn earthwork_volume_sql(petal_id: &str) -> SpatialNodeQuery {
    (
        format!(
            "SELECT math::sum(properties.{CUT_VOLUME_KEY}) AS total_cut_m3, \
             math::sum(properties.{FILL_VOLUME_KEY}) AS total_fill_m3, count() AS region_count \
             FROM node WHERE petal_id = $petal_id AND properties.{KIND_KEY} = '{}' \
             AND tombstone = NONE GROUP ALL",
            NodeKind::EarthworkRegion.as_tag(),
        ),
        vec![("petal_id".to_string(), petal_id.to_string())],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_tag_round_trips() {
        for k in [NodeKind::Stamp, NodeKind::EarthworkRegion] {
            assert_eq!(NodeKind::from_tag(k.as_tag()), Some(k));
        }
        assert_eq!(NodeKind::from_tag("unknown"), None);
    }

    #[test]
    fn nodes_of_kind_sql_binds_petal_id() {
        let (sql, binds) = nodes_of_kind_sql("p1", NodeKind::Stamp);
        assert_eq!(
            sql,
            "SELECT * FROM node WHERE petal_id = $petal_id AND properties.node_kind = 'stamp' \
             AND tombstone = NONE ORDER BY created_at ASC",
        );
        // The id is bound as `$petal_id`, never interpolated into the text.
        assert_eq!(binds, vec![("petal_id".to_string(), "p1".to_string())]);
        assert!(!sql.contains("'p1'"), "{sql}");
    }

    #[test]
    fn stamps_on_path_sql_orders_by_instance_index() {
        let (sql, binds) = stamps_on_path_sql("path-7");
        assert!(sql.contains("properties.node_kind = 'stamp'"), "{sql}");
        assert!(sql.contains("properties.path_id = $path_id"), "{sql}");
        assert!(
            sql.contains("ORDER BY properties.instance_index ASC"),
            "{sql}"
        );
        assert!(sql.contains("tombstone = NONE"), "{sql}");
        assert_eq!(binds, vec![("path_id".to_string(), "path-7".to_string())]);
        assert!(!sql.contains("path-7"), "{sql}");
    }

    #[test]
    fn earthwork_volume_sql_sums_real_units() {
        let (sql, binds) = earthwork_volume_sql("p1");
        assert!(
            sql.contains("math::sum(properties.cut_volume_m3) AS total_cut_m3"),
            "{sql}"
        );
        assert!(
            sql.contains("math::sum(properties.fill_volume_m3) AS total_fill_m3"),
            "{sql}"
        );
        assert!(
            sql.contains("properties.node_kind = 'earthwork_region'"),
            "{sql}"
        );
        assert!(sql.contains("petal_id = $petal_id"), "{sql}");
        assert!(sql.contains("GROUP ALL"), "{sql}");
        assert_eq!(binds, vec![("petal_id".to_string(), "p1".to_string())]);
    }

    #[test]
    fn ids_are_bound_not_interpolated() {
        // A hostile id with a quote must ride in the binds, never in the SQL
        // text, so it cannot break out of the statement (the old `esc()`
        // interpolation is gone — bind params are the injection defense).
        let hostile = "p1' OR '1'='1";

        let (sql, binds) = nodes_of_kind_sql(hostile, NodeKind::Stamp);
        assert!(!sql.contains(hostile), "id leaked into SQL text: {sql}");
        assert!(sql.contains("petal_id = $petal_id"), "{sql}");
        assert_eq!(binds, vec![("petal_id".to_string(), hostile.to_string())]);

        let (sql, binds) = stamps_on_path_sql(hostile);
        assert!(!sql.contains(hostile), "id leaked into SQL text: {sql}");
        assert!(sql.contains("properties.path_id = $path_id"), "{sql}");
        assert_eq!(binds, vec![("path_id".to_string(), hostile.to_string())]);

        let (sql, binds) = earthwork_volume_sql(hostile);
        assert!(!sql.contains(hostile), "id leaked into SQL text: {sql}");
        assert!(sql.contains("petal_id = $petal_id"), "{sql}");
        assert_eq!(binds, vec![("petal_id".to_string(), hostile.to_string())]);
    }
}
