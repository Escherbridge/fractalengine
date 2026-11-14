//! Tile source over one `.hexon` tileset: lazy offset index + pinned byte cache; see `src/AGENTS.md` §lazy-tiles.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use anyhow::{bail, Context, Result};
use fe_format::manifest::{TilesetBundleIndex, TilesetMeta};
use fe_format::{ArchiveChanged, HexonArchive, HexonArchiveIndex, Layout};
use fe_format::entitlement::unix_now;
use fe_hexon::entitlement_gate::{EntitlementGate, GateRefusal, GateStatus, LicenseTerms};

use crate::splat::format::{decode_baked_splats, read_baked_splats_from_archive, BakedSplatBuffer};

use super::entitlement::{license_terms, read_license, EntitlementRequired};
use super::residency::{ByteLru, TILE_BYTE_BUDGET};
use super::source::TileCoord;

/// `(zoom, x, y)` — the tile identity every residency structure keys on.
pub type TileKey = (u8, u32, u32);

/// One per-tile member kind of a tileset archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TileLayer {
    /// `terrain/tiles/{z}/{x}/{y}.png`
    Elevation,
    /// `terrain/satellite/{z}/{x}/{y}.jpg`
    Satellite,
    /// `terrain/splats/{z}/{x}/{y}.bin`
    BakedSplats,
}

impl TileLayer {
    /// Every layer, in lookup order.
    pub const ALL: [TileLayer; 3] = [
        TileLayer::Elevation,
        TileLayer::Satellite,
        TileLayer::BakedSplats,
    ];

    fn prefix(self) -> &'static str {
        match self {
            TileLayer::Elevation => "terrain/tiles/",
            TileLayer::Satellite => "terrain/satellite/",
            TileLayer::BakedSplats => "terrain/splats/",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            TileLayer::Elevation => ".png",
            TileLayer::Satellite => ".jpg",
            TileLayer::BakedSplats => ".bin",
        }
    }

    /// Archive member name for this layer's tile at `key`.
    pub fn member_name(self, key: TileKey) -> String {
        format!(
            "{}{}/{}/{}{}",
            self.prefix(),
            key.0,
            key.1,
            key.2,
            self.suffix()
        )
    }
}

type LayerKey = (TileLayer, TileKey);

/// A resident per-tile value: raw tile bytes or a decoded baked splat buffer.
#[derive(Clone)]
pub enum ResidentTile {
    Bytes(Arc<[u8]>),
    Splats(Arc<BakedSplatBuffer>),
}

impl ResidentTile {
    /// Bytes this value charges against the cache budget.
    pub fn byte_cost(&self) -> u64 {
        match self {
            ResidentTile::Bytes(bytes) => bytes.len() as u64,
            ResidentTile::Splats(splats) => {
                // positions + colors + scales + normals per splat, plus the header.
                (splats.positions.len() * (12 + 16 + 8 + 12) + 64) as u64
            }
        }
    }

    /// The raw bytes, for elevation/satellite values.
    pub fn into_bytes(self) -> Option<Arc<[u8]>> {
        match self {
            ResidentTile::Bytes(bytes) => Some(bytes),
            ResidentTile::Splats(_) => None,
        }
    }

    /// The splat buffer, for baked-splat values.
    pub fn into_splats(self) -> Option<Arc<BakedSplatBuffer>> {
        match self {
            ResidentTile::Splats(splats) => Some(splats),
            ResidentTile::Bytes(_) => None,
        }
    }
}

/// Result of an I/O-free lookup.
pub enum Probe {
    /// Resident now.
    Hit(ResidentTile),
    /// The archive has the member, but it must be read off-thread first.
    Miss,
    /// The archive has no such member.
    Absent,
}

/// Eager backing (`from_archive` / `from_directory`): everything decoded up front.
struct MemoryTiles {
    layers: HashMap<LayerKey, ResidentTile>,
    bytes: u64,
}

/// Lazy backing: the archive index plus the set of per-tile members it lists.
struct IndexedArchive {
    index: HexonArchiveIndex,
    members: HashSet<LayerKey>,
}

impl IndexedArchive {
    /// Open a monolithic archive from its path and build its membership set
    /// (used only by the re-index-on-change path below — `HexonTileSource::
    /// open` goes through `from_index` instead, since it has already opened
    /// the index to check `layout()`).
    fn open(path: &Path) -> Result<Self> {
        Self::from_index(HexonArchiveIndex::open(path)?)
    }

    /// Build the membership set over an already-opened, monolithic index
    /// (header phase done by the caller — p2p_asset_streaming S6 splits
    /// `open` on `index.layout()` before this ever runs).
    fn from_index(index: HexonArchiveIndex) -> Result<Self> {
        let mut members = HashSet::new();
        for name in index.member_names() {
            for layer in TileLayer::ALL {
                if let Some(key) = name
                    .strip_prefix(layer.prefix())
                    .and_then(|rest| rest.strip_suffix(layer.suffix()))
                    .and_then(parse_tile_key)
                {
                    members.insert((layer, key));
                }
            }
        }
        Ok(Self { index, members })
    }
}

/// Lazy backing for a sealed chunked container (p2p_asset_streaming S6): the
/// outer archive's header phase only (`chunks/{seq}.hexon` bodies are never
/// touched at `open`), plus the parsed, signed `chunks/index.json`. A tile
/// resolves to its covering chunk via [`TilesetBundleIndex::bundle_for_tile`]
/// (fe-format's one tile→chunk lookup, critic C4 — no second table here,
/// mirroring `StreamedTileSource`, `src/AGENTS.md` §streamed-tilesets); the
/// chunk is then imported ON DEMAND (verified against the signed chunk hash
/// when the outer container is sealed) and the ONE requested tile is
/// extracted and handed to the shared `ByteLru` — the decoded chunk itself
/// is never cached beyond that single lookup, so touching every chunk of a
/// huge container still can never hold more than `TILE_BYTE_BUDGET` bytes
/// resident at once (see `src/AGENTS.md` §chunked-containers "residency
/// budget").
struct ContainerArchive {
    index: HexonArchiveIndex,
    chunk_index: TilesetBundleIndex,
}

