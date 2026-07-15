//! Shared builders for the repo-root `sample-hexons/` sources — see `sample-hexons/README.md`.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fe_format::{
    AssetEntry, EntryKind, ExportNode, HexonArchive, HexonManifest, License, SchemaDefinition,
    TilesetMeta,
};
use image::{ImageBuffer, ImageFormat, Rgb};

/// Sample source directory names.
pub const ALPINE: &str = "alpine-demo-terrain";
pub const MORNING_RUN: &str = "morning-run-gpx";
pub const LAKESIDE: &str = "lakeside-scene";

/// Repo-root `sample-hexons/` directory.
pub fn samples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../sample-hexons")
}

/// Parse a JSON source file into `T`.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("invalid JSON in {}", path.display()))
}

/// Zurich demo tile coordinates: 2x2 blocks at zooms 10-12 around (47.3769, 8.5417).
pub fn demo_tile_coords() -> Vec<(u8, u32, u32)> {
    let anchors = [(10u8, 536u32, 358u32), (11, 1072, 717), (12, 2145, 1434)];
    let mut coords = Vec::new();
    for (z, x0, y0) in anchors {
        for dx in 0..2 {
            for dy in 0..2 {
                coords.push((z, x0 + dx, y0 + dy));
            }
        }
    }
    coords
}

/// 64x64 Terrain-RGB PNG encoding a gentle slope off a ~400 m base.
fn elevation_tile_png(z: u8, x: u32, y: u32) -> Result<Vec<u8>> {
    let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(64, 64);
    for (px, py, pixel) in img.enumerate_pixels_mut() {
        let elevation = 400.0
            + f64::from(z)
            + f64::from((x + y) % 2) * 20.0
            + f64::from(px) * 0.5
            + f64::from(py) * 0.25;
        let encoded = ((elevation + 10000.0) / 0.1).round() as u32;
        *pixel = Rgb([(encoded >> 16) as u8, (encoded >> 8) as u8, encoded as u8]);
    }
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .context("elevation PNG encode failed")?;
    Ok(buf.into_inner())
}

/// 64x64 flat-color JPEG "satellite" tile, shaded per tile coordinate.
fn satellite_tile_jpg(x: u32, y: u32) -> Result<Vec<u8>> {
    let shade = (((x + y) % 4) * 16) as u8;
    let img: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(64, 64, Rgb([60 + shade, 110 + shade, 70 + shade]));
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Jpeg)
        .context("satellite JPEG encode failed")?;
    Ok(buf.into_inner())
}

/// Build `alpine-demo-terrain.hexon` — tileset with generated tiles, via `export_tileset`.
pub fn build_alpine_demo_terrain(root: &Path) -> Result<Vec<u8>> {
    let dir = root.join(ALPINE);
    let manifest: HexonManifest = read_json(&dir.join("manifest.json"))?;
    let mut meta: TilesetMeta = read_json(&dir.join("tileset.json"))?;

    let mut elevation = Vec::new();
    let mut satellite = Vec::new();
    for (z, x, y) in demo_tile_coords() {
        let cache_key = format!("{z}/{x}/{y}");
        elevation.push((cache_key.clone(), elevation_tile_png(z, x, y)?));
        satellite.push((cache_key, satellite_tile_jpg(x, y)?));
    }
    meta.tile_count = elevation.len() as u32;
    meta.satellite_tile_count = satellite.len() as u32;
    meta.has_satellite = !satellite.is_empty();

    HexonArchive::export_tileset(
        manifest,
        &meta,
        &elevation,
        &satellite,
        Some(License::default()),
    )
}

/// Build `morning-run-gpx.hexon` — GPX collection with terrain config, via `export`.
pub fn build_morning_run_gpx(root: &Path) -> Result<Vec<u8>> {
    let dir = root.join(MORNING_RUN);
    let manifest: HexonManifest = read_json(&dir.join("manifest.json"))?;
    let terrain_config: serde_json::Value = read_json(&dir.join("terrain/config.json"))?;
    let gpx_path = dir.join("terrain/tracks/morning_run.gpx");
    let gpx_bytes = std::fs::read(&gpx_path)
        .with_context(|| format!("failed to read {}", gpx_path.display()))?;

    let entry = AssetEntry {
        entry_id: "morning_run".into(),
        kind: EntryKind::GpxTrack,
        asset_hash: blake3::hash(&gpx_bytes).to_hex().to_string(),
        format: "gpx".into(),
        label: "Morning Run — Lake Zurich".into(),
        tags: vec!["gpx".into(), "running".into()],
        description: Some("30-point run along the Lake Zurich shore".into()),
        is_placeable: false,
        is_private: false,
        auto_scale: false,
        preview_image: None,
        center: None,
        extents: None,
        sub_assets: None,
        address: None,
        metadata: Some(
            serde_json::json!({ "path": "terrain/tracks/morning_run.gpx", "points": 30 }),
        ),
    };

    HexonArchive::export(
        manifest,
        vec![entry],
        vec![],
        Some(License::default()),
        Some(&terrain_config),
        Some(&[("morning_run.gpx".to_string(), gpx_bytes)]),
        None,
    )
}

/// Build `lakeside-scene.hexon` — scene nodes + schema, via `export_scene`.
pub fn build_lakeside_scene(root: &Path) -> Result<Vec<u8>> {
    let dir = root.join(LAKESIDE);
    let manifest: HexonManifest = read_json(&dir.join("manifest.json"))?;
    let nodes: Vec<ExportNode> = read_json(&dir.join("entities/nodes.json"))?;
    let schema: SchemaDefinition = read_json(&dir.join("schema.json"))?;

    HexonArchive::export_scene(nodes, schema.field_defs, vec![], manifest)
}

/// Build all three samples, returning `(name, hexon_bytes)` pairs.
pub fn build_all(root: &Path) -> Result<Vec<(&'static str, Vec<u8>)>> {
    Ok(vec![
        (ALPINE, build_alpine_demo_terrain(root)?),
        (MORNING_RUN, build_morning_run_gpx(root)?),
        (LAKESIDE, build_lakeside_scene(root)?),
    ])
}
