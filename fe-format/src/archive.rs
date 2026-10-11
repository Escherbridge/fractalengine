//! HexonArchive — `.hexon` ZIP archive read/write.
//!
//! Replaces the old `FractalArchive`. Handles both scene-type hexons (with
//! `entities/` directory) and generic hexons (models, materials, terrain, etc.).

use std::io::{Cursor, Read, Seek, Write};

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;

use crate::{
    entries::{AssetEntry, EntryCatalog},
    license::License,
    limits,
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
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

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
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

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
        let stored_opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflate_opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

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
        let mut archive = zip::ZipArchive::new(cursor).context("failed to open ZIP archive")?;
        limits::check_entry_count(archive.len())?;
        // One shared decompression budget for the whole import (zip-bomb guard).
        let mut budget = limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES;
        let json = limits::MAX_JSON_ENTRY_BYTES;
        let binary = limits::MAX_ENTRY_UNCOMPRESSED_BYTES;

        // manifest.json (required)
        let manifest: HexonManifest = {
            let buf = read_optional(&mut archive, "manifest.json", json, &mut budget)?
                .context("missing manifest.json")?;
            serde_json::from_slice(&buf).context("invalid manifest.json")?
        };

        // entries.json (required)
        let entries: Vec<AssetEntry> = {
            let buf = read_optional(&mut archive, "entries.json", json, &mut budget)?
                .context("missing entries.json")?;
            let catalog: EntryCatalog =
                serde_json::from_slice(&buf).context("invalid entries.json")?;
            catalog.entries
        };

        // license.json (optional)
        let license: Option<License> =
            match read_optional(&mut archive, "license.json", json, &mut budget)? {
                Some(buf) => Some(serde_json::from_slice(&buf).context("invalid license.json")?),
                None => None,
            };

        // schema.json (optional)
        let schema: SchemaDefinition =
            match read_optional(&mut archive, "schema.json", json, &mut budget)? {
                Some(buf) => serde_json::from_slice(&buf).context("invalid schema.json")?,
                None => SchemaDefinition { field_defs: vec![] },
            };

        // entities/nodes.json (optional — scene type only)
        let nodes: Vec<ExportNode> =
            match read_optional(&mut archive, "entities/nodes.json", binary, &mut budget)? {
                Some(buf) => serde_json::from_slice(&buf).context("invalid entities/nodes.json")?,
                None => vec![],
            };

        // entities/field_defs.json (optional — scene type only)
        let field_defs: Vec<FieldDef> =
            match read_optional(&mut archive, "entities/field_defs.json", json, &mut budget)? {
                Some(buf) => {
                    serde_json::from_slice(&buf).context("invalid entities/field_defs.json")?
                }
                None => vec![],
            };

        // terrain/config.json (optional)
        let terrain_config: Option<serde_json::Value> =
            match read_optional(&mut archive, "terrain/config.json", json, &mut budget)? {
                Some(buf) => {
                    Some(serde_json::from_slice(&buf).context("invalid terrain/config.json")?)
                }
                None => None,
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

        // Every remaining section: entries under `prefix` ending in `suffix`, keyed by
        // the name with `prefix` stripped.
        let mut collect = |prefix: &str, suffix: &str| -> Result<Vec<(String, Vec<u8>)>> {
            let mut out = Vec::new();
            for name in all_names
                .iter()
                .filter(|n| n.starts_with(prefix) && n.ends_with(suffix) && n.len() > prefix.len())
            {
                let data = read_optional(&mut archive, name, binary, &mut budget)?
                    .with_context(|| format!("unreadable archive entry {name}"))?;
                out.push((name[prefix.len()..].to_string(), data));
            }
            Ok(out)
        };
        let strip = |suffix: &'static str| {
            move |(key, data): (String, Vec<u8>)| {
                (key.strip_suffix(suffix).unwrap_or(&key).to_string(), data)
            }
        };

        // assets/*
        let assets = collect("assets/", "")?;
        // terrain/tracks/*.gpx
        let gpx_files = collect("terrain/tracks/", ".gpx")?;
        // terrain/overlays/*.geojson
        let geojson_overlays = collect("terrain/overlays/", ".geojson")?;
        // terrain/tiles/{z}/{x}/{y}.png (elevation tiles, keyed `{z}/{x}/{y}`)
        let elevation_tiles = collect("terrain/tiles/", ".png")?
            .into_iter()
            .map(strip(".png"))
            .collect();
        // terrain/satellite/{z}/{x}/{y}.jpg (satellite imagery tiles, keyed `{z}/{x}/{y}`)
        let satellite_tiles = collect("terrain/satellite/", ".jpg")?
            .into_iter()
            .map(strip(".jpg"))
            .collect();

        // terrain/tileset_meta.json (optional — TerrainTileset type)
        let tileset_meta: Option<crate::manifest::TilesetMeta> =
            match read_optional(&mut archive, "terrain/tileset_meta.json", json, &mut budget)? {
                Some(buf) => Some(
                    serde_json::from_slice(&buf).context("invalid terrain/tileset_meta.json")?,
                ),
                None => None,
            };

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

/// Read `name` under `cap` and the shared `budget`; `Ok(None)` when the entry is absent.
fn read_optional<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    cap: u64,
    budget: &mut u64,
) -> Result<Option<Vec<u8>>> {
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let declared = file.size();
    limits::read_entry_capped(&mut file, declared, cap, budget)
        .with_context(|| format!("reading {name}"))
        .map(Some)
}

