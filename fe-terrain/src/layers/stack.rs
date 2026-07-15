//! Layer stack management for terrain compositing.
//!
//! A [`LayerStack`] holds an ordered collection of [`MapLayer`] instances.
//! Layers are sorted by z-order for compositing.

use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Opaque identifier for a layer in the stack.
pub type LayerId = Ulid;

/// The type of content a layer holds.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayerType {
    /// Base terrain mesh layer.
    Terrain,
    /// Satellite imagery texture layer.
    Satellite,
    /// GPX track overlay.
    GpxTrack { node_id: String, color_mode: String },
    /// GeoJSON feature overlay.
    GeoJsonOverlay { source_path: String },
    /// Kernel-density heatmap.
    Heatmap { radius_m: f32, color_ramp: String },
    /// Waypoint marker collection.
    Waypoints,
}

/// Map a petal-config layer `name` (+ optional `source` id) to a [`LayerType`].
///
/// The config carries only a free-form name and optional source URL; the variants
/// that need an identifier (`GpxTrack.node_id`, `GeoJsonOverlay.source_path`) take
/// it from `source` so a matching track/overlay entity binds for visibility (empty
/// when absent). Unknown names return `None` — callers warn and skip.
pub fn layer_type_from_config_name(name: &str, source: Option<&str>) -> Option<LayerType> {
    match name {
        "satellite" => Some(LayerType::Satellite),
        "terrain" => Some(LayerType::Terrain),
        "gpx_track" => Some(LayerType::GpxTrack {
            node_id: source.unwrap_or_default().to_string(),
            color_mode: "solid".to_string(),
        }),
        "geojson_overlay" => Some(LayerType::GeoJsonOverlay {
            source_path: source.unwrap_or_default().to_string(),
        }),
        _ => None,
    }
}

/// A single compositable layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapLayer {
    pub id: LayerId,
    pub layer_type: LayerType,
    pub visible: bool,
    pub opacity: f32,
    pub z_order: i32,
}

impl MapLayer {
    /// Create a new layer with the given type and z-order.
    pub fn new(layer_type: LayerType, z_order: i32) -> Self {
        Self {
            id: Ulid::new(),
            layer_type,
            visible: true,
            opacity: 1.0,
            z_order,
        }
    }

    /// Create a layer with a known ID (useful for deserialization or P2P sync).
    pub fn with_id(id: LayerId, layer_type: LayerType, z_order: i32) -> Self {
        Self {
            id,
            layer_type,
            visible: true,
            opacity: 1.0,
            z_order,
        }
    }
}

/// An ordered stack of compositable layers.
#[derive(Debug, Clone, Default, Serialize, Deserialize, bevy::prelude::Resource)]
pub struct LayerStack {
    layers: Vec<MapLayer>,
}

