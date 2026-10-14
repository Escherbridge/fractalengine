//! Lazy offset-index tile sources, shared `Arc` residency and exit-ring byte eviction
//! (p2p_asset_streaming S5); see `fe-terrain/src/AGENTS.md` §lazy-tiles / §tile-residency.

use std::path::PathBuf;

use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
use fe_format::HexonArchive;
use fe_terrain::splat::format::{
    append_baked_splats_to_archive, encode_baked_splats, BakedSplatBuffer,
};
use fe_terrain::tiles::{HexonStore, HexonTileSource, Probe, TileCoord, TileLayer};

/// Fresh scratch directory under the system temp dir.
fn scratch_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "fe_lazy_tiles_{tag}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn manifest(hexon_id: &str) -> HexonManifest {
    HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: hexon_id.into(),
        hexon_type: HexonType::TerrainTileset,
        publisher_did: "did:key:z6MkLazyTiles".into(),
        publisher_name: None,
        version: "0.1.0".into(),
        build_id: None,
        name: "Lazy tiles".into(),
        description: None,
        tags: vec![],
        created_at: "2026-10-13T00:00:00Z".into(),
        updated_at: "2026-10-13T00:00:00Z".into(),
        source_peer_did: None,
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        integrity: None,
        signature: None,
    }
}

/// A Terrain-RGB PNG whose every pixel decodes to `elevation_m`.
fn terrain_rgb_png(elevation_m: f64, size: u32) -> Vec<u8> {
    let v = ((elevation_m + 10_000.0) * 10.0).round() as u32;
    let pixel = image::Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
    let img = image::RgbImage::from_pixel(size, size, pixel);
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("encode png");
    out
}

/// `[min_lat, min_lon, max_lat, max_lon]` of the tile block `[x0, x1) × [y0, y1)` at `zoom`.
fn block_bounds(zoom: u8, x0: u32, x1: u32, y0: u32, y1: u32) -> [f64; 4] {
    let (north, west) = TileCoord::new(x0, y0, zoom).to_lat_lon();
    let (south, east) = TileCoord::new(x1, y1, zoom).to_lat_lon();
    [south, west, north, east]
}

/// `(cache_key, bytes)` members of one tile layer.
type Members = Vec<(String, Vec<u8>)>;

