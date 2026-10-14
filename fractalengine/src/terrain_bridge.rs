//! Bridges DB petal-terrain results and UI hexon ops into fe-terrain. See AGENTS.md §petal-map
//! and §tileset-p2p.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;

use fe_runtime::messages::DbResult;
use fe_terrain::petal_binding::{
    terrain_config_from_petal_json, ActiveTileSource, PetalTerrainAssignment,
    SharedTilesetRegistry, TerrainAssignmentMsg,
};
use fe_terrain::ruler_plugin::RulerSettings;
use fe_terrain::splat::{view_mode_from_terrain_json, TerrainViewModeMsg};
use fe_terrain::terrain_plugin::{ProposalOverlayBudget, MAX_PROPOSAL_OVERLAYS};
use fe_terrain::tiles::{HexonTileSource, StreamedTileSource, TileBundleDemand, TilesetRegistry};
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::plugin::{
    ActiveDialog, HexonOp, InstalledTilesetDto, MeshInstanceBudget, PendingHexonOps,
    StorageInfoDto, UiManager,
};
use fe_ui::settings::AppSettings;
use fe_ui::terrain_map::{DownloadStatus, TilesetDownloads};

/// New proposal ghosts the mesh-instance budget can absorb: none once the
/// watchdog has tripped, else the headroom under the ceiling (capped).
pub fn proposal_overlay_allowance(exceeded: bool, ceiling: usize, last_count: usize) -> usize {
    if exceeded {
        return 0;
    }
    ceiling
        .saturating_sub(last_count)
        .min(MAX_PROPOSAL_OVERLAYS)
}

/// Mirrors fe-ui's `MeshInstanceBudget` into fe-terrain's `ProposalOverlayBudget`
/// (PreUpdate, so ghosts see last frame's watchdog count). See src/AGENTS.md §proposal-budget.
pub fn sync_proposal_overlay_budget(
    budget: Res<MeshInstanceBudget>,
    mut out: ResMut<ProposalOverlayBudget>,
) {
    let allowance = proposal_overlay_allowance(budget.exceeded, budget.ceiling, budget.last_count);
    if out.allowance != allowance {
        out.allowance = allowance;
    }
}

/// Mirrors fe-ui's ruler toggles (`AppSettings`) into fe-terrain's
/// `RulerSettings` — fe-ui has no fe-terrain dependency. See src/AGENTS.md §ruler-settings.
pub fn sync_ruler_settings(settings: Res<AppSettings>, mut ruler: ResMut<RulerSettings>) {
    if ruler.show_scale_bar != settings.show_scale_bar {
        ruler.show_scale_bar = settings.show_scale_bar;
    }
    if ruler.show_grid != settings.show_world_grid {
        ruler.show_grid = settings.show_world_grid;
    }
}

/// Mirrors fe-terrain tile residency (streamed bundles + resident tile bytes)
/// into fe-runtime `ResidencyStats` for fe-ui's Settings readout (fe-ui has no
/// fe-terrain dependency). See src/AGENTS.md §residency-stats.
pub fn mirror_tile_residency_stats(
    tile_source: Option<Res<ActiveTileSource>>,
    mut stats: ResMut<fe_runtime::diag15m::ResidencyStats>,
) {
    let next = tile_source
        .as_deref()
        .and_then(|source| source.composite.as_ref())
        .map(|composite| fe_runtime::diag15m::ResidencyStats {
            tile_bundles: composite.streamed_resident_bundles(),
            tile_bytes: composite
                .resident_bytes()
                .saturating_add(composite.streamed_resident_bytes()),
        })
        .unwrap_or_default();
    if *stats != next {
        *stats = next;
    }
}

/// The tile mode the active composite should honour: the global `Offline`
/// setting forces no-network, otherwise the petal's own mode governs.
pub fn effective_tile_mode(
    global: fe_ui::settings::TileSourceMode,
    petal: &fe_terrain::config::TileSourceMode,
) -> fe_terrain::config::TileSourceMode {
    if global == fe_ui::settings::TileSourceMode::Offline {
        fe_terrain::config::TileSourceMode::Offline
    } else {
        petal.clone()
    }
}

/// Applies `AppSettings.tile_mode` onto the active composite (after each
/// assignment rebuilds it, before chunk fetch). See src/AGENTS.md §tile-mode.
pub fn apply_global_tile_mode(
    settings: Res<AppSettings>,
    active: Res<fe_terrain::petal_binding::ActivePetalTerrain>,
    mut tile_source: ResMut<fe_terrain::petal_binding::ActiveTileSource>,
) {
    let petal_mode = active
        .config
        .as_ref()
        .map(|config| config.tile_source_mode.clone())
        .unwrap_or_default();
    let desired = effective_tile_mode(settings.tile_mode, &petal_mode);
    let needs_update = tile_source
        .composite
        .as_ref()
        .is_some_and(|composite| *composite.tile_source_mode() != desired);
    if needs_update {
        if let Some(composite) = tile_source.composite.as_mut() {
            composite.set_tile_source_mode(desired);
        }
    }
}

/// Forwards the ACTIVE petal's `DbResult::PetalTerrainLoaded` to the terrain
/// runtime as an assignment; other petals' echoes are ignored (AGENTS.md §petal-map).
pub fn bridge_petal_terrain(
    mut db_results: MessageReader<DbResult>,
    nav: Res<NavigationManager>,
    mut assignments: MessageWriter<TerrainAssignmentMsg>,
    mut view_modes: MessageWriter<TerrainViewModeMsg>,
) {
    for result in db_results.read() {
        // Correlated (API-initiated) sets apply too — see fe-runtime §api-reply-correlation.
        if let DbResult::PetalTerrainLoaded {
            petal_id, terrain, ..
        } = result
        {
            // An API write (or API GET fallback) can target any petal; the
            // runtime holds only the active one, so a foreign echo must not
            // replace the viewport's terrain.
            if nav.active_petal_id.as_deref() != Some(petal_id.as_str()) {
                continue;
            }
            let config = terrain.as_ref().and_then(terrain_config_from_petal_json);
            assignments.write(TerrainAssignmentMsg(PetalTerrainAssignment {
                petal_id: petal_id.clone(),
                config,
            }));
            // view_mode is an additive JSON field TerrainConfig drops — forward raw.
            view_modes.write(TerrainViewModeMsg(view_mode_from_terrain_json(
                terrain.as_ref(),
            )));
        }
    }
}

/// Outcome of a background `AssembleDownload` task (campaign plan S4).
/// `TilesetRegistry` is internally lock-protected, so the install + reload
/// itself runs off-thread; only the dialog's `ActiveDialog` carrier needs the
/// main thread, which `drain_hexon_assemble_results` provides.
pub enum HexonAssembleOutcome {
    Installed { tileset_id: String },
    Failed { tileset_id: String, reason: String },
}

/// Receiver half for [`HexonAssembleOutcome`]s — polled each frame by
/// `drain_hexon_assemble_results`.
#[derive(Resource)]
pub struct HexonAssembleResults(pub crossbeam::channel::Receiver<HexonAssembleOutcome>);

