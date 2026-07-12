//! Pure terrain-layer JSON read/mutate helpers backing the Layer Manager
//! card. Operates on the raw petal terrain JSON (`PetalMapState.terrain_json`)
//! shaped like fe-terrain's `TerrainConfig`/`LayerConfig` — fe-ui never
//! depends on fe-terrain (boundary rule), so this is all `serde_json`. See
//! `fe-ui/src/AGENTS.md` §gis-query-ui and `fe-terrain/src/AGENTS.md`
//! §petal_binding for the consuming side.

/// A single layer row for display: name, visibility, opacity.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerUiEntry {
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
}

/// Extracts the `layers` array of a petal terrain JSON doc for display.
/// Returns an empty vec for a missing/malformed `layers` field.
pub(crate) fn layer_entries_from_terrain_json(terrain_json: &serde_json::Value) -> Vec<LayerUiEntry> {
    terrain_json
        .get("layers")
        .and_then(|l| l.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|l| {
                    let name = l.get("name")?.as_str()?.to_string();
                    let visible = l.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
                    let opacity = l.get("opacity").and_then(|o| o.as_f64()).unwrap_or(1.0) as f32;
                    Some(LayerUiEntry { name, visible, opacity })
                })
                .collect()
        })
        .unwrap_or_default()
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
    let Some(obj) = doc.as_object_mut() else { return doc };
    let layers = obj
        .entry("layers".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let Some(arr) = layers.as_array_mut() else { return doc };

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
pub(crate) fn set_view_mode_field(terrain_json: &serde_json::Value, mode: ViewMode) -> serde_json::Value {
    let mut doc = terrain_json.clone();
    let Some(obj) = doc.as_object_mut() else { return doc };
    obj.insert("view_mode".to_string(), serde_json::Value::String(mode.as_str().to_string()));
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

    #[test]
    fn layer_entries_extracts_defaults() {
        let entries = layer_entries_from_terrain_json(&sample_doc());
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], LayerUiEntry { name: "satellite".into(), visible: true, opacity: 1.0 });
        assert_eq!(entries[1], LayerUiEntry { name: "terrain".into(), visible: true, opacity: 0.8 });
    }

    #[test]
    fn layer_entries_empty_for_missing_layers_field() {
        assert!(layer_entries_from_terrain_json(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn set_layer_field_updates_existing_layer_visible_only() {
        let updated = set_layer_field(&sample_doc(), "satellite", Some(false), None);
        let entries = layer_entries_from_terrain_json(&updated);
        assert!(!entries[0].visible);
        // opacity untouched (still default 1.0 since it wasn't set before either)
        assert_eq!(entries[0].opacity, 1.0);
        // sibling fields preserved
        assert_eq!(updated["world_scale"], serde_json::json!(0.01));
    }

    #[test]
    fn set_layer_field_updates_opacity_only() {
        let updated = set_layer_field(&sample_doc(), "terrain", None, Some(0.3));
        let entries = layer_entries_from_terrain_json(&updated);
        assert_eq!(entries[1].opacity, 0.3);
        assert!(entries[1].visible, "visible must be untouched when only opacity is set");
    }

    #[test]
    fn set_layer_field_inserts_missing_layer() {
        let updated = set_layer_field(&sample_doc(), "gpx_track", Some(true), Some(0.5));
        let entries = layer_entries_from_terrain_json(&updated);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2].name, "gpx_track");
        assert!(entries[2].visible);
        assert_eq!(entries[2].opacity, 0.5);
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
        assert_eq!(layer_entries_from_terrain_json(&updated).len(), 2);
    }

    #[test]
    fn set_view_mode_field_noop_on_non_object_doc() {
        let doc = serde_json::json!([1, 2, 3]);
        let updated = set_view_mode_field(&doc, ViewMode::Hybrid);
        assert_eq!(updated, doc);
    }
}
