//! Egress CRS/terrain-origin resolution (FR-5, plan D3) — see `fe-api/AGENTS.md` §crs.

use fe_terrain::projection::Projection;
use serde_json::{json, Value};

use crate::server::ApiState;

/// CRS label for lat/lon (WGS84) egress output.
pub const CRS_EPSG_4326: &str = "EPSG:4326";
/// Documented placeholder when a petal has neither hexon CRS metadata nor a
/// terrain origin — matches fe-query's GeoParquet default (never a silent EPSG:4326).
pub const CRS_LOCAL_PLACEHOLDER: &str = "PETAL-LOCAL:meters;origin=unset";
/// Marker for /query results whose token scope spans more than one petal.
pub const CRS_MULTI_PETAL: &str = "PETAL-LOCAL:meters;origin=per-petal";

/// Resolved CRS context for one petal's egress.
pub struct EgressCrs {
    /// Human/tool-readable CRS label stamped on files and envelopes.
    pub label: String,
    /// Terrain-origin projection, when configured (enables `coords=latlon`).
    pub projection: Option<Projection>,
}

/// Pure D3 label builder — the three resolution branches, testable without a DB.
///
/// 1. hexon tileset metadata present → local-meters label + datum/scale from the hexon,
/// 2. terrain origin only → local-meters label with the origin,
/// 3. neither → documented placeholder.
pub fn crs_label(
    origin: Option<(f64, f64, f64)>,
    tileset_crs: Option<&str>,
    native_scale: Option<f64>,
) -> String {
    let Some((lat, lon, ele)) = origin else {
        return CRS_LOCAL_PLACEHOLDER.to_string();
    };
    let mut label = format!("PETAL-LOCAL:meters;origin={lat},{lon},{ele}");
    if let Some(datum) = tileset_crs {
        label.push_str(&format!(";datum={datum}"));
    }
    if let Some(scale) = native_scale {
        label.push_str(&format!(";native_scale={scale}"));
    }
    label
}

/// Resolve a petal's egress CRS from its terrain config + installed hexon tileset metadata.
pub async fn resolve_petal_crs(state: &ApiState, petal_id: &str) -> EgressCrs {
    let terrain = load_petal_terrain(state, petal_id).await;

    let origin = terrain.as_ref().and_then(|t| {
        let o = t.get("origin")?;
        Some((
            o.get("origin_lat")?.as_f64()?,
            o.get("origin_lon")?.as_f64()?,
            o.get("origin_ele").and_then(Value::as_f64).unwrap_or(0.0),
        ))
    });

    // Branch 1 input: CRS/scale from the first configured hexon tileset that the
    // registry knows about (hexon_scale_orchestration's TilesetMeta fields).
    let tileset_meta = terrain
        .as_ref()
        .and_then(|t| t.get("tileset_hexon_uris")?.as_array().cloned())
        .and_then(|uris| {
            let registry = state.tileset_registry.as_ref()?;
            uris.iter()
                .filter_map(Value::as_str)
                .find_map(|uri| registry.get_meta(uri))
        });

    let tileset_crs: Option<String> =
        tileset_meta.as_ref().map(|m| m.crs_or_default().to_string());
    let label = crs_label(
        origin,
        tileset_crs.as_deref(),
        tileset_meta.as_ref().and_then(|m| m.native_scale),
    );

    EgressCrs {
        label,
        projection: origin.map(|(lat, lon, ele)| Projection::new(lat, lon, ele)),
    }
}

/// CRS for a /query response: petal-scoped tokens resolve their petal; broader
/// scopes get the multi-petal marker (positions remain per-petal local meters).
pub async fn scope_crs(state: &ApiState, token_scope: &str) -> String {
    match fe_database::parse_scope(token_scope) {
        Ok(parts) => match parts.petal_id {
            Some(petal_id) => resolve_petal_crs(state, &petal_id).await.label,
            None => CRS_MULTI_PETAL.to_string(),
        },
        Err(_) => CRS_MULTI_PETAL.to_string(),
    }
}

/// Load the petal's raw `terrain` JSON (direct read, gateway-channel fallback).
async fn load_petal_terrain(state: &ApiState, petal_id: &str) -> Option<Value> {
    let sql = "SELECT terrain FROM petal WHERE petal_id = $pid LIMIT 1";
    let rows =
        crate::gis::run_select(state, sql, vec![("pid".to_string(), json!(petal_id))]).await?;
    let terrain = rows.first()?.get("terrain")?.clone();
    if terrain.is_null() {
        None
    } else {
        Some(terrain)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // D3 branch 1: hexon tileset metadata present.
    #[test]
    fn crs_label_with_hexon_metadata() {
        let label = crs_label(
            Some((47.6062, -122.3321, 56.0)),
            Some("EPSG:4326"),
            Some(1.0),
        );
        assert_eq!(
            label,
            "PETAL-LOCAL:meters;origin=47.6062,-122.3321,56;datum=EPSG:4326;native_scale=1"
        );
        // Landmine guard: local meters is labeled PETAL-LOCAL even when the
        // hexon's datum is EPSG:4326.
        assert!(label.starts_with("PETAL-LOCAL:meters"));
    }

    // D3 branch 2: terrain origin only.
    #[test]
    fn crs_label_with_origin_only() {
        let label = crs_label(Some((47.5, -122.25, 0.0)), None, None);
        assert_eq!(label, "PETAL-LOCAL:meters;origin=47.5,-122.25,0");
        assert!(!label.contains("datum"));
    }

    // D3 branch 3: neither → documented placeholder, never silent EPSG:4326.
    #[test]
    fn crs_label_placeholder_when_unconfigured() {
        let label = crs_label(None, None, None);
        assert_eq!(label, CRS_LOCAL_PLACEHOLDER);
        assert!(!label.contains("4326"));
    }
}
