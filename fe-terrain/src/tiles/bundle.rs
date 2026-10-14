//! Deterministic spatial bundler for terrain tilesets (campaign plan S1,
//! D-73 A4): splits a tileset into Hilbert-ordered, size-capped bundles
//! instead of flat chunks, each independently addressable and verifiable.
//! See `fe-terrain/src/AGENTS.md` §bundles.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use fe_format::manifest::{BundleEntry, ChunkIndex, TilesetBundleIndex, TilesetMeta};
use fe_format::{HexonArchive, HexonArchiveData};

use super::source::TileCoord;
use super::store::{HexonStore, InstalledTileset};

/// Default bundle size cap (evidence §3: a real elevation+satellite tile
/// pair is ~110 KB, so 1 MiB holds roughly 8-9 pairs per bundle).
pub const DEFAULT_BUNDLE_BYTES: u64 = 1024 * 1024;

/// A bundled tileset: the index plus every bundle's exported archive bytes,
/// in `index.bundles` order.
pub struct TilesetBundles {
    pub index: TilesetBundleIndex,
    pub index_json: Vec<u8>,
    pub bundles: Vec<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// Hilbert ordering + cache-key parsing
// ---------------------------------------------------------------------------

/// Hilbert-curve distance of `(x, y)` on a `2^order × 2^order` grid
/// (Wikipedia's `xy2d`, reimplemented here to avoid a new dependency).
/// PMTiles uses the same construction for its tile ordering (D-73): it keeps
/// spatially-adjacent tiles adjacent in the bundle sequence.
fn hilbert_distance(order: u32, x: u32, y: u32) -> u64 {
    let n: i64 = 1i64 << order;
    let (mut x, mut y) = (x as i64, y as i64);
    let mut d: u64 = 0;
    let mut s = n / 2;
    while s > 0 {
        let rx: i64 = if (x & s) > 0 { 1 } else { 0 };
        let ry: i64 = if (y & s) > 0 { 1 } else { 0 };
        d += (s as u64) * (s as u64) * ((3 * rx) ^ ry) as u64;
        if ry == 0 {
            if rx == 1 {
                x = n - 1 - x;
                y = n - 1 - y;
            }
            std::mem::swap(&mut x, &mut y);
        }
        s /= 2;
    }
    d
}

/// Parse a `{z}/{x}/{y}` cache key into its tile coordinate.
fn parse_cache_key(key: &str) -> Option<(u8, u32, u32)> {
    let mut parts = key.splitn(3, '/');
    let z: u8 = parts.next()?.parse().ok()?;
    let x: u32 = parts.next()?.parse().ok()?;
    let y: u32 = parts.next()?.parse().ok()?;
    Some((z, x, y))
}

/// Geographic bounds `[min_lat, min_lon, max_lat, max_lon]` covering an
/// inclusive tile range at one zoom level — the bundle's *real* extent,
/// not the whole tileset's bounds (the drift `package_chunked` had; fixed
/// here for both callers, step 4).
pub(crate) fn bounds_for_tile_range(zoom: u8, x_range: [u32; 2], y_range: [u32; 2]) -> [f64; 4] {
    let (nw_lat, nw_lon) = TileCoord::new(x_range[0], y_range[0], zoom).to_lat_lon();
    let (se_lat, se_lon) = TileCoord::new(
        x_range[1].saturating_add(1),
        y_range[1].saturating_add(1),
        zoom,
    )
    .to_lat_lon();
    [
        se_lat.min(nw_lat),
        nw_lon.min(se_lon),
        se_lat.max(nw_lat),
        nw_lon.max(se_lon),
    ]
}

// ---------------------------------------------------------------------------
// Shared splitter
// ---------------------------------------------------------------------------

/// An elevation/satellite tile pair for one `(z, x, y)`, kept together so a
/// pair never splits across bundles.
#[derive(Default)]
struct TileGroup {
    elevation: Option<Vec<u8>>,
    satellite: Option<Vec<u8>>,
}

/// One bundle's tiles plus the tile-range metadata describing it — the piece
/// shared between `bundle_tileset` (import-from-archive) and
/// `TilesetBuilder::package_chunked` (download-from-network), so there is
/// one split implementation (step 4; both callers build their own manifest
/// around this).
pub(crate) struct BundleSlice {
    pub seq: u32,
    pub elevation_tiles: Vec<(String, Vec<u8>)>,
    pub satellite_tiles: Vec<(String, Vec<u8>)>,
    pub zoom: u8,
    pub x_range: [u32; 2],
    pub y_range: [u32; 2],
    pub hilbert_range: [u64; 2],
}

/// Group elevation/satellite tiles by `(z, x, y)`, order coordinates by
/// `(z, hilbert(x, y))` (PMTiles-style, D-73), then greedily fill bundles up
/// to `max_bundle_bytes` (a single oversize pair is allowed alone).
/// Deterministic: depends only on the tile bytes and cache keys, never on
/// wall-clock time or map/hash iteration order.
pub(crate) fn split_tiles_into_bundles(
    elevation_tiles: &[(String, Vec<u8>)],
    satellite_tiles: &[(String, Vec<u8>)],
    max_bundle_bytes: u64,
) -> Result<Vec<BundleSlice>> {
    let mut groups: HashMap<(u8, u32, u32), TileGroup> = HashMap::new();
    for (key, bytes) in elevation_tiles {
        let coord =
            parse_cache_key(key).with_context(|| format!("bad elevation cache key: {key}"))?;
        groups.entry(coord).or_default().elevation = Some(bytes.clone());
    }
    for (key, bytes) in satellite_tiles {
        let coord =
            parse_cache_key(key).with_context(|| format!("bad satellite cache key: {key}"))?;
        groups.entry(coord).or_default().satellite = Some(bytes.clone());
    }

    // Sorting a Vec collected from the map keys removes any HashMap
    // iteration-order dependence before packing — the determinism invariant.
    let mut coords: Vec<(u8, u32, u32)> = groups.keys().copied().collect();
    coords.sort_by_key(|&(z, x, y)| (z, hilbert_distance(z as u32, x, y)));

    let mut coord_groups: Vec<Vec<(u8, u32, u32)>> = Vec::new();
    let mut current: Vec<(u8, u32, u32)> = Vec::new();
    let mut current_bytes: u64 = 0;
    for coord in coords {
        let group = &groups[&coord];
        let pair_bytes = group
            .elevation
            .as_ref()
            .map(|b| b.len() as u64)
            .unwrap_or(0)
            + group
                .satellite
                .as_ref()
                .map(|b| b.len() as u64)
                .unwrap_or(0);
        if !current.is_empty() && current_bytes + pair_bytes > max_bundle_bytes {
            coord_groups.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        current_bytes += pair_bytes;
        current.push(coord);
    }
    if !current.is_empty() {
        coord_groups.push(current);
    }
    // An entity hexon / sub-threshold tileset (including zero tiles) still
    // ships as exactly one bundle.
    if coord_groups.is_empty() {
        coord_groups.push(Vec::new());
    }

    let mut out = Vec::with_capacity(coord_groups.len());
    for (seq, coord_group) in coord_groups.into_iter().enumerate() {
        let seq = seq as u32;
        let mut elevation = Vec::new();
        let mut satellite = Vec::new();
        let (mut x_min, mut x_max, mut y_min, mut y_max) = (u32::MAX, 0u32, u32::MAX, 0u32);
        let (mut hmin, mut hmax) = (u64::MAX, 0u64);
        let mut zoom = 0u8;
        for &(z, x, y) in &coord_group {
            zoom = z;
            x_min = x_min.min(x);
            x_max = x_max.max(x);
            y_min = y_min.min(y);
            y_max = y_max.max(y);
            let h = hilbert_distance(z as u32, x, y);
            hmin = hmin.min(h);
            hmax = hmax.max(h);
            let g = &groups[&(z, x, y)];
            let key = format!("{z}/{x}/{y}");
            if let Some(bytes) = &g.elevation {
                elevation.push((key.clone(), bytes.clone()));
            }
            if let Some(bytes) = &g.satellite {
                satellite.push((key, bytes.clone()));
            }
        }
        if coord_group.is_empty() {
            x_min = 0;
            x_max = 0;
            y_min = 0;
            y_max = 0;
            hmin = 0;
            hmax = 0;
        }
        out.push(BundleSlice {
            seq,
            elevation_tiles: elevation,
            satellite_tiles: satellite,
            zoom,
            x_range: [x_min, x_max],
            y_range: [y_min, y_max],
            hilbert_range: [hmin, hmax],
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Bundling (import-from-archive)
// ---------------------------------------------------------------------------

/// Bundle an in-memory tileset into deterministic, Hilbert-ordered,
/// size-capped bundles (D-73 A4). Pure function of the source bytes: the
/// same input gives identical bundle bytes, so identical blake3 values —
/// the identity the pointer-set and cross-hexon dedup depend on.
pub fn bundle_tileset(source: &HexonArchiveData, max_bundle_bytes: u64) -> Result<TilesetBundles> {
    let meta: TilesetMeta = source
        .tileset_meta
        .clone()
        .context("source archive has no tileset_meta (not a TerrainTileset hexon)")?;
    let tileset_id = source.manifest.hexon_id.clone();

    let slices = split_tiles_into_bundles(
        &source.elevation_tiles,
        &source.satellite_tiles,
        max_bundle_bytes,
    )?;
    let total = slices.len() as u32;

    let mut bundle_entries = Vec::with_capacity(slices.len());
    let mut bundle_bytes = Vec::with_capacity(slices.len());

    for slice in slices {
        let chunk_bounds = bounds_for_tile_range(slice.zoom, slice.x_range, slice.y_range);

        let mut bundle_manifest = source.manifest.clone();
        bundle_manifest.hexon_id = format!("{tileset_id}-bundle-{}", slice.seq);
        bundle_manifest.name = format!("{} (bundle {}/{total})", meta.region_name, slice.seq + 1);
        // The bundle's bytes differ from whatever the source manifest's
        // signature (if any) covered, so a stale signature never carries
        // forward — bundles ship unsigned until PAS-P5 signs the index
        // (assumption: P2P-assembled tilesets install unsigned until then).
        bundle_manifest.signature = None;
        // created_at/updated_at are left as the source manifest's — never
        // `chrono::Utc::now()` — so bundling twice yields identical bytes.

        let mut bundle_meta = meta.clone();
        bundle_meta.tile_count = slice.elevation_tiles.len() as u32;
        bundle_meta.satellite_tile_count = slice.satellite_tiles.len() as u32;
        bundle_meta.chunk_index = Some(ChunkIndex {
            tileset_id: tileset_id.clone(),
            chunk_seq: slice.seq,
            total_chunks: total,
            chunk_bounds,
            // An archive can't embed its own hash, so this self-referential
            // slot stays `None`; `BundleEntry.blake3_hex` below (the index,
            // not the archive) carries the verifiable digest.
            blake3: None,
        });

        let archive_bytes = HexonArchive::export_tileset(
            bundle_manifest,
            &bundle_meta,
            &slice.elevation_tiles,
            &slice.satellite_tiles,
            None,
        )
        .with_context(|| format!("failed to export bundle {}", slice.seq))?;

        let blake3_hex = blake3::hash(&archive_bytes).to_hex().to_string();

        bundle_entries.push(BundleEntry {
            seq: slice.seq,
            blake3_hex,
            byte_len: archive_bytes.len() as u64,
            tile_count: bundle_meta.tile_count,
            zoom: slice.zoom,
            x_range: slice.x_range,
            y_range: slice.y_range,
            hilbert_range: slice.hilbert_range,
        });
        bundle_bytes.push(archive_bytes);
    }

    let mut index_meta = meta;
    index_meta.chunk_index = None;
    let index = TilesetBundleIndex {
        schema: 1,
        tileset_id,
        tileset_meta: index_meta,
        bundles: bundle_entries,
    };
    let index_json = serde_json::to_vec(&index).context("failed to serialize bundle index")?;

    Ok(TilesetBundles {
        index,
        index_json,
        bundles: bundle_bytes,
    })
}

// ---------------------------------------------------------------------------
// Assembly (verify + install)
// ---------------------------------------------------------------------------

/// Verify every bundle file's blake3 against the index, then delegate to the
/// existing [`super::distribution::assemble_chunks`]. A bundle's place in
/// the index is read from its own `ChunkIndex.chunk_seq` (not file-list
/// order), so bundles may arrive in any order — mirroring `assemble_chunks`'s
/// existing "chunks can arrive in any order" contract. Any hash mismatch
/// fails before installation runs: nothing is written for a bad bundle.
pub fn assemble_bundles(
    store: &HexonStore,
    index: &TilesetBundleIndex,
    bundle_paths: &[PathBuf],
) -> Result<InstalledTileset> {
    if bundle_paths.len() != index.bundles.len() {
        bail!(
            "bundle count mismatch: index names {} bundles, got {} paths",
            index.bundles.len(),
            bundle_paths.len()
        );
    }

    let mut chunk_archives = Vec::with_capacity(bundle_paths.len());
    for path in bundle_paths {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read bundle file: {}", path.display()))?;

        let data = HexonArchive::import(&bytes)
            .with_context(|| format!("failed to import bundle file: {}", path.display()))?;
        let seq = data
            .tileset_meta
            .as_ref()
            .and_then(|m| m.chunk_index.as_ref())
            .map(|c| c.chunk_seq)
            .with_context(|| format!("bundle file {} missing chunk_index", path.display()))?;

        let entry = index
            .bundles
            .iter()
            .find(|b| b.seq == seq)
            .with_context(|| format!("bundle seq {seq} not present in the index"))?;

        let actual_hash = blake3::hash(&bytes).to_hex().to_string();
        if actual_hash != entry.blake3_hex {
            bail!(
                "bundle seq {seq} hash mismatch: index says {}, file hashes to {actual_hash}",
                entry.blake3_hex
            );
        }

        chunk_archives.push(bytes);
    }

    // `assemble_chunks` installs under `tileset_id`, so the installed
    // hexon_id is `index.tileset_id` unchanged (keeps the original id).
    super::distribution::assemble_chunks(store, &index.tileset_id, &chunk_archives)
        .context("failed to assemble verified bundles")
}

// ---------------------------------------------------------------------------
// Seeding cache (campaign plan S4, critic step 5 — touched outside S1's
// owned-files list; minimal, additive: see that slice's reported open_issues)
// ---------------------------------------------------------------------------

/// `bundles_dir`'s `source.blake3` marker name (the bundled-from hash).
const SOURCE_HASH_MARKER: &str = "source.blake3";
/// `bundles_dir`'s bundle index file name.
const BUNDLE_INDEX_FILE: &str = "index.json";

fn bundles_dir(store: &HexonStore, hexon_id: &str) -> PathBuf {
    store.base_dir().join("bundles").join(hexon_id)
}

/// Bundle an **installed** hexon tileset for P2P seeding, writing the index
/// and bundle files to `<hexon store>/bundles/<id>/` and returning the paths
/// `SyncCommand::PublishTileset` needs.
///
/// Reuses the on-disk bundle set when present and its recorded source hash
/// still matches the installed `.hexon` file — a seeding toggle or a startup
/// re-seed does not re-bundle (and does not thrash disk) on every call. The
/// ONE shared implementation `fractalengine::terrain_bridge` (GUI) and
/// `fractalengine-relay` (headless) both call — see
/// `fractalengine/src/AGENTS.md` §tileset-p2p.
pub fn bundle_for_seeding(store: &HexonStore, hexon_id: &str) -> Result<(PathBuf, Vec<PathBuf>)> {
    let hexon_path = store.base_dir().join(format!("{hexon_id}.hexon"));
    let source_bytes = std::fs::read(&hexon_path)
        .with_context(|| format!("failed to read installed hexon: {}", hexon_path.display()))?;
    let source_hash = blake3::hash(&source_bytes).to_hex().to_string();

    let dir = bundles_dir(store, hexon_id);
    let index_path = dir.join(BUNDLE_INDEX_FILE);
    let hash_marker = dir.join(SOURCE_HASH_MARKER);

    if index_path.exists() {
        if let Ok(existing_hash) = std::fs::read_to_string(&hash_marker) {
            if existing_hash.trim() == source_hash {
                if let Some(cached) = try_reuse_cached_bundles(&dir, &index_path) {
                    return Ok(cached);
                }
            }
        }
    }

    let source = HexonArchive::import(&source_bytes)
        .with_context(|| format!("failed to import hexon for bundling: {hexon_id}"))?;
    let bundles = bundle_tileset(&source, DEFAULT_BUNDLE_BYTES)?;

    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create bundles dir: {}", dir.display()))?;
    std::fs::write(&index_path, &bundles.index_json)
        .with_context(|| format!("failed to write bundle index: {}", index_path.display()))?;
    let mut bundle_paths = Vec::with_capacity(bundles.bundles.len());
    for (seq, bytes) in bundles.bundles.iter().enumerate() {
        let path = dir.join(format!("{seq}.bundle"));
        std::fs::write(&path, bytes)
            .with_context(|| format!("failed to write bundle file: {}", path.display()))?;
        bundle_paths.push(path);
    }
    std::fs::write(&hash_marker, &source_hash).with_context(|| {
        format!(
            "failed to write source-hash marker: {}",
            hash_marker.display()
        )
    })?;

    Ok((index_path, bundle_paths))
}

/// Parse a verified bundle index (`SyncEvent::TilesetDownloadComplete`'s
/// `index_path` bytes) and assemble+install it. The one entry point a host
/// binary needs for a completed P2P download, so it never needs an
/// `fe_format` dependency just to deserialize the index (S4).
pub fn assemble_bundles_from_index_json(
    store: &HexonStore,
    index_json: &[u8],
    bundle_paths: &[PathBuf],
) -> Result<InstalledTileset> {
    let index: TilesetBundleIndex =
        serde_json::from_slice(index_json).context("failed to parse bundle index JSON")?;
    assemble_bundles(store, &index, bundle_paths)
}

/// `None` makes the caller rebuild — a missing bundle file, a corrupt index,
/// or anything else that would make a cached reuse unsafe.
fn try_reuse_cached_bundles(
    dir: &std::path::Path,
    index_path: &std::path::Path,
) -> Option<(PathBuf, Vec<PathBuf>)> {
    let index_json = std::fs::read(index_path).ok()?;
    let index: TilesetBundleIndex = serde_json::from_slice(&index_json).ok()?;
    let bundle_paths: Vec<PathBuf> = (0..index.bundles.len())
        .map(|seq| dir.join(format!("{seq}.bundle")))
        .collect();
    if bundle_paths.iter().all(|p| p.exists()) {
        Some((index_path.to_path_buf(), bundle_paths))
    } else {
        None
    }
}
