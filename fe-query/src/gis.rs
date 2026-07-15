//! Petal-scoped GIS query builders rendering raw SurrealQL for `DbCommand::RawQuery`.
//!
//! See `AGENTS.md` §gis for the annotation-property contract, the
//! local-coordinates convention, and why spatial filters use plain Euclidean
//! arithmetic on `position.coordinates` instead of `geo::*` functions
//! (positions are petal-local meters, not lon/lat degrees).

use crate::builder::render::BuiltQuery;

/// Reserved node-property key: annotation title (see AGENTS.md §gis).
pub const ANNOTATION_TITLE_KEY: &str = "gis.annotation.title";
/// Reserved node-property key: annotation body text.
pub const ANNOTATION_BODY_KEY: &str = "gis.annotation.body";
/// Reserved node-property key: annotation color (hex string, optional).
pub const ANNOTATION_COLOR_KEY: &str = "gis.annotation.color";

/// Errors produced validating builder inputs before rendering SQL.
#[derive(Debug, Clone, PartialEq)]
pub enum GisQueryError {
    /// `petal_id` was empty, too long, or contained characters outside the
    /// safe id charset (alnum, `-`, `_`).
    InvalidPetalId(String),
    /// A coordinate/radius argument was NaN or infinite.
    NonFinite(&'static str),
    /// A min/max/radius argument was out of a sane range (e.g. min > max).
    InvalidRange(&'static str),
}

impl std::fmt::Display for GisQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GisQueryError::InvalidPetalId(id) => write!(f, "invalid petal_id: '{id}'"),
            GisQueryError::NonFinite(field) => write!(f, "{field} must be a finite number"),
            GisQueryError::InvalidRange(msg) => write!(f, "invalid range: {msg}"),
        }
    }
}

impl std::error::Error for GisQueryError {}

/// `petal_id` is always ULID-shaped, but this defends any caller that ends
/// up embedding it outside a bind parameter -- see AGENTS.md §gis trade-off.
fn validate_petal_id(petal_id: &str) -> Result<(), GisQueryError> {
    let ok = !petal_id.is_empty()
        && petal_id.len() <= 128
        && petal_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(GisQueryError::InvalidPetalId(petal_id.to_string()))
    }
}

fn validate_finite(name: &'static str, v: f64) -> Result<(), GisQueryError> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(GisQueryError::NonFinite(name))
    }
}

