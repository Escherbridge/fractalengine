//! Bridges DB petal-terrain results and UI hexon ops into fe-terrain. See AGENTS.md §petal-map
//! and §tileset-p2p.

use std::collections::HashSet;
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
    fe_terrain::tiles::assemble_bundles_from_index_json(
        registry.store(),
        &index_json,
        bundle_paths,
    )?;
    registry.reload(tileset_id)?;
    Ok(())
}

/// Drains completed background assemble results (`HexonOp::AssembleDownload`)
/// and refreshes the Hexon Manager dialog — the one step that needs the main
/// thread, since the install itself already happened off-thread.
pub fn drain_hexon_assemble_results(
    results: Option<Res<HexonAssembleResults>>,
    registry: Option<Res<SharedTilesetRegistry>>,
    mut ui_mgr: ResMut<UiManager>,
) {
    let Some(results) = results else { return };
    let mut refreshed = false;
    while let Ok(outcome) = results.0.try_recv() {
        match outcome {
            HexonAssembleOutcome::Installed { tileset_id } => {
                tracing::info!(tileset_id, "P2P tileset assembled and installed");
                refreshed = true;
            }
            HexonAssembleOutcome::Failed { tileset_id, reason } => {
                tracing::warn!(tileset_id, reason, "P2P tileset assemble/install failed");
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
    /// tileset_ids this node has seen at least one peer advertise.
    advertised: HashSet<String>,
    /// tileset_ids with a `RequestTilesetMeta` in flight (sent, no reply yet).
    meta_requested: HashSet<String>,
    /// tileset_ids whose meta resolution failed — not retried every frame.
    /// Cleared only by a fresh process or (future) an explicit retry action;
    /// no ask for either beyond that here.
    meta_failed: HashSet<String>,
    /// `(tileset_id, bundle_seq)` pairs with a `RequestChunk` in flight.
    chunk_requested: HashSet<(String, u32)>,
}

/// One streamed bundle, decoded off-thread (file read + archive import never
/// run on the Bevy main thread — S4/S7 invariant).
struct StreamedBundleReady {
    tileset_id: String,
    chunk_seq: u32,
    source: HexonTileSource,
}

/// Receiver half, polled each frame by `drain_streamed_bundle_results`.
#[derive(Resource)]
pub struct StreamedBundleResults(crossbeam::channel::Receiver<StreamedBundleReady>);

/// Sender half, cloned into each spawned decode thread.
#[derive(Resource, Clone)]
pub struct StreamedBundleResultSender(crossbeam::channel::Sender<StreamedBundleReady>);

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
/// resolves a `StreamedTileSource` once an index arrives, and hands each
/// `ChunkReceived` WE requested to a background thread for decoding.
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
                                state.advertised.insert(id.to_string());
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
                    state.meta_requested.remove(&tileset_id);
                    if root_hash.is_empty() {
                        tracing::warn!(tileset_id, "streamed tileset meta resolution failed");
                        state.meta_failed.insert(tileset_id);
                        continue;
                    }
                    match StreamedTileSource::from_meta_json(tileset_id.clone(), &meta_json) {
                        Ok(source) => {
                            if let Some(composite) = tile_source.composite.as_mut() {
                                composite.add_streamed_source(source);
                            }
                        }
                        Err(err) => {
                            tracing::warn!(tileset_id, error = %err, "failed to parse streamed bundle index");
                            state.meta_failed.insert(tileset_id);
                        }
                    }
                }
                fe_sync::SyncEvent::ChunkReceived {
                    tileset_id,
                    chunk_seq,
                    path,
                    ..
                } => {
                    if !state
                        .chunk_requested
                        .remove(&(tileset_id.clone(), chunk_seq))
                    {
                        // Not a bundle we asked for on the streamed path —
                        // fe-ui's install-flow owns its own completion
                        // through `TilesetEventBuffer`.
                        continue;
                    }
                    let Some(tx) = bundle_tx.as_ref().map(|t| t.0.clone()) else {
                        continue;
                    };
                    std::thread::spawn(move || {
                        let outcome = std::fs::read(&path)
                            .map_err(anyhow::Error::from)
                            .and_then(|bytes| HexonTileSource::from_archive(&bytes));
                        match outcome {
                            Ok(source) => {
                                if tx
                                    .send(StreamedBundleReady {
                                        tileset_id,
                                        chunk_seq,
                                        source,
                                    })
                                    .is_err()
                                {
                                    tracing::warn!("streamed bundle result channel closed");
                                }
                            }
                            Err(err) => {
                                tracing::warn!(tileset_id, chunk_seq, error = %err, "failed to decode streamed bundle")
                            }
                        }
                    });
                }
                fe_sync::SyncEvent::ChunkFailed {
                    tileset_id,
                    chunk_seq,
                    reason,
                } => {
                    state
                        .chunk_requested
                        .remove(&(tileset_id.clone(), chunk_seq));
                    tracing::warn!(
                        tileset_id,
                        chunk_seq,
                        reason,
                        "streamed bundle fetch failed"
                    );
                }
                _ => {}
            }
        }
    }

    // Request meta for every uri the active composite is waiting on, once
    // we've seen SOME peer advertise it.
    let Some(sender) = sync_sender.as_ref() else {
        return;
    };
    let Some(composite) = tile_source.composite.as_ref() else {
        return;
    };
    let to_request: Vec<String> = composite
        .pending_streamed_uris()
        .filter(|id| {
            state.advertised.contains(*id)
                && !state.meta_requested.contains(*id)
                && !state.meta_failed.contains(*id)
        })
        .map(str::to_string)
        .collect();
    for tileset_id in to_request {
        if let Err(e) = sender.0.send(fe_sync::SyncCommand::RequestTilesetMeta {
            peer_id: String::new(),
            tileset_id: tileset_id.clone(),
        }) {
            tracing::warn!(tileset_id, "RequestTilesetMeta send failed: {e}");
            continue;
        }
        state.meta_requested.insert(tileset_id);
    }
}

