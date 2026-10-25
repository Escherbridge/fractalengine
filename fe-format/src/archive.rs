//! HexonArchive — `.hexon` ZIP archive read/write.
//!
//! Replaces the old `FractalArchive`. Handles both scene-type hexons (with
//! `entities/` directory) and generic hexons (models, materials, terrain, etc.).

use std::io::{Cursor, Read, Seek, Write};

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;

use crate::{
    entries::{AssetEntry, EntryCatalog},
    integrity::{verify_archive_with_budget, IntegrityPolicy, IntegrityStatus},
    license::License,
    limits,
    manifest::{BundleEntry, HexonManifest, TilesetBundleIndex, TilesetMeta},
    ExportNode, FieldDef, SchemaDefinition,
};

/// The fixed empty entry catalog `export_stored_deterministic` writes for
/// every producer that has no catalogued entries (SPEC-10 §3.1 rule 5).
const EMPTY_ENTRY_CATALOG_JSON: &[u8] = b"{\"entries\":[]}";

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
    /// What header-phase verification established (PAS-P5 S2): `Sealed{publisher_did}`
    /// for a sealed archive whose every member checked out, `Unsealed` for a
    /// legacy archive (nothing was checked — see [`IntegrityPolicy::VerifyIfSealed`]).
    pub integrity: IntegrityStatus,
}

/// One already-built chunk for [`HexonArchive::export_chunked`]: the chunk's own
/// (ideally already-sealed, "recursive trust" — fe-format/src/AGENTS.md §integrity)
/// `.hexon` bytes, plus the tile-range metadata `chunks/index.json` records.
///
/// fe-format has no tile-coordinate math of its own (that lives in
/// `fe-terrain/src/tiles/bundle.rs`, AGENTS.md §bundles); this struct is the
/// seam — the caller supplies the spatial range, `export_chunked` computes the
/// digest and size from `archive_bytes` itself.
#[derive(Debug, Clone)]
pub struct ChunkInput {
    /// This chunk's sequence number (0-indexed).
    pub seq: u32,
    /// Number of elevation tiles the chunk carries.
    pub tile_count: u32,
    /// Zoom level this chunk's tile range belongs to.
    pub zoom: u8,
    /// Inclusive `[min, max]` tile-x range covered by this chunk.
    pub x_range: [u32; 2],
    /// Inclusive `[min, max]` tile-y range covered by this chunk.
    pub y_range: [u32; 2],
    /// Inclusive `[min, max]` Hilbert-distance range covered (debugging only).
    pub hilbert_range: [u64; 2],
    /// The chunk's own `.hexon` archive bytes, stored verbatim at
    /// `chunks/{seq}.hexon`.
    pub archive_bytes: Vec<u8>,
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

    /// Pack already-built chunk archives into one container: `chunks/index.json`
    /// (a signed [`TilesetBundleIndex`]) plus `chunks/{seq}.hexon` (Stored) per
    /// chunk, `entries.json` (empty — chunks aren't individually addressable
    /// entries, matching `export_tileset`), and `terrain/tileset_meta.json`.
    ///
    /// The result is UNSEALED — [`crate::integrity::seal`] it last, after any
    /// further transform (fe-format/src/AGENTS.md §integrity "seal is always
    /// the last transform"). `seal`'s own `chunks/index.json` check requires
    /// each `BundleEntry.blake3_hex` to equal the stored `chunks/{seq}.hexon`
    /// digest, which holds here by construction (both are `digest_hex` of the
    /// same `chunk.archive_bytes`).
    pub fn export_chunked(
        manifest: HexonManifest,
        tileset_meta: &TilesetMeta,
        chunks: &[ChunkInput],
    ) -> Result<Vec<u8>> {
        let bundles: Vec<BundleEntry> = chunks
            .iter()
            .map(|c| BundleEntry {
                seq: c.seq,
                blake3_hex: crate::integrity::digest_hex(&c.archive_bytes),
                byte_len: c.archive_bytes.len() as u64,
                tile_count: c.tile_count,
                zoom: c.zoom,
                x_range: c.x_range,
                y_range: c.y_range,
                hilbert_range: c.hilbert_range,
            })
            .collect();
        let index = TilesetBundleIndex {
            schema: 1,
            tileset_id: manifest.hexon_id.clone(),
            // The index's own copy always has `chunk_index: None` — per-chunk
            // metadata lives in `bundles`, not here (fe-format AGENTS §bundles).
            tileset_meta: TilesetMeta {
                chunk_index: None,
                ..tileset_meta.clone()
            },
            bundles,
        };

        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let deflate_opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        // Stored: each chunk is itself a compressed/signed archive; recompressing
        // it would spend CPU for no size gain.
        let stored_opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);

