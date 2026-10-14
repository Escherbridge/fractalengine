//! Runtime registry of installed hexon tilesets — the single owner of shared tile sources.
//!
//! `TilesetRegistry` wraps a `HexonStore` and keeps one lazily-indexed
//! `Arc<HexonTileSource>` per tileset; the active petal's composite shares the
//! same `Arc` (see `src/AGENTS.md` §lazy-tiles).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use fe_format::manifest::{ElevationEncoding, TilesetMeta};
use serde::{Deserialize, Serialize};

use super::hexon_source::HexonTileSource;
use super::source::TileCoord;
use super::store::{HexonStore, InstalledTileset};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Lightweight summary of a tileset — no tile data, safe to clone and send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TilesetInfo {
    pub tileset_id: String,
    pub region_name: String,
    pub bounds: [f64; 4],
    pub zoom_range: (u8, u8),
    pub tile_count: u32,
    pub seeding: bool,
    /// Elevation encoding from the tileset meta, when the tileset is loaded in memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_encoding: Option<ElevationEncoding>,
    /// Hexon-declared world units per real meter (FR-5), when the tileset is
    /// loaded in memory (backfilled per FR-3 if the archive predates it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_scale: Option<f64>,
    /// Hexon-declared `[min, max]` world-scale clamp bounds (FR-5/FR-6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_bounds: Option<[f64; 2]>,
}

// ---------------------------------------------------------------------------
// TilesetRegistry
// ---------------------------------------------------------------------------

/// Runtime registry wrapping `HexonStore` with loaded tile sources.
///
/// Thread-safe: wrap in `Arc<TilesetRegistry>` for use across threads or as a
/// Bevy `Resource` (derive handled externally).
pub struct TilesetRegistry {
    store: HexonStore,
    sources: RwLock<HashMap<String, Arc<HexonTileSource>>>,
}

impl TilesetRegistry {
    /// Create a new registry from the given store.  No tilesets are loaded
    /// eagerly; call [`load_all`] to populate the sources cache.
    pub fn new(store: HexonStore) -> Self {
        Self {
            store,
            sources: RwLock::new(HashMap::new()),
        }
    }

    // -----------------------------------------------------------------------
    // Bulk loading
    // -----------------------------------------------------------------------

    /// Scan the store for installed tilesets and open each one's index (header phase only).
    ///
    /// Any tileset that fails to open is skipped (a `tracing::warn` is
    /// emitted).  Returns the list of `hexon_id`s that were successfully
    /// indexed.
    pub fn load_all(&self) -> Vec<String> {
        let installed = self.store.list_installed();
        let mut loaded = Vec::with_capacity(installed.len());

        for entry in installed {
            match self.store.load_tileset(&entry.hexon_id) {
                Ok(source) => {
                    let id = entry.hexon_id.clone();
                    self.sources
                        .write()
                        .expect("sources lock poisoned")
                        .insert(id.clone(), Arc::new(source));
                    loaded.push(id);
                }
                Err(err) => {
                    tracing::warn!(
                        hexon_id = %entry.hexon_id,
                        error = %err,
                        "failed to load tileset; skipping"
                    );
                }
            }
        }

        loaded
    }

    // -----------------------------------------------------------------------
    // Tile lookups
    // -----------------------------------------------------------------------

    /// The shared source for `tileset_id`, when indexed.
    pub fn source(&self, tileset_id: &str) -> Option<Arc<HexonTileSource>> {
        self.sources
            .read()
            .expect("sources lock poisoned")
            .get(tileset_id)
            .cloned()
    }

    /// Register (or replace) the shared source for `tileset_id`.
    pub fn insert_source(&self, tileset_id: &str, source: Arc<HexonTileSource>) {
        self.sources
            .write()
            .expect("sources lock poisoned")
            .insert(tileset_id.to_string(), source);
    }

    /// Read an elevation tile (blocking: call from `spawn_blocking`, never the main thread).
    ///
    /// `None` when the tileset is not indexed, the tile is absent, or the read failed (warned).
    pub fn get_tile(&self, tileset_id: &str, zoom: u8, x: u32, y: u32) -> Option<Vec<u8>> {
        let src = self.source(tileset_id)?;
        read_or_warn(
            tileset_id,
            src.load_tile_blocking(TileCoord::new(x, y, zoom)),
        )
    }