/// Applies decoded streamed bundles into the active composite (the one step
/// needing the main thread — decoding itself already ran off-thread).
pub fn drain_streamed_bundle_results(
    results: Option<Res<StreamedBundleResults>>,
    mut tile_source: ResMut<ActiveTileSource>,
) {
    let Some(results) = results else { return };
    while let Ok(ready) = results.0.try_recv() {
        if let Some(composite) = tile_source.composite.as_mut() {
            composite.insert_streamed_bundle(&ready.tileset_id, ready.chunk_seq, ready.source);
        }
    }
}

/// Forwards the tile pointer-set's unmet bundle demand (`TileBundleDemand`,
/// rebuilt each frame by `fetch_and_spawn_terrain_chunks`) to the sync
/// thread, bounded to one `RequestChunk` per not-yet-requested bundle.
pub fn request_demanded_bundles(
    demand: Res<TileBundleDemand>,
    mut state: ResMut<StreamedTilesetState>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
) {
    let Some(sender) = sync_sender.as_ref() else {
        return;
    };
    for (tileset_id, seq) in demand.wanted() {
        let key = (tileset_id.clone(), *seq);
        if state.chunk_requested.contains(&key) {
            continue;
        }
        if let Err(e) = sender.0.send(fe_sync::SyncCommand::RequestChunk {
            peer_id: String::new(),
            tileset_id: tileset_id.clone(),
            chunk_seq: *seq,
            provider: None,
        }) {
            tracing::warn!(tileset_id, chunk_seq = seq, "RequestChunk send failed: {e}");
            continue;
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
            .init_resource::<StreamedTilesetState>();
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
            (drive_streamed_tilesets, drain_streamed_bundle_results).chain(),
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
    }
}
