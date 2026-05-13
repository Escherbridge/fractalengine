//! HexonArchive — `.hexon` ZIP archive read/write.
//!
//! Replaces the old `FractalArchive`. Handles both scene-type hexons (with
//! `entities/` directory) and generic hexons (models, materials, terrain, etc.).

use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;

use crate::{
    entries::{AssetEntry, EntryCatalog},
    license::License,
    manifest::HexonManifest,
    ExportNode, FieldDef, SchemaDefinition,
};

// ---------------------------------------------------------------------------
// Archive data
// ---------------------------------------------------------------------------

/// All data extracted from a `.hexon` archive.
#[derive(Debug, Clone)]
pub struct HexonArchiveData {
    pub manifest: HexonManifest,
    pub entries: Vec<AssetEntry>,
    pub license: Option<License>,
    pub nodes: Vec<ExportNode>,
    pub field_defs: Vec<FieldDef>,
    pub schema: SchemaDefinition,
    pub assets: Vec<(String, Vec<u8>)>,
    pub terrain_config: Option<serde_json::Value>,
    pub gpx_files: Vec<(String, Vec<u8>)>,
    pub geojson_overlays: Vec<(String, Vec<u8>)>,
    /// Tileset metadata (for `TerrainTileset` hexon type).
    pub tileset_meta: Option<crate::manifest::TilesetMeta>,
    /// Elevation tiles: `(cache_key, png_bytes)` where cache_key = `{z}/{x}/{y}`.
    pub elevation_tiles: Vec<(String, Vec<u8>)>,
    /// Satellite tiles: `(cache_key, jpg_bytes)` where cache_key = `{z}/{x}/{y}`.
    pub satellite_tiles: Vec<(String, Vec<u8>)>,
}

// ---------------------------------------------------------------------------
// HexonArchive
// ---------------------------------------------------------------------------

/// Builder for creating and reading `.hexon` ZIP archives.
pub struct HexonArchive;