impl ContainerArchive {
    /// Read chunk `seq`'s whole archive, decode it under the streamed-bundle
    /// decompression budget, and return the ONE `(layer, key)` member it
    /// carries, if any. `Ok(None)` covers both "key not in this chunk" and
    /// "chunk missing" — neither is a format error once the container opened.
    fn read_tile(&self, layer: TileLayer, key: TileKey) -> Result<Option<ResidentTile>> {
        let seq = match self.chunk_index.bundle_for_tile(key.0, key.1, key.2) {
            Some(seq) => seq,
            None => return Ok(None),
        };
        let Some(bytes) = self.index.chunk(seq)? else {
            return Ok(None);
        };
        let cache_key = format!("{}/{}/{}", key.0, key.1, key.2);
        if layer == TileLayer::BakedSplats {
            // Baked splats live in their own `terrain/splats/*.bin` section
            // (`crate::splat::format`), not in `HexonArchiveData` — read it
            // the same way the eager `from_archive` path does
            // (`read_baked_splats_from_archive`), over the already-fetched
            // chunk bytes. `builder.rs::package_tiles_chunked` bakes splats
            // per chunk, so this is NOT a live-synthesis-only path.
            // Same nested budget as the tile import below (R8: a bomb chunk
            // cannot inflate past it, and the refusal is an error, not a miss).
            let splats = crate::splat::format::read_baked_splats_from_archive_with_budget(
                &bytes,
                STREAMED_BUNDLE_DECODE_BUDGET,
            )
            .with_context(|| format!("failed to read baked splats from chunk {seq}"))?;
            return Ok(splats
                .into_iter()
                .find(|(k, _)| *k == cache_key)
                .map(|(_, buf)| ResidentTile::Splats(Arc::new(buf))));
        }
        let data = HexonArchive::import_with_budget(&bytes, STREAMED_BUNDLE_DECODE_BUDGET)
            .with_context(|| format!("failed to import chunk {seq}"))?;
        let found = match layer {
            TileLayer::Elevation => data
                .elevation_tiles
                .into_iter()
                .find(|(k, _)| *k == cache_key),
            TileLayer::Satellite => data
                .satellite_tiles
                .into_iter()
                .find(|(k, _)| *k == cache_key),
            TileLayer::BakedSplats => unreachable!("handled above"),
        };
        Ok(found.map(|(_, bytes)| ResidentTile::Bytes(Arc::from(bytes))))
    }
}

enum Backing {
    Memory(MemoryTiles),
    Indexed(RwLock<Arc<IndexedArchive>>),
    // Boxed: `ContainerArchive` embeds a `HexonArchiveIndex` (central
    // directory + header JSON), making it far larger than the other
    // variants — clippy::large_enum_variant.
    Container(Box<ContainerArchive>),
}

/// Total decoded bytes one streamed bundle may expand to (honest bundles are ~1 MiB).
pub const STREAMED_BUNDLE_DECODE_BUDGET: u64 = 64 * 1024 * 1024;

/// Tile source for one `.hexon` tileset.
///
/// `get_*` lookups are resident-only (never touch the disk); `load_*_blocking`
/// reads one member off-thread and caches it under [`TILE_BYTE_BUDGET`].
pub struct HexonTileSource {
    pub tileset_meta: TilesetMeta,
    backing: Backing,
    cache: Mutex<ByteLru<LayerKey, ResidentTile>>,
    /// Terms from the archive's own `license.json` (unlicensed when absent).
    license_terms: LicenseTerms,
    /// Gate over the archive's licensed members; `None` loads only free members of a licensed archive.
    entitlement_gate: Option<Arc<EntitlementGate>>,
}

impl HexonTileSource {
    /// Eagerly decode a tileset from raw `.hexon` archive bytes (tests, assembly, bundles).
    pub fn from_archive(bytes: &[u8]) -> Result<Self> {
        let data = HexonArchive::import(bytes).context("failed to import hexon archive")?;
        Self::from_imported(data, Some(bytes))
    }

    /// Decode one peer-supplied streamed bundle under [`STREAMED_BUNDLE_DECODE_BUDGET`]
    /// (S8b zip-bomb bound; bundles carry no baked splats, so none are read).
    pub fn from_streamed_bundle(bytes: &[u8]) -> Result<Self> {
        let data = HexonArchive::import_with_budget(bytes, STREAMED_BUNDLE_DECODE_BUDGET)
            .context("failed to import streamed bundle")?;
        Self::from_imported(data, None)
    }

    /// Build a memory-backed source from an imported archive; `splat_archive` is re-read for baked splats.
    fn from_imported(
        data: fe_format::HexonArchiveData,
        splat_archive: Option<&[u8]>,
    ) -> Result<Self> {
        let tileset_meta = data
            .tileset_meta
            .context("archive is missing tileset_meta (not a TerrainTileset hexon)")?;
        let terms = license_terms(data.license.as_ref());
        // An eager source never takes a gate: a licensed archive keeps only its free members.
        let admitted = |layer: TileLayer, key: TileKey| {
            !terms.is_licensed() || terms.is_free_entry(&layer.member_name(key))
        };

        let mut tiles = MemoryTiles {
            layers: HashMap::new(),
            bytes: 0,
        };
        for (layer, members) in [
            (TileLayer::Elevation, data.elevation_tiles),
            (TileLayer::Satellite, data.satellite_tiles),
        ] {
            for (cache_key, bytes) in members {
                if parse_tile_key(&cache_key).is_some_and(|key| admitted(layer, key)) {
                    tiles.insert_bytes(layer, &cache_key, bytes);
                }
            }
        }
        // Best-effort (FR-5): a malformed/absent splats section never fails the load.
        let splats = splat_archive
            .map(|bytes| read_baked_splats_from_archive(bytes).unwrap_or_default())
            .unwrap_or_default();
        for (cache_key, splats) in splats {
            if let Some(key) = parse_tile_key(&cache_key) {
                if admitted(TileLayer::BakedSplats, key) {
                    tiles.insert(
                        (TileLayer::BakedSplats, key),
                        ResidentTile::Splats(Arc::new(splats)),
                    );
                }
            }
        }
        let mut source = Self::with_backing(tileset_meta, Backing::Memory(tiles));
        source.license_terms = terms;
        Ok(source)
    }