/// Render a query for nodes in `petal_id` whose local (x, z) position falls
/// within the axis-aligned box `[min_x, max_x] x [min_z, max_z]` (meters).
///
/// All inputs are bound as SurrealQL parameters. Filters compare the point's
/// raw `position.coordinates` components -- NOT `geo::inside`, which treats
/// coordinates as lon/lat degrees and is wrong for local meters (AGENTS.md §gis).
pub fn nodes_in_bbox(
    petal_id: &str,
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Result<BuiltQuery, GisQueryError> {
    validate_petal_id(petal_id)?;
    validate_finite("min_x", min_x)?;
    validate_finite("min_z", min_z)?;
    validate_finite("max_x", max_x)?;
    validate_finite("max_z", max_z)?;
    if min_x > max_x {
        return Err(GisQueryError::InvalidRange("min_x must be <= max_x"));
    }
    if min_z > max_z {
        return Err(GisQueryError::InvalidRange("min_z must be <= max_z"));
    }

    let sql = "SELECT * FROM node WHERE petal_id = $petal_id \
        AND position.coordinates[0] >= $min_x AND position.coordinates[0] <= $max_x \
        AND position.coordinates[1] >= $min_z AND position.coordinates[1] <= $max_z"
        .to_string();

    Ok(BuiltQuery {
        sql,
        params: vec![
            ("petal_id".into(), serde_json::json!(petal_id)),
            ("min_x".into(), serde_json::json!(min_x)),
            ("min_z".into(), serde_json::json!(min_z)),
            ("max_x".into(), serde_json::json!(max_x)),
            ("max_z".into(), serde_json::json!(max_z)),
        ],
    })
}

/// Render a query for nodes in `petal_id` within `radius_m` meters of local
/// point `(cx, cz)`, via squared Euclidean distance (`$r2 = radius_m^2`) --
/// NOT `geo::distance`, which is geodesic over lon/lat (AGENTS.md §gis).
pub fn nodes_within_radius(
    petal_id: &str,
    cx: f64,
    cz: f64,
    radius_m: f64,
) -> Result<BuiltQuery, GisQueryError> {
    validate_petal_id(petal_id)?;
    validate_finite("cx", cx)?;
    validate_finite("cz", cz)?;
    validate_finite("radius_m", radius_m)?;
    if radius_m <= 0.0 {
        return Err(GisQueryError::InvalidRange("radius_m must be > 0"));
    }

    let sql = "SELECT * FROM node WHERE petal_id = $petal_id \
        AND ((position.coordinates[0] - $cx) * (position.coordinates[0] - $cx) \
        + (position.coordinates[1] - $cz) * (position.coordinates[1] - $cz)) <= $r2"
        .to_string();

    Ok(BuiltQuery {
        sql,
        params: vec![
            ("petal_id".into(), serde_json::json!(petal_id)),
            ("cx".into(), serde_json::json!(cx)),
            ("cz".into(), serde_json::json!(cz)),
            ("r2".into(), serde_json::json!(radius_m * radius_m)),
        ],
    })
}

/// Render a query for nodes in `petal_id` carrying any `gis.annotation.*`
/// reserved property (title, body, or color -- see AGENTS.md §gis).
///
/// `properties` is `option<object>` and NONE on nodes that never had a
/// property set; `?? {}` coalesces before indexing so the filter never
/// trips on an absent `properties` field.
pub fn annotated_nodes(petal_id: &str) -> Result<BuiltQuery, GisQueryError> {
    validate_petal_id(petal_id)?;

    let sql = format!(
        "SELECT * FROM node WHERE petal_id = $petal_id AND \
         ((properties ?? {{}})[\"{title}\"] IS NOT NONE \
         OR (properties ?? {{}})[\"{body}\"] IS NOT NONE \
         OR (properties ?? {{}})[\"{color}\"] IS NOT NONE)",
        title = ANNOTATION_TITLE_KEY,
        body = ANNOTATION_BODY_KEY,
        color = ANNOTATION_COLOR_KEY,
    );

    Ok(BuiltQuery {
        sql,
        params: vec![("petal_id".into(), serde_json::json!(petal_id))],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── nodes_in_bbox ──

    #[test]
    fn bbox_renders_expected_sql_and_params() {
        let q = nodes_in_bbox("p1", 0.0, 1.0, 10.0, 11.0).unwrap();
        assert!(q
            .sql
            .starts_with("SELECT * FROM node WHERE petal_id = $petal_id"));
        assert!(
            !q.sql.contains("geo::"),
            "spatial filters must not use geo::* on local meters"
        );
        assert!(q.sql.contains("position.coordinates[0] >= $min_x"));
        assert!(q.sql.contains("position.coordinates[0] <= $max_x"));
        assert!(q.sql.contains("position.coordinates[1] >= $min_z"));
        assert!(q.sql.contains("position.coordinates[1] <= $max_z"));
        assert_eq!(q.params.len(), 5);
        assert_eq!(q.params[0], ("petal_id".into(), serde_json::json!("p1")));
        assert_eq!(q.params[1], ("min_x".into(), serde_json::json!(0.0)));
        assert_eq!(q.params[2], ("min_z".into(), serde_json::json!(1.0)));
        assert_eq!(q.params[3], ("max_x".into(), serde_json::json!(10.0)));
        assert_eq!(q.params[4], ("max_z".into(), serde_json::json!(11.0)));
    }

    #[test]
    fn bbox_rejects_invalid_petal_id() {
        let err = nodes_in_bbox("p1'; DROP TABLE node; --", 0.0, 0.0, 1.0, 1.0).unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidPetalId(_)));
    }

    #[test]
    fn bbox_rejects_empty_petal_id() {
        let err = nodes_in_bbox("", 0.0, 0.0, 1.0, 1.0).unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidPetalId(_)));
    }

    #[test]
    fn bbox_rejects_non_finite_coords() {
        let err = nodes_in_bbox("p1", f64::NAN, 0.0, 1.0, 1.0).unwrap_err();
        assert!(matches!(err, GisQueryError::NonFinite("min_x")));

        let err = nodes_in_bbox("p1", 0.0, 0.0, f64::INFINITY, 1.0).unwrap_err();
        assert!(matches!(err, GisQueryError::NonFinite("max_x")));
    }

    #[test]
    fn bbox_rejects_min_greater_than_max() {
        let err = nodes_in_bbox("p1", 10.0, 0.0, 0.0, 1.0).unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidRange(_)));

        let err = nodes_in_bbox("p1", 0.0, 10.0, 1.0, 0.0).unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidRange(_)));
    }

    // ── nodes_within_radius ──

    #[test]
    fn radius_renders_expected_sql_and_params() {
        let q = nodes_within_radius("p1", 1.5, 2.5, 100.0).unwrap();
        assert!(q
            .sql
            .starts_with("SELECT * FROM node WHERE petal_id = $petal_id"));
        assert!(
            !q.sql.contains("geo::"),
            "spatial filters must not use geo::* on local meters"
        );
        assert!(q.sql.contains("(position.coordinates[0] - $cx)"));
        assert!(q.sql.contains("(position.coordinates[1] - $cz)"));
        assert!(q.sql.contains("<= $r2"));
        assert_eq!(q.params.len(), 4);
        assert_eq!(q.params[0], ("petal_id".into(), serde_json::json!("p1")));
        assert_eq!(q.params[1], ("cx".into(), serde_json::json!(1.5)));
        assert_eq!(q.params[2], ("cz".into(), serde_json::json!(2.5)));
        assert_eq!(q.params[3], ("r2".into(), serde_json::json!(10000.0)));
    }

    #[test]
    fn radius_rejects_non_positive_radius() {
        let err = nodes_within_radius("p1", 0.0, 0.0, 0.0).unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidRange(_)));

        let err = nodes_within_radius("p1", 0.0, 0.0, -5.0).unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidRange(_)));
    }

    #[test]
    fn radius_rejects_non_finite_center() {
        let err = nodes_within_radius("p1", f64::NAN, 0.0, 10.0).unwrap_err();
        assert!(matches!(err, GisQueryError::NonFinite("cx")));
    }

    #[test]
    fn radius_rejects_invalid_petal_id() {
        let err = nodes_within_radius("bad id!", 0.0, 0.0, 10.0).unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidPetalId(_)));
    }

    // ── annotated_nodes ──

    #[test]
    fn annotated_renders_all_reserved_keys() {
        let q = annotated_nodes("p1").unwrap();
        assert!(q
            .sql
            .starts_with("SELECT * FROM node WHERE petal_id = $petal_id"));
        assert!(q.sql.contains(ANNOTATION_TITLE_KEY));
        assert!(q.sql.contains(ANNOTATION_BODY_KEY));
        assert!(q.sql.contains(ANNOTATION_COLOR_KEY));
        assert!(q.sql.contains("IS NOT NONE"));
        assert_eq!(q.params.len(), 1);
        assert_eq!(q.params[0], ("petal_id".into(), serde_json::json!("p1")));
    }

    #[test]
    fn annotated_rejects_invalid_petal_id() {
        let err = annotated_nodes("../etc").unwrap_err();
        assert!(matches!(err, GisQueryError::InvalidPetalId(_)));
    }

    // ── RawQuery keyword-filter compatibility ──
    //
    // fe-database's DbCommand::RawQuery handler blocks bare-word keywords
    // (CREATE, UPDATE, DELETE, DEFINE, REMOVE, RELATE, INSERT, LET, RETURN,
    // INFO, FOR, THROW, SLEEP, BREAK, LIVE, KILL, IF, BEGIN, COMMIT, CANCEL)
    // and rejects anything not starting with SELECT or containing ';'.
    // These tests are a regression guard: if a future edit to these builders
    // trips the filter, RawQuery would reject every request at runtime.
    fn assert_rawquery_compatible(sql: &str) {
        let upper = sql.to_uppercase();
        assert!(
            upper.trim_start().starts_with("SELECT"),
            "must start with SELECT: {sql}"
        );
        assert!(!sql.contains(';'), "must not contain ';': {sql}");
        let blocked = [
            "CREATE", "UPDATE", "DELETE", "DEFINE", "REMOVE", "RELATE", "INSERT", "LET", "RETURN",
            "INFO", "FOR", "THROW", "SLEEP", "BREAK", "LIVE", "KILL", "IF", "BEGIN", "COMMIT",
            "CANCEL",
        ];
        for kw in blocked {
            for (idx, _) in upper.match_indices(kw) {
                let before_ok = idx == 0
                    || !(upper.as_bytes()[idx - 1].is_ascii_alphanumeric()
                        || upper.as_bytes()[idx - 1] == b'_');
                let after = idx + kw.len();
                let after_ok = after >= upper.len()
                    || !(upper.as_bytes()[after].is_ascii_alphanumeric()
                        || upper.as_bytes()[after] == b'_');
                assert!(
                    !(before_ok && after_ok),
                    "blocked bare-word '{kw}' found in: {sql}"
                );
            }
        }
    }

    #[test]
    fn all_builders_pass_rawquery_keyword_filter() {
        assert_rawquery_compatible(&nodes_in_bbox("p1", 0.0, 0.0, 10.0, 10.0).unwrap().sql);
        assert_rawquery_compatible(&nodes_within_radius("p1", 0.0, 0.0, 10.0).unwrap().sql);
        assert_rawquery_compatible(&annotated_nodes("p1").unwrap().sql);
    }
}