/// Mixed-layer tileset: elevation + satellite for every key, baked splats for the first.
fn mixed_archive(hexon_id: &str) -> (Vec<u8>, Members, Members) {
    let zoom = 12u8;
    let keys: Vec<(u32, u32)> = (0..4).flat_map(|x| (0..3).map(move |y| (x, y))).collect();
    let (bx, by) = (655u32, 1420u32);
    let elevation: Vec<(String, Vec<u8>)> = keys
        .iter()
        .map(|(dx, dy)| {
            (
                format!("{zoom}/{}/{}", bx + dx, by + dy),
                terrain_rgb_png(100.0 + (*dx * 10 + *dy) as f64, 8),
            )
        })
        .collect();
    let satellite: Vec<(String, Vec<u8>)> = keys
        .iter()
        .map(|(dx, dy)| {
            let len = 512 + (*dx * 97 + *dy * 31) as usize;
            (
                format!("{zoom}/{}/{}", bx + dx, by + dy),
                (0..len).map(|i| (i as u32 * 7 + dx + dy) as u8).collect(),
            )
        })
        .collect();
    let meta = TilesetMeta {
        bounds: block_bounds(zoom, bx, bx + 4, by, by + 3),
        min_zoom: zoom,
        max_zoom: zoom,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        has_satellite: true,
        tile_count: elevation.len() as u32,
        satellite_tile_count: satellite.len() as u32,
        region_name: "Lazy region".into(),
        ..Default::default()
    };
    let base =
        HexonArchive::export_tileset(manifest(hexon_id), &meta, &elevation, &satellite, None)
            .expect("export tileset");
    let splats = BakedSplatBuffer {
        version: 1,
        positions: vec![[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
        colors: vec![[1.0; 4], [0.5; 4]],
        scales: vec![[1.0; 2], [2.0; 2]],
        normals: vec![[0.0, 1.0, 0.0]; 2],
    };
    let archive = append_baked_splats_to_archive(
        &base,
        &[(
            elevation[0].0.clone(),
            encode_baked_splats(&splats).expect("encode splats"),
        )],
    )
    .expect("append splats");
    (archive, elevation, satellite)
}

fn coord_of(cache_key: &str) -> TileCoord {
    let parts: Vec<u32> = cache_key.split('/').map(|p| p.parse().unwrap()).collect();
    TileCoord::new(parts[1], parts[2], parts[0] as u8)
}

#[test]
fn a_lazy_source_serves_exactly_what_the_eager_import_did() {
    let dir = scratch_dir("eager_vs_lazy");
    let (archive, elevation, satellite) = mixed_archive("lazy-vs-eager");
    let path = dir.join("lazy-vs-eager.hexon");
    std::fs::write(&path, &archive).unwrap();

    let eager = HexonArchive::import(&archive).expect("eager import");
    let eager_source = HexonTileSource::from_archive(&archive).expect("eager source");
    let lazy = HexonTileSource::open(&path).expect("lazy open");
    assert!(lazy.is_lazy() && !eager_source.is_lazy());
    assert_eq!(lazy.tileset_meta, eager.tileset_meta.clone().unwrap());
    assert_eq!(
        lazy.resident_bytes(),
        0,
        "opening reads the header only — no tile bytes"
    );

    // Table: every member of every layer, cold → loaded → resident, byte-equal to the import.
    let layers = [
        (TileLayer::Elevation, &eager.elevation_tiles, &elevation),
        (TileLayer::Satellite, &eager.satellite_tiles, &satellite),
    ];
    for (layer, imported, generated) in layers {
        assert_eq!(imported.len(), generated.len());
        assert_eq!(
            lazy.member_keys(layer),
            eager_source.member_keys(layer),
            "{layer:?}"
        );
        for (key, bytes) in imported.iter() {
            let coord = coord_of(key);
            assert!(
                matches!(lazy.probe(layer, coord), Probe::Miss),
                "{layer:?} {key} must start cold"
            );
            let loaded = lazy
                .load_layer_blocking(layer, coord)
                .expect("member read")
                .and_then(|value| value.into_bytes())
                .unwrap_or_else(|| panic!("{layer:?} {key} missing"));
            assert_eq!(&*loaded, bytes.as_slice(), "{layer:?} {key}");
            let resident = lazy
                .get_layer(layer, coord)
                .and_then(|value| value.into_bytes())
                .expect("resident after load");
            assert_eq!(&*resident, bytes.as_slice(), "{layer:?} {key} resident");
        }
    }

    // Baked splats load the same lazy way, decoded identically.
    let splat_coord = coord_of(&elevation[0].0);
    assert!(
        lazy.get_baked_splats(splat_coord).is_none(),
        "splats start cold"
    );
    let lazy_splats = lazy
        .load_baked_splats_blocking(splat_coord)
        .expect("splat read")
        .expect("splats present");
    let eager_splats = eager_source.get_baked_splats(splat_coord).unwrap();
    assert_eq!(lazy_splats.positions, eager_splats.positions);
    assert_eq!(lazy_splats.colors, eager_splats.colors);

    // A coordinate outside the archive is absent without any read.
    let outside = TileCoord::new(1, 1, 12);
    assert!(matches!(
        lazy.probe(TileLayer::Elevation, outside),
        Probe::Absent
    ));
    assert!(lazy.load_tile_blocking(outside).unwrap().is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reinstalled_archive_is_reindexed_and_serves_the_new_bytes() {
    let dir = scratch_dir("reinstall");
    let store = HexonStore::with_dir(dir.clone()).unwrap();
    let v1 = vec![
        ("12/10/10".to_string(), terrain_rgb_png(10.0, 8)),
        ("12/11/10".to_string(), terrain_rgb_png(11.0, 8)),
    ];
    let v2 = vec![
        ("12/10/10".to_string(), terrain_rgb_png(20.0, 16)),
        ("12/11/10".to_string(), terrain_rgb_png(21.0, 16)),
    ];
    let archive = |tiles: &[(String, Vec<u8>)]| {
        let meta = TilesetMeta {
            bounds: block_bounds(12, 10, 12, 10, 11),
            min_zoom: 12,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 2,
            region_name: "Reinstall".into(),
            ..Default::default()
        };
        HexonArchive::export_tileset(manifest("reinstall"), &meta, tiles, &[], None).unwrap()
    };
    store.install_tileset(&archive(&v1)).unwrap();
    let source = store.load_tileset("reinstall").unwrap();
    let first = TileCoord::new(10, 10, 12);
    let second = TileCoord::new(11, 10, 12);
    assert_eq!(
        &*source.load_tile_blocking(first).unwrap().unwrap(),
        v1[0].1.as_slice()
    );

    // The same id is reinstalled in place while the old source is still in use.
    store.install_tileset(&archive(&v2)).unwrap();
    let cold = source
        .load_tile_blocking(second)
        .expect("a replaced file re-indexes instead of failing");
    assert_eq!(
        &*cold.unwrap(),
        v2[1].1.as_slice(),
        "new archive's bytes, never offsets misapplied to it"
    );
    assert_eq!(
        &*source.load_tile_blocking(first).unwrap().unwrap(),
        v2[0].1.as_slice(),
        "stale unpinned bytes were dropped with the old index"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(feature = "render")]
mod bevy_flows {
    use std::sync::Arc;
    use std::time::Duration;

    use bevy::prelude::*;
    use bevy::transform::TransformPlugin;
    use fe_renderer::terrain_height::TerrainHeightField;
    use fe_terrain::config::{ElevationSourceKind, TerrainConfig, TileSourceMode};
    use fe_terrain::layers::LayerStack;
    use fe_terrain::petal_binding::{
        apply_terrain_assignments, ActivePetalTerrain, ActiveTileSource, PetalTerrainAssignment,
        SharedTilesetRegistry, TerrainAssignmentMsg,
    };
    use fe_terrain::projection::Projection as GeoProjection;
    use fe_terrain::terrain_plugin::{
        fetch_and_spawn_terrain_chunks, update_terrain_lod, FailedTiles, TerrainChunk,
        TerrainLodConfig,
    };
    use fe_terrain::tiles::{TileResidency, TilesetRegistry};

    use super::*;

    const ZOOM: u8 = 14;
    const TILESET: &str = "lazy-ring";

    /// A 5×5 block of zoom-14 tiles around the tile holding (0.01°, 0.01°).
    struct Fixture {
        dir: PathBuf,
        registry: Arc<TilesetRegistry>,
        center: TileCoord,
        origin: (f64, f64),
    }

    impl Fixture {
        fn install() -> Self {
            let dir = scratch_dir("ring");
            let center = TileCoord::from_lat_lon(0.01, 0.01, ZOOM);
            let mut tiles = Vec::new();
            for dx in 0..5u32 {
                for dy in 0..5u32 {
                    let (x, y) = (center.x + dx - 2, center.y + dy - 2);
                    let elevation = 100.0 + (dx * 10 + dy) as f64;
                    tiles.push((format!("{ZOOM}/{x}/{y}"), terrain_rgb_png(elevation, 8)));
                }
            }
            let meta = TilesetMeta {
                bounds: block_bounds(ZOOM, center.x - 2, center.x + 3, center.y - 2, center.y + 3),
                min_zoom: ZOOM,
                max_zoom: ZOOM,
                tile_size: 256,
                elevation_encoding: ElevationEncoding::TerrainRgb,
                tile_count: tiles.len() as u32,
                region_name: "Ring".into(),
                ..Default::default()
            };
            let archive =
                HexonArchive::export_tileset(manifest(TILESET), &meta, &tiles, &[], None).unwrap();
            let store = HexonStore::with_dir(dir.clone()).unwrap();
            store.install_tileset(&archive).unwrap();
            let registry = Arc::new(TilesetRegistry::new(store));
            assert_eq!(registry.load_all(), vec![TILESET.to_string()]);

            // Origin at the center tile's middle, so the camera above it sees a 3×3 ring.
            let (north, west) = center.to_lat_lon();
            let (south, east) = TileCoord::new(center.x + 1, center.y + 1, ZOOM).to_lat_lon();
            Self {
                dir,
                registry,
                center,
                origin: ((north + south) / 2.0, (west + east) / 2.0),
            }
        }

        fn config(&self) -> TerrainConfig {
            TerrainConfig {
                enabled: true,
                // origin_ele = 0 → grounded from the center tile, off-thread.
                origin: GeoProjection::new(self.origin.0, self.origin.1, 0.0),
                elevation_source: ElevationSourceKind::TerrainRgb,
                min_zoom: ZOOM,
                max_zoom: ZOOM,
                cache_dir: String::new(),
                tileset_hexon_uris: vec![TILESET.to_string()],
                tile_source_mode: TileSourceMode::Hybrid,
                ..TerrainConfig::default()
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Headless app running the real assignment → LOD → residency/spawn chain.
    fn terrain_app(registry: &Arc<TilesetRegistry>) -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), TransformPlugin))
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .init_asset::<Image>()
            .add_message::<TerrainAssignmentMsg>()
            .init_resource::<TerrainLodConfig>()
            .init_resource::<ActivePetalTerrain>()
            .init_resource::<ActiveTileSource>()
            .init_resource::<FailedTiles>()
            .init_resource::<TileResidency>()
            .init_resource::<TerrainHeightField>()
            // S7: `fetch_and_spawn_terrain_chunks` now requires this
            // (streamed-tileset tile pointer-set demand), even though this
            // suite has no streamed sources.
            .init_resource::<fe_terrain::tiles::TileBundleDemand>()
            .insert_resource(LayerStack::new())
            .insert_resource(SharedTilesetRegistry(Arc::clone(registry)))
            .add_systems(
                Update,
                (
                    apply_terrain_assignments,
                    update_terrain_lod,
                    fetch_and_spawn_terrain_chunks,
                )
                    .chain(),
            );
        app
    }

    fn assign(app: &mut App, config: TerrainConfig) {
        app.world_mut()
            .write_message(TerrainAssignmentMsg(PetalTerrainAssignment {
                petal_id: "petal-lazy".into(),
                config: Some(config),
            }));
    }

    fn spawn_camera(app: &mut App, at: Vec3) -> Entity {
        let transform =
            Transform::from_translation(at).looking_at(Vec3::new(at.x, 0.0, at.z - 1.0), Vec3::Y);
        app.world_mut()
            .spawn((
                Camera::default(),
                Projection::Perspective(PerspectiveProjection::default()),
                transform,
                GlobalTransform::from(transform),
            ))
            .id()
    }

    fn move_camera(app: &mut App, camera: Entity, at: Vec3) {
        let transform =
            Transform::from_translation(at).looking_at(Vec3::new(at.x, 0.0, at.z - 1.0), Vec3::Y);
        let mut entity = app.world_mut().entity_mut(camera);
        entity.insert((transform, GlobalTransform::from(transform)));
    }

    fn chunk_keys(app: &mut App) -> Vec<(u8, u32, u32)> {
        let world = app.world_mut();
        let mut keys: Vec<_> = world
            .query::<&TerrainChunk>()
            .iter(world)
            .map(|c| c.tile_coords)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Step frames until `done` holds (loads land on IoTaskPool threads between frames).
    fn run_until(app: &mut App, what: &str, mut done: impl FnMut(&mut App) -> bool) {
        for _ in 0..2_000 {
            app.update();
            if done(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for {what}");
    }

    fn ring_of(center: TileCoord) -> Vec<(u8, u32, u32)> {
        let mut ring: Vec<_> = (0..3u32)
            .flat_map(|dx| (0..3u32).map(move |dy| (ZOOM, center.x + dx - 1, center.y + dy - 1)))
            .collect();
        ring.sort_unstable();
        ring
    }

    #[test]
    fn registry_and_active_petal_share_one_source() {
        let fixture = Fixture::install();
        let mut app = terrain_app(&fixture.registry);
        assign(&mut app, fixture.config());
        app.update();

        let registry_source = fixture
            .registry
            .source(TILESET)
            .expect("indexed at startup");
        let shared = |app: &App| {
            let tile_source = app.world().resource::<ActiveTileSource>();
            let composite = tile_source.composite.as_ref().expect("composite built");
            Arc::clone(&composite.hexon_sources()[0])
        };
        assert!(
            Arc::ptr_eq(&registry_source, &shared(&app)),
            "the petal renders from the registry's own source, not a second copy"
        );

        // A registry that never indexed the tileset registers the one it opens.
        let cold_registry = Arc::new(TilesetRegistry::new(
            HexonStore::with_dir(fixture.dir.clone()).unwrap(),
        ));
        assert!(cold_registry.source(TILESET).is_none());
        let mut cold_app = terrain_app(&cold_registry);
        assign(&mut cold_app, fixture.config());
        cold_app.update();
        let registered = cold_registry
            .source(TILESET)
            .expect("registered on first use");
        assert!(Arc::ptr_eq(&registered, &shared(&cold_app)));
    }

    #[test]
    fn a_cold_tile_is_pending_then_spawns_never_failed() {
        let fixture = Fixture::install();
        let mut app = terrain_app(&fixture.registry);
        spawn_camera(&mut app, Vec3::new(0.0, 300.0, 0.0));
        assign(&mut app, fixture.config());
        app.update();

        assert!(
            chunk_keys(&mut app).is_empty(),
            "nothing is resident on frame one"
        );
        let failed = app.world().resource::<FailedTiles>();
        assert!(
            !failed.contains(fixture.center) && failed.is_empty(),
            "a tile whose load is in flight is pending, not failed"
        );

        let want = ring_of(fixture.center);
        run_until(&mut app, "the 3x3 ring to spawn", |app| {
            chunk_keys(app) == want
        });
        assert!(app.world().resource::<FailedTiles>().is_empty());

        // Grounding waited for the center tile (elevation 100 + 2*10 + 2 = 122 m).
        let origin_ele = app
            .world()
            .resource::<ActivePetalTerrain>()
            .config
            .as_ref()
            .unwrap()
            .origin
            .origin_ele;
        assert!((origin_ele - 122.0).abs() < 0.2, "grounded at {origin_ele}");
        let projection_ele = app
            .world()
            .resource::<ActiveTileSource>()
            .projection
            .as_ref()
            .unwrap()
            .origin_ele;
        assert_eq!(projection_ele, origin_ele, "projection and config agree");
    }

    #[test]
    fn tile_bytes_leave_with_the_exit_ring_and_return_on_reentry() {
        let fixture = Fixture::install();
        let source = fixture.registry.source(TILESET).unwrap();
        let mut app = terrain_app(&fixture.registry);
        let home = Vec3::new(0.0, 300.0, 0.0);
        let camera = spawn_camera(&mut app, home);
        assign(&mut app, fixture.config());

        let ring = ring_of(fixture.center);
        run_until(&mut app, "the home ring", |app| chunk_keys(app) == ring);
        assert_eq!(
            source.pinned_len(),
            ring.len(),
            "one pinned tile per live chunk"
        );
        let resident_home = source.resident_bytes();
        assert!(resident_home > 0);
        let probes = [(0.0, 0.0), (800.0, -600.0), (-1500.0, 1500.0)];
        let heights = |app: &App| -> Vec<Option<f32>> {
            let field = app.world().resource::<TerrainHeightField>();
            probes
                .iter()
                .map(|(x, z)| field.height_at(*x, *z))
                .collect()
        };
        let home_heights = heights(&app);
        assert!(home_heights.iter().all(Option::is_some), "{home_heights:?}");

        // Fly ~40 tiles east: far past 1.5 × the spawn radius, over unmapped ground.
        move_camera(&mut app, camera, Vec3::new(100_000.0, 300.0, 0.0));
        run_until(&mut app, "the home ring to leave", |app| {
            chunk_keys(app).is_empty()
        });
        app.update();
        assert_eq!(source.pinned_len(), 0);
        assert_eq!(
            source.resident_bytes(),
            0,
            "no tile bytes stay resident once their chunks left the exit ring"
        );

        // Fly back: the same tiles reload and rebuild the same ground.
        move_camera(&mut app, camera, home);
        run_until(&mut app, "the home ring to return", |app| {
            chunk_keys(app) == ring
        });
        assert_eq!(heights(&app), home_heights);
        assert_eq!(source.resident_bytes(), resident_home);
    }
}