impl HexonArchive {
    /// Export a scene-type hexon (nodes + field_defs + assets).
    ///
    /// Convenience wrapper that writes `entities/nodes.json`,
    /// `entities/field_defs.json`, and `schema.json` alongside the standard
    /// `manifest.json`, `entries.json`, and `license.json`.
    pub fn export_scene(
        nodes: Vec<ExportNode>,
        field_defs: Vec<FieldDef>,
        assets: Vec<(String, Vec<u8>)>,
        manifest: HexonManifest,
    ) -> Result<Vec<u8>> {
        // Build entries from assets
        let entries: Vec<AssetEntry> = assets
            .iter()
            .map(|(hash, _)| AssetEntry {
                entry_id: hash.clone(),
                kind: crate::entries::EntryKind::Model,
                asset_hash: hash.clone(),
                format: "bin".into(),
                label: hash.clone(),
                tags: vec![],
                description: None,
                is_placeable: false,
                is_private: false,
                auto_scale: false,
                preview_image: None,
                center: None,
                extents: None,
                sub_assets: None,
                address: None,
                metadata: None,
            })
            .collect();

        let schema = SchemaDefinition {
            field_defs: field_defs.clone(),
        };
        let license = License::default();

        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // manifest.json
        zip.start_file("manifest.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

        // entries.json
        let catalog = EntryCatalog { entries };
        zip.start_file("entries.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&catalog)?.as_bytes())?;

        // schema.json
        zip.start_file("schema.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&schema)?.as_bytes())?;

        // license.json
        zip.start_file("license.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&license)?.as_bytes())?;

        // entities/nodes.json
        zip.start_file("entities/nodes.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&nodes)?.as_bytes())?;

        // entities/field_defs.json
        zip.start_file("entities/field_defs.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&field_defs)?.as_bytes())?;

        // assets/{hash}
        for (hash, data) in &assets {
            zip.start_file(format!("assets/{hash}"), options)?;
            zip.write_all(data)?;
        }

        let cursor = zip.finish()?;
        Ok(cursor.into_inner())
    }

    /// Generic export for any hexon type.
    ///
    /// Writes `manifest.json`, `entries.json`, optional `license.json`,
    /// `assets/` blobs, and optionally a `terrain/` subdirectory with
    /// `config.json`, `tracks/*.gpx`, and `overlays/*.geojson`.
    pub fn export(
        manifest: HexonManifest,
        entries: Vec<AssetEntry>,
        assets: Vec<(String, Vec<u8>)>,
        license: Option<License>,
        terrain_config: Option<&serde_json::Value>,
        gpx_files: Option<&[(String, Vec<u8>)]>,
        geojson_overlays: Option<&[(String, Vec<u8>)]>,
    ) -> Result<Vec<u8>> {
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // manifest.json
        zip.start_file("manifest.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

        // entries.json
        let catalog = EntryCatalog { entries };
        zip.start_file("entries.json", options)?;
        zip.write_all(serde_json::to_string_pretty(&catalog)?.as_bytes())?;

        // license.json (optional)
        if let Some(lic) = &license {
            zip.start_file("license.json", options)?;
            zip.write_all(serde_json::to_string_pretty(lic)?.as_bytes())?;
        }

        // assets/{hash}
        for (hash, data) in &assets {
            zip.start_file(format!("assets/{hash}"), options)?;
            zip.write_all(data)?;
        }

        // terrain/ subdirectory (optional)
        if let Some(config) = terrain_config {
            zip.start_file("terrain/config.json", options)?;
            zip.write_all(serde_json::to_string_pretty(config)?.as_bytes())?;
        }
        if let Some(gpx) = gpx_files {
            for (name, data) in gpx {
                zip.start_file(format!("terrain/tracks/{name}"), options)?;
                zip.write_all(data)?;
            }
        }
        if let Some(overlays) = geojson_overlays {
            for (name, data) in overlays {
                zip.start_file(format!("terrain/overlays/{name}"), options)?;
                zip.write_all(data)?;
            }
        }

        let cursor = zip.finish()?;
        Ok(cursor.into_inner())
    }

    /// Export a terrain tileset hexon.
    ///
    /// Writes `manifest.json`, `terrain/tileset_meta.json`,
    /// `terrain/tiles/{z}/{x}/{y}.png` for elevation, and optionally
    /// `terrain/satellite/{z}/{x}/{y}.jpg` for imagery.
    pub fn export_tileset(
        manifest: HexonManifest,
        tileset_meta: &crate::manifest::TilesetMeta,
        elevation_tiles: &[(String, Vec<u8>)],
        satellite_tiles: &[(String, Vec<u8>)],
        license: Option<License>,
    ) -> Result<Vec<u8>> {
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        // Use Stored (no compression) for tile images — they're already compressed
        let stored_opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let deflate_opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        // manifest.json
        zip.start_file("manifest.json", deflate_opts)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

        // entries.json (empty for tilesets — tiles aren't individually addressable entries)
        let catalog = EntryCatalog { entries: vec![] };
        zip.start_file("entries.json", deflate_opts)?;
        zip.write_all(serde_json::to_string_pretty(&catalog)?.as_bytes())?;

        // license.json
        if let Some(lic) = &license {
            zip.start_file("license.json", deflate_opts)?;
            zip.write_all(serde_json::to_string_pretty(lic)?.as_bytes())?;
        }

        // terrain/tileset_meta.json
        zip.start_file("terrain/tileset_meta.json", deflate_opts)?;
        zip.write_all(serde_json::to_string_pretty(tileset_meta)?.as_bytes())?;

        // terrain/tiles/{z}/{x}/{y}.png — elevation
        for (cache_key, data) in elevation_tiles {
            zip.start_file(format!("terrain/tiles/{cache_key}.png"), stored_opts)?;
            zip.write_all(data)?;
        }

        // terrain/satellite/{z}/{x}/{y}.jpg — imagery
        for (cache_key, data) in satellite_tiles {
            zip.start_file(format!("terrain/satellite/{cache_key}.jpg"), stored_opts)?;
            zip.write_all(data)?;
        }

        let cursor = zip.finish()?;
        Ok(cursor.into_inner())
    }

    /// Import a `.hexon` ZIP archive from bytes.
    ///
    /// Handles both scene hexons (with `entities/` directory) and non-scene
    /// hexons. Optional sections (`license.json`, `entities/`, `terrain/`,
    /// `schema.json`) default to empty/None if missing.
    pub fn import(bytes: &[u8]) -> Result<HexonArchiveData> {
        let cursor = Cursor::new(bytes);
        let mut archive =
            zip::ZipArchive::new(cursor).context("failed to open ZIP archive")?;

        // manifest.json (required)
        let manifest: HexonManifest = {
            let mut file = archive
                .by_name("manifest.json")
                .context("missing manifest.json")?;
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            serde_json::from_str(&buf).context("invalid manifest.json")?
        };

        // entries.json (required)
        let entries: Vec<AssetEntry> = {
            let mut file = archive
                .by_name("entries.json")
                .context("missing entries.json")?;
            let mut buf = String::new();
            file.read_to_string(&mut buf)?;
            let catalog: EntryCatalog =
                serde_json::from_str(&buf).context("invalid entries.json")?;
            catalog.entries
        };

        // license.json (optional)
        let license: Option<License> = match archive.by_name("license.json") {
            Ok(mut file) => {
                let mut buf = String::new();
                file.read_to_string(&mut buf)?;
                Some(serde_json::from_str(&buf).context("invalid license.json")?)
            }
            Err(_) => None,
        };

        // schema.json (optional)
        let schema: SchemaDefinition = match archive.by_name("schema.json") {
            Ok(mut file) => {
                let mut buf = String::new();
                file.read_to_string(&mut buf)?;
                serde_json::from_str(&buf).context("invalid schema.json")?
            }
            Err(_) => SchemaDefinition {
                field_defs: vec![],
            },
        };

        // entities/nodes.json (optional — scene type only)
        let nodes: Vec<ExportNode> = match archive.by_name("entities/nodes.json") {
            Ok(mut file) => {
                let mut buf = String::new();
                file.read_to_string(&mut buf)?;
                serde_json::from_str(&buf).context("invalid entities/nodes.json")?
            }
            Err(_) => vec![],
        };

        // entities/field_defs.json (optional — scene type only)
        let field_defs: Vec<FieldDef> =
            match archive.by_name("entities/field_defs.json") {
                Ok(mut file) => {
                    let mut buf = String::new();
                    file.read_to_string(&mut buf)?;
                    serde_json::from_str(&buf)
                        .context("invalid entities/field_defs.json")?
                }
                Err(_) => vec![],
            };

        // terrain/config.json (optional)
        let terrain_config: Option<serde_json::Value> =
            match archive.by_name("terrain/config.json") {
                Ok(mut file) => {
                    let mut buf = String::new();
                    file.read_to_string(&mut buf)?;
                    Some(
                        serde_json::from_str(&buf)
                            .context("invalid terrain/config.json")?,
                    )
                }
                Err(_) => None,
            };

        // Collect file names for pattern-based extraction.
        // We need to collect names first because we can't borrow archive
        // mutably while iterating.
        let all_names: Vec<String> = (0..archive.len())
            .filter_map(|i| {
                let file = archive.by_index(i).ok()?;
                Some(file.name().to_string())
            })
            .collect();

        // assets/*
        let mut assets = Vec::new();
        for name in all_names.iter().filter(|n| {
            n.starts_with("assets/") && n.len() > "assets/".len()
        }) {
            let mut file = archive.by_name(name)?;
            let hash = name
                .strip_prefix("assets/")
                .unwrap_or(name)
                .to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            assets.push((hash, data));
        }

        // terrain/tracks/*.gpx
        let mut gpx_files = Vec::new();
        for name in all_names.iter().filter(|n| {
            n.starts_with("terrain/tracks/") && n.ends_with(".gpx")
        }) {
            let mut file = archive.by_name(name)?;
            let filename = name
                .strip_prefix("terrain/tracks/")
                .unwrap_or(name)
                .to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            gpx_files.push((filename, data));
        }

        // terrain/overlays/*.geojson
        let mut geojson_overlays = Vec::new();
        for name in all_names.iter().filter(|n| {
            n.starts_with("terrain/overlays/") && n.ends_with(".geojson")
        }) {
            let mut file = archive.by_name(name)?;
            let filename = name
                .strip_prefix("terrain/overlays/")
                .unwrap_or(name)
                .to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            geojson_overlays.push((filename, data));
        }

        // terrain/tileset_meta.json (optional — TerrainTileset type)
        let tileset_meta: Option<crate::manifest::TilesetMeta> =
            match archive.by_name("terrain/tileset_meta.json") {
                Ok(mut file) => {
                    let mut buf = String::new();
                    file.read_to_string(&mut buf)?;
                    Some(
                        serde_json::from_str(&buf)
                            .context("invalid terrain/tileset_meta.json")?,
                    )
                }
                Err(_) => None,
            };

        // terrain/tiles/{z}/{x}/{y}.png (elevation tiles)
        let mut elevation_tiles = Vec::new();
        for name in all_names.iter().filter(|n| {
            n.starts_with("terrain/tiles/") && n.ends_with(".png")
        }) {
            let mut file = archive.by_name(name)?;
            let cache_key = name
                .strip_prefix("terrain/tiles/")
                .unwrap_or(name)
                .strip_suffix(".png")
                .unwrap_or(name)
                .to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            elevation_tiles.push((cache_key, data));
        }

        // terrain/satellite/{z}/{x}/{y}.jpg (satellite imagery tiles)
        let mut satellite_tiles = Vec::new();
        for name in all_names.iter().filter(|n| {
            n.starts_with("terrain/satellite/") && n.ends_with(".jpg")
        }) {
            let mut file = archive.by_name(name)?;
            let cache_key = name
                .strip_prefix("terrain/satellite/")
                .unwrap_or(name)
                .strip_suffix(".jpg")
                .unwrap_or(name)
                .to_string();
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            satellite_tiles.push((cache_key, data));
        }

        Ok(HexonArchiveData {
            manifest,
            entries,
            license,
            nodes,
            field_defs,
            schema,
            assets,
            terrain_config,
            gpx_files,
            geojson_overlays,
            tileset_meta,
            elevation_tiles,
            satellite_tiles,
        })
    }
}