/// Sender half, cloned into each spawned assemble thread.
#[derive(Resource, Clone)]
pub struct HexonAssembleResultSender(pub crossbeam::channel::Sender<HexonAssembleOutcome>);

/// Refreshes the Hexon Manager dialog's installed list + storage summary
/// from the registry's store. Shared by the synchronous ops (Install/Remove/
/// SetSeeding/RefreshList) and the background assemble-result drain.
fn refresh_installed_dialog(reg: &TilesetRegistry, ui_mgr: &mut UiManager) {
    if let ActiveDialog::HexonManager {
        ref mut installed_tilesets,
        ref mut storage_info,
        ref mut loading,
        ..
    } = ui_mgr.active_dialog
    {
        let store = reg.store();
        *installed_tilesets = store
            .list_installed()
            .into_iter()
            .map(|t| InstalledTilesetDto {
                hexon_id: t.hexon_id,
                region_name: t.region_name,
                bounds: t.bounds,
                zoom_range: t.zoom_range,
                tile_count: t.tile_count,
                size_bytes: t.size_bytes,
                seeding_enabled: t.seeding_enabled,
                installed_at: t.installed_at.to_rfc3339(),
            })
            .collect();
        *storage_info = StorageInfoDto {
            base_dir: store.base_dir().to_string_lossy().to_string(),
            total_bytes: store.total_disk_usage(),
            count: store.list_installed().len() as u32,
        };
        *loading = false;
    }
}

/// Bundles an **installed** tileset for seeding and publishes it — spawned
/// so bundling (disk I/O + the bundler's CPU work) never runs on the Bevy
/// main thread (S4 invariant). The ONE bundling implementation
/// (`fe_terrain::tiles::bundle_for_seeding`) is shared with the headless
/// relay's startup seeding (`fractalengine-relay/src/main.rs`) — this
/// function is the GUI's thin thread+channel wrapper around it, not a second
/// implementation (see AGENTS.md §tileset-p2p).
pub fn spawn_bundle_and_publish(
    registry: Arc<TilesetRegistry>,
    sync_tx: fe_sync::SyncCommandSender,
    tileset_id: String,
) {
    std::thread::spawn(move || {
        match fe_terrain::tiles::bundle_for_seeding(registry.store(), &tileset_id) {
            Ok((index_path, bundle_paths)) => {
                if let Err(e) = sync_tx.send(fe_sync::SyncCommand::PublishTileset {
                    tileset_id: tileset_id.clone(),
                    index_path,
                    bundle_paths,
                }) {
                    tracing::warn!(tileset_id, "PublishTileset send failed: {e}");
                }
            }
            Err(e) => tracing::warn!(tileset_id, "Bundling for seeding failed: {e}"),
        }
    });
}

/// Drains fe-ui's queued hexon registry ops and refreshes the Hexon Manager dialog.
pub fn drain_hexon_ops(
    mut ops: ResMut<PendingHexonOps>,
    registry: Option<Res<SharedTilesetRegistry>>,
    mut ui_mgr: ResMut<UiManager>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    assemble_tx: Option<Res<HexonAssembleResultSender>>,
) {
    if ops.0.is_empty() {
        return;
    }
    let Some(registry) = registry else {
        tracing::warn!("Hexon ops dropped — tileset registry unavailable");
        ops.0.clear();
        return;
    };
    let reg = &registry.0;

    let mut refresh = false;
    for op in ops.0.drain(..) {
        match op {
            HexonOp::Install(path) => match std::fs::read(&path) {
                Ok(bytes) => match reg.install(&bytes) {
                    Ok(installed) => {
                        tracing::info!(hexon_id = %installed.hexon_id, "Installed hexon tileset");
                        refresh = true;
                    }
                    Err(e) => tracing::warn!("Hexon install failed for {}: {e}", path.display()),
                },
                Err(e) => tracing::warn!("Could not read hexon file {}: {e}", path.display()),
            },
            HexonOp::Remove(id) => {
                match reg.remove(&id) {
                    Ok(()) => tracing::info!(hexon_id = %id, "Removed hexon tileset"),
                    Err(e) => tracing::warn!("Hexon remove failed for {id}: {e}"),
                }
                refresh = true;
                if let Some(sender) = sync_sender.as_ref() {
                    if let Err(e) = sender.0.send(fe_sync::SyncCommand::UnpublishTileset {
                        tileset_id: id.clone(),
                    }) {
                        tracing::warn!(hexon_id = %id, "UnpublishTileset send failed: {e}");
                    }
                }
            }
            HexonOp::SetSeeding(id, enabled) => {
                if let Err(e) = reg.set_seeding(&id, enabled) {
                    tracing::warn!("Hexon set_seeding failed for {id}: {e}");
                }
                refresh = true;
                if enabled {
                    if let Some(sender) = sync_sender.as_ref() {
                        spawn_bundle_and_publish(registry.0.clone(), sender.0.clone(), id.clone());
                    } else {
                        tracing::warn!(
                            hexon_id = %id,
                            "SetSeeding(true) but no sync thread — nothing to publish to"
                        );
                    }
                } else if let Some(sender) = sync_sender.as_ref() {
                    if let Err(e) = sender.0.send(fe_sync::SyncCommand::UnpublishTileset {
                        tileset_id: id.clone(),
                    }) {
                        tracing::warn!(hexon_id = %id, "UnpublishTileset send failed: {e}");
                    }
                }
            }
            HexonOp::RefreshList => refresh = true,
            HexonOp::AssembleDownload {
                tileset_id,
                index_path,
                bundle_paths,
            } => {
                let Some(tx) = assemble_tx.as_ref().map(|t| t.0.clone()) else {
                    tracing::warn!(
                        tileset_id,
                        "AssembleDownload dropped — no result channel registered"
                    );
                    continue;
                };
                let reg_for_task = registry.0.clone();
                std::thread::spawn(move || {
                    let outcome = assemble_and_install(
                        &reg_for_task,
                        &tileset_id,
                        &index_path,
                        &bundle_paths,
                    );
                    let result = match outcome {
                        Ok(()) => HexonAssembleOutcome::Installed {
                            tileset_id: tileset_id.clone(),
                        },
                        Err(e) => HexonAssembleOutcome::Failed {
                            tileset_id: tileset_id.clone(),
                            reason: e.to_string(),
                        },
                    };
                    if tx.send(result).is_err() {
                        tracing::warn!(tileset_id, "AssembleDownload result channel closed");
                    }
                });
            }
        }
    }

    if refresh {
        refresh_installed_dialog(reg, &mut ui_mgr);
    }
}

/// Verify + assemble + install one completed P2P download (background
/// thread — `TilesetRegistry`/`HexonStore` do their own locking, so this
/// needs no Bevy access). Re-verifies the bundle hashes against the index
/// even though `TilesetTransfer` already verified each on receipt (defense
/// in depth for peer-supplied bytes — `assemble_bundles` does this).
fn assemble_and_install(
    registry: &TilesetRegistry,
    tileset_id: &str,
    index_path: &std::path::Path,
    bundle_paths: &[std::path::PathBuf],
) -> anyhow::Result<()> {
    let index_json = std::fs::read(index_path)?;
    // S8b: bound to the REQUESTED id — a peer index naming another tileset is refused.
    fe_terrain::tiles::assemble_bundles_from_index_json(
        registry.store(),
        tileset_id,
        &index_json,
        bundle_paths,
    )?;
    registry.reload(tileset_id)?;
    Ok(())
}

