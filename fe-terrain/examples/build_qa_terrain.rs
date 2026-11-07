//! Build and install the offline QA terrain; see examples/AGENTS.md.

use std::{io::Cursor, path::PathBuf};

use anyhow::{ensure, Context, Result};
use fe_format::{manifest::ElevationEncoding, manifest::TilesetMeta, HexonArchive, HexonManifest};
use fe_terrain::{
    config::TileSourceMode,
    petal_binding::config_for_tileset,
    projection::Projection,
    tiles::{assemble_bundles, bundle_tileset, HexonStore, TileCoord, TilesetRegistry},
};
use image::{ImageFormat, Rgb, RgbImage};
use serde_json::json;

/// Named encoded tile payloads in generation order.
type NamedTiles = Vec<(String, Vec<u8>)>;

const TILESET_ID: &str = "qa-analytic-terrain";
const ZOOM: u8 = 17;
const PIXELS: u32 = 256;
const TILE_COUNT: usize = 9;
const HILL_HEIGHT: f64 = 8.0;
const HILL_RADIUS: f64 = 40.0;
const HILL_CENTER: [f64; 2] = [80.0, 80.0];

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let run = PathBuf::from(args.next().context("usage: build_qa_terrain <run-root>")?);
    ensure!(args.next().is_none(), "expected only the run root");
    let run = run.canonicalize().context("QA run root must exist")?;
    ensure!(run.is_dir(), "QA run root must be a directory");
    let output = run.join("terrain-fixture");
    std::fs::create_dir_all(&output)?;

    let center = TileCoord::from_lat_lon(0.0, 0.0, ZOOM);
    let (north, west) = TileCoord::new(center.x - 1, center.y - 1, ZOOM).to_lat_lon();
    let (south, east) = TileCoord::new(center.x + 2, center.y + 2, ZOOM).to_lat_lon();
    let bounds = [south, west, north, east];
    let origin = Projection::new((south + north) / 2.0, (west + east) / 2.0, 0.0);
    let (tiles, flat_samples, peak_m) = elevation_tiles(center, &origin)?;
    ensure!(
        tiles.len() == TILE_COUNT && flat_samples > 0,
        "empty fixture coverage"
    );
    ensure!(peak_m > 7.0, "hill did not survive TerrainRGB encoding");

    let gsd = fe_terrain::splat::format::tile_world_size_m(origin.origin_lat, ZOOM) / PIXELS as f64;
    let meta = TilesetMeta {
        bounds,
        min_zoom: ZOOM,
        max_zoom: ZOOM,
        tile_size: PIXELS as u16,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        has_satellite: false,
        tile_count: TILE_COUNT as u32,
        satellite_tile_count: 0,
        region_name: "QA flat sector and analytic tent hill".into(),
        native_scale: Some(1.0),
        ground_sample_distance_m: Some(gsd),
        crs: Some("EPSG:4326".into()),
        scale_bounds: Some([1.0, 1.0]),
        ..Default::default()
    };
    let archive = HexonArchive::export_tileset(manifest()?, &meta, &tiles, &[], None)?;
    std::fs::write(output.join("source.hexon"), &archive)?;
    let source = HexonArchive::import(&archive)?;
    let bundles = bundle_tileset(&source, 32 * 1024)?;
    ensure!(!bundles.bundles.is_empty(), "no generated bundles");
    std::fs::write(output.join("index.json"), &bundles.index_json)?;
    let mut paths = Vec::new();
    for (index, bytes) in bundles.bundles.iter().enumerate() {
        let path = output.join(format!("bundle-{index}.hexon"));
        std::fs::write(&path, bytes)?;
        paths.push(path);
    }
    let store = HexonStore::with_dir(run.join("hexons"))?;
    let installed = assemble_bundles(&store, TILESET_ID, &bundles.index, &paths)?;
    ensure!(
        installed.hexon_id == TILESET_ID,
        "installed a different tileset"
    );
    let registry = TilesetRegistry::new(store);
    ensure!(
        registry.load_all().contains(&TILESET_ID.to_owned()),
        "installed terrain not indexed"
    );
    for (key, expected) in &tiles {
        let parts: Vec<u32> = key
            .split('/')
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        let actual = registry
            .get_tile(TILESET_ID, parts[0] as u8, parts[1], parts[2])
            .with_context(|| format!("installed tile missing: {key}"))?;
        ensure!(actual == *expected, "installed tile differs: {key}");
    }
    let info = registry
        .list_tilesets()
        .into_iter()
        .find(|item| item.tileset_id == TILESET_ID)
        .context("installed tileset metadata missing")?;
    let mut config = config_for_tileset(&info);
    config.tile_source_mode = TileSourceMode::Offline;
    config.cache_dir = "terrain_cache/qa-analytic".into();
    ensure!(config.world_scale == 1.0, "unexpected terrain scale");
    let descriptor = json!({
        "schema": "fe-qa-terrain-v1", "tileset_id": TILESET_ID,
        "source_blake3": blake3::hash(&archive).to_hex().to_string(),
        "tile_count": TILE_COUNT, "tile_size": PIXELS, "zoom": ZOOM,
        "center_tile": center, "bounds": bounds, "origin": origin,
        "ground_sample_distance_m": gsd, "encoding_quantization_m": 0.1,
        "flat_pixel_samples": flat_samples, "encoded_peak_m": peak_m,
        "hill": {"center_xz_m": HILL_CENTER, "height_m": HILL_HEIGHT,
            "radius_m": HILL_RADIUS, "analytic_volume_m3": HILL_HEIGHT * HILL_RADIUS.powi(2),
            "purpose": "visual landmark; raster quantization excludes exact hill-volume acceptance"},
        "oracle": {"footprint_xz_m": [[-5,-5],[5,-5],[5,5],[-5,5]],
            "base_elevation_m": 0, "world_scale": 1, "area_m2": 100,
            "lattice_axis": 128, "expected_sample_count": 16384,
            "raw_absolute_tolerance": 0.001,
            "raise": {"positive_delta_m": 2, "cut_m3": 0, "fill_m3": 200, "net_m3": 200},
            "lower": {"positive_delta_m": 1, "cut_m3": 100, "fill_m3": 0, "net_m3": -100}},
        "query": {"values": [10,20,30], "expected_count": 3, "expected_sum": 60},
        "terrain_config": config
    });
    let descriptor_path = output.join("descriptor.json");
    std::fs::write(&descriptor_path, serde_json::to_vec_pretty(&descriptor)?)?;
    println!("{}", descriptor_path.display());
    Ok(())
}

