//! Pure terrain-layer JSON read/mutate helpers backing the Layer Manager
//! card. Operates on the raw petal terrain JSON (`PetalMapState.terrain_json`)
//! shaped like fe-terrain's `TerrainConfig`/`LayerConfig` — fe-ui never
//! depends on fe-terrain (boundary rule), so this is all `serde_json`. See
//! `fe-ui/src/AGENTS.md` §gis-query-ui and `fe-terrain/src/AGENTS.md`
//! §petal_binding for the consuming side.

use crate::actions::UiAction;
use crate::settings::AppSettings;
use crate::visibility::{Group, VisibilityState};

/// What one Layer Manager row controls (D-11, ui_ux §7 unified overlay
/// registry): a terrain layer by config name, a visibility Group by id, or one
/// of the ruler overlays held in `AppSettings`.
#[derive(Debug, Clone, PartialEq)]
pub enum OverlayKey {
    TerrainLayer(String),
    Group(String),
    ScaleBar,
    WorldGrid,
}

/// One row of the unified overlay registry: `(name, visible, opacity)`.
/// `opacity` is `None` for overlays that have no opacity knob.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayEntry {
    pub key: OverlayKey,
    pub name: String,
    pub visible: bool,
    pub opacity: Option<f32>,
}

/// Terrain layer config names the registry offers, in row order — every one is
/// rendered by fe-terrain (`gpx_track`/`geojson_overlay` as CATEGORY layers,
/// fe-terrain `src/AGENTS.md` §petal_binding).
pub(crate) const TERRAIN_LAYER_NAMES: [&str; 4] =
    ["satellite", "terrain", "gpx_track", "geojson_overlay"];

/// Display name for a terrain layer config name.
fn terrain_layer_label(name: &str) -> String {
    match name {
        "satellite" => "Satellite".to_string(),
        "terrain" => "Terrain".to_string(),
        "gpx_track" => "GPX tracks".to_string(),
        "geojson_overlay" => "GeoJSON overlays".to_string(),
        other => other.to_string(),
    }
}