/// Drains completed background assemble results (`HexonOp::AssembleDownload`):
/// settles the download tracker (Complete / Failed), discards an installed
/// download's now-redundant cache (`CancelTilesetDownload` — otherwise it sits
/// on disk forever and is re-reported at every start) and refreshes the
/// Hexon Manager dialog. See `src/AGENTS.md` §tileset-p2p.
pub fn drain_hexon_assemble_results(
    results: Option<Res<HexonAssembleResults>>,
    registry: Option<Res<SharedTilesetRegistry>>,
    mut ui_mgr: ResMut<UiManager>,
    mut downloads: Option<ResMut<TilesetDownloads>>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
) {
    let Some(results) = results else { return };
    let mut refreshed = false;
    while let Ok(outcome) = results.0.try_recv() {
        match outcome {
            HexonAssembleOutcome::Installed { tileset_id } => {
                tracing::info!(tileset_id, "P2P tileset assembled and installed");
                refreshed = true;
                if let Some(dl) = downloads.as_mut().and_then(|d| d.0.get_mut(&tileset_id)) {
                    dl.status = DownloadStatus::Complete;
                }
                if let Some(sender) = sync_sender.as_ref() {
                    if let Err(e) = sender.0.send(fe_sync::SyncCommand::CancelTilesetDownload {
                        tileset_id: tileset_id.clone(),
                    }) {
                        tracing::warn!(tileset_id, "download cleanup not sent: {e}");
                    }
                }
            }
            HexonAssembleOutcome::Failed { tileset_id, reason } => {
                tracing::warn!(tileset_id, reason, "P2P tileset assemble/install failed");
                if let Some(dl) = downloads.as_mut().and_then(|d| d.0.get_mut(&tileset_id)) {
                    dl.status = DownloadStatus::Failed(format!("Install failed: {reason}"));
                }
            }
        }
    }
    if refreshed {
        if let Some(registry) = registry.as_ref() {
            refresh_installed_dialog(&registry.0, &mut ui_mgr);
        }
    }
}

// ---------------------------------------------------------------------------
// S7: streamed (not-installed) tilesets — tile pointer-set -> bundle demand
// over P2P. See src/AGENTS.md §tileset-p2p, fe-terrain `src/AGENTS.md`
// §streamed-tilesets.
// ---------------------------------------------------------------------------

/// Per-tileset streaming bookkeeping this binary owns: fe-terrain has no
/// network/provider knowledge and fe-sync stays format-agnostic (root
/// `AGENTS.md` §petal-map), so this is the one place bridging the two for
/// the streamed path.
#[derive(Resource, Default)]
pub struct StreamedTilesetState {
    /// tileset_ids this node has seen at least one peer advertise (capped at
    /// [`MAX_ADVERTISED_TILESETS`] — ad ids come from the network).
    advertised: HashSet<String>,
    /// tileset_ids with a `RequestTilesetMeta` in flight (sent, no reply yet).
    meta_requested: HashSet<String>,
    /// tileset_ids whose meta resolution failed: consecutive failures and the
    /// instant before which it is not requested again (retried, never forever).
    meta_backoff: HashMap<String, (u32, std::time::Instant)>,
    /// `(tileset_id, bundle_seq)` pairs with a `RequestChunk` in flight.
    chunk_requested: HashSet<(String, u32)>,
    /// `(tileset_id, bundle_seq)` pairs received and being decoded off-thread.
    decoding: HashSet<(String, u32)>,
    /// S8b demand-storm guard: per failed bundle, consecutive failures and
    /// the instant before which it is not requested again.
    chunk_backoff: HashMap<(String, u32), (u32, std::time::Instant)>,
}

