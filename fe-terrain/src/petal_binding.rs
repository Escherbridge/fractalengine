//! Petal→terrain binding runtime support; see `src/AGENTS.md` §petal_binding.

use crate::config::{ElevationSourceKind, LayerConfig, TerrainConfig, TileSourceMode};
use crate::projection::Projection;
use fe_format::manifest::ElevationEncoding;

/// A petal→terrain assignment; `config: None` clears the petal's terrain.
#[derive(Debug, Clone)]
pub struct PetalTerrainAssignment {
    pub petal_id: String,
    pub config: Option<TerrainConfig>,
}

/// Parse a petal record's `terrain` JSON field; returns `None` for null/missing/invalid.
pub fn terrain_config_from_petal_json(value: &serde_json::Value) -> Option<TerrainConfig> {
    if value.is_null() {
        return None;
    }
    match serde_json::from_value::<TerrainConfig>(value.clone()) {
        Ok(cfg) => Some(cfg),
        Err(err) => {
            tracing::warn!(error = %err, "invalid petal terrain JSON; ignoring");
            None
        }
    }
}

/// Default [`TerrainConfig`] using an installed tileset as the petal's map.
pub fn config_for_tileset(info: &crate::tiles::TilesetInfo) -> TerrainConfig {
    // bounds = [min_lat, min_lon, max_lat, max_lon] (see hexon_source.rs covers()).
    let [min_lat, min_lon, max_lat, max_lon] = info.bounds;
    let origin = Projection::new(
        (min_lat + max_lat) / 2.0,
        (min_lon + max_lon) / 2.0,
        0.0,
    );
    let elevation_source = match info.elevation_encoding {
        Some(ElevationEncoding::TerrainRgb) => ElevationSourceKind::TerrainRgb,
        Some(ElevationEncoding::Terrarium) => ElevationSourceKind::Terrarium,
        _ => ElevationSourceKind::None,
    };
    TerrainConfig {
        enabled: true,
        origin,
        tile_source_url: String::new(),
        elevation_source,
        elevation_api_key_env: None,
        min_zoom: info.zoom_range.0,
        max_zoom: info.zoom_range.1,
        layers: vec![
            LayerConfig {
                name: "satellite".into(),
                visible: true,
                opacity: None,
                source_url: None,
            },
            LayerConfig {
                name: "terrain".into(),
                visible: true,
                opacity: None,
                source_url: None,
            },
        ],
        tileset_hexon_uris: vec![info.tileset_id.clone()],
        tile_source_mode: TileSourceMode::Hybrid,
        world_scale: info.native_scale.unwrap_or(1.0),
        scale_bounds: info.scale_bounds,
        ..TerrainConfig::default()
    }
}

#[cfg(feature = "render")]
mod render_support {
    use bevy::prelude::*;

    use super::PetalTerrainAssignment;
    use crate::config::{ElevationSourceKind, TerrainConfig};
    use crate::layers::{layer_type_from_config_name, LayerStack, MapLayer};
    use crate::terrain_plugin::{GeoJsonOverlay, GeoJsonProcessed, GpxTrackLine, TerrainChunk};
    use crate::tiles::{
        decode_png_pixels, CompositeTileSource, DiskTileCache, ElevationDecoder,
        TerrainRgbDecoder, TerrariumDecoder, TileCoord, TilesetRegistry,
    };
    use fe_format::manifest::ElevationEncoding;

    /// Default disk cache budget for the active petal's tile cache.
    const DEFAULT_TILE_CACHE_BYTES: u64 = 512 * 1024 * 1024;

    /// Buffered message driving terrain assignment (bridged from `DbResult::PetalTerrainLoaded`).
    #[derive(bevy::prelude::Message, Debug, Clone)]
    pub struct TerrainAssignmentMsg(pub PetalTerrainAssignment);

    /// Live terrain state for the active petal.
    #[derive(bevy::prelude::Resource, Default)]
    pub struct ActivePetalTerrain {
        pub petal_id: Option<String>,
        pub config: Option<TerrainConfig>,
        /// Bumped on every applied assignment.
        pub revision: u64,
    }

    /// Shared handle to the tileset registry (inserted by the main binary).
    #[derive(bevy::prelude::Resource, Clone)]
    pub struct SharedTilesetRegistry(pub std::sync::Arc<TilesetRegistry>);

    /// Tile source + projection rebuilt whenever the assignment changes.
    #[derive(bevy::prelude::Resource, Default)]
    pub struct ActiveTileSource {
        pub composite: Option<CompositeTileSource>,
        pub projection: Option<crate::projection::Projection>,
    }