        zip.start_file("manifest.json", deflate_opts)?;
        zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

        let catalog = EntryCatalog { entries: vec![] };
        zip.start_file("entries.json", deflate_opts)?;
        zip.write_all(serde_json::to_string_pretty(&catalog)?.as_bytes())?;

        zip.start_file("terrain/tileset_meta.json", deflate_opts)?;
        zip.write_all(serde_json::to_string_pretty(tileset_meta)?.as_bytes())?;

        zip.start_file(crate::integrity::CHUNK_INDEX_PATH, deflate_opts)?;
        zip.write_all(&serde_json::to_vec(&index)?)?;

        for chunk in chunks {
            zip.start_file(crate::integrity::chunk_member_path(chunk.seq), stored_opts)?;
            zip.write_all(&chunk.archive_bytes)?;
        }

        let cursor = zip.finish()?;
        Ok(cursor.into_inner())
    }

    /// Deterministic `Stored`-only `.hexon` packer (SPEC-10 §3.1 rules 2-4):
    /// `manifest.json` (canonical JSON, keys sorted recursively, no whitespace
    /// — the same canonicalizer `sign_manifest`/`verify_manifest` use) and
    /// `entries.json` (the fixed empty catalog `{"entries":[]}`, since a
    /// columnar batch is not a scene asset and is not catalogued), followed
    /// by `extra_members` in the exact order given. Every member is written
    /// `Stored` (uncompressed) with the MS-DOS epoch timestamp
    /// (`zip::DateTime::default()`), no extra fields, no file/archive
    /// comment, and the crate-wide default `0o644` external attributes — so
    /// the archive bytes are a pure function of the member bytes, which is
    /// what lets a kind-5 observation batch's hexon be reproduced byte-for-byte
    /// from the same rows (see `fe-query/src/AGENTS.md` §observation-hexon).
    /// This packer is generic (any future byte-stable-archive producer can
    /// reuse it) — it is NOT a new reader; pair it with [`Self::import`] /
    /// [`Self::import_with_budget`] to read the result back, same as every
    /// other `.hexon` producer in this crate.
    pub fn export_stored_deterministic(
        manifest: &HexonManifest,
        extra_members: &[(&str, &[u8])],
    ) -> Result<Vec<u8>> {
        let manifest_json = crate::signature::canonical_json_bytes(manifest)
            .context("canonicalising manifest.json")?;

        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());

        zip.start_file("manifest.json", options)?;
        zip.write_all(&manifest_json)?;

        zip.start_file("entries.json", options)?;
        zip.write_all(EMPTY_ENTRY_CATALOG_JSON)?;

        for (name, data) in extra_members {
            zip.start_file(*name, options)?;
            zip.write_all(data)?;
        }

        let cursor = zip.finish()?;
        Ok(cursor.into_inner())
    }

    /// Every member's name and whether it is stored (uncompressed), in
    /// physical central-directory order — a cheap read of the central
    /// directory only (no decompression), capped by the same
    /// [`limits::MAX_ARCHIVE_ENTRIES`] guard `import` uses. Used by format
    /// profiles that pin an exact member order and storage method (e.g. the
    /// observation hexon, SPEC-10 §3.1 rules 2-3); general consumers should
    /// use [`Self::import`]'s extracted fields instead.
    pub fn member_order(bytes: &[u8]) -> Result<Vec<(String, bool)>> {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).context("failed to open ZIP archive")?;
        limits::check_entry_count(archive.len())?;
        let mut out = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            let file = archive.by_index(i)?;
            out.push((
                file.name().to_string(),
                file.compression() == zip::CompressionMethod::Stored,
            ));
        }
        Ok(out)
    }

    /// Read one named member's raw bytes under `cap`, with the same
    /// [`limits::read_entry_capped`] zip-bomb guard `import`/`import_with_budget`
    /// use, for a member those entry points do not themselves extract (e.g.
    /// the observation hexon's `observations/*.parquet`,
    /// `fe-query/src/AGENTS.md` §observation-hexon). `Ok(None)` when absent.
    /// This does not re-run the sealed-archive integrity check — a caller
    /// needing that calls [`Self::import`]/[`Self::import_with_budget`] too
    /// (same bytes, same budget discipline, not a second reader).
    pub fn read_member(bytes: &[u8], name: &str, cap: u64) -> Result<Option<Vec<u8>>> {
        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).context("failed to open ZIP archive")?;
        limits::check_entry_count(archive.len())?;
        let mut budget = cap;
        read_optional(&mut archive, name, cap, &mut budget)
    }

    /// Import a `.hexon` ZIP archive from bytes.
    ///
    /// Handles both scene hexons (with `entities/` directory) and non-scene
    /// hexons. Optional sections (`license.json`, `entities/`, `terrain/`,
    /// `schema.json`) default to empty/None if missing.
    pub fn import(bytes: &[u8]) -> Result<HexonArchiveData> {
        Self::import_with_budget(bytes, limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES)
    }

    /// [`Self::import`] under a caller-chosen total decompression budget
    /// (never above [`limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES`]).
    ///
    /// Runs the full integrity pass first (PAS-P5 S2), under the SAME budget,
    /// so a sealed archive's member walk can never bypass the caller's cap —
    /// a legacy unsealed archive only costs a `manifest.json` read here (see
    /// `verify_sealed_header`), so this stays cheap for the common case. A
    /// tampered sealed archive is refused before any extraction is attempted.
    pub fn import_with_budget(bytes: &[u8], total_budget: u64) -> Result<HexonArchiveData> {
        let total_budget = total_budget.min(limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES);
        let integrity: IntegrityStatus =
            verify_archive_with_budget(bytes, &IntegrityPolicy::VerifyIfSealed, total_budget)?;

        let cursor = Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(cursor).context("failed to open ZIP archive")?;
        limits::check_entry_count(archive.len())?;
        // One shared decompression budget for the whole import (zip-bomb guard).
        let mut budget = total_budget;
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
            integrity,
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
            integrity: None,
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

    /// S8b: a caller-chosen budget (the streamed-bundle path) refuses an
    /// archive the default 1 GiB budget would accept.
    #[test]
    fn a_smaller_import_budget_refuses_what_the_default_accepts() {
        let bytes = bomb_hexon("assets/big", 4 << 20);
        HexonArchive::import(&bytes).expect("within the default budget");
        let err =
            HexonArchive::import_with_budget(&bytes, 1 << 20).expect_err("over a 1 MiB budget");
        assert!(
            format!("{err:#}").contains(DECOMPRESSION_LIMIT_ERROR),
            "unexpected error: {err:#}"
        );
    }
}