/// A stored layer's `(visible, opacity)`; absent layer → `(true, 1.0)`.
fn terrain_layer_state(terrain_json: &serde_json::Value, name: &str) -> (bool, f32) {
    let layer = terrain_json
        .get("layers")
        .and_then(|l| l.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|l| l.get("name").and_then(|n| n.as_str()) == Some(name))
        });
    let visible = layer
        .and_then(|l| l.get("visible"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let opacity = layer
        .and_then(|l| l.get("opacity"))
        .and_then(|o| o.as_f64())
        .unwrap_or(1.0) as f32;
    (visible, opacity)
}

/// The ONE overlay registry the Layer Manager renders: terrain layers (only
/// when the petal has a map), then Groups in registry order, then the scale
/// bar and metric grid. Pure.
pub(crate) fn overlay_registry(
    terrain_json: Option<&serde_json::Value>,
    visibility: &VisibilityState,
    settings: &AppSettings,
) -> Vec<OverlayEntry> {
    let mut rows = Vec::new();
    if let Some(doc) = terrain_json {
        for name in TERRAIN_LAYER_NAMES {
            let (visible, opacity) = terrain_layer_state(doc, name);
            rows.push(OverlayEntry {
                key: OverlayKey::TerrainLayer(name.to_string()),
                name: terrain_layer_label(name),
                visible,
                opacity: Some(opacity),
            });
        }
    }
    rows.extend(visibility.groups().iter().map(|g| OverlayEntry {
        key: OverlayKey::Group(g.id.clone()),
        name: g.name.clone(),
        visible: g.visible,
        opacity: None,
    }));
    rows.push(OverlayEntry {
        key: OverlayKey::ScaleBar,
        name: "Scale bar".to_string(),
        visible: settings.show_scale_bar,
        opacity: None,
    });
    rows.push(OverlayEntry {
        key: OverlayKey::WorldGrid,
        name: "Metric grid".to_string(),
        visible: settings.show_world_grid,
        opacity: None,
    });
    rows
}

/// The action a visibility toggle on `key` dispatches. A Group toggle is a
/// whole-registry replace (D-3), so it needs the current `groups`; terrain
/// layers persist through the petal terrain doc (`GisSetLayer`). Pure.
pub(crate) fn apply_overlay_toggle(
    key: &OverlayKey,
    visible: bool,
    petal_id: &str,
    groups: &[Group],
) -> UiAction {
    match key {
        OverlayKey::TerrainLayer(name) => UiAction::GisSetLayer {
            petal_id: petal_id.to_string(),
            layer_name: name.clone(),
            visible: Some(visible),
            opacity: None,
        },
        OverlayKey::Group(id) => UiAction::SetGroupRegistry {
            petal_id: petal_id.to_string(),
            groups: groups
                .iter()
                .map(|g| Group {
                    visible: if &g.id == id { visible } else { g.visible },
                    ..g.clone()
                })
                .collect(),
        },
        OverlayKey::ScaleBar | OverlayKey::WorldGrid => UiAction::SetAppOverlay {
            key: key.clone(),
            visible,
        },
    }
}

/// Writes a ruler-overlay toggle into `AppSettings`; `false` for a key that is
/// not an app overlay (nothing written).
pub(crate) fn apply_app_overlay(
    settings: &mut AppSettings,
    key: &OverlayKey,
    visible: bool,
) -> bool {
    match key {
        OverlayKey::ScaleBar => settings.show_scale_bar = visible,
        OverlayKey::WorldGrid => settings.show_world_grid = visible,
        OverlayKey::TerrainLayer(_) | OverlayKey::Group(_) => return false,
    }
    true
}

/// Returns a mutated clone of `terrain_json` with the named layer's
/// `visible`/`opacity` fields updated (only the `Some(..)` ones), inserting a
/// new layer entry if `layer_name` isn't present yet. Every other field of
/// the document (origin, elevation source, world_scale, other layers, ...)
/// is preserved verbatim — this is the "mutate one field, round-trip via
/// SetPetalTerrain" idiom from `actions::hexon::set_petal_map_scale`, applied
/// to the actual stored doc rather than a freshly rebuilt one.
pub(crate) fn set_layer_field(
    terrain_json: &serde_json::Value,
    layer_name: &str,
    visible: Option<bool>,
    opacity: Option<f32>,
) -> serde_json::Value {
    let mut doc = terrain_json.clone();
    let Some(obj) = doc.as_object_mut() else {
        return doc;
    };
    let layers = obj
        .entry("layers".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(arr) = layers.as_array_mut() else {
        return doc;
    };

    let existing = arr
        .iter_mut()
        .find(|l| l.get("name").and_then(|n| n.as_str()) == Some(layer_name));

    match existing {
        Some(entry) => {
            if let Some(map) = entry.as_object_mut() {
                if let Some(v) = visible {
                    map.insert("visible".to_string(), serde_json::Value::Bool(v));
                }
                if let Some(o) = opacity {
                    map.insert("opacity".to_string(), serde_json::json!(o));
                }
            }
        }
        None => {
            arr.push(serde_json::json!({
                "name": layer_name,
                "visible": visible.unwrap_or(true),
                "opacity": opacity,
            }));
        }
    }
    doc
}

/// Splat rendering mode for a petal's terrain — additive `"view_mode"`
/// field on the stored terrain JSON, consumed by the renderer (another
/// track, not fe-ui — fe-ui only round-trips the field name/values, which
/// must not deviate: `"mesh" | "splats" | "hybrid"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Mesh,
    Splats,
    Hybrid,
}

impl ViewMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ViewMode::Mesh => "mesh",
            ViewMode::Splats => "splats",
            ViewMode::Hybrid => "hybrid",
        }
    }

    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "mesh" => Some(ViewMode::Mesh),
            "splats" => Some(ViewMode::Splats),
            "hybrid" => Some(ViewMode::Hybrid),
            _ => None,
        }
    }

    #[cfg(test)] // only exercised by the round-trip test below
    pub(crate) const ALL: [ViewMode; 3] = [ViewMode::Mesh, ViewMode::Splats, ViewMode::Hybrid];
}