    /// Applies the last queued assignment: state, tile source, layer stack, entity reset.
    pub fn apply_terrain_assignments(
        mut reader: MessageReader<TerrainAssignmentMsg>,
        registry: Option<Res<SharedTilesetRegistry>>,
        mut active: ResMut<ActivePetalTerrain>,
        mut tile_source: ResMut<ActiveTileSource>,
        mut layer_stack: ResMut<LayerStack>,
        mut commands: Commands,
        spawned: Query<
            Entity,
            Or<(
                With<TerrainChunk>,
                With<GpxTrackLine>,
                With<GeoJsonOverlay>,
                With<GeoJsonProcessed>,
            )>,
        >,
    ) {
        // Only the last assignment per frame wins.
        let Some(TerrainAssignmentMsg(assignment)) = reader.read().last().cloned() else {
            return;
        };

        active.petal_id = Some(assignment.petal_id.clone());
        active.config = assignment.config.clone();
        active.revision += 1;

        match assignment.config.filter(|c| c.enabled) {
            Some(mut config) => {
                let mut composite = CompositeTileSource::new();
                let mut hexon_encoding: Option<ElevationEncoding> = None;
                let mut hexon_scale_bounds: Option<[f64; 2]> = None;
                for uri in &config.tileset_hexon_uris {
                    let Some(reg) = registry.as_ref() else {
                        tracing::warn!(uri = %uri, "no SharedTilesetRegistry resource; cannot load tileset");
                        continue;
                    };
                    match reg.0.store().load_tileset(uri) {
                        Ok(src) => {
                            hexon_encoding.get_or_insert(src.tileset_meta.elevation_encoding.clone());
                            if let Some(bounds) = src.tileset_meta.scale_bounds {
                                hexon_scale_bounds.get_or_insert(bounds);
                            }
                            composite.add_hexon_source(src);
                        }
                        Err(err) => {
                            tracing::warn!(uri = %uri, error = %err, "failed to load tileset for terrain assignment");
                        }
                    }
                }
                // Loaded hexons are authoritative for scale (C1): the stored
                // config's world_scale is a clamped nudge within hexon bounds.
                if hexon_scale_bounds.is_some() {
                    config.scale_bounds = hexon_scale_bounds;
                }
                if config.scale_bounds.is_some() {
                    config.world_scale = crate::scale::clamp_world_scale_to_bounds(
                        config.world_scale,
                        config.scale_bounds,
                    );
                }
                // Loaded hexons are authoritative for decoding — the stored
                // config may guess wrong (fe-ui emits terrain_rgb; see AGENTS.md).
                if let Some(enc) = hexon_encoding {
                    let authoritative = match enc {
                        ElevationEncoding::TerrainRgb => ElevationSourceKind::TerrainRgb,
                        ElevationEncoding::Terrarium => ElevationSourceKind::Terrarium,
                        ElevationEncoding::Raw16 => ElevationSourceKind::None,
                    };
                    if config.elevation_source != authoritative {
                        tracing::info!(
                            stored = ?config.elevation_source,
                            hexon = ?authoritative,
                            "overriding elevation source from loaded tileset metadata"
                        );
                        config.elevation_source = authoritative;
                    }
                }
                composite.set_tile_source_mode(config.tile_source_mode.clone());
                if config.cache_dir.is_empty() {
                    tracing::warn!("terrain cache_dir is empty; skipping disk tile cache");
                } else {
                    composite.set_cache(DiskTileCache::new(
                        config.cache_dir.clone(),
                        DEFAULT_TILE_CACHE_BYTES,
                    ));
                }
                // Ground the projection: mesh world-Y = elevation − origin_ele,
                // so an unset origin_ele leaves real terrain hundreds of meters
                // above the camera (and past the far plane).
                if config.origin.origin_ele == 0.0 {
                    if let Some(ele) = sample_center_elevation(&composite, &config) {
                        tracing::info!(origin_ele = ele, "grounded terrain origin from center-tile elevation");
                        config.origin.origin_ele = ele;
                    }
                }
                tile_source.composite = Some(composite);
                tile_source.projection = Some(config.origin.clone());

                layer_stack.clear();
                for (index, layer) in config.layers.iter().enumerate() {
                    // `source_url` = GPX node_id / GeoJSON path; see `src/AGENTS.md` §petal_binding.
                    let Some(layer_type) =
                        layer_type_from_config_name(layer.name.as_str(), layer.source_url.as_deref())
                    else {
                        tracing::warn!(layer = %layer.name, "unknown terrain layer name; skipping");
                        continue;
                    };
                    let mut map_layer = MapLayer::new(layer_type, index as i32);
                    map_layer.visible = layer.visible;
                    map_layer.opacity = layer.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
                    layer_stack.add_layer(map_layer);
                }

                // Publish the reconciled config (encoding + grounded origin).
                active.config = Some(config);
            }
            None => {
                tile_source.composite = None;
                tile_source.projection = None;
                layer_stack.clear();
            }
        }

        // Despawn all terrain-derived entities so they respawn under the new config.
        for entity in spawned.iter() {
            commands.entity(entity).despawn();
        }
    }