impl LayerStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a layer to the stack. Returns the newly assigned [`LayerId`].
    pub fn add_layer(&mut self, layer: MapLayer) -> LayerId {
        let id = layer.id;
        self.layers.push(layer);
        id
    }

    /// Remove a layer by ID. Returns the removed layer if found.
    pub fn remove_layer(&mut self, id: LayerId) -> Option<MapLayer> {
        if let Some(pos) = self.layers.iter().position(|l| l.id == id) {
            Some(self.layers.remove(pos))
        } else {
            None
        }
    }

    /// Set visibility of a layer.
    pub fn set_visibility(&mut self, id: LayerId, visible: bool) -> bool {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == id) {
            layer.visible = visible;
            true
        } else {
            false
        }
    }

    /// Set opacity of a layer (clamped to 0.0–1.0).
    pub fn set_opacity(&mut self, id: LayerId, opacity: f32) -> bool {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == id) {
            layer.opacity = opacity.clamp(0.0, 1.0);
            true
        } else {
            false
        }
    }

    /// Reorder a layer to a new z-order.
    pub fn reorder(&mut self, id: LayerId, new_z_order: i32) -> bool {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == id) {
            layer.z_order = new_z_order;
            true
        } else {
            false
        }
    }

    /// Get all visible layers sorted by z-order (lowest first).
    pub fn get_visible_layers(&self) -> Vec<&MapLayer> {
        let mut visible: Vec<&MapLayer> = self.layers.iter().filter(|l| l.visible).collect();
        visible.sort_by_key(|l| l.z_order);
        visible
    }

    /// Get all layers (visible and invisible) sorted by z-order.
    pub fn get_all_layers_sorted(&self) -> Vec<&MapLayer> {
        let mut all: Vec<&MapLayer> = self.layers.iter().collect();
        all.sort_by_key(|l| l.z_order);
        all
    }

    /// Look up a layer by ID.
    pub fn get_layer(&self, id: LayerId) -> Option<&MapLayer> {
        self.layers.iter().find(|l| l.id == id)
    }

    /// Look up a layer by ID (mutable).
    pub fn get_layer_mut(&mut self, id: LayerId) -> Option<&mut MapLayer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Iterate over all layers.
    pub fn iter(&self) -> impl Iterator<Item = &MapLayer> {
        self.layers.iter()
    }

    /// Number of layers in the stack.
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// Whether the stack is empty.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Clear all layers.
    pub fn clear(&mut self) {
        self.layers.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_layer(z: i32) -> MapLayer {
        MapLayer::new(LayerType::Terrain, z)
    }

    #[test]
    fn test_add_and_remove() {
        let mut stack = LayerStack::new();
        let id = stack.add_layer(make_layer(0));
        assert_eq!(stack.len(), 1);
        let removed = stack.remove_layer(id);
        assert!(removed.is_some());
        assert_eq!(stack.len(), 0);
    }

    #[test]
    fn test_visibility_toggle() {
        let mut stack = LayerStack::new();
        let id = stack.add_layer(make_layer(0));
        assert!(stack.set_visibility(id, false));
        assert_eq!(stack.get_visible_layers().len(), 0);
        stack.set_visibility(id, true);
        assert_eq!(stack.get_visible_layers().len(), 1);
    }

    #[test]
    fn test_opacity_clamped() {
        let mut stack = LayerStack::new();
        let id = stack.add_layer(make_layer(0));
        stack.set_opacity(id, 2.0);
        assert_eq!(stack.get_layer(id).unwrap().opacity, 1.0);
        stack.set_opacity(id, -0.5);
        assert_eq!(stack.get_layer(id).unwrap().opacity, 0.0);
    }

    #[test]
    fn test_z_order_sorting() {
        let mut stack = LayerStack::new();
        let id3 = stack.add_layer(MapLayer::new(LayerType::Satellite, 3));
        let id1 = stack.add_layer(MapLayer::new(LayerType::Terrain, 1));
        let id2 = stack.add_layer(MapLayer::new(LayerType::Waypoints, 2));

        let visible = stack.get_visible_layers();
        assert_eq!(visible[0].id, id1);
        assert_eq!(visible[1].id, id2);
        assert_eq!(visible[2].id, id3);
    }

    #[test]
    fn test_reorder() {
        let mut stack = LayerStack::new();
        let id = stack.add_layer(make_layer(5));
        stack.reorder(id, 10);
        assert_eq!(stack.get_layer(id).unwrap().z_order, 10);
    }

    #[test]
    fn test_invisible_not_in_visible_list() {
        let mut stack = LayerStack::new();
        let id = stack.add_layer(make_layer(0));
        stack.set_visibility(id, false);
        assert!(stack.get_visible_layers().is_empty());
        // But get_all_layers_sorted still includes it
        assert_eq!(stack.get_all_layers_sorted().len(), 1);
    }

    #[test]
    fn layer_type_from_config_name_known_names() {
        assert!(matches!(
            layer_type_from_config_name("satellite", None),
            Some(LayerType::Satellite)
        ));
        assert!(matches!(
            layer_type_from_config_name("terrain", None),
            Some(LayerType::Terrain)
        ));
    }

    #[test]
    fn layer_type_from_config_name_maps_gpx_and_geojson_with_source() {
        match layer_type_from_config_name("gpx_track", Some("node-42")) {
            Some(LayerType::GpxTrack { node_id, .. }) => assert_eq!(node_id, "node-42"),
            other => panic!("expected GpxTrack, got {other:?}"),
        }
        match layer_type_from_config_name("geojson_overlay", Some("/tmp/a.geojson")) {
            Some(LayerType::GeoJsonOverlay { source_path }) => {
                assert_eq!(source_path, "/tmp/a.geojson")
            }
            other => panic!("expected GeoJsonOverlay, got {other:?}"),
        }
    }

    #[test]
    fn layer_type_from_config_name_source_defaults_empty() {
        match layer_type_from_config_name("gpx_track", None) {
            Some(LayerType::GpxTrack { node_id, .. }) => assert!(node_id.is_empty()),
            other => panic!("expected GpxTrack, got {other:?}"),
        }
    }

    #[test]
    fn layer_type_from_config_name_unknown_is_none() {
        assert!(layer_type_from_config_name("heatmap", None).is_none());
        assert!(layer_type_from_config_name("", None).is_none());
    }
}