/// Shortest wait before a failed streamed bundle is requested again.
pub const STREAMED_RETRY_BASE: std::time::Duration = std::time::Duration::from_secs(2);
/// Longest wait between retries of a repeatedly failing streamed bundle.
pub const STREAMED_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(120);
/// Largest streamed bundle file decoded (the transfer's per-bundle cap).
pub const MAX_STREAMED_BUNDLE_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Most streamed bundles requested or decoding at once (bounds the sync
/// thread's fetch tasks and the command burst one frame can send).
pub const MAX_STREAMED_BUNDLES_IN_FLIGHT: usize = 8;
/// Most distinct advertised tileset ids remembered for the streamed path.
pub const MAX_ADVERTISED_TILESETS: usize = 4096;
/// How long a decoded streamed bundle may stay resident with no live chunk
/// pinning it (it landed after the ring moved on).
pub const STREAMED_BUNDLE_IDLE_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Record one more failure for `key` in `backoff`; returns (failures, retry delay).
fn note_failure<K: std::hash::Hash + Eq>(
    backoff: &mut HashMap<K, (u32, std::time::Instant)>,
    key: K,
) -> (u32, std::time::Duration) {
    let failures = backoff
        .get(&key)
        .map_or(1, |(failures, _)| failures.saturating_add(1));
    let delay = streamed_retry_delay(failures);
    backoff.insert(key, (failures, std::time::Instant::now() + delay));
    (failures, delay)
}

/// Exponential backoff after `failures` consecutive failures of one bundle (capped).
pub fn streamed_retry_delay(failures: u32) -> std::time::Duration {
    let doublings = failures.saturating_sub(1).min(16);
    STREAMED_RETRY_BASE
        .saturating_mul(1u32 << doublings)
        .min(STREAMED_RETRY_MAX)
}

/// One streamed bundle's off-thread decode outcome (file read + archive
/// import never run on the Bevy main thread — S4/S7 invariant).
enum StreamedBundleOutcome {
    Ready {
        tileset_id: String,
        chunk_seq: u32,
        source: Box<HexonTileSource>,
    },
    Failed {
        tileset_id: String,
        chunk_seq: u32,
        reason: String,
    },
}

/// Receiver half, polled each frame by `drain_streamed_bundle_results`.
#[derive(Resource)]
pub struct StreamedBundleResults(crossbeam::channel::Receiver<StreamedBundleOutcome>);

/// Sender half, cloned into each spawned decode thread.
#[derive(Resource, Clone)]
pub struct StreamedBundleResultSender(crossbeam::channel::Sender<StreamedBundleOutcome>);

/// Builds the channel pair `main.rs` inserts as two resources (mirrors
/// `HexonAssembleResults`/`HexonAssembleResultSender`).
pub fn streamed_bundle_channel() -> (StreamedBundleResultSender, StreamedBundleResults) {
    let (tx, rx) = crossbeam::channel::unbounded();
    (StreamedBundleResultSender(tx), StreamedBundleResults(rx))
}

/// Drains fe-sync's streamed-tileset event feed (`StreamedTilesetEventBuffer`
/// — a duplicate of `TilesetEventBuffer` dedicated to this path, so this
/// never races fe-ui's own destructive drain of the install-flow buffer; see
/// fe-sync `src/AGENTS.md` §tileset-transfer): tracks advertisements,
/// requests meta for the active composite's unresolved streamed uris,
/// resolves a `StreamedTileSource` once an index WE asked for arrives, and
/// hands each `ChunkReceived` we requested to a background thread for
/// decoding. See `src/AGENTS.md` §tileset-p2p.
pub fn drive_streamed_tilesets(
    mut events: ResMut<fe_sync::StreamedTilesetEventBuffer>,
    mut state: ResMut<StreamedTilesetState>,
    mut tile_source: ResMut<ActiveTileSource>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    bundle_tx: Option<Res<StreamedBundleResultSender>>,
) {
    if !events.events.is_empty() {
        for evt in std::mem::take(&mut events.events) {
            match evt {
                fe_sync::SyncEvent::PeerTilesetAdvertisement {
                    advertisements_json,
                    ..
                } => {
                    if let Ok(ads) =
                        serde_json::from_str::<Vec<serde_json::Value>>(&advertisements_json)
                    {
                        for ad in ads {
                            if let Some(id) = ad.get("tileset_id").and_then(|v| v.as_str()) {
                                if state.advertised.len() < MAX_ADVERTISED_TILESETS {
                                    state.advertised.insert(id.to_string());
                                }
                            }
                        }
                    }
                }
                fe_sync::SyncEvent::TilesetMetaReceived {
                    tileset_id,
                    meta_json,
                    root_hash,
                    ..
                } => {
                    // Only a reply to OUR request: the install flow's meta
                    // replies ride the same feed and must not add sources here.
                    if !state.meta_requested.remove(&tileset_id) {
                        continue;
                    }
                    if root_hash.is_empty() {
                        let (failures, delay) =
                            note_failure(&mut state.meta_backoff, tileset_id.clone());
                        tracing::warn!(
                            tileset_id,
                            failures,
                            retry_in = ?delay,
                            "streamed tileset meta resolution failed"
                        );
                        continue;
                    }
                    let Some(composite) = tile_source.composite.as_mut() else {
                        continue;
                    };
                    // A stale reply (the petal no longer wants this uri) is dropped.
                    if !composite.pending_streamed_uris().any(|id| id == tileset_id) {
                        continue;
                    }
                    match StreamedTileSource::from_meta_json(tileset_id.clone(), &meta_json) {
                        Ok(source) => {
                            state.meta_backoff.remove(&tileset_id);
                            composite.add_streamed_source(source);
                        }
                        Err(err) => {
                            let (failures, delay) =
                                note_failure(&mut state.meta_backoff, tileset_id.clone());
                            tracing::warn!(
                                tileset_id,
                                error = %err,
                                failures,
                                retry_in = ?delay,
                                "failed to parse streamed bundle index"
                            );
                        }
                    }
                }
                fe_sync::SyncEvent::ChunkReceived {
                    tileset_id,
                    chunk_seq,
                    path,
                    ..
                } => {
                    let key = (tileset_id.clone(), chunk_seq);
                    if !state.chunk_requested.remove(&key) {
                        // Not a bundle we asked for on the streamed path —
                        // fe-ui's install-flow owns its own completion
                        // through `TilesetEventBuffer`.
                        continue;
                    }
                    let Some(tx) = bundle_tx.as_ref().map(|t| t.0.clone()) else {
                        continue;
                    };
                    state.decoding.insert(key);
                    std::thread::spawn(move || {
                        // S8b: peer bytes — size-capped before the read and
                        // decoded under the streamed-bundle zip-bomb budget.
                        let outcome = match read_streamed_bundle(&path)
                            .and_then(|bytes| HexonTileSource::from_streamed_bundle(&bytes))
                        {
                            Ok(source) => StreamedBundleOutcome::Ready {
                                tileset_id,
                                chunk_seq,
                                source: Box::new(source),
                            },
                            Err(err) => StreamedBundleOutcome::Failed {
                                tileset_id,
                                chunk_seq,
                                reason: err.to_string(),
                            },
                        };
                        if tx.send(outcome).is_err() {
                            tracing::warn!("streamed bundle result channel closed");
                        }
                    });
                }
                fe_sync::SyncEvent::ChunkFailed {
                    tileset_id,
                    chunk_seq,
                    reason,
                } => {
                    let key = (tileset_id.clone(), chunk_seq);
                    if !state.chunk_requested.remove(&key) {
                        continue;
                    }
                    let (failures, delay) = note_failure(&mut state.chunk_backoff, key);
                    tracing::warn!(
                        tileset_id,
                        chunk_seq,
                        reason,
                        failures,
                        retry_in = ?delay,
                        "streamed bundle fetch failed"
                    );
                }
                _ => {}
            }
        }
    }

    // Request meta for every uri the active composite is waiting on, once
    // we've seen SOME peer advertise it and it is not backing off.
    let Some(sender) = sync_sender.as_ref() else {
        return;
    };
    let Some(composite) = tile_source.composite.as_ref() else {
        return;
    };
    let now = std::time::Instant::now();
    let to_request: Vec<String> = composite
        .pending_streamed_uris()
        .filter(|id| {
            state.advertised.contains(*id)
                && !state.meta_requested.contains(*id)
                && !state
                    .meta_backoff
                    .get(*id)
                    .is_some_and(|(_, retry_at)| *retry_at > now)
        })
        .map(str::to_string)
        .collect();
    for tileset_id in to_request {
        if let Err(e) = sender.0.try_send(fe_sync::SyncCommand::RequestTilesetMeta {
            peer_id: String::new(),
            tileset_id: tileset_id.clone(),
        }) {
            tracing::warn!(
                tileset_id,
                "RequestTilesetMeta not sent (retried next frame): {e}"
            );
            break;
        }
        state.meta_requested.insert(tileset_id);
    }
}

/// Read one streamed bundle file, refusing it past [`MAX_STREAMED_BUNDLE_FILE_BYTES`].
fn read_streamed_bundle(path: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_STREAMED_BUNDLE_FILE_BYTES {
        anyhow::bail!(
            "streamed bundle {} is {len} bytes (cap {MAX_STREAMED_BUNDLE_FILE_BYTES})",
            path.display()
        );
    }
    Ok(std::fs::read(path)?)
}

/// Applies off-thread decode outcomes (a decoded bundle joins the active
/// composite; a failed decode backs off like a failed fetch) and evicts
/// streamed bundles no chunk pinned within [`STREAMED_BUNDLE_IDLE_GRACE`].
pub fn drain_streamed_bundle_results(
    results: Option<Res<StreamedBundleResults>>,
    mut state: ResMut<StreamedTilesetState>,
    mut tile_source: ResMut<ActiveTileSource>,
) {
    let Some(results) = results else { return };
    while let Ok(outcome) = results.0.try_recv() {
        match outcome {
            StreamedBundleOutcome::Ready {
                tileset_id,
                chunk_seq,
                source,
            } => {
                let key = (tileset_id, chunk_seq);
                state.decoding.remove(&key);
                state.chunk_backoff.remove(&key);
                if let Some(composite) = tile_source.composite.as_mut() {
                    composite.insert_streamed_bundle(&key.0, chunk_seq, *source);
                }
            }
            StreamedBundleOutcome::Failed {
                tileset_id,
                chunk_seq,
                reason,
            } => {
                let key = (tileset_id.clone(), chunk_seq);
                state.decoding.remove(&key);
                let (failures, delay) = note_failure(&mut state.chunk_backoff, key);
                tracing::warn!(
                    tileset_id,
                    chunk_seq,
                    reason,
                    failures,
                    retry_in = ?delay,
                    "failed to decode streamed bundle"
                );
            }
        }
    }
    if let Some(composite) = tile_source.composite.as_ref() {
        composite
            .evict_idle_streamed_bundles(STREAMED_BUNDLE_IDLE_GRACE, std::time::Instant::now());
    }
}

/// Forwards the tile pointer-set's unmet bundle demand (`TileBundleDemand`,
/// rebuilt each frame by `fetch_and_spawn_terrain_chunks`) to the sync
/// thread: one `RequestChunk` per bundle not already requested, decoding or
/// backing off, at most [`MAX_STREAMED_BUNDLES_IN_FLIGHT`] outstanding, sent
/// with `try_send` so a full command channel never blocks the main thread.
pub fn request_demanded_bundles(
    demand: Res<TileBundleDemand>,
    mut state: ResMut<StreamedTilesetState>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
) {
    let Some(sender) = sync_sender.as_ref() else {
        return;
    };
    let now = std::time::Instant::now();
    for (tileset_id, seq) in demand.wanted() {
        if state.chunk_requested.len() + state.decoding.len() >= MAX_STREAMED_BUNDLES_IN_FLIGHT {
            return;
        }
        let key = (tileset_id.clone(), *seq);
        if state.chunk_requested.contains(&key) || state.decoding.contains(&key) {
            continue;
        }
        // S8b: a failing bundle waits out its backoff instead of re-requesting every frame.
        if state
            .chunk_backoff
            .get(&key)
            .is_some_and(|(_, retry_at)| *retry_at > now)
        {
            continue;
        }
        if let Err(e) = sender.0.try_send(fe_sync::SyncCommand::RequestChunk {
            peer_id: String::new(),
            tileset_id: tileset_id.clone(),
            chunk_seq: *seq,
            provider: None,
        }) {
            tracing::warn!(
                tileset_id,
                chunk_seq = seq,
                "RequestChunk not sent (retried next frame): {e}"
            );
            return;
        }
        state.chunk_requested.insert(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_overlay_allowance_tracks_mesh_headroom() {
        // (exceeded, ceiling, last_count) -> allowance
        let cases = [
            (false, 100, 40, 60),
            (false, 100, 100, 0),
            (false, 100, 250, 0),
            (true, 100, 40, 0),
            (false, usize::MAX, 0, MAX_PROPOSAL_OVERLAYS),
        ];
        for (exceeded, ceiling, count, want) in cases {
            assert_eq!(
                proposal_overlay_allowance(exceeded, ceiling, count),
                want,
                "{exceeded} {ceiling} {count}"
            );
        }
    }

    #[test]
    fn tile_residency_mirror_clears_a_stale_readout() {
        use fe_runtime::diag15m::ResidencyStats;

        let mut app = App::new();
        app.init_resource::<ResidencyStats>()
            .insert_resource(ResidencyStats {
                tile_bundles: 9,
                tile_bytes: 9,
            })
            .insert_resource(ActiveTileSource {
                composite: None,
                projection: None,
            })
            .add_systems(Update, mirror_tile_residency_stats);
        app.update();
        assert_eq!(
            *app.world().resource::<ResidencyStats>(),
            ResidencyStats::default(),
            "a stale count is cleared once no composite is active"
        );
    }

    #[test]
    fn settings_toggles_reach_the_ruler_overlays() {
        let mut app = App::new();
        app.init_resource::<AppSettings>()
            .init_resource::<RulerSettings>()
            .add_systems(Update, sync_ruler_settings);
        app.update();
        let ruler = app.world().resource::<RulerSettings>();
        assert!(ruler.show_scale_bar && !ruler.show_grid, "calm defaults");

        {
            let mut settings = app.world_mut().resource_mut::<AppSettings>();
            settings.show_world_grid = true;
            settings.show_scale_bar = false;
        }
        app.update();
        let ruler = app.world().resource::<RulerSettings>();
        assert!(ruler.show_grid, "grid follows show_world_grid");
        assert!(!ruler.show_scale_bar, "scale bar follows show_scale_bar");
    }

    #[test]
    fn global_offline_setting_overrides_the_petal_tile_mode() {
        use fe_terrain::config::{TerrainConfig, TileSourceMode as PetalMode};
        use fe_terrain::petal_binding::{ActivePetalTerrain, ActiveTileSource};
        use fe_ui::settings::TileSourceMode as GlobalMode;

        let mut app = App::new();
        app.init_resource::<AppSettings>()
            .insert_resource(ActivePetalTerrain {
                petal_id: Some("petal".into()),
                config: Some(TerrainConfig {
                    enabled: true,
                    tile_source_mode: PetalMode::Online,
                    ..Default::default()
                }),
                revision: 1,
            })
            .insert_resource(ActiveTileSource {
                composite: Some(fe_terrain::tiles::CompositeTileSource::new()),
                projection: None,
            })
            .add_systems(Update, apply_global_tile_mode);
        let mode = |app: &App| {
            app.world()
                .resource::<ActiveTileSource>()
                .composite
                .as_ref()
                .unwrap()
                .tile_source_mode()
                .clone()
        };

        app.update();
        assert_eq!(
            mode(&app),
            PetalMode::Online,
            "petal mode governs by default"
        );

        app.world_mut().resource_mut::<AppSettings>().tile_mode = GlobalMode::Offline;
        app.update();
        assert_eq!(
            mode(&app),
            PetalMode::Offline,
            "global Offline forces no network"
        );

        app.world_mut().resource_mut::<AppSettings>().tile_mode = GlobalMode::Hybrid;
        app.update();
        assert_eq!(
            mode(&app),
            PetalMode::Online,
            "turning Offline off restores the petal mode"
        );
    }

    fn loaded(petal_id: &str, enabled: bool, correlation_id: Option<&str>) -> DbResult {
        let config = fe_terrain::config::TerrainConfig {
            enabled,
            ..Default::default()
        };
        DbResult::PetalTerrainLoaded {
            petal_id: petal_id.to_string(),
            terrain: Some(serde_json::to_value(config).expect("serialize TerrainConfig")),
            correlation_id: correlation_id.map(str::to_string),
        }
    }

    /// The desktop views petal A; API writes to petal B (and B's GUI-less
    /// echoes) must not reach the live terrain runtime, while A's echo —
    /// GUI or API-initiated — still does.
    #[test]
    fn only_the_active_petals_terrain_reaches_the_runtime() {
        let mut app = App::new();
        app.add_message::<DbResult>()
            .add_message::<TerrainAssignmentMsg>()
            .add_message::<TerrainViewModeMsg>()
            .insert_resource(NavigationManager {
                active_petal_id: Some("petal-a".to_string()),
                ..Default::default()
            })
            .add_systems(Update, bridge_petal_terrain);

        let world = app.world_mut();
        world.write_message(loaded("petal-b", true, Some("api-1")));
        world.write_message(loaded("petal-b", false, None));
        world.write_message(loaded("petal-a", true, Some("api-2")));
        app.update();

        let assignments: Vec<PetalTerrainAssignment> = app
            .world_mut()
            .resource_mut::<Messages<TerrainAssignmentMsg>>()
            .drain()
            .map(|TerrainAssignmentMsg(a)| a)
            .collect();
        assert_eq!(assignments.len(), 1, "{assignments:?}");
        assert_eq!(assignments[0].petal_id, "petal-a");
        assert!(assignments[0].config.as_ref().is_some_and(|c| c.enabled));
        assert_eq!(
            app.world_mut()
                .resource_mut::<Messages<TerrainViewModeMsg>>()
                .drain()
                .count(),
            1,
            "view mode follows the same active-petal filter"
        );
    }

    fn synthetic_tileset_manifest(id: &str) -> fe_format::manifest::HexonManifest {
        fe_format::manifest::HexonManifest {
            schema_version: "1.0.0".into(),
            hexon_id: id.into(),
            hexon_type: fe_format::manifest::HexonType::TerrainTileset,
            publisher_did: "did:key:ztest".into(),
            publisher_name: None,
            version: "0.1.0".into(),
            build_id: None,
            name: "S4 host-wiring test tileset".into(),
            description: None,
            tags: vec!["terrain".into(), "tileset".into()],
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

    /// A tiny (2-tile, single-zoom) synthetic tileset archive, enough to
    /// exercise the real `bundle_tileset` → disk → `assemble_and_install`
    /// path without the size-tuning this slice doesn't own (S1 owns bundle
    /// sizing/exhaustiveness; this test only proves the host wiring).
    fn synthetic_tileset(id: &str) -> fe_format::HexonArchiveData {
        let elevation_tiles = vec![
            ("10/0/0".to_string(), vec![1u8, 2, 3, 4, 5, 6, 7, 8]),
            ("10/0/1".to_string(), vec![9u8, 10, 11, 12, 13, 14, 15, 16]),
        ];
        let meta = fe_format::manifest::TilesetMeta {
            bounds: [45.0, -122.0, 46.0, -121.0],
            min_zoom: 10,
            max_zoom: 10,
            tile_size: 256,
            elevation_encoding: fe_format::manifest::ElevationEncoding::TerrainRgb,
            has_satellite: false,
            tile_count: elevation_tiles.len() as u32,
            satellite_tile_count: 0,
            region_name: "Host Wiring Test".into(),
            parent_tileset: None,
            chunk_index: None,
            native_scale: None,
            ground_sample_distance_m: None,
            crs: None,
            scale_bounds: None,
        };
        fe_format::HexonArchiveData {
            manifest: synthetic_tileset_manifest(id),
            entries: vec![],
            license: None,
            nodes: vec![],
            field_defs: vec![],
            schema: fe_format::SchemaDefinition { field_defs: vec![] },
            assets: vec![],
            terrain_config: None,
            gpx_files: vec![],
            geojson_overlays: vec![],
            tileset_meta: Some(meta),
            elevation_tiles,
            satellite_tiles: vec![],
        }
    }

    /// `fractalengine::terrain_bridge`'s half of the S4 download→install
    /// flow: a real S1 bundle set on disk, verified + assembled + installed
    /// by `assemble_and_install` (the function `drain_hexon_ops` spawns onto
    /// a background thread for `HexonOp::AssembleDownload`), and read back
    /// through the registry exactly as `drain_hexon_assemble_results`'
    /// refresh would see it.
    #[test]
    fn a_completed_download_is_assembled_and_installed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = fe_terrain::tiles::HexonStore::with_dir(tmp.path().join("hexons"))
            .expect("fresh hexon store");
        let registry = fe_terrain::tiles::TilesetRegistry::new(store);

        let tileset_id = "s4-host-wiring-tileset";
        let source = synthetic_tileset(tileset_id);
        let bundles =
            fe_terrain::tiles::bundle_tileset(&source, fe_terrain::tiles::DEFAULT_BUNDLE_BYTES)
                .expect("bundle_tileset");

        let download_dir = tmp.path().join("downloads").join(tileset_id);
        std::fs::create_dir_all(&download_dir).expect("mkdir download dir");
        let index_path = download_dir.join("index.json");
        std::fs::write(&index_path, &bundles.index_json).expect("write index");
        let mut bundle_paths = Vec::new();
        for (seq, bytes) in bundles.bundles.iter().enumerate() {
            let path = download_dir.join(format!("{seq}.bundle"));
            std::fs::write(&path, bytes).expect("write bundle");
            bundle_paths.push(path);
        }

        assemble_and_install(&registry, tileset_id, &index_path, &bundle_paths)
            .expect("assemble_and_install");

        let installed = registry.list_tilesets();
        assert!(
            installed.iter().any(|t| t.tileset_id == tileset_id),
            "installed tilesets: {installed:?}"
        );
        let tile = registry.get_tile(tileset_id, 10, 0, 0);
        assert_eq!(
            tile.as_deref(),
            Some(&[1u8, 2, 3, 4, 5, 6, 7, 8][..]),
            "a tile reads back byte-identical through the registry"
        );
    }

    /// S8b mandatory finding #2, host wiring: a completed download whose
    /// index names a DIFFERENT tileset than the one requested is refused by
    /// `assemble_and_install`, and the already-installed tileset of that
    /// other id keeps its bytes.
    #[test]
    fn a_download_naming_another_tileset_never_overwrites_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = fe_terrain::tiles::HexonStore::with_dir(tmp.path().join("hexons"))
            .expect("fresh hexon store");
        let victim = fe_format::HexonArchive::export_tileset(
            synthetic_tileset_manifest("victim"),
            synthetic_tileset("victim").tileset_meta.as_ref().unwrap(),
            &[("10/0/0".to_string(), vec![42u8; 8])],
            &[],
            None,
        )
        .expect("export victim");
        store.install_tileset(&victim).expect("install victim");
        let victim_path = store.base_dir().join("victim.hexon");
        let before = std::fs::read(&victim_path).unwrap();
        let registry = fe_terrain::tiles::TilesetRegistry::new(store);

        let bundles = fe_terrain::tiles::bundle_tileset(
            &synthetic_tileset("victim"),
            fe_terrain::tiles::DEFAULT_BUNDLE_BYTES,
        )
        .expect("bundle_tileset");
        let download_dir = tmp.path().join("downloads").join("requested");
        std::fs::create_dir_all(&download_dir).unwrap();
        let index_path = download_dir.join("index.json");
        std::fs::write(&index_path, &bundles.index_json).unwrap();
        let mut bundle_paths = Vec::new();
        for (seq, bytes) in bundles.bundles.iter().enumerate() {
            let path = download_dir.join(format!("{seq}.bundle"));
            std::fs::write(&path, bytes).unwrap();
            bundle_paths.push(path);
        }

        let err = assemble_and_install(&registry, "requested", &index_path, &bundle_paths)
            .expect_err("an index naming another tileset is refused");
        assert!(err.to_string().contains("requested"), "{err:#}");
        assert_eq!(std::fs::read(&victim_path).unwrap(), before);
        let installed = registry.list_tilesets();
        assert!(
            installed.iter().all(|t| t.tileset_id != "requested"),
            "{installed:?}"
        );
        assert_eq!(
            registry.get_tile("victim", 10, 0, 0).as_deref(),
            Some(&[42u8; 8][..]),
            "the victim still serves its own tile"
        );
    }

    /// S8b demand-storm guard: after a streamed bundle fails, the next
    /// frames do NOT re-request it (the sync thread is not hammered at
    /// frame rate); a bundle that never failed is still requested.
    #[test]
    fn a_failed_streamed_bundle_is_not_re_requested_every_frame() {
        let (cmd_tx, cmd_rx) = crossbeam::channel::unbounded();
        let mut app = App::new();
        app.init_resource::<fe_sync::StreamedTilesetEventBuffer>()
            .init_resource::<StreamedTilesetState>()
            .init_resource::<fe_terrain::tiles::TileBundleDemand>()
            .insert_resource(ActiveTileSource::default())
            .insert_resource(fe_sync::SyncCommandSenderRes(cmd_tx));
        app.add_systems(
            Update,
            (drive_streamed_tilesets, request_demanded_bundles).chain(),
        );
        app.world_mut()
            .resource_mut::<fe_terrain::tiles::TileBundleDemand>()
            .demand("ts", 0);

        let requested = |rx: &crossbeam::channel::Receiver<fe_sync::SyncCommand>| {
            rx.try_iter()
                .filter(|cmd| matches!(cmd, fe_sync::SyncCommand::RequestChunk { .. }))
                .count()
        };
        app.update();
        assert_eq!(requested(&cmd_rx), 1, "first demand is requested");

        app.world_mut()
            .resource_mut::<fe_sync::StreamedTilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::ChunkFailed {
                tileset_id: "ts".to_string(),
                chunk_seq: 0,
                reason: "hash mismatch".to_string(),
            });
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(
            requested(&cmd_rx),
            0,
            "a failed bundle waits out its backoff"
        );

        app.world_mut()
            .resource_mut::<fe_terrain::tiles::TileBundleDemand>()
            .demand("ts", 1);
        app.update();
        assert_eq!(requested(&cmd_rx), 1, "other bundles are unaffected");
    }

    #[test]
    fn streamed_retry_delay_grows_and_caps() {
        let table = [
            (1, STREAMED_RETRY_BASE),
            (2, STREAMED_RETRY_BASE * 2),
            (3, STREAMED_RETRY_BASE * 4),
            (40, STREAMED_RETRY_MAX),
        ];
        for (failures, expected) in table {
            assert_eq!(
                streamed_retry_delay(failures),
                expected,
                "failures={failures}"
            );
        }
    }

    /// S7 flow test: a `ChunkReceived` for a bundle this node actually
    /// requested (`state.chunk_requested` seeded, mirroring what
    /// `request_demanded_bundles` would have done) is decoded off-thread and
    /// becomes servable through the composite's resident (I/O-free) tile
    /// path — exactly what `spawn_chunk` consumes to build a terrain chunk,
    /// without needing the full render pipeline to prove the data path.
    #[test]
    fn answered_bundles_become_terrain_chunks() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tileset_id = "s7-streamed-tileset";
        let source = synthetic_tileset(tileset_id);
        let bundles =
            fe_terrain::tiles::bundle_tileset(&source, fe_terrain::tiles::DEFAULT_BUNDLE_BYTES)
                .expect("bundle_tileset");
        assert_eq!(
            bundles.bundles.len(),
            1,
            "small synthetic tileset -> one bundle"
        );
        let bundle_path = tmp.path().join("0.bundle");
        std::fs::write(&bundle_path, &bundles.bundles[0]).expect("write bundle");

        let mut app = App::new();
        app.init_resource::<fe_sync::StreamedTilesetEventBuffer>()
            .init_resource::<StreamedTilesetState>()
            .init_resource::<fe_runtime::diag15m::ResidencyStats>();
        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.add_streamed_source(StreamedTileSource::new(
            tileset_id.to_string(),
            bundles.index.clone(),
        ));
        app.insert_resource(ActiveTileSource {
            composite: Some(composite),
            projection: None,
        });
        let (tx, rx) = streamed_bundle_channel();
        app.insert_resource(tx);
        app.insert_resource(rx);
        app.add_systems(
            Update,
            (
                drive_streamed_tilesets,
                drain_streamed_bundle_results,
                mirror_tile_residency_stats,
            )
                .chain(),
        );

        // Simulate a prior `request_demanded_bundles` call: this bundle is
        // one WE asked for, so `ChunkReceived` is ours to decode.
        app.world_mut()
            .resource_mut::<StreamedTilesetState>()
            .chunk_requested
            .insert((tileset_id.to_string(), 0));
        app.world_mut()
            .resource_mut::<fe_sync::StreamedTilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::ChunkReceived {
                tileset_id: tileset_id.to_string(),
                chunk_seq: 0,
                path: bundle_path,
                byte_len: bundles.bundles[0].len() as u64,
            });

        let coord = fe_terrain::tiles::TileCoord::new(0, 0, 10);
        let resident = |app: &mut App| {
            app.world()
                .resource::<ActiveTileSource>()
                .composite
                .as_ref()
                .expect("composite")
                .get_tile_sync(coord)
        };

        // The decode runs on a background thread; poll a few frames for it
        // (a fast, local, in-memory operation — bounded, not a sleep loop).
        let mut tile = None;
        for _ in 0..200 {
            app.update();
            if let Some(bytes) = resident(&mut app) {
                tile = Some(bytes);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            tile.as_deref(),
            Some(&[1u8, 2, 3, 4, 5, 6, 7, 8][..]),
            "the streamed bundle's tile is byte-identical and servable with zero further I/O"
        );
        assert!(
            !app.world_mut()
                .resource_mut::<StreamedTilesetState>()
                .chunk_requested
                .contains(&(tileset_id.to_string(), 0)),
            "the completed request is no longer tracked as in-flight"
        );
        let stats = *app
            .world()
            .resource::<fe_runtime::diag15m::ResidencyStats>();
        assert_eq!(
            stats.tile_bundles, 1,
            "S8 readout counts the resident bundle"
        );
        assert!(
            stats.tile_bytes >= 8,
            "and its decoded tile bytes: {stats:?}"
        );
    }

    /// The streamed-path systems, chained as `main.rs` registers them, over a
    /// recording command channel.
    fn streamed_app(
        composite: Option<fe_terrain::tiles::CompositeTileSource>,
    ) -> (App, crossbeam::channel::Receiver<fe_sync::SyncCommand>) {
        let (cmd_tx, cmd_rx) = crossbeam::channel::unbounded();
        let mut app = App::new();
        let (tx, rx) = streamed_bundle_channel();
        app.init_resource::<fe_sync::StreamedTilesetEventBuffer>()
            .init_resource::<StreamedTilesetState>()
            .init_resource::<fe_terrain::tiles::TileBundleDemand>()
            .insert_resource(ActiveTileSource {
                composite,
                projection: None,
            })
            .insert_resource(fe_sync::SyncCommandSenderRes(cmd_tx))
            .insert_resource(tx)
            .insert_resource(rx)
            .add_systems(
                Update,
                (
                    drive_streamed_tilesets,
                    drain_streamed_bundle_results,
                    request_demanded_bundles,
                )
                    .chain(),
            );
        (app, cmd_rx)
    }

    fn chunk_requests(rx: &crossbeam::channel::Receiver<fe_sync::SyncCommand>) -> usize {
        rx.try_iter()
            .filter(|cmd| matches!(cmd, fe_sync::SyncCommand::RequestChunk { .. }))
            .count()
    }

    /// Final review: a received bundle that fails to DECODE (corrupt or
    /// over-budget archive) backs off exactly like a failed fetch. Before,
    /// it fell out of every in-flight set, so the next frame re-requested
    /// it, the sync thread re-read + re-hashed it, and a new decode thread
    /// failed again — a loop at frame rate.
    #[test]
    fn a_bundle_that_fails_to_decode_is_not_refetched_every_frame() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let corrupt = tmp.path().join("0.bundle");
        std::fs::write(&corrupt, b"not a hexon archive").unwrap();
        let (mut app, cmd_rx) = streamed_app(None);
        app.world_mut()
            .resource_mut::<fe_terrain::tiles::TileBundleDemand>()
            .demand("ts", 0);
        app.update();
        assert_eq!(chunk_requests(&cmd_rx), 1, "first demand is requested");

        app.world_mut()
            .resource_mut::<fe_sync::StreamedTilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::ChunkReceived {
                tileset_id: "ts".to_string(),
                chunk_seq: 0,
                path: corrupt,
                byte_len: 19,
            });
        let key = ("ts".to_string(), 0);
        let mut settled = false;
        for _ in 0..200 {
            app.update();
            let state = app.world().resource::<StreamedTilesetState>();
            if state.chunk_backoff.contains_key(&key) && !state.decoding.contains(&key) {
                settled = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(settled, "the decode failure reached the backoff");
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(
            chunk_requests(&cmd_rx),
            0,
            "neither the decode window nor the failure re-requested the bundle"
        );
    }

    /// Final review: a ring that suddenly wants many bundles sends at most
    /// `MAX_STREAMED_BUNDLES_IN_FLIGHT` requests, and none more while they
    /// are unanswered (the sync thread runs one fetch task per request).
    #[test]
    fn streamed_bundle_requests_stay_bounded_in_flight() {
        let (mut app, cmd_rx) = streamed_app(None);
        {
            let mut demand = app
                .world_mut()
                .resource_mut::<fe_terrain::tiles::TileBundleDemand>();
            for seq in 0..20 {
                demand.demand("ts", seq);
            }
        }
        app.update();
        assert_eq!(chunk_requests(&cmd_rx), MAX_STREAMED_BUNDLES_IN_FLIGHT);
        app.update();
        assert_eq!(chunk_requests(&cmd_rx), 0, "nothing answered yet");
    }

    /// Final review: the streamed path shares the meta feed with the install
    /// flow. A meta reply it never asked for must not add a streamed source;
    /// the reply to its own request (after an advertisement) must.
    #[test]
    fn only_a_requested_meta_reply_becomes_a_streamed_source() {
        let tileset_id = "s7-meta-tileset";
        let bundles = fe_terrain::tiles::bundle_tileset(
            &synthetic_tileset(tileset_id),
            fe_terrain::tiles::DEFAULT_BUNDLE_BYTES,
        )
        .expect("bundle_tileset");
        let meta_json = String::from_utf8(bundles.index_json.clone()).expect("utf8 index");
        let reply = || fe_sync::SyncEvent::TilesetMetaReceived {
            peer_id: String::new(),
            tileset_id: tileset_id.to_string(),
            meta_json: meta_json.clone(),
            total_chunks: 1,
            approx_size_bytes: 0,
            root_hash: "ab".repeat(32),
            chunk_hashes: Vec::new(),
            index_path: std::path::PathBuf::new(),
        };
        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.mark_streamed_pending(tileset_id);
        let (mut app, cmd_rx) = streamed_app(Some(composite));
        let pending = |app: &App| {
            app.world()
                .resource::<ActiveTileSource>()
                .composite
                .as_ref()
                .expect("composite")
                .pending_streamed_uris()
                .any(|id| id == tileset_id)
        };
        let buffer = |app: &mut App, event| {
            app.world_mut()
                .resource_mut::<fe_sync::StreamedTilesetEventBuffer>()
                .events
                .push(event)
        };

        buffer(&mut app, reply());
        app.update();
        assert!(pending(&app), "an unsolicited reply adds no source");

        buffer(
            &mut app,
            fe_sync::SyncEvent::PeerTilesetAdvertisement {
                peer_id: "peer".into(),
                advertisements_json: format!(r#"[{{"tileset_id":"{tileset_id}"}}]"#),
            },
        );
        app.update();
        assert_eq!(
            cmd_rx
                .try_iter()
                .filter(|cmd| matches!(cmd, fe_sync::SyncCommand::RequestTilesetMeta { .. }))
                .count(),
            1
        );
        buffer(&mut app, reply());
        app.update();
        assert!(!pending(&app), "our own reply resolves the streamed source");
    }

    /// Final review: an assembled + installed download settles its tracker to
    /// Complete and discards the download cache (otherwise it stays on disk
    /// and is re-reported at every start); a failed install shows Failed.
    #[test]
    fn an_installed_download_completes_and_discards_its_cache() {
        let (cmd_tx, cmd_rx) = crossbeam::channel::unbounded();
        let (result_tx, result_rx) = crossbeam::channel::unbounded();
        let mut app = App::new();
        let mut downloads = TilesetDownloads::default();
        for id in ["installed", "broken"] {
            downloads.0.insert(
                id.to_string(),
                fe_ui::terrain_map::DownloadProgress {
                    tileset_id: id.to_string(),
                    chunks_received: 1,
                    total_chunks: 1,
                    bytes_received: 0,
                    total_bytes_estimate: 0,
                    status: DownloadStatus::Verifying,
                },
            );
        }
        app.init_resource::<UiManager>()
            .insert_resource(downloads)
            .insert_resource(HexonAssembleResults(result_rx))
            .insert_resource(fe_sync::SyncCommandSenderRes(cmd_tx))
            .add_systems(Update, drain_hexon_assemble_results);
        result_tx
            .send(HexonAssembleOutcome::Installed {
                tileset_id: "installed".into(),
            })
            .unwrap();
        result_tx
            .send(HexonAssembleOutcome::Failed {
                tileset_id: "broken".into(),
                reason: "bad bundle".into(),
            })
            .unwrap();
        app.update();

        let downloads = app.world().resource::<TilesetDownloads>();
        assert!(matches!(
            downloads.0["installed"].status,
            DownloadStatus::Complete
        ));
        assert!(matches!(
            &downloads.0["broken"].status,
            DownloadStatus::Failed(reason) if reason.contains("bad bundle")
        ));
        let cancels: Vec<String> = cmd_rx
            .try_iter()
            .filter_map(|cmd| match cmd {
                fe_sync::SyncCommand::CancelTilesetDownload { tileset_id } => Some(tileset_id),
                _ => None,
            })
            .collect();
        assert_eq!(cancels, vec!["installed".to_string()]);
    }
}