    /// Mean elevation of the tile at the config origin (min zoom); `None` when
    /// no elevation tile or decoder is available.
    fn sample_center_elevation(
        composite: &CompositeTileSource,
        config: &TerrainConfig,
    ) -> Option<f64> {
        let coord = TileCoord::from_lat_lon(
            config.origin.origin_lat,
            config.origin.origin_lon,
            config.min_zoom,
        );
        let png = composite.get_tile_sync(coord)?;
        let (pixels, w, h) = decode_png_pixels(&png).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let decoded = match config.elevation_source {
            ElevationSourceKind::TerrainRgb => TerrainRgbDecoder.decode(&pixels, w, h),
            ElevationSourceKind::Terrarium => TerrariumDecoder.decode(&pixels, w, h),
            ElevationSourceKind::None => return None,
        };
        if decoded.is_empty() {
            return None;
        }
        let sum: f64 = decoded.iter().map(|v| *v as f64).sum();
        Some(sum / decoded.len() as f64)
    }
}

#[cfg(feature = "render")]
pub use render_support::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ElevationSourceKind, TerrainConfig, TileSourceMode};
    use crate::tiles::TilesetInfo;

    fn info() -> TilesetInfo {
        TilesetInfo {
            tileset_id: "ts-1".into(),
            region_name: "Test".into(),
            bounds: [45.0, -122.0, 46.0, -121.0],
            zoom_range: (10, 12),
            tile_count: 4,
            seeding: false,
            elevation_encoding: Some(ElevationEncoding::TerrainRgb),
            native_scale: None,
            scale_bounds: None,
        }
    }

    #[test]
    fn terrain_config_json_roundtrip() {
        let cfg = TerrainConfig {
            enabled: true,
            min_zoom: 8,
            ..TerrainConfig::default()
        };
        let value = serde_json::to_value(&cfg).unwrap();
        let parsed = terrain_config_from_petal_json(&value).expect("valid config parses");
        assert!(parsed.enabled);
        assert_eq!(parsed.min_zoom, 8);
        assert_eq!(parsed.max_zoom, cfg.max_zoom);
    }

    #[test]
    fn terrain_config_null_is_none() {
        assert!(terrain_config_from_petal_json(&serde_json::Value::Null).is_none());
    }

    #[test]
    fn terrain_config_garbage_is_none() {
        let v = serde_json::json!({ "enabled": "yes", "origin": 42 });
        assert!(terrain_config_from_petal_json(&v).is_none());
    }

    #[test]
    fn config_for_tileset_centers_origin_and_binds_uri() {
        let cfg = config_for_tileset(&info());
        assert!(cfg.enabled);
        assert!((cfg.origin.origin_lat - 45.5).abs() < 1e-9);
        assert!((cfg.origin.origin_lon - (-121.5)).abs() < 1e-9);
        assert_eq!(cfg.origin.origin_ele, 0.0);
        assert_eq!(cfg.min_zoom, 10);
        assert_eq!(cfg.max_zoom, 12);
        assert_eq!(cfg.tileset_hexon_uris, vec!["ts-1".to_string()]);
        assert_eq!(cfg.elevation_source, ElevationSourceKind::TerrainRgb);
        assert_eq!(cfg.tile_source_mode, TileSourceMode::Hybrid);
        assert_eq!(cfg.layers.len(), 2);
        assert!(cfg.layers.iter().all(|l| l.visible));
        assert_eq!(cfg.layers[0].name, "satellite");
        assert_eq!(cfg.layers[1].name, "terrain");
    }

    #[test]
    fn config_for_tileset_without_encoding_is_flat() {
        let mut i = info();
        i.elevation_encoding = None;
        let cfg = config_for_tileset(&i);
        assert_eq!(cfg.elevation_source, ElevationSourceKind::None);
    }

    #[test]
    fn config_for_tileset_sets_world_scale_from_native_scale() {
        let mut i = info();
        i.native_scale = Some(0.001);
        i.scale_bounds = Some([0.0001, 0.01]);
        let cfg = config_for_tileset(&i);
        assert_eq!(cfg.world_scale, 0.001);
        assert_eq!(cfg.scale_bounds, Some([0.0001, 0.01]));
    }

    #[test]
    fn config_for_tileset_without_native_scale_defaults_to_human_scale() {
        let cfg = config_for_tileset(&info());
        assert_eq!(cfg.world_scale, 1.0);
        assert!(cfg.scale_bounds.is_none());
    }

    #[test]
    fn user_world_scale_outside_bounds_is_clamped_via_effective_world_scale() {
        let mut i = info();
        i.native_scale = Some(0.001);
        i.scale_bounds = Some([0.0001, 0.01]);
        let mut cfg = config_for_tileset(&i);
        // Simulate a user nudge outside hexon bounds.
        cfg.world_scale = 5.0;
        assert_eq!(cfg.effective_world_scale(), 0.01);
    }
}