fn manifest() -> Result<HexonManifest> {
    Ok(serde_json::from_value(json!({
        "schema_version": "1.0.0", "hexon_id": TILESET_ID, "hexon_type": "terrain_tileset",
        "publisher_did": "local:qa-fixture", "version": "1.0.0", "name": "QA analytic terrain",
        "description": "Offline synthetic TerrainRGB; not external geographic data",
        "created_at": "2026-11-01T00:00:00Z", "updated_at": "2026-11-01T00:00:00Z",
        "tags": ["qa", "synthetic", "offline"]
    }))?)
}

fn elevation_tiles(
    center: TileCoord,
    origin: &Projection,
) -> Result<(NamedTiles, usize, f64)> {
    let mut tiles = Vec::with_capacity(TILE_COUNT);
    let mut flat_samples = 0;
    let mut peak_m: f64 = 0.0;
    for y in center.y - 1..=center.y + 1 {
        for x in center.x - 1..=center.x + 1 {
            let coord = TileCoord::new(x, y, ZOOM);
            let (north, west) = coord.to_lat_lon();
            let (south, east) = TileCoord::new(x + 1, y + 1, ZOOM).to_lat_lon();
            let mut image = RgbImage::new(PIXELS, PIXELS);
            for (px, py, pixel) in image.enumerate_pixels_mut() {
                let u = px as f64 / (PIXELS - 1) as f64;
                let v = py as f64 / (PIXELS - 1) as f64;
                let [local_x, _, local_z] = origin.wgs84_to_local(
                    north + (south - north) * v,
                    west + (east - west) * u,
                    0.0,
                )?;
                let tent_x = (1.0 - (local_x - HILL_CENTER[0]).abs() / HILL_RADIUS).max(0.0);
                let tent_z = (1.0 - (local_z - HILL_CENTER[1]).abs() / HILL_RADIUS).max(0.0);
                let height = (HILL_HEIGHT * tent_x * tent_z * 10.0).round() / 10.0;
                if local_x.abs() <= 8.0 && local_z.abs() <= 8.0 {
                    ensure!(height == 0.0, "oracle sector must remain flat");
                    flat_samples += 1;
                }
                peak_m = peak_m.max(height);
                let encoded = ((height + 10_000.0) * 10.0).round() as u32;
                *pixel = Rgb([(encoded >> 16) as u8, (encoded >> 8) as u8, encoded as u8]);
            }
            let mut png = Cursor::new(Vec::new());
            image.write_to(&mut png, ImageFormat::Png)?;
            tiles.push((coord.cache_key(), png.into_inner()));
        }
    }
    Ok((tiles, flat_samples, peak_m))
}