    /// Lazily open an installed `.hexon` file: header phase only, members on
    /// demand. A chunked container (p2p_asset_streaming S6) takes the
    /// `Container` backing instead of `Indexed` — its tiles live inside
    /// `chunks/{seq}.hexon`, not at the top level, so `IndexedArchive`'s
    /// membership scan (which only ever looks at the outer archive's own
    /// member names) would otherwise see zero tiles.
    pub fn open(path: &Path) -> Result<Self> {
        let index = HexonArchiveIndex::open(path)
            .with_context(|| format!("failed to index hexon archive {}", path.display()))?;
        let terms = license_terms(
            read_license(&index)
                .with_context(|| format!("failed to read the licence of {}", path.display()))?
                .as_ref(),
        );
        let mut source = Self::open_index(path, index)?;
        source.license_terms = terms;
        Ok(source)
    }

    /// Build the lazy backing for an opened index (container or monolithic).
    fn open_index(path: &Path, index: HexonArchiveIndex) -> Result<Self> {
        match index.layout() {
            Layout::Container { .. } => {
                let chunk_index_bytes = index
                    .read_member(fe_format::integrity::CHUNK_INDEX_PATH)
                    .with_context(|| {
                        format!("failed to read chunks/index.json from {}", path.display())
                    })?
                    .context("container archive missing chunks/index.json")?;
                let chunk_index: TilesetBundleIndex = serde_json::from_slice(&chunk_index_bytes)
                    .context("invalid chunks/index.json")?;
                let tileset_meta = chunk_index.tileset_meta.clone();
                Ok(Self::with_backing(
                    tileset_meta,
                    Backing::Container(Box::new(ContainerArchive { index, chunk_index })),
                ))
            }
            Layout::Monolithic => {
                let archive = IndexedArchive::from_index(index)
                    .with_context(|| format!("failed to index hexon archive {}", path.display()))?;
                let tileset_meta = archive
                    .index
                    .tileset_meta()
                    .cloned()
                    .context("archive is missing tileset_meta (not a TerrainTileset hexon)")?;
                Ok(Self::with_backing(
                    tileset_meta,
                    Backing::Indexed(RwLock::new(Arc::new(archive))),
                ))
            }
        }
    }

    /// Load a tileset from a directory layout (for development/pre-packaging).
    ///
    /// Expected layout:
    /// ```text
    /// dir/
    ///   tileset_meta.json
    ///   tiles/{z}/{x}/{y}.png
    ///   satellite/{z}/{x}/{y}.jpg   (optional)
    /// ```
    pub fn from_directory(dir: &Path) -> Result<Self> {
        let meta_path = dir.join("tileset_meta.json");
        let meta_str =
            std::fs::read_to_string(&meta_path).context("failed to read tileset_meta.json")?;
        let tileset_meta: TilesetMeta =
            serde_json::from_str(&meta_str).context("invalid tileset_meta.json")?;

        let mut tiles = MemoryTiles {
            layers: HashMap::new(),
            bytes: 0,
        };
        for (layer, sub, ext) in [
            (TileLayer::Elevation, "tiles", "png"),
            (TileLayer::Satellite, "satellite", "jpg"),
        ] {
            let layer_dir = dir.join(sub);
            if layer_dir.is_dir() {
                let mut found = HashMap::new();
                walk_tiles(&layer_dir, ext, &mut found)?;
                for (cache_key, bytes) in found {
                    tiles.insert_bytes(layer, &cache_key, bytes);
                }
            }
        }
        // No baked-splats convention for the dev layout — live synthesis (FR-5).
        Ok(Self::with_backing(tileset_meta, Backing::Memory(tiles)))
    }

    fn with_backing(tileset_meta: TilesetMeta, backing: Backing) -> Self {
        Self {
            tileset_meta,
            backing,
            cache: Mutex::new(ByteLru::new(TILE_BYTE_BUDGET)),
            license_terms: LicenseTerms::unlicensed(),
            entitlement_gate: None,
        }
    }

    /// Terms from the archive's `license.json`.
    pub fn license_terms(&self) -> &LicenseTerms {
        &self.license_terms
    }

    /// Attach (or with `None`, drop) the gate from `tiles::entitlement::tileset_entitlement`; empties the tile cache.
    ///
    /// Refuses a gate whose terms differ from this archive's own `license.json` (a gate for another build).
    pub fn set_entitlement_gate(&mut self, gate: Option<Arc<EntitlementGate>>) -> Result<()> {
        if let Some(gate) = &gate {
            if gate.terms() != &self.license_terms {
                bail!("entitlement gate terms do not match this tileset's license.json");
            }
        }
        self.entitlement_gate = gate;
        let cache = self
            .cache
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.release_all_pins();
        cache.clear_unpinned();
        Ok(())
    }

    /// True when `member` is refused whatever its bytes are (licensed, non-free, no entitled gate): skip the read.
    fn refused_before_read(&self, member: &str) -> bool {
        if !self.license_terms.is_licensed() || self.license_terms.is_free_entry(member) {
            return false;
        }
        self.entitlement_gate
            .as_ref()
            .is_none_or(|gate| matches!(gate.status_at(unix_now()), GateStatus::Preview(_)))
    }