#[cfg(test)]
mod scale_roundtrip_tests {
    use super::*;
    use crate::manifest::{ElevationEncoding, HexonType, TilesetMeta};

    pub(super) fn manifest() -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "scale-test".into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Scale Test".into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            source_peer_did: None,
            approx_size_bytes: None,
            min_engine_version: None,
            homepage_url: None,
            dependencies: vec![],
            platforms: vec![],
            address: None,
            signature: None,
        }
    }

    /// FR-1 acceptance: scale fields survive export → import via `export_tileset`.
    #[test]
    fn tileset_meta_scale_fields_survive_export_import() {
        let meta = TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 1,
            region_name: "Scale Region".into(),
            native_scale: Some(0.001),
            ground_sample_distance_m: Some(4.77),
            crs: Some("EPSG:4326".into()),
            scale_bounds: Some([0.0001, 0.01]),
            ..Default::default()
        };
        let bytes =
            HexonArchive::export_tileset(manifest(), &meta, &[], &[], None).expect("export failed");
        let data = HexonArchive::import(&bytes).expect("import failed");
        let round_tripped = data.tileset_meta.expect("tileset_meta must be present");
        assert_eq!(round_tripped.native_scale, Some(0.001));
        assert_eq!(round_tripped.ground_sample_distance_m, Some(4.77));
        assert_eq!(round_tripped.crs.as_deref(), Some("EPSG:4326"));
        assert_eq!(round_tripped.scale_bounds, Some([0.0001, 0.01]));
    }

    /// The seam test for the `Default` contract itself: a `TilesetMeta` built
    /// with only the fields a real producer sets (see
    /// `fe-terrain/src/tiles/builder.rs`) plus `..Default::default()` must
    /// round-trip byte-for-byte through the same archive writer/reader
    /// producers and consumers actually use.
    #[test]
    fn default_constructed_meta_roundtrips_through_archive() {
        let meta = TilesetMeta {
            bounds: [47.0, 8.0, 47.1, 8.1],
            min_zoom: 5,
            max_zoom: 8,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 12,
            region_name: "Default Contract Region".into(),
            ..Default::default()
        };
        let bytes =
            HexonArchive::export_tileset(manifest(), &meta, &[], &[], None).expect("export failed");
        let data = HexonArchive::import(&bytes).expect("import failed");
        let round_tripped = data.tileset_meta.expect("tileset_meta must be present");
        assert_eq!(round_tripped, meta);
    }
}

