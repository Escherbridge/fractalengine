//! Pure query-building, row-parsing, and bbox-filter logic for the GIS Query
//! panel. No `db_sender`/Bevy access here — see `crate::actions::gis` for the
//! I/O side (`DbCommand::RawQuery` submission). See `fe-ui/src/AGENTS.md`
//! §gis-query-ui for the "why".

use std::collections::HashMap;

use super::GisResultRow;

/// Reserved annotation-title/color property keys (mirrors `actions::node_props`,
/// duplicated here rather than pulled in as a dependency to keep this module
/// import-free of the `actions` layer — see AGENTS.md).
const ANNOTATION_TITLE_KEY: &str = "gis.annotation.title";
const ANNOTATION_COLOR_KEY: &str = "gis.annotation.color";
/// Mirrors `actions::node_props::TRACK_NAME_KEY` (duplicated for the same
/// import-free reason as the annotation keys above).
const TRACK_NAME_KEY: &str = "gis.track.name";

// ---------------------------------------------------------------------------
// Query builders (pure). Single SELECT, no `;`, no banned keywords — see
// fe-database's `RawQuery` security filter (fe-database/src/lib.rs). Bind
// vars carry all user-controlled values; only the compile-time-constant
// annotation key is inlined as a literal.
// ---------------------------------------------------------------------------

/// Builds the "nodes with annotations" query for a petal: every node whose
/// `gis.annotation.title` property is set. Also selects the annotation color
/// (may be `NONE`) for the Annotations tab's swatch.
pub(crate) fn annotation_query(petal_id: &str) -> (String, HashMap<String, serde_json::Value>) {
    let sql = format!(
        "SELECT node_id, display_name, position, elevation, properties[\"{title_key}\"] AS annotation_title, \
         properties[\"{color_key}\"] AS annotation_color \
         FROM node WHERE petal_id = $petal_id AND properties[\"{title_key}\"] != NONE",
        title_key = ANNOTATION_TITLE_KEY,
        color_key = ANNOTATION_COLOR_KEY,
    );
    let mut vars = HashMap::new();
    vars.insert("petal_id".to_string(), serde_json::Value::String(petal_id.to_string()));
    (sql, vars)
}

/// Builds the "track nodes" query for a petal: every node whose
/// `gis.track.name` property is set. Reuses `annotation_title` as the row's
/// display slot for the track name (via `parse_gis_row`'s `matched_value`/
/// `annotation_title` fallback chain).
pub(crate) fn track_query(petal_id: &str) -> (String, HashMap<String, serde_json::Value>) {
    let sql = format!(
        "SELECT node_id, display_name, position, elevation, properties[\"{name_key}\"] AS annotation_title \
         FROM node WHERE petal_id = $petal_id AND properties[\"{name_key}\"] != NONE",
        name_key = TRACK_NAME_KEY,
    );
    let mut vars = HashMap::new();
    vars.insert("petal_id".to_string(), serde_json::Value::String(petal_id.to_string()));
    (sql, vars)
}

/// Builds the property key/value filter query for a petal: nodes whose
/// `properties[key] == value` (equality only).
pub(crate) fn property_filter_query(
    petal_id: &str,
    key: &str,
    value: serde_json::Value,
) -> (String, HashMap<String, serde_json::Value>) {
    let sql = "SELECT node_id, display_name, position, elevation, properties[$prop_key] AS matched_value \
               FROM node WHERE petal_id = $petal_id AND properties[$prop_key] = $prop_val"
        .to_string();
    let mut vars = HashMap::new();
    vars.insert("petal_id".to_string(), serde_json::Value::String(petal_id.to_string()));
    vars.insert("prop_key".to_string(), serde_json::Value::String(key.to_string()));
    vars.insert("prop_val".to_string(), value);
    (sql, vars)
}