    /// Read a satellite tile (blocking: call from `spawn_blocking`, never the main thread).
    pub fn get_satellite_tile(
        &self,
        tileset_id: &str,
        zoom: u8,
        x: u32,
        y: u32,
    ) -> Option<Vec<u8>> {
        let src = self.source(tileset_id)?;
        read_or_warn(
            tileset_id,
            src.load_satellite_tile_blocking(TileCoord::new(x, y, zoom)),
        )
    }

    // -----------------------------------------------------------------------
    // Metadata queries
    // -----------------------------------------------------------------------

    /// List all installed tilesets as lightweight `TilesetInfo` summaries.
    pub fn list_tilesets(&self) -> Vec<TilesetInfo> {
        let sources = self.sources.read().expect("sources lock poisoned");
        self.store
            .list_installed()
            .into_iter()
            .map(|t| {
                let meta = sources.get(&t.hexon_id).map(|src| &src.tileset_meta);
                let elevation_encoding = meta.map(|m| m.elevation_encoding.clone());
                let native_scale = meta.and_then(|m| m.native_scale);
                let scale_bounds = meta.and_then(|m| m.scale_bounds);
                TilesetInfo {
                    tileset_id: t.hexon_id,
                    region_name: t.region_name,
                    bounds: t.bounds,
                    zoom_range: t.zoom_range,
                    tile_count: t.tile_count,
                    seeding: t.seeding_enabled,
                    elevation_encoding,
                    native_scale,
                    scale_bounds,
                }
            })
            .collect()
    }

    /// Return a clone of the `TilesetMeta` for a loaded tileset, or `None` if
    /// the tileset is not currently in the sources cache.
    pub fn get_meta(&self, tileset_id: &str) -> Option<TilesetMeta> {
        let sources = self.sources.read().expect("sources lock poisoned");
        sources.get(tileset_id).map(|src| src.tileset_meta.clone())
    }

    /// Return all installed tilesets that have seeding enabled.
    pub fn get_seeding_tilesets(&self) -> Vec<InstalledTileset> {
        self.store
            .list_installed()
            .into_iter()
            .filter(|t| t.seeding_enabled)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Mutations
    // -----------------------------------------------------------------------

    /// Install a hexon archive and index it; the new `Arc` replaces any previous source.
    pub fn install(&self, bytes: &[u8]) -> Result<InstalledTileset> {
        let installed = self.store.install_tileset(bytes)?;
        let source = self.store.load_tileset(&installed.hexon_id)?;
        self.insert_source(&installed.hexon_id, Arc::new(source));
        Ok(installed)
    }

    /// Remove a tileset from the sources cache and from disk.
    pub fn remove(&self, hexon_id: &str) -> Result<()> {
        self.sources
            .write()
            .expect("sources lock poisoned")
            .remove(hexon_id);
        self.store.remove_tileset(hexon_id)
    }

    /// Enable or disable P2P seeding for a tileset.
    pub fn set_seeding(&self, hexon_id: &str, enabled: bool) -> Result<()> {
        self.store.set_seeding(hexon_id, enabled)
    }

    /// Re-index a single tileset from the store (after an external `.hexon` update).
    pub fn reload(&self, hexon_id: &str) -> Result<()> {
        let source = self.store.load_tileset(hexon_id)?;
        self.insert_source(hexon_id, Arc::new(source));
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Return a reference to the underlying `HexonStore`.
    pub fn store(&self) -> &HexonStore {
        &self.store
    }
}

/// Flatten a blocking tile read, warning on I/O or format errors.
fn read_or_warn(tileset_id: &str, read: Result<Option<Arc<[u8]>>>) -> Option<Vec<u8>> {
    match read {
        Ok(bytes) => bytes.map(|bytes| bytes.to_vec()),
        Err(err) => {
            tracing::warn!(tileset_id, error = %err, "tile read failed");
            None
        }
    }
}