    /// Licence check for one stored member: free/unlicensed pass, sealed ones open only through an entitled gate.
    fn admit_member<'stored>(&self, member: &str, stored: &'stored [u8]) -> Result<Cow<'stored, [u8]>> {
        match &self.entitlement_gate {
            Some(gate) => gate.load(member, stored).map_err(|refusal| match refusal {
                GateRefusal::EntitlementRequired { entry_id } => {
                    anyhow::Error::new(EntitlementRequired { member: entry_id })
                }
                other => anyhow::Error::new(other),
            }),
            None if self.license_terms.is_licensed() && !self.license_terms.is_free_entry(member) => {
                Err(EntitlementRequired { member: member.to_owned() }.into())
            }
            None => Ok(Cow::Borrowed(stored)),
        }
    }

    /// [`admit_member`](Self::admit_member) over owned bytes, without copying a pass-through member.
    fn admit_owned(&self, member: &str, stored: Vec<u8>) -> Result<Vec<u8>> {
        let opened = match self.admit_member(member, &stored)? {
            Cow::Borrowed(_) => None,
            Cow::Owned(plaintext) => Some(plaintext),
        };
        Ok(opened.unwrap_or(stored))
    }

    /// Licence check for a container member already decoded from its chunk (cannot be opened): free or unlicensed only.
    fn admit_container_value(&self, layer: TileLayer, key: TileKey, value: ResidentTile) -> Result<ResidentTile> {
        if !self.license_terms.is_licensed() {
            return Ok(value);
        }
        let member = layer.member_name(key);
        match value {
            ResidentTile::Bytes(bytes) => Ok(ResidentTile::Bytes(Arc::from(
                self.admit_member(&member, &bytes)?.into_owned(),
            ))),
            ResidentTile::Splats(_) if self.license_terms.is_free_entry(&member) => Ok(value),
            ResidentTile::Splats(_) => Err(EntitlementRequired { member }.into()),
        }
    }

    fn cache(&self) -> MutexGuard<'_, ByteLru<LayerKey, ResidentTile>> {
        self.cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn archive(lock: &RwLock<Arc<IndexedArchive>>) -> Arc<IndexedArchive> {
        Arc::clone(&lock.read().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// True for a lazy source (index- or container-backed); false for an
    /// eagerly decoded one.
    pub fn is_lazy(&self) -> bool {
        matches!(self.backing, Backing::Indexed(_) | Backing::Container(_))
    }

    /// Resident value for `layer` at `coord` (no I/O); `None` when absent or not loaded.
    pub fn get_layer(&self, layer: TileLayer, coord: TileCoord) -> Option<ResidentTile> {
        let key = (layer, key_of(coord));
        match &self.backing {
            Backing::Memory(tiles) => tiles.layers.get(&key).cloned(),
            Backing::Indexed(_) | Backing::Container(_) => self.cache().get(&key),
        }
    }

    /// Resident elevation PNG (no I/O).
    pub fn get_tile(&self, coord: TileCoord) -> Option<Arc<[u8]>> {
        self.get_layer(TileLayer::Elevation, coord)
            .and_then(ResidentTile::into_bytes)
    }

    /// Resident satellite JPG (no I/O).
    pub fn get_satellite_tile(&self, coord: TileCoord) -> Option<Arc<[u8]>> {
        self.get_layer(TileLayer::Satellite, coord)
            .and_then(ResidentTile::into_bytes)
    }

    /// Resident baked splat buffer (no I/O); `None` → live `synthesize_splats` (FR-5).
    pub fn get_baked_splats(&self, coord: TileCoord) -> Option<Arc<BakedSplatBuffer>> {
        self.get_layer(TileLayer::BakedSplats, coord)
            .and_then(ResidentTile::into_splats)
    }

    /// Whether the archive holds `layer` at `coord` (membership only, no I/O).
    ///
    /// For a container this is the same coverage test `bundle_for_tile` IS
    /// (a covering chunk range exists) rather than an exact per-tile
    /// membership check — the same approximation `StreamedTileSource::covers`
    /// already makes for not-yet-installed tilesets (`src/AGENTS.md`
    /// §streamed-tilesets); an exact check would require decoding the chunk.
    pub fn has_member(&self, layer: TileLayer, coord: TileCoord) -> bool {
        let key = (layer, key_of(coord));
        match &self.backing {
            Backing::Memory(tiles) => tiles.layers.contains_key(&key),
            Backing::Indexed(lock) => Self::archive(lock).members.contains(&key),
            Backing::Container(c) => c
                .chunk_index
                .bundle_for_tile(key.1 .0, key.1 .1, key.1 .2)
                .is_some(),
        }
    }

    /// Whether an elevation tile exists for `coord` (membership only, no I/O).
    pub fn has_tile(&self, coord: TileCoord) -> bool {
        self.has_member(TileLayer::Elevation, coord)
    }

    /// I/O-free lookup distinguishing "resident", "needs a read" and "not in this archive".
    pub fn probe(&self, layer: TileLayer, coord: TileCoord) -> Probe {
        if let Some(hit) = self.get_layer(layer, coord) {
            return Probe::Hit(hit);
        }
        if self.is_lazy() && self.has_member(layer, coord) {
            Probe::Miss
        } else {
            Probe::Absent
        }
    }

    /// Read `layer` at `coord` (blocking; call off the main thread) and cache it unpinned.
    ///
    /// A replaced archive file is re-indexed once and retried; see §lazy-tiles.
    pub fn load_layer_blocking(
        &self,
        layer: TileLayer,
        coord: TileCoord,
    ) -> Result<Option<ResidentTile>> {
        let lock = match &self.backing {
            Backing::Memory(tiles) => {
                return Ok(tiles.layers.get(&(layer, key_of(coord))).cloned())
            }
            Backing::Indexed(lock) => lock,
            Backing::Container(container) => {
                let key = (layer, key_of(coord));
                if let Some(hit) = self.cache().get(&key) {
                    return Ok(Some(hit));
                }
                if self.license_terms.is_licensed() {
                    let member = layer.member_name(key.1);
                    let (zoom, x, y) = key.1;
                    if self.refused_before_read(&member)
                        && container.chunk_index.bundle_for_tile(zoom, x, y).is_some()
                    {
                        return Err(EntitlementRequired { member }.into());
                    }
                }
                let Some(value) = container.read_tile(layer, key.1)? else {
                    return Ok(None);
                };
                let value = self.admit_container_value(layer, key.1, value)?;
                self.cache().insert(key, value.clone(), value.byte_cost());
                return Ok(Some(value));
            }
        };
        let key = (layer, key_of(coord));
        if let Some(hit) = self.cache().get(&key) {
            return Ok(Some(hit));
        }
        let member = layer.member_name(key.1);
        let mut archive = Self::archive(lock);
        if self.refused_before_read(&member) && archive.members.contains(&key) {
            return Err(EntitlementRequired { member }.into());
        }
        let mut reindexed = false;
        let data = loop {
            if !archive.members.contains(&key) {
                return Ok(None);
            }
            match archive.index.read_member(&member) {
                Ok(data) => break data,
                Err(err) if !reindexed && err.downcast_ref::<ArchiveChanged>().is_some() => {
                    reindexed = true;
                    let current = Self::archive(lock);
                    if !Arc::ptr_eq(&archive, &current) {
                        // Another reader already re-indexed; retry against its index.
                        archive = current;
                        continue;
                    }
                    let path = archive.index.path().to_path_buf();
                    tracing::info!(path = %path.display(), "hexon archive replaced on disk; re-indexing");
                    let reopened = Arc::new(IndexedArchive::open(&path)?);
                    *lock
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::clone(&reopened);
                    self.cache().clear_unpinned();
                    archive = reopened;
                }
                Err(err) => return Err(err),
            }
        };
        let Some(data) = data else {
            return Ok(None);
        };
        let data = self.admit_owned(&member, data)?;
        let value = match layer {
            TileLayer::Elevation | TileLayer::Satellite => ResidentTile::Bytes(Arc::from(data)),
            TileLayer::BakedSplats => match decode_baked_splats(&data) {
                Ok(splats) => ResidentTile::Splats(Arc::new(splats)),
                // Corrupt/future-version buffer: no baked data, live synth (FR-5).
                Err(_) => return Ok(None),
            },
        };
        self.cache().insert(key, value.clone(), value.byte_cost());
        Ok(Some(value))
    }

    /// Blocking elevation read (see [`load_layer_blocking`](Self::load_layer_blocking)).
    pub fn load_tile_blocking(&self, coord: TileCoord) -> Result<Option<Arc<[u8]>>> {
        Ok(self
            .load_layer_blocking(TileLayer::Elevation, coord)?
            .and_then(ResidentTile::into_bytes))
    }

    /// Blocking satellite read (see [`load_layer_blocking`](Self::load_layer_blocking)).
    pub fn load_satellite_tile_blocking(&self, coord: TileCoord) -> Result<Option<Arc<[u8]>>> {
        Ok(self
            .load_layer_blocking(TileLayer::Satellite, coord)?
            .and_then(ResidentTile::into_bytes))
    }

    /// Blocking baked-splat read (see [`load_layer_blocking`](Self::load_layer_blocking)).
    pub fn load_baked_splats_blocking(
        &self,
        coord: TileCoord,
    ) -> Result<Option<Arc<BakedSplatBuffer>>> {
        Ok(self
            .load_layer_blocking(TileLayer::BakedSplats, coord)?
            .and_then(ResidentTile::into_splats))
    }

    /// Pin `value` for a live chunk (inserting it if it was evicted meanwhile).
    pub fn pin(&self, layer: TileLayer, coord: TileCoord, value: &ResidentTile) {
        if self.is_lazy() {
            self.cache()
                .insert_pinned((layer, key_of(coord)), value.clone(), value.byte_cost());
        }
    }

    /// Release every layer of `coord` (its chunk left the exit ring).
    pub fn release_tile(&self, coord: TileCoord) {
        if self.is_lazy() {
            let mut cache = self.cache();
            for layer in TileLayer::ALL {
                cache.release(&(layer, key_of(coord)));
            }
        }
    }

    /// Drop every pinned tile (the composite that pinned them was replaced).
    pub fn release_all_pins(&self) {
        if self.is_lazy() {
            self.cache().release_all_pins();
        }
    }

    /// Bytes held in RAM: the cache for a lazy source, everything for an eager one.
    pub fn resident_bytes(&self) -> u64 {
        match &self.backing {
            Backing::Memory(tiles) => tiles.bytes,
            Backing::Indexed(_) | Backing::Container(_) => self.cache().total_bytes(),
        }
    }

    /// Number of pinned (live-chunk) cache entries.
    pub fn pinned_len(&self) -> usize {
        self.cache().pinned_len()
    }

    /// Every tile key the archive holds for `layer`, sorted (no I/O for
    /// `Memory`/`Indexed`; a `Container` only has chunk RANGES at header
    /// time, so this is a debug/test helper that decodes every chunk —
    /// never called from a production hot path).
    pub fn member_keys(&self, layer: TileLayer) -> Vec<TileKey> {
        let mut keys: Vec<TileKey> = match &self.backing {
            Backing::Memory(tiles) => tiles
                .layers
                .keys()
                .filter(|(l, _)| *l == layer)
                .map(|(_, key)| *key)
                .collect(),
            Backing::Indexed(lock) => Self::archive(lock)
                .members
                .iter()
                .filter(|(l, _)| *l == layer)
                .map(|(_, key)| *key)
                .collect(),
            Backing::Container(container) => container
                .chunk_index
                .bundles
                .iter()
                .filter_map(|bundle| container.index.chunk(bundle.seq).ok().flatten())
                .filter_map(|bytes| {
                    HexonArchive::import_with_budget(&bytes, STREAMED_BUNDLE_DECODE_BUDGET).ok()
                })
                .flat_map(|data| match layer {
                    TileLayer::Elevation => data.elevation_tiles,
                    TileLayer::Satellite => data.satellite_tiles,
                    TileLayer::BakedSplats => Vec::new(),
                })
                .filter_map(|(cache_key, _)| parse_tile_key(&cache_key))
                .collect(),
        };
        keys.sort_unstable();
        keys
    }

    /// Geographic bounding box `[min_lat, min_lon, max_lat, max_lon]`.
    pub fn bounds(&self) -> [f64; 4] {
        self.tileset_meta.bounds
    }

    /// `(min_zoom, max_zoom)` range covered by this tileset.
    pub fn zoom_range(&self) -> (u8, u8) {
        (self.tileset_meta.min_zoom, self.tileset_meta.max_zoom)
    }

    /// Check if a coordinate falls within this tileset's bounds and zoom range.
    ///
    /// Uses the tile's full geographic extent (its NW corner to the NW corner of
    /// the next tile), not just its NW-corner point — a coarse-zoom tile can serve
    /// a region while its own NW corner lies just outside a tightly-drawn bounds box.
    pub fn covers(&self, coord: TileCoord) -> bool {
        let (min_z, max_z) = self.zoom_range();
        if coord.zoom < min_z || coord.zoom > max_z {
            return false;
        }
        // Tile geographic extent: NW corner of this tile → NW corner of the (x+1, y+1) tile.
        let (nw_lat, nw_lon) = coord.to_lat_lon();
        let (se_lat, se_lon) = TileCoord::new(coord.x + 1, coord.y + 1, coord.zoom).to_lat_lon();
        let tile_min_lat = se_lat.min(nw_lat);
        let tile_max_lat = se_lat.max(nw_lat);
        let tile_min_lon = nw_lon.min(se_lon);
        let tile_max_lon = nw_lon.max(se_lon);
        let [min_lat, min_lon, max_lat, max_lon] = self.tileset_meta.bounds;
        // Rectangle intersection between the tile's extent and the tileset bounds.
        tile_min_lat <= max_lat
            && tile_max_lat >= min_lat
            && tile_min_lon <= max_lon
            && tile_max_lon >= min_lon
    }
}

impl MemoryTiles {
    fn insert(&mut self, key: LayerKey, value: ResidentTile) {
        self.bytes += value.byte_cost();
        self.layers.insert(key, value);
    }

    fn insert_bytes(&mut self, layer: TileLayer, cache_key: &str, bytes: Vec<u8>) {
        if let Some(key) = parse_tile_key(cache_key) {
            self.insert((layer, key), ResidentTile::Bytes(Arc::from(bytes)));
        }
    }
}

#[cfg(feature = "fetch")]
#[async_trait::async_trait]
impl super::source::TileSource for HexonTileSource {
    async fn fetch_tile(&self, coord: TileCoord) -> Result<Vec<u8>> {
        self.load_tile_blocking(coord)?
            .map(|bytes| bytes.to_vec())
            .ok_or_else(|| anyhow::anyhow!("tile not found in hexon: {}", coord.cache_key()))
    }

    fn source_id(&self) -> &str {
        &self.tileset_meta.region_name
    }
}

fn key_of(coord: TileCoord) -> TileKey {
    (coord.zoom, coord.x, coord.y)
}

/// Parse a `{z}/{x}/{y}` cache key.
pub fn parse_tile_key(cache_key: &str) -> Option<TileKey> {
    let mut parts = cache_key.split('/');
    let z = parts.next()?.parse().ok()?;
    let x = parts.next()?.parse().ok()?;
    let y = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((z, x, y))
}

/// Walk a `{z}/{x}/{y}.{ext}` directory tree, collecting tiles into the map.
fn walk_tiles(base: &Path, ext: &str, out: &mut HashMap<String, Vec<u8>>) -> Result<()> {
    let z_entries = std::fs::read_dir(base).context("failed to read tiles directory")?;
    for z_entry in z_entries.flatten() {
        let z_path = z_entry.path();
        if !z_path.is_dir() {
            continue;
        }
        let z_name = z_entry.file_name();
        let z_str = z_name.to_string_lossy();

        let x_entries = match std::fs::read_dir(&z_path) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for x_entry in x_entries.flatten() {
            let x_path = x_entry.path();
            if !x_path.is_dir() {
                continue;
            }
            let x_name = x_entry.file_name();
            let x_str = x_name.to_string_lossy();

            let y_entries = match std::fs::read_dir(&x_path) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for y_entry in y_entries.flatten() {
                let y_path = y_entry.path();
                let y_name = y_entry.file_name();
                let y_str = y_name.to_string_lossy();
                let suffix = format!(".{ext}");
                if y_str.ends_with(&suffix) {
                    let y_num = y_str.strip_suffix(&suffix).unwrap_or(&y_str);
                    let cache_key = format!("{z_str}/{x_str}/{y_num}");
                    let data = std::fs::read(&y_path)
                        .with_context(|| format!("failed to read tile {cache_key}"))?;
                    out.insert(cache_key, data);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_format::manifest::{ElevationEncoding, HexonManifest, HexonType, TilesetMeta};
    use fe_format::HexonArchive;

    fn test_tileset_meta() -> TilesetMeta {
        TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 12,
            tile_size: 256,
            elevation_encoding: ElevationEncoding::TerrainRgb,
            tile_count: 1,
            region_name: "Test Region".into(),
            crs: None, // deliberately legacy — differs from Default's Some("EPSG:4326")
            ..Default::default()
        }
    }

    fn test_manifest() -> HexonManifest {
        HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: "test-tileset".into(),
            hexon_type: HexonType::TerrainTileset,
            publisher_did: "did:key:z6Mktest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "Test Tileset".into(),
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

    #[test]
    fn roundtrip_hexon_tile_source() {
        let meta = test_tileset_meta();
        let elevation = vec![("10/512/340".to_string(), vec![0u8; 64])];
        let satellite = vec![];

        let archive_bytes =
            HexonArchive::export_tileset(test_manifest(), &meta, &elevation, &satellite, None)
                .expect("export failed");

        let source = HexonTileSource::from_archive(&archive_bytes).expect("from_archive failed");

        assert_eq!(source.bounds(), [45.0, -122.0, 46.0, -121.0]);
        assert_eq!(source.zoom_range(), (10, 12));
        assert!(source.has_tile(TileCoord::new(512, 340, 10)));
        assert!(!source.has_tile(TileCoord::new(0, 0, 10)));

        let data = source.get_tile(TileCoord::new(512, 340, 10)).unwrap();
        assert_eq!(data.len(), 64);
    }

    #[test]
    fn loads_baked_splats_when_present() {
        use crate::splat::format::{append_baked_splats_to_archive, encode_baked_splats};

        let meta = test_tileset_meta();
        let elevation = vec![("10/512/340".to_string(), vec![0u8; 64])];
        let base = HexonArchive::export_tileset(test_manifest(), &meta, &elevation, &[], None)
            .expect("export failed");

        let baked = BakedSplatBuffer {
            version: 1,
            positions: vec![[1.0, 2.0, 3.0]],
            colors: vec![[1.0, 1.0, 1.0, 1.0]],
            scales: vec![[1.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]],
        };
        let entries = vec![(
            "10/512/340".to_string(),
            encode_baked_splats(&baked).unwrap(),
        )];
        let augmented = append_baked_splats_to_archive(&base, &entries).unwrap();

        let source = HexonTileSource::from_archive(&augmented).expect("from_archive failed");
        let coord = TileCoord::new(512, 340, 10);
        let loaded = source
            .get_baked_splats(coord)
            .expect("baked splats should load");
        assert_eq!(loaded.positions, baked.positions);
        assert!(source.get_baked_splats(TileCoord::new(0, 0, 10)).is_none());
    }

    #[test]
    fn no_baked_splats_falls_back_to_empty_map() {
        let meta = test_tileset_meta();
        let archive_bytes =
            HexonArchive::export_tileset(test_manifest(), &meta, &[], &[], None).unwrap();
        let source = HexonTileSource::from_archive(&archive_bytes).unwrap();
        assert!(source.member_keys(TileLayer::BakedSplats).is_empty());
        assert!(source
            .get_baked_splats(TileCoord::new(512, 340, 10))
            .is_none());
    }

    #[test]
    fn covers_checks_bounds_and_zoom() {
        let meta = test_tileset_meta();
        let archive_bytes =
            HexonArchive::export_tileset(test_manifest(), &meta, &[], &[], None).unwrap();
        let source = HexonTileSource::from_archive(&archive_bytes).unwrap();

        // A tile at zoom 11 within bounds should be covered
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 11);
        assert!(source.covers(coord));

        // Zoom 5 is outside range
        let coord = TileCoord::from_lat_lon(45.5, -121.5, 5);
        assert!(!source.covers(coord));
    }

    /// p2p_asset_streaming S6: `HexonTileSource::open` on a chunked container
    /// serves tiles from EVERY chunk (lazily, one chunk decoded per distinct
    /// chunk touched) through the exact same resident-cache API an installed
    /// monolithic tileset uses.
    #[test]
    fn container_backed_source_serves_tiles_from_every_chunk() {
        use fe_format::ChunkInput;

        let mut chunks = Vec::new();
        for seq in 0..3u32 {
            let chunk_tiles = vec![(format!("10/{seq}/0"), vec![seq as u8; 64])];
            let chunk_bytes = HexonArchive::export_tileset(
                test_manifest(),
                &test_tileset_meta(),
                &chunk_tiles,
                &[],
                None,
            )
            .unwrap();
            chunks.push(ChunkInput {
                seq,
                tile_count: 1,
                zoom: 10,
                x_range: [seq, seq],
                y_range: [0, 0],
                hilbert_range: [seq as u64, seq as u64],
                archive_bytes: chunk_bytes,
            });
        }
        let mut outer_meta = test_tileset_meta();
        outer_meta.tile_count = 3;
        let mut outer_manifest = test_manifest();
        outer_manifest.hexon_id = "container-src".into();
        let container_bytes =
            HexonArchive::export_chunked(outer_manifest, &outer_meta, &chunks).unwrap();

        let path = std::env::temp_dir().join(format!(
            "fe_hexon_source_container_test_{}.hexon",
            std::process::id()
        ));
        std::fs::write(&path, &container_bytes).unwrap();

        let source = HexonTileSource::open(&path).expect("open container");
        assert!(source.is_lazy());
        for seq in 0..3u32 {
            let coord = TileCoord::new(seq, 0, 10);
            assert!(source.has_tile(coord), "chunk {seq} tile must be a member");
            let data = source
                .load_tile_blocking(coord)
                .expect("load must not error")
                .expect("tile must be present");
            assert_eq!(data.as_ref(), vec![seq as u8; 64].as_slice());
            // Resident now (no further I/O needed).
            assert!(source.get_tile(coord).is_some());
        }
        assert!(!source.has_tile(TileCoord::new(99, 0, 10)));

        let _ = std::fs::remove_file(&path);
    }

    /// S6: a container chunk that carries a baked-splats section (built via
    /// `append_baked_splats_to_archive`, same as `builder.rs::
    /// package_tiles_chunked` does per chunk) serves it through the
    /// container backing too, not just the eager `from_archive` path.
    #[test]
    fn container_backed_source_serves_baked_splats() {
        use crate::splat::format::{append_baked_splats_to_archive, encode_baked_splats};
        use fe_format::ChunkInput;

        let coord_key = "10/0/0".to_string();
        let chunk_tiles = vec![(coord_key.clone(), vec![9u8; 32])];
        let base = HexonArchive::export_tileset(
            test_manifest(),
            &test_tileset_meta(),
            &chunk_tiles,
            &[],
            None,
        )
        .unwrap();
        let baked = BakedSplatBuffer {
            version: 1,
            positions: vec![[1.0, 2.0, 3.0]],
            colors: vec![[1.0, 1.0, 1.0, 1.0]],
            scales: vec![[1.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]],
        };
        let splat_entries = vec![(coord_key, encode_baked_splats(&baked).unwrap())];
        let chunk_bytes = append_baked_splats_to_archive(&base, &splat_entries).unwrap();

        let mut outer_meta = test_tileset_meta();
        outer_meta.tile_count = 1;
        let mut outer_manifest = test_manifest();
        outer_manifest.hexon_id = "container-splats".into();
        let container_bytes = HexonArchive::export_chunked(
            outer_manifest,
            &outer_meta,
            &[ChunkInput {
                seq: 0,
                tile_count: 1,
                zoom: 10,
                x_range: [0, 0],
                y_range: [0, 0],
                hilbert_range: [0, 0],
                archive_bytes: chunk_bytes,
            }],
        )
        .unwrap();

        let path = std::env::temp_dir().join(format!(
            "fe_hexon_source_container_splats_test_{}.hexon",
            std::process::id()
        ));
        std::fs::write(&path, &container_bytes).unwrap();

        let source = HexonTileSource::open(&path).expect("open container");
        let coord = TileCoord::new(0, 0, 10);
        let loaded = source
            .load_baked_splats_blocking(coord)
            .expect("read must not error")
            .expect("splats must be present");
        assert_eq!(loaded.positions, baked.positions);

        let _ = std::fs::remove_file(&path);
    }

    /// S8 (R8 nested budget): a container chunk whose splat entries are each
    /// under the per-entry cap but together inflate past the streamed-bundle
    /// budget is refused with the decompression-limit error, instead of being
    /// inflated under the whole-archive budget on every tile miss.
    #[test]
    fn container_splat_reads_are_bounded_by_the_streamed_bundle_budget() {
        use crate::splat::format::encode_baked_splats;
        use fe_format::ChunkInput;
        use std::io::Write;

        let base = HexonArchive::export_tileset(
            test_manifest(),
            &test_tileset_meta(),
            &[("10/0/0".to_string(), vec![9u8; 32])],
            &[],
            None,
        )
        .unwrap();
        let baked = BakedSplatBuffer {
            version: 1,
            positions: vec![[1.0, 2.0, 3.0]],
            colors: vec![[1.0, 1.0, 1.0, 1.0]],
            scales: vec![[1.0, 1.0]],
            normals: vec![[0.0, 1.0, 0.0]],
        };
        let mut source_zip = zip::ZipArchive::new(std::io::Cursor::new(&base)).unwrap();
        let mut chunk = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for i in 0..source_zip.len() {
            chunk
                .raw_copy_file(source_zip.by_index_raw(i).unwrap())
                .unwrap();
        }
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let zeros = vec![0u8; 1 << 20];
        let per_entry_mib = (STREAMED_BUNDLE_DECODE_BUDGET >> 20) * 5 / 8;
        for key in ["10/1/0", "10/2/0"] {
            chunk
                .start_file(format!("terrain/splats/{key}.bin"), deflated)
                .unwrap();
            for _ in 0..per_entry_mib {
                chunk.write_all(&zeros).unwrap();
            }
        }
        chunk
            .start_file("terrain/splats/10/0/0.bin", deflated)
            .unwrap();
        chunk
            .write_all(&encode_baked_splats(&baked).unwrap())
            .unwrap();
        let chunk_bytes = chunk.finish().unwrap().into_inner();

        let mut outer_manifest = test_manifest();
        outer_manifest.hexon_id = "container-splat-bomb".into();
        let container_bytes = HexonArchive::export_chunked(
            outer_manifest,
            &test_tileset_meta(),
            &[ChunkInput {
                seq: 0,
                tile_count: 1,
                zoom: 10,
                x_range: [0, 2],
                y_range: [0, 0],
                hilbert_range: [0, 2],
                archive_bytes: chunk_bytes,
            }],
        )
        .unwrap();
        let path = std::env::temp_dir().join(format!(
            "fe_hexon_source_container_splat_bomb_{}.hexon",
            std::process::id()
        ));
        std::fs::write(&path, &container_bytes).unwrap();

        let source = HexonTileSource::open(&path).expect("open container");
        let err = source
            .load_baked_splats_blocking(TileCoord::new(0, 0, 10))
            .expect_err("splats past the nested budget must be refused");
        assert!(
            format!("{err:#}").contains(fe_format::limits::DECOMPRESSION_LIMIT_ERROR),
            "{err:#}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_tile_key_table() {
        let cases: &[(&str, Option<TileKey>)] = &[
            ("10/512/340", Some((10, 512, 340))),
            ("0/0/0", Some((0, 0, 0))),
            ("10/512", None),
            ("10/512/340/1", None),
            ("a/1/2", None),
            ("300/1/2", None), // zoom overflows u8
            ("", None),
        ];
        for (input, want) in cases {
            assert_eq!(parse_tile_key(input), *want, "{input:?}");
        }
    }
}