/// Reads the petal terrain JSON's `"view_mode"` field, defaulting to `Mesh`
/// when absent or unrecognized (back-compat with terrain docs predating
/// this field).
pub(crate) fn view_mode_from_terrain_json(terrain_json: &serde_json::Value) -> ViewMode {
    terrain_json
        .get("view_mode")
        .and_then(|v| v.as_str())
        .and_then(ViewMode::from_str)
        .unwrap_or_default()
}

/// Returns a mutated clone of `terrain_json` with `"view_mode"` set,
/// preserving every other field — same "mutate one field, round-trip"
/// idiom as `set_layer_field`.
pub(crate) fn set_view_mode_field(
    terrain_json: &serde_json::Value,
    mode: ViewMode,
) -> serde_json::Value {
    let mut doc = terrain_json.clone();
    let Some(obj) = doc.as_object_mut() else {
        return doc;
    };
    obj.insert(
        "view_mode".to_string(),
        serde_json::Value::String(mode.as_str().to_string()),
    );
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_doc() -> serde_json::Value {
        serde_json::json!({
            "enabled": true,
            "world_scale": 0.01,
            "layers": [
                { "name": "satellite", "visible": true },
                { "name": "terrain", "visible": true, "opacity": 0.8 },
            ],
        })
    }

    fn group(id: &str, name: &str, visible: bool) -> Group {
        Group {
            id: id.into(),
            name: name.into(),
            visible,
        }
    }

    #[test]
    fn overlay_registry_lists_layers_then_groups_then_ruler_overlays() {
        let mut visibility = VisibilityState::default();
        visibility.replace_groups(vec![
            group("g1", "Utilities", false),
            group("g2", "Phase 2", true),
        ]);
        let settings = AppSettings {
            show_scale_bar: false,
            show_world_grid: true,
            ..AppSettings::default()
        };
        let rows = overlay_registry(Some(&sample_doc()), &visibility, &settings);
        let summary: Vec<(OverlayKey, &str, bool, Option<f32>)> = rows
            .iter()
            .map(|r| (r.key.clone(), r.name.as_str(), r.visible, r.opacity))
            .collect();
        let layer = |n: &str| OverlayKey::TerrainLayer(n.into());
        assert_eq!(
            summary,
            vec![
                (layer("satellite"), "Satellite", true, Some(1.0)),
                (layer("terrain"), "Terrain", true, Some(0.8)),
                (layer("gpx_track"), "GPX tracks", true, Some(1.0)),
                (
                    layer("geojson_overlay"),
                    "GeoJSON overlays",
                    true,
                    Some(1.0)
                ),
                (OverlayKey::Group("g1".into()), "Utilities", false, None),
                (OverlayKey::Group("g2".into()), "Phase 2", true, None),
                (OverlayKey::ScaleBar, "Scale bar", false, None),
                (OverlayKey::WorldGrid, "Metric grid", true, None),
            ]
        );
    }

    #[test]
    fn overlay_registry_without_a_map_has_no_terrain_rows() {
        let rows = overlay_registry(None, &VisibilityState::default(), &AppSettings::default());
        let keys: Vec<OverlayKey> = rows.into_iter().map(|r| r.key).collect();
        assert_eq!(keys, vec![OverlayKey::ScaleBar, OverlayKey::WorldGrid]);
    }

    #[test]
    fn overlay_toggle_maps_each_key_to_its_action() {
        let groups = vec![group("g1", "A", true), group("g2", "B", true)];
        match apply_overlay_toggle(
            &OverlayKey::TerrainLayer("gpx_track".into()),
            false,
            "p1",
            &groups,
        ) {
            UiAction::GisSetLayer {
                petal_id,
                layer_name,
                visible,
                opacity,
            } => {
                assert_eq!(
                    (petal_id.as_str(), layer_name.as_str()),
                    ("p1", "gpx_track")
                );
                assert_eq!((visible, opacity), (Some(false), None));
            }
            other => panic!("expected GisSetLayer, got {other:?}"),
        }
        match apply_overlay_toggle(&OverlayKey::Group("g2".into()), false, "p1", &groups) {
            UiAction::SetGroupRegistry { petal_id, groups } => {
                assert_eq!(petal_id, "p1");
                let flags: Vec<(&str, bool)> =
                    groups.iter().map(|g| (g.id.as_str(), g.visible)).collect();
                assert_eq!(flags, vec![("g1", true), ("g2", false)]);
            }
            other => panic!("expected SetGroupRegistry, got {other:?}"),
        }
        assert!(matches!(
            apply_overlay_toggle(&OverlayKey::WorldGrid, true, "p1", &groups),
            UiAction::SetAppOverlay {
                key: OverlayKey::WorldGrid,
                visible: true
            }
        ));
    }

    #[test]
    fn app_overlay_writes_only_its_own_setting() {
        let mut settings = AppSettings::default();
        assert!(apply_app_overlay(
            &mut settings,
            &OverlayKey::ScaleBar,
            false
        ));
        assert!(!settings.show_scale_bar);
        assert!(apply_app_overlay(
            &mut settings,
            &OverlayKey::WorldGrid,
            true
        ));
        assert!(settings.show_world_grid);
        assert!(!apply_app_overlay(
            &mut settings,
            &OverlayKey::Group("g".into()),
            false
        ));
    }

    #[test]
    fn set_layer_field_updates_existing_layer_visible_only() {
        let updated = set_layer_field(&sample_doc(), "satellite", Some(false), None);
        // opacity untouched (still default 1.0 since it wasn't set before either)
        assert_eq!(terrain_layer_state(&updated, "satellite"), (false, 1.0));
        // sibling fields preserved
        assert_eq!(updated["world_scale"], serde_json::json!(0.01));
    }

    #[test]
    fn set_layer_field_updates_opacity_only() {
        let updated = set_layer_field(&sample_doc(), "terrain", None, Some(0.3));
        // visible must be untouched when only opacity is set
        assert_eq!(terrain_layer_state(&updated, "terrain"), (true, 0.3));
    }

    #[test]
    fn set_layer_field_inserts_missing_layer() {
        let updated = set_layer_field(&sample_doc(), "gpx_track", Some(false), Some(0.5));
        assert_eq!(updated["layers"].as_array().map(Vec::len), Some(3));
        assert_eq!(terrain_layer_state(&updated, "gpx_track"), (false, 0.5));
    }

    #[test]
    fn set_layer_field_noop_on_non_object_doc() {
        let doc = serde_json::json!([1, 2, 3]);
        let updated = set_layer_field(&doc, "satellite", Some(true), None);
        assert_eq!(updated, doc);
    }

    #[test]
    fn view_mode_str_round_trips_for_all_variants() {
        for mode in ViewMode::ALL {
            assert_eq!(ViewMode::from_str(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn view_mode_from_str_rejects_unknown() {
        assert_eq!(ViewMode::from_str("wireframe"), None);
    }

    #[test]
    fn view_mode_from_terrain_json_defaults_to_mesh_when_absent() {
        assert_eq!(view_mode_from_terrain_json(&sample_doc()), ViewMode::Mesh);
    }

    #[test]
    fn view_mode_from_terrain_json_defaults_to_mesh_when_unrecognized() {
        let doc = serde_json::json!({ "view_mode": "not_a_mode" });
        assert_eq!(view_mode_from_terrain_json(&doc), ViewMode::Mesh);
    }

    #[test]
    fn set_view_mode_field_reads_back_and_preserves_siblings() {
        let updated = set_view_mode_field(&sample_doc(), ViewMode::Splats);
        assert_eq!(view_mode_from_terrain_json(&updated), ViewMode::Splats);
        // sibling fields preserved
        assert_eq!(updated["world_scale"], serde_json::json!(0.01));
        assert_eq!(updated["layers"], sample_doc()["layers"]);
    }

    #[test]
    fn set_view_mode_field_noop_on_non_object_doc() {
        let doc = serde_json::json!([1, 2, 3]);
        let updated = set_view_mode_field(&doc, ViewMode::Hybrid);
        assert_eq!(updated, doc);
    }
}