/// Converts the property-filter value text buffer to a typed JSON value,
/// mirroring the Add Property row's type coercion in `panels::inspector`.
pub(crate) fn parse_filter_value(value_type: &str, raw: &str) -> serde_json::Value {
    match value_type {
        "number" => raw
            .parse::<f64>()
            .map(serde_json::Value::from)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string())),
        "bool" => match raw.to_lowercase().as_str() {
            "true" | "1" | "yes" => serde_json::Value::Bool(true),
            "false" | "0" | "no" => serde_json::Value::Bool(false),
            _ => serde_json::Value::String(raw.to_string()),
        },
        _ => serde_json::Value::String(raw.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Row parsing (pure) — shapes a `DbResult::QueryResult { data }` array into
// `GisResultRow`s.
// ---------------------------------------------------------------------------

/// Parses `RawQuery` result rows (as delivered in `DbResult::QueryResult`)
/// into result rows, silently skipping malformed rows (missing `node_id`).
pub(crate) fn parse_gis_rows(data: &[serde_json::Value]) -> Vec<GisResultRow> {
    data.iter().filter_map(parse_gis_row).collect()
}

fn parse_gis_row(v: &serde_json::Value) -> Option<GisResultRow> {
    let node_id = v.get("node_id")?.as_str()?.to_string();
    let name = v
        .get("display_name")
        .and_then(|n| n.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| node_id.clone());
    let elevation = v.get("elevation").and_then(|e| e.as_f64()).unwrap_or(0.0) as f32;
    let (x, z) = extract_xz(v.get("position"));
    let annotation_title = v
        .get("annotation_title")
        .and_then(|a| a.as_str())
        .or_else(|| v.get("matched_value").and_then(|a| a.as_str()))
        .map(str::to_string);
    let annotation_color = v.get("annotation_color").and_then(|a| a.as_str()).map(str::to_string);
    Some(GisResultRow { node_id, name, position: [x, elevation, z], annotation_title, annotation_color })
}

/// Extracts `(x, z)` from a node's `position` field. Handles both the
/// GeoJSON Point shape (`{"type":"Point","coordinates":[x,z]}`, per
/// fe-database's `node.position` schema comment) and a bare `[x, z]` array,
/// defensively, since SurrealDB's raw-query serialization of `geometry<point>`
/// isn't pinned down by an existing fe-ui precedent.
fn extract_xz(pos: Option<&serde_json::Value>) -> (f32, f32) {
    let Some(pos) = pos else { return (0.0, 0.0) };
    if let Some(coords) = pos.get("coordinates").and_then(|c| c.as_array()) {
        let x = coords.first().and_then(|n| n.as_f64()).unwrap_or(0.0) as f32;
        let z = coords.get(1).and_then(|n| n.as_f64()).unwrap_or(0.0) as f32;
        return (x, z);
    }
    if let Some(arr) = pos.as_array() {
        let x = arr.first().and_then(|n| n.as_f64()).unwrap_or(0.0) as f32;
        let z = arr.get(1).and_then(|n| n.as_f64()).unwrap_or(0.0) as f32;
        return (x, z);
    }
    (0.0, 0.0)
}

// ---------------------------------------------------------------------------
// Bbox mode (pure, client-side — no DB round-trip; see AGENTS.md residual
// note on why this mode doesn't use RawQuery).
// ---------------------------------------------------------------------------

/// `true` when local `[x, _, z]` position falls within the `[min, max]` XZ
/// box; tolerant of a reversed min/max pair.
pub(crate) fn bbox_contains(pos: [f32; 3], min: [f32; 2], max: [f32; 2]) -> bool {
    let (min_x, max_x) = (min[0].min(max[0]), min[0].max(max[0]));
    let (min_z, max_z) = (min[1].min(max[1]), min[1].max(max[1]));
    pos[0] >= min_x && pos[0] <= max_x && pos[2] >= min_z && pos[2] <= max_z
}

/// Parses a `[x_buf, z_buf]` text-field pair into `[f32; 2]`; `None` if
/// either field fails to parse.
pub(crate) fn parse_bbox_fields(fields: &[String; 2]) -> Option<[f32; 2]> {
    let x = fields[0].trim().parse::<f32>().ok()?;
    let z = fields[1].trim().parse::<f32>().ok()?;
    Some([x, z])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_query_binds_petal_id_and_has_no_semicolon() {
        let (sql, vars) = annotation_query("petal-1");
        assert!(sql.starts_with("SELECT"));
        assert!(!sql.contains(';'));
        assert!(sql.contains("petal_id = $petal_id"));
        assert_eq!(vars.get("petal_id"), Some(&serde_json::Value::String("petal-1".into())));
    }

    #[test]
    fn property_filter_query_binds_all_three_vars() {
        let (sql, vars) = property_filter_query("petal-1", "status", serde_json::json!("open"));
        assert!(sql.starts_with("SELECT"));
        assert!(!sql.contains(';'));
        assert_eq!(vars.get("petal_id"), Some(&serde_json::Value::String("petal-1".into())));
        assert_eq!(vars.get("prop_key"), Some(&serde_json::Value::String("status".into())));
        assert_eq!(vars.get("prop_val"), Some(&serde_json::json!("open")));
    }

    #[test]
    fn parse_filter_value_number() {
        assert_eq!(parse_filter_value("number", "42.5"), serde_json::json!(42.5));
        assert_eq!(parse_filter_value("number", "nope"), serde_json::json!("nope"));
    }

    #[test]
    fn parse_filter_value_bool() {
        assert_eq!(parse_filter_value("bool", "true"), serde_json::json!(true));
        assert_eq!(parse_filter_value("bool", "No"), serde_json::json!(false));
    }

    #[test]
    fn parse_filter_value_string_default() {
        assert_eq!(parse_filter_value("string", "hello"), serde_json::json!("hello"));
    }

    #[test]
    fn parse_gis_rows_handles_geojson_position() {
        let data = serde_json::json!([{
            "node_id": "n1",
            "display_name": "Trailhead",
            "position": { "type": "Point", "coordinates": [10.0, 20.0] },
            "elevation": 5.0,
            "annotation_title": "Start",
            "annotation_color": "#ff8800",
        }]);
        let rows = parse_gis_rows(data.as_array().unwrap());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].node_id, "n1");
        assert_eq!(rows[0].name, "Trailhead");
        assert_eq!(rows[0].position, [10.0, 5.0, 20.0]);
        assert_eq!(rows[0].annotation_title.as_deref(), Some("Start"));
        assert_eq!(rows[0].annotation_color.as_deref(), Some("#ff8800"));
    }

    #[test]
    fn parse_gis_rows_annotation_color_absent_yields_none() {
        let data = serde_json::json!([{
            "node_id": "n1",
            "position": [0.0, 0.0],
            "annotation_title": "Start",
        }]);
        let rows = parse_gis_rows(data.as_array().unwrap());
        assert!(rows[0].annotation_color.is_none());
    }

    #[test]
    fn track_query_binds_petal_id_and_has_no_semicolon() {
        let (sql, vars) = track_query("petal-1");
        assert!(sql.starts_with("SELECT"));
        assert!(!sql.contains(';'));
        assert!(sql.contains("gis.track.name"));
        assert_eq!(vars.get("petal_id"), Some(&serde_json::Value::String("petal-1".into())));
    }

    #[test]
    fn annotation_query_selects_color_field() {
        let (sql, _) = annotation_query("petal-1");
        assert!(sql.contains("annotation_color"));
        assert!(sql.contains("gis.annotation.color"));
    }

    #[test]
    fn parse_gis_rows_handles_bare_array_position() {
        let data = serde_json::json!([{
            "node_id": "n2",
            "position": [1.0, 2.0],
            "elevation": 0.0,
        }]);
        let rows = parse_gis_rows(data.as_array().unwrap());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "n2", "falls back to node_id when display_name is absent");
        assert_eq!(rows[0].position, [1.0, 0.0, 2.0]);
        assert!(rows[0].annotation_title.is_none());
    }

    #[test]
    fn parse_gis_rows_skips_rows_missing_node_id() {
        let data = serde_json::json!([{ "position": [1.0, 2.0] }]);
        assert!(parse_gis_rows(data.as_array().unwrap()).is_empty());
    }

    #[test]
    fn parse_gis_rows_empty_yields_empty() {
        assert!(parse_gis_rows(&[]).is_empty());
    }

    #[test]
    fn bbox_contains_normal_bounds() {
        assert!(bbox_contains([5.0, 0.0, 5.0], [0.0, 0.0], [10.0, 10.0]));
        assert!(!bbox_contains([15.0, 0.0, 5.0], [0.0, 0.0], [10.0, 10.0]));
    }

    #[test]
    fn bbox_contains_tolerates_reversed_bounds() {
        assert!(bbox_contains([5.0, 0.0, 5.0], [10.0, 10.0], [0.0, 0.0]));
    }

    #[test]
    fn parse_bbox_fields_valid_and_invalid() {
        assert_eq!(parse_bbox_fields(&["1.5".into(), "-2.0".into()]), Some([1.5, -2.0]));
        assert_eq!(parse_bbox_fields(&["abc".into(), "1.0".into()]), None);
    }
}