#[cfg(test)]
mod decompression_limit_tests {
    use super::*;
    use crate::limits::{DECOMPRESSION_LIMIT_ERROR, MAX_ENTRY_UNCOMPRESSED_BYTES};

    /// Stream `inflated` zero bytes into a Deflate entry without holding them in memory.
    fn write_zeros(zip: &mut zip::ZipWriter<Cursor<Vec<u8>>>, name: &str, inflated: u64) {
        let opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1));
        zip.start_file(name, opts).unwrap();
        let chunk = vec![0u8; 1 << 20];
        let mut left = inflated;
        while left > 0 {
            let n = left.min(chunk.len() as u64) as usize;
            zip.write_all(&chunk[..n]).unwrap();
            left -= n as u64;
        }
    }

    /// A hexon with a valid manifest whose `bomb_entry` inflates to `inflated` zero bytes.
    fn bomb_hexon(bomb_entry: &str, inflated: u64) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default();
        zip.start_file("manifest.json", opts).unwrap();
        let manifest = super::scale_roundtrip_tests::manifest();
        zip.write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        if bomb_entry != "entries.json" {
            zip.start_file("entries.json", opts).unwrap();
            zip.write_all(br#"{"entries":[]}"#).unwrap();
        }
        write_zeros(&mut zip, bomb_entry, inflated);
        zip.finish().unwrap().into_inner()
    }

    /// Rewrite the declared uncompressed size of `name` in both the local and
    /// central headers — the "lying header" bomb variant.
    fn falsify_declared_size(zip: &mut [u8], name: &str, declared: u32) {
        let name = name.as_bytes();
        let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]) as usize;
        let mut patched = 0;
        for i in 0..zip.len().saturating_sub(46) {
            let (sig, size_at, name_len_at, name_at) = match &zip[i..i + 4] {
                [0x50, 0x4b, 3, 4] => (0, 22, 26, 30),
                [0x50, 0x4b, 1, 2] => (1, 24, 28, 46),
                _ => continue,
            };
            let len = u16_at(zip, i + name_len_at);
            if zip.get(i + name_at..i + name_at + len) == Some(name) {
                zip[i + size_at..i + size_at + 4].copy_from_slice(&declared.to_le_bytes());
                patched |= 1 << sig;
            }
        }
        assert_eq!(
            patched, 0b11,
            "both headers of the bomb entry must be patched"
        );
    }

    fn assert_refused(bytes: &[u8]) {
        let err = HexonArchive::import(bytes).expect_err("bomb must be refused");
        assert!(
            format!("{err:#}").contains(DECOMPRESSION_LIMIT_ERROR),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn asset_bomb_is_refused_on_declared_and_on_actual_size() {
        let inflated = MAX_ENTRY_UNCOMPRESSED_BYTES + (8 << 20);
        let mut bytes = bomb_hexon("assets/bomb", inflated);
        assert!(
            bytes.len() < (1 << 20),
            "fixture must be a real bomb ({} bytes)",
            bytes.len()
        );
        assert_refused(&bytes);

        // Lying header: the declared size passes the pre-check, take() still stops it.
        falsify_declared_size(&mut bytes, "assets/bomb", 1024);
        assert_refused(&bytes);
    }

    #[test]
    fn json_metadata_bomb_is_refused() {
        assert_refused(&bomb_hexon("entries.json", 9 << 20));
    }

    #[test]
    fn archive_within_limits_still_imports_its_assets() {
        let asset = vec![42u8; 3 << 20];
        let bytes = HexonArchive::export(
            super::scale_roundtrip_tests::manifest(),
            vec![],
            vec![("abc".into(), asset.clone())],
            None,
            None,
            Some(&[("run.gpx".into(), b"<gpx/>".to_vec())]),
            None,
        )
        .unwrap();
        let data = HexonArchive::import(&bytes).expect("normal archive imports");
        assert_eq!(data.assets, vec![("abc".to_string(), asset)]);
        assert_eq!(
            data.gpx_files,
            vec![("run.gpx".to_string(), b"<gpx/>".to_vec())]
        );
    }
}