#[cfg(test)]
mod import_integrity_tests {
    use super::*;
    use crate::integrity::{seal, IntegrityError};
    use ed25519_dalek::SigningKey;

    /// S2 acceptance (`HexonArchive::import` enforces `VerifyIfSealed`): an
    /// unsealed (legacy) archive still imports exactly as before, reporting
    /// `Unsealed`.
    #[test]
    fn legacy_unsealed_archive_imports_as_unsealed() {
        let bytes = HexonArchive::export(
            super::scale_roundtrip_tests::manifest(),
            vec![],
            vec![],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let data = HexonArchive::import(&bytes).expect("legacy archive still imports");
        assert_eq!(data.integrity, IntegrityStatus::Unsealed);
    }

    /// A sealed, untampered archive imports and reports its publisher.
    #[test]
    fn sealed_archive_imports_and_reports_its_publisher() {
        let key = SigningKey::from_bytes(&[13u8; 32]);
        let bytes = HexonArchive::export(
            super::scale_roundtrip_tests::manifest(),
            vec![],
            vec![],
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let sealed = seal(&bytes, &key).expect("seal");
        let data = HexonArchive::import(&sealed).expect("sealed archive imports");
        assert_eq!(
            data.integrity,
            IntegrityStatus::Sealed {
                publisher_did: crate::verifying_key_to_did(&key.verifying_key())
            }
        );
    }

    /// A tampered sealed archive is refused at import time, before extraction.
    #[test]
    fn tampered_sealed_archive_is_refused_at_import() {
        let key = SigningKey::from_bytes(&[14u8; 32]);
        let marker = vec![77u8; 64];
        let bytes = HexonArchive::export(
            super::scale_roundtrip_tests::manifest(),
            vec![],
            vec![],
            None,
            None,
            Some(&[("run.gpx".into(), marker.clone())]),
            None,
        )
        .unwrap();
        let sealed = seal(&bytes, &key).expect("seal");
        // The GPX member is deflated, so tamper its decoded bytes and re-zip (fresh CRC).
        let sealed = crate::test_support::tamper_member(&sealed, |_, data| data == marker);

        let err = HexonArchive::import(&sealed).expect_err("tampered sealed archive is refused");
        assert!(
            err.downcast_ref::<IntegrityError>().is_some(),
            "expected an IntegrityError, got {err:#}"
        );
    }
}

#[cfg(test)]
mod observation_packer_tests {
    use super::*;
    use crate::manifest::HexonType;

    fn obs_manifest() -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "obs-test".into(),
            hexon_type: HexonType::ObservationBatch,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "1".into(),
            build_id: None,
            name: "observation-batch".into(),
            description: None,
            tags: vec![],
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
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

    /// SPEC-10 §3.1 rule 3 + rule 8's archive-level half: the same manifest +
    /// members produce byte-identical archive bytes regardless of wall-clock
    /// time (MS-DOS epoch timestamp, not "now").
    #[test]
    fn export_stored_deterministic_is_byte_identical_and_stored_in_order() {
        let manifest = obs_manifest();
        let members: [(&str, &[u8]); 2] = [
            ("observations/readings.parquet", b"RB1"),
            ("observations/payload_refs.parquet", b"RB2"),
        ];
        let a = HexonArchive::export_stored_deterministic(&manifest, &members).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = HexonArchive::export_stored_deterministic(&manifest, &members).unwrap();
        assert_eq!(a, b, "packer bytes must not depend on wall-clock time");

        let order = HexonArchive::member_order(&a).unwrap();
        assert_eq!(
            order,
            vec![
                ("manifest.json".to_string(), true),
                ("entries.json".to_string(), true),
                ("observations/readings.parquet".to_string(), true),
                ("observations/payload_refs.parquet".to_string(), true),
            ]
        );
    }

    #[test]
    fn export_stored_deterministic_round_trips_through_import_and_read_member() {
        let manifest = obs_manifest();
        let members: [(&str, &[u8]); 2] = [
            ("observations/readings.parquet", b"RB1"),
            ("observations/payload_refs.parquet", b"RB2"),
        ];
        let bytes = HexonArchive::export_stored_deterministic(&manifest, &members).unwrap();

        let data = HexonArchive::import(&bytes).unwrap();
        assert!(matches!(
            data.manifest.hexon_type,
            HexonType::ObservationBatch
        ));
        assert!(data.entries.is_empty());

        assert_eq!(
            HexonArchive::read_member(&bytes, "observations/readings.parquet", 1024)
                .unwrap()
                .unwrap(),
            b"RB1"
        );
        assert_eq!(
            HexonArchive::read_member(&bytes, "observations/payload_refs.parquet", 1024)
                .unwrap()
                .unwrap(),
            b"RB2"
        );
        assert!(HexonArchive::read_member(&bytes, "missing", 1024)
            .unwrap()
            .is_none());
    }

    /// `read_member`'s decompression-limit guard applies before any
    /// allocation, matching `HexonArchive::import`'s own bomb refusals.
    #[test]
    fn read_member_refuses_a_declared_size_bomb_without_allocation() {
        let manifest = obs_manifest();
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        zip.start_file("manifest.json", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&crate::signature::canonical_json_bytes(&manifest).unwrap())
            .unwrap();
        let bomb_opts = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1));
        zip.start_file("bomb", bomb_opts).unwrap();
        let chunk = vec![0u8; 1 << 20];
        for _ in 0..(limits::MAX_ENTRY_UNCOMPRESSED_BYTES / (1 << 20) + 16) {
            zip.write_all(&chunk).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        assert!(bytes.len() < (4 << 20), "fixture must compress small");

        let err = HexonArchive::read_member(&bytes, "bomb", limits::MAX_ENTRY_UNCOMPRESSED_BYTES)
            .expect_err("a member over the cap must be refused");
        assert!(format!("{err:#}").contains(limits::DECOMPRESSION_LIMIT_ERROR));
    }
}
