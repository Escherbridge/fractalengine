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
use fe_ui::terrain_map::{DownloadProgress, DownloadStatus, TilesetDownloads};

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
#[derive(Debug)]
pub enum HexonAssembleOutcome {
    Installed {
        tileset_id: String,
    },
    Failed {
        tileset_id: String,
        reason: String,
    },
    /// PAS-P5 S7b: one "install from registry" fetch made progress (header
    /// resolved → `chunks_received == 0`; each verified chunk after that).
    /// Reuses the Downloads tab's existing progress bar, keyed by the
    /// REQUESTED uri (the `hexon_id` the signed header confirms is only known
    /// once the fetch succeeds — see `RegistryInstalled`).
    RegistryProgress {
        uri: String,
        chunks_received: u32,
        total_chunks: u32,
    },
    /// An "install from registry" fetch finished and the archive is on disk,
    /// indexed and reloaded.
    RegistryInstalled {
        uri: String,
        hexon_id: String,
    },
    /// An "install from registry" fetch was refused — a transport failure, or
    /// (the common/important case) every source failed the signed integrity
    /// root (`FetchStrategy`'s `RequireSealed`), so nothing was ever written
    /// to the store.
    RegistryFailed {
        uri: String,
        reason: String,
    },
    /// W3b fixup (AC-30b): `HexonOp::Remove`'s `HexonStore::remove_tileset`
    /// moved off the Bevy main thread — it takes the same `write_lock` a
    /// worker install can hold across a write + `sync_all` of up to 1 GiB
    /// (`fe-terrain/src/AGENTS.md` §install-lock), so running it inline in
    /// `drain_hexon_ops` could freeze frames for that long. The P2P
    /// `UnpublishTileset` side effect moves here too, to stay ordered after
    /// the store write actually lands.
    RemoveApplied {
        tileset_id: String,
        result: Result<(), String>,
    },
    /// Same rationale as `RemoveApplied`, for `HexonOp::SetSeeding`'s
    /// `HexonStore::set_seeding`. `enabled` carries the requested value
    /// through so the apply side can still choose publish vs. unpublish.
    SetSeedingApplied {
        tileset_id: String,
        enabled: bool,
        result: Result<(), String>,
    },
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
                // RP-4b: the sealed publisher "Set petal map" authors as the pin
                // (a header-only read, on the same dialog-op path as list_installed).
                publisher_did: store.installed_publisher(&t.hexon_id),
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
/// main thread (S4 invariant). Sealed tilesets are never re-bundled (PAS-P5
/// seeder rule, applied inside `bundle_for_seeding`). The ONE bundling implementation
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

/// One queued `HexonStore` mutation for [`HexonMutationWorker`].
type HexonMutationJob = Box<dyn FnOnce() + Send>;

/// Serial (FIFO) worker thread for `HexonOp::Remove`/`SetSeeding`: off the
/// Bevy main thread, yet applied in click order — see `src/AGENTS.md`
/// §tileset-p2p (W3b review).
#[derive(Default)]
pub struct HexonMutationWorker(Option<crossbeam::channel::Sender<HexonMutationJob>>);

impl HexonMutationWorker {
    /// Queue `job` behind every earlier one; (re)starts the worker thread on
    /// first use or if a previous one is gone.
    fn submit(&mut self, job: HexonMutationJob) {
        let job = match &self.0 {
            Some(tx) => match tx.send(job) {
                Ok(()) => return,
                Err(crossbeam::channel::SendError(job)) => job,
            },
            None => job,
        };
        let (tx, rx) = crossbeam::channel::unbounded::<HexonMutationJob>();
        let spawned = std::thread::Builder::new()
            .name("hexon-store-mutations".to_string())
            .spawn(move || {
                for job in rx {
                    job();
                }
            });
        match spawned {
            Ok(_) => {
                if tx.send(job).is_err() {
                    tracing::warn!("Hexon store mutation dropped — worker thread exited");
                }
                self.0 = Some(tx);
            }
            Err(e) => {
                tracing::warn!("Hexon store mutation dropped — worker thread failed to start: {e}");
                self.0 = None;
            }
        }
    }
}

/// Drains fe-ui's queued hexon registry ops and refreshes the Hexon Manager dialog.
pub fn drain_hexon_ops(
    mut ops: ResMut<PendingHexonOps>,
    registry: Option<Res<SharedTilesetRegistry>>,
    mut ui_mgr: ResMut<UiManager>,
    assemble_tx: Option<Res<HexonAssembleResultSender>>,
    app_settings: Res<AppSettings>,
    mut downloads: ResMut<TilesetDownloads>,
    mut mutation_worker: Local<HexonMutationWorker>,
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
            // W2b F2: a local `.hexon` file's read + `TilesetRegistry::install`
            // (parse, verify, atomic write, index) moves to a worker thread,
            // mirroring `HexonOp::AssembleDownload` below — neither disk I/O
            // nor archive/signature verification ever runs on the Bevy main
            // thread (S4 invariant). The dialog refreshes once the real
            // outcome lands, via `drain_hexon_assemble_results` — not here,
            // where nothing has happened yet.
            HexonOp::Install(path) => {
                let Some(tx) = assemble_tx.as_ref().map(|t| t.0.clone()) else {
                    tracing::warn!(
                        path = %path.display(),
                        "Hexon install dropped — no result channel registered"
                    );
                    continue;
                };
                let reg_for_task = registry.0.clone();
                std::thread::spawn(move || {
                    let outcome = std::fs::read(&path)
                        .map_err(|e| format!("could not read hexon file {}: {e}", path.display()))
                        .and_then(|bytes| reg_for_task.install(&bytes).map_err(|e| e.to_string()));
                    let result = match outcome {
                        Ok(installed) => HexonAssembleOutcome::Installed {
                            tileset_id: installed.hexon_id,
                        },
                        Err(reason) => HexonAssembleOutcome::Failed {
                            // The hexon_id isn't known until a successful
                            // parse — the local file path is still useful
                            // context for the warn log it reaches.
                            tileset_id: path.display().to_string(),
                            reason,
                        },
                    };
                    if tx.send(result).is_err() {
                        tracing::warn!("Hexon install result channel closed");
                    }
                });
            }
            // W3b fixup (AC-30b): `remove`/`set_seeding` take the same
            // `write_lock` a worker install can hold across a write +
            // `sync_all` of up to 1 GiB (fe-terrain `src/AGENTS.md`
            // §install-lock) — running them inline here could freeze the
            // Bevy main thread for that long. Both run on ONE serial
            // `HexonMutationWorker` (not a thread per op, which would let a
            // quick on→off seeding toggle land off→on); the dialog refresh +
            // P2P side effect apply once the real outcome lands, in
            // `drain_hexon_assemble_results`.
            HexonOp::Remove(id) => {
                let Some(tx) = assemble_tx.as_ref().map(|t| t.0.clone()) else {
                    tracing::warn!(
                        hexon_id = %id,
                        "Hexon remove dropped — no result channel registered"
                    );
                    continue;
                };
                let reg_for_task = registry.0.clone();
                mutation_worker.submit(Box::new(move || {
                    let result = reg_for_task.remove(&id).map_err(|e| e.to_string());
                    if tx
                        .send(HexonAssembleOutcome::RemoveApplied {
                            tileset_id: id.clone(),
                            result,
                        })
                        .is_err()
                    {
                        tracing::warn!(hexon_id = %id, "Hexon remove result channel closed");
                    }
                }));
            }
            HexonOp::SetSeeding(id, enabled) => {
                let Some(tx) = assemble_tx.as_ref().map(|t| t.0.clone()) else {
                    tracing::warn!(
                        hexon_id = %id,
                        "Hexon set_seeding dropped — no result channel registered"
                    );
                    continue;
                };
                let reg_for_task = registry.0.clone();
                mutation_worker.submit(Box::new(move || {
                    let result = reg_for_task
                        .set_seeding(&id, enabled)
                        .map_err(|e| e.to_string());
                    if tx
                        .send(HexonAssembleOutcome::SetSeedingApplied {
                            tileset_id: id.clone(),
                            enabled,
                            result,
                        })
                        .is_err()
                    {
                        tracing::warn!(hexon_id = %id, "Hexon set_seeding result channel closed");
                    }
                }));
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
            HexonOp::InstallFromRegistry { uri } => {
                let Some(tx) = assemble_tx.as_ref().map(|t| t.0.clone()) else {
                    tracing::warn!(
                        uri,
                        "InstallFromRegistry dropped — no result channel registered"
                    );
                    continue;
                };
                // Same precedence as the UI's enable/disable check
                // (`right_sidebar::render_maps_section`): the env var can
                // enable this even when the persisted setting is blank.
                let env_registry_url =
                    std::env::var(fe_ui::settings::HEXON_REGISTRY_URL_ENV_VAR).ok();
                let Some(registry_url) = fe_ui::settings::resolve_hexon_registry_url(
                    env_registry_url.as_deref(),
                    app_settings.hexon_registry_url.as_deref(),
                ) else {
                    tracing::warn!(
                        uri,
                        "InstallFromRegistry dropped — no registry URL configured"
                    );
                    if tx
                        .send(HexonAssembleOutcome::RegistryFailed {
                            uri: uri.clone(),
                            reason: "no hexon registry URL configured".to_string(),
                        })
                        .is_err()
                    {
                        tracing::warn!(uri, "InstallFromRegistry result channel closed");
                    }
                    continue;
                };
                downloads.0.insert(
                    uri.clone(),
                    DownloadProgress {
                        tileset_id: uri.clone(),
                        chunks_received: 0,
                        total_chunks: 0,
                        bytes_received: 0,
                        total_bytes_estimate: 0,
                        status: DownloadStatus::Queued,
                    },
                );
                // W2b F4: resolved once, here at the `drain_hexon_ops` seam
                // — never inside `install_from_registry`/the pure fetch
                // core, and never logged (RUNBOOK §5).
                let token = resolve_hexon_registry_token(
                    std::env::var(HEXON_REGISTRY_TOKEN_ENV_VAR).ok().as_deref(),
                );
                let reg_for_task = registry.0.clone();
                std::thread::spawn(move || {
                    install_from_registry(&reg_for_task, &registry_url, &uri, token, &tx);
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

/// Env var for an optional bearer token on registry reads — **env-only**:
/// there is deliberately no `AppSettings` field, so the token is never
/// persisted to `settings.ron` (RUNBOOK §5: secrets never in files) and
/// never appears in a log line. Resolved once, at the `drain_hexon_ops`
/// seam, by [`resolve_hexon_registry_token`].
pub const HEXON_REGISTRY_TOKEN_ENV_VAR: &str = "FE_HEXON_REGISTRY_TOKEN";

/// A blank env value means "no token" — same shape as
/// `fe_ui::settings::resolve_hexon_registry_url`'s env half, and, like it, a
/// pure function so a table test exercises every case without mutating the
/// process's actual environment (which races across parallel test threads).
pub fn resolve_hexon_registry_token(env: Option<&str>) -> Option<String> {
    env.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Fetch `uri` from `registry_url` and install it (PAS-P5 S7b): header first
/// (`RequireSealed`, no pin — the operator-chosen registry is the trust
/// anchor here, matching `/download`'s existing trust model), then every
/// chunk a container names, reporting progress after the header and after
/// each chunk. No chunk is ever written to disk before `FetchStrategy`
/// verifies it against the signed integrity root — a tampered or unsealed
/// source fails inside `fetch_header`/`fetch_chunk` and nothing reaches
/// [`HexonStore::install_tileset`][store]. Runs its own single-threaded tokio
/// runtime (mirrors `main.rs`'s one-off `block_on` bridges) — never the Bevy
/// main thread (S4 invariant); the caller (`drain_hexon_ops`) is what
/// spawns the OS thread this runs on.
///
/// `token`: W2b F4, resolved by the caller via
/// [`resolve_hexon_registry_token`] — never read from the environment in
/// here, so this function stays plain-token-in, no hidden env access.
///
/// [store]: fe_terrain::tiles::HexonStore::install_tileset
fn install_from_registry(
    registry: &TilesetRegistry,
    registry_url: &str,
    uri: &str,
    token: Option<String>,
    tx: &crossbeam::channel::Sender<HexonAssembleOutcome>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            if tx
                .send(HexonAssembleOutcome::RegistryFailed {
                    uri: uri.to_string(),
                    reason: format!("could not start a runtime for the fetch: {e}"),
                })
                .is_err()
            {
                tracing::warn!(uri, "InstallFromRegistry result channel closed");
            }
            return;
        }
    };

    let client: std::sync::Arc<dyn fe_hexon::partial::PartialHexonFetch> = std::sync::Arc::new(
        fe_hexon::remote::RemoteRegistryClient::new(registry_url.to_string(), token),
    );
    let strategy = fe_hexon::partial::FetchStrategy::new(fe_hexon::p2p::P2pConfig::from_env());
    let tx_progress = tx.clone();
    let uri_for_progress = uri.to_string();
    let result = rt.block_on(fetch_and_install_from_registry(
        &strategy,
        client,
        registry.store(),
        uri,
        fe_format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES,
        move |chunks_received, total_chunks| {
            if tx_progress
                .send(HexonAssembleOutcome::RegistryProgress {
                    uri: uri_for_progress.clone(),
                    chunks_received,
                    total_chunks,
                })
                .is_err()
            {
                tracing::warn!(
                    uri = %uri_for_progress,
                    "InstallFromRegistry progress channel closed"
                );
            }
        },
    ));

    let outcome = match result {
        Ok(installed) => {
            // `assemble_sealed_container`/`install_tileset` write through the
            // store directly (no `TilesetRegistry::install`), so the
            // in-memory source cache needs an explicit reload — same as
            // `assemble_and_install` above.
            if let Err(e) = registry.reload(&installed.hexon_id) {
                tracing::warn!(
                    hexon_id = %installed.hexon_id,
                    "reload after registry install failed: {e}"
                );
            }
            HexonAssembleOutcome::RegistryInstalled {
                uri: uri.to_string(),
                hexon_id: installed.hexon_id,
            }
        }
        Err(e) => HexonAssembleOutcome::RegistryFailed {
            uri: uri.to_string(),
            reason: e.to_string(),
        },
    };
    if tx.send(outcome).is_err() {
        tracing::warn!(uri, "InstallFromRegistry result channel closed");
    }
}

/// Pure fetch+install core (PAS-P5 S7b step 3): `FetchStrategy::fetch_header`
/// then `fetch_chunk` for every chunk a container names (seq `0` is the whole
/// archive for a monolithic hexon). `on_progress(chunks_received, total)` is
/// called once after the header resolves (`0, total`) and once per verified
/// chunk — the "header → 1/N … N/N" progress sequence the Downloads tab
/// renders. A container is reassembled with its seal intact
/// (`assemble_sealed_container`, S6) so the installed file can be re-seeded
/// unchanged; a monolithic archive's single verified chunk IS the archive, so
/// it installs directly.
///
/// `max_total_bytes` bounds the chunk bytes held in memory before reassembly
/// (S8: a signed index naming many large chunks cannot exhaust memory).
async fn fetch_and_install_from_registry(
    strategy: &fe_hexon::partial::FetchStrategy,
    client: std::sync::Arc<dyn fe_hexon::partial::PartialHexonFetch>,
    store: &fe_terrain::tiles::HexonStore,
    uri: &str,
    max_total_bytes: u64,
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<fe_terrain::tiles::InstalledTileset> {
    let sources = [client];
    let header = strategy
        .fetch_header(uri, &sources, None)
        .await
        .map_err(|e| anyhow::anyhow!("header fetch refused: {e}"))?
        .value;

    match header.layout {
        fe_format::Layout::Monolithic => {
            on_progress(0, 1);
            let bytes = strategy
                .fetch_chunk(&header, 0, &sources)
                .await
                .map_err(|e| anyhow::anyhow!("archive fetch refused: {e}"))?
                .value;
            on_progress(1, 1);
            store.install_tileset(&bytes)
        }
        fe_format::Layout::Container { chunk_count } => {
            on_progress(0, chunk_count);
            let mut chunks = Vec::new();
            let mut total: u64 = 0;
            for seq in 0..chunk_count {
                let bytes = strategy
                    .fetch_chunk(&header, seq, &sources)
                    .await
                    .map_err(|e| anyhow::anyhow!("chunk {seq} fetch refused: {e}"))?
                    .value;
                total = total.saturating_add(bytes.len() as u64);
                if total > max_total_bytes {
                    anyhow::bail!("container exceeds {max_total_bytes} bytes at chunk {seq}");
                }
                chunks.push((seq, bytes.to_vec()));
                on_progress(seq + 1, chunk_count);
            }
            fe_terrain::tiles::distribution::assemble_sealed_container(store, &header.doc, &chunks)
        }
    }
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
                tracing::info!(tileset_id, "Hexon tileset installed (P2P or local)");
                refreshed = true;
                // W2b F2: `HexonOp::Install` (a local file, no P2P download
                // behind it) shares this variant with `AssembleDownload`
                // (which always has one), so only a TRACKED download's
                // cache is settled/cancelled — a local install sends no
                // spurious `CancelTilesetDownload` for an id the sync
                // thread never started a download for.
                let was_tracked = downloads
                    .as_mut()
                    .and_then(|d| d.0.get_mut(&tileset_id))
                    .map(|dl| dl.status = DownloadStatus::Complete)
                    .is_some();
                if was_tracked {
                    if let Some(sender) = sync_sender.as_ref() {
                        if let Err(e) = sender.0.send(fe_sync::SyncCommand::CancelTilesetDownload {
                            tileset_id: tileset_id.clone(),
                        }) {
                            tracing::warn!(tileset_id, "download cleanup not sent: {e}");
                        }
                    }
                }
            }
            HexonAssembleOutcome::Failed { tileset_id, reason } => {
                tracing::warn!(tileset_id, reason, "P2P tileset assemble/install failed");
                if let Some(dl) = downloads.as_mut().and_then(|d| d.0.get_mut(&tileset_id)) {
                    dl.status = DownloadStatus::Failed(format!("Install failed: {reason}"));
                }
            }
            // PAS-P5 S7b: "install from registry" — keyed by the requested
            // uri, never routed through `CancelTilesetDownload` (no P2P
            // `DownloadTileset` was ever sent for it, so there is nothing on
            // the sync thread to cancel).
            HexonAssembleOutcome::RegistryProgress {
                uri,
                chunks_received,
                total_chunks,
            } => {
                if let Some(dl) = downloads.as_mut().and_then(|d| d.0.get_mut(&uri)) {
                    dl.status = DownloadStatus::Downloading;
                    dl.chunks_received = chunks_received;
                    dl.total_chunks = total_chunks;
                }
            }
            HexonAssembleOutcome::RegistryInstalled { uri, hexon_id } => {
                tracing::info!(uri, hexon_id, "Installed hexon tileset from registry");
                refreshed = true;
                if let Some(dl) = downloads.as_mut().and_then(|d| d.0.get_mut(&uri)) {
                    dl.status = DownloadStatus::Complete;
                }
            }
            HexonAssembleOutcome::RegistryFailed { uri, reason } => {
                tracing::warn!(uri, reason, "Install from registry failed");
                if let Some(dl) = downloads.as_mut().and_then(|d| d.0.get_mut(&uri)) {
                    dl.status = DownloadStatus::Failed(reason);
                }
            }
            // W3b fixup (AC-30b): applies the op the worker thread actually
            // ran — same ordering as the old inline arms (`drain_hexon_ops`
            // §W3b comment), just deferred until the real outcome lands.
            HexonAssembleOutcome::RemoveApplied { tileset_id, result } => {
                match &result {
                    Ok(()) => tracing::info!(hexon_id = %tileset_id, "Removed hexon tileset"),
                    Err(e) => tracing::warn!(hexon_id = %tileset_id, "Hexon remove failed: {e}"),
                }
                refreshed = true;
                if let Some(sender) = sync_sender.as_ref() {
                    if let Err(e) = sender.0.send(fe_sync::SyncCommand::UnpublishTileset {
                        tileset_id: tileset_id.clone(),
                    }) {
                        tracing::warn!(hexon_id = %tileset_id, "UnpublishTileset send failed: {e}");
                    }
                }
            }
            HexonAssembleOutcome::SetSeedingApplied {
                tileset_id,
                enabled,
                result,
            } => {
                refreshed = true;
                // Fail closed: a seeding ENABLE that never persisted leaves the
                // store saying "not seeding", so it is never advertised. A
                // disable still withdraws the ad even if its persist failed —
                // withdrawing is always the safe direction.
                if let Err(e) = &result {
                    tracing::warn!(hexon_id = %tileset_id, "Hexon set_seeding failed: {e}");
                }
                if enabled && result.is_err() {
                    tracing::warn!(
                        hexon_id = %tileset_id,
                        "SetSeeding(true) not persisted — not publishing"
                    );
                } else if enabled {
                    if let (Some(sender), Some(registry)) =
                        (sync_sender.as_ref(), registry.as_ref())
                    {
                        spawn_bundle_and_publish(
                            registry.0.clone(),
                            sender.0.clone(),
                            tileset_id.clone(),
                        );
                    } else {
                        tracing::warn!(
                            hexon_id = %tileset_id,
                            "SetSeeding(true) but no sync thread — nothing to publish to"
                        );
                    }
                } else if let Some(sender) = sync_sender.as_ref() {
                    if let Err(e) = sender.0.send(fe_sync::SyncCommand::UnpublishTileset {
                        tileset_id: tileset_id.clone(),
                    }) {
                        tracing::warn!(hexon_id = %tileset_id, "UnpublishTileset send failed: {e}");
                    }
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
    /// tileset_ids with a `RequestTilesetMeta` in flight, and when each was sent.
    meta_requested: HashMap<String, std::time::Instant>,
    /// tileset_ids whose meta reply is being verified off-thread (W2b
    /// review), and when verification started: not re-requested meanwhile,
    /// and expired like a stalled request if no verdict ever lands.
    meta_verifying: HashMap<String, std::time::Instant>,
    /// tileset_ids whose meta resolution failed: consecutive failures and the
    /// instant before which it is not requested again (retried, never forever).
    meta_backoff: HashMap<String, (u32, std::time::Instant)>,
    /// `(tileset_id, bundle_seq)` pairs with a `RequestChunk` in flight, and when each was sent.
    chunk_requested: HashMap<(String, u32), std::time::Instant>,
    /// `(tileset_id, bundle_seq)` pairs received and being decoded off-thread.
    decoding: HashSet<(String, u32)>,
    /// S8b demand-storm guard: per failed bundle, consecutive failures and
    /// the instant before which it is not requested again.
    chunk_backoff: HashMap<(String, u32), (u32, std::time::Instant)>,
}

/// PAS-P5 S7a: this node's TOFU-known publisher DID per `tileset_id`, learned
/// the first time an install-flow OR streamed header verifies (RP-4b), and
/// replaced by a petal-authored pin once one verifies. In-memory only (does not
/// survive a process restart — the campaign's own flow test only requires
/// that an UNCONFIRMED download never fetches a chunk across a restart, not
/// that a pin itself persists; a later slice could persist it if a real gap
/// is found). See `src/AGENTS.md` §tileset-p2p.
#[derive(Resource, Default)]
pub struct KnownTilesetPublishers(HashMap<String, String>);

/// Most tileset ids this node ever pins a publisher for (ad-storm bound,
/// mirrors [`MAX_ADVERTISED_TILESETS`]).
pub const MAX_KNOWN_PUBLISHERS: usize = 4096;

/// Which link of the RP-4b pin chain (installed > petal-authored > TOFU)
/// supplied the pin a header was verified against. See `src/AGENTS.md`
/// §tileset-p2p "Publisher pin chain".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinSource {
    /// The locally installed sealed archive's publisher.
    Installed,
    /// The Editor-authored `TerrainConfig::tileset_publisher_dids` entry.
    PetalAuthored,
    /// This node's recorded first-contact pin ([`KnownTilesetPublishers`]).
    Tofu,
    /// No pin at all: the header that verifies becomes the TOFU pin.
    FirstContact,
}

/// RP-4b pin chain: installed > petal-authored > TOFU (AC-47 precedence).
fn resolve_publisher_pin(
    installed: Option<String>,
    petal_authored: Option<String>,
    tofu: Option<String>,
) -> (Option<String>, PinSource) {
    if let Some(pin) = installed {
        return (Some(pin), PinSource::Installed);
    }
    if let Some(pin) = petal_authored {
        return (Some(pin), PinSource::PetalAuthored);
    }
    match tofu {
        Some(pin) => (Some(pin), PinSource::Tofu),
        None => (None, PinSource::FirstContact),
    }
}

/// Record a verified publisher into the node-global TOFU map: node-global
/// sources fill an empty slot only; a petal-authored pin is petal-scoped and
/// never recorded (pin-poisoning fix — AC-47). Bounded by
/// [`MAX_KNOWN_PUBLISHERS`]. See `src/AGENTS.md` §tileset-p2p.
fn record_publisher_pin(
    known: &mut KnownTilesetPublishers,
    tileset_id: &str,
    publisher_did: String,
    source: PinSource,
) {
    if known.0.len() >= MAX_KNOWN_PUBLISHERS && !known.0.contains_key(tileset_id) {
        return;
    }
    match source {
        // Petal-scoped trust never writes node-global state; the petal pin is
        // re-read from the active petal's TerrainConfig on every check.
        PinSource::PetalAuthored => {}
        PinSource::Installed | PinSource::Tofu | PinSource::FirstContact => {
            known
                .0
                .entry(tileset_id.to_string())
                .or_insert(publisher_did);
        }
    }
}

/// Apply-time re-check of an off-thread streamed verdict (T25): the pin
/// chain may have moved while it verified — a first-contact race lost to
/// another publisher, or a petal pin authored meanwhile. `publisher` is
/// `None` for a legacy unsealed index, which no pin ever admits.
fn streamed_verdict_still_trusted(
    source: PinSource,
    publisher: Option<&str>,
    petal_pin: Option<&str>,
    known_pin: Option<&str>,
) -> bool {
    if source == PinSource::Installed {
        return true;
    }
    match petal_pin.or(known_pin) {
        Some(pin) => publisher == Some(pin),
        None => true,
    }
}

/// One streamed meta reply verified under `pin` (worker thread): the source
/// plus the verified sealed publisher (`None` for a legacy unsealed index),
/// which the main thread records as the pin. The header is verified first
/// so the publisher is known even for a container (whose source keeps none).
fn verify_streamed_meta(
    tileset_id: &str,
    meta_json: &str,
    pin: Option<&str>,
) -> Result<(StreamedTileSource, Option<String>), String> {
    if let Ok(doc) = serde_json::from_str::<fe_format::HeaderDoc>(meta_json) {
        let header = fe_hexon::partial::verify_fetched_header(tileset_id, doc.clone(), pin)
            .map_err(|e| format!("the streamed tileset's signed header does not verify: {e}"))?;
        let publisher_did = header.publisher_did().to_string();
        let source =
            StreamedTileSource::from_header_doc(tileset_id.to_string(), &doc, Some(&publisher_did))
                .map_err(|e| e.to_string())?;
        return Ok((source, (!publisher_did.is_empty()).then_some(publisher_did)));
    }
    StreamedTileSource::from_meta_json(tileset_id.to_string(), meta_json, pin)
        .map(|source| (source, None))
        .map_err(|e| e.to_string())
}

/// The active petal's authored publisher pin for `tileset_id`, if any.
fn petal_publisher_pin(
    active: Option<&fe_terrain::petal_binding::ActivePetalTerrain>,
    tileset_id: &str,
) -> Option<String> {
    active
        .and_then(|a| a.config.as_ref())
        .and_then(|c| c.publisher_pin(tileset_id))
        .map(str::to_string)
}

/// PAS-P5 S7a two-phase download gate, pure core: decide whether an
/// install-flow `TilesetMetaReceived` reply may proceed to a chunk fetch.
/// Pure (no I/O, no Bevy) so it is fast-unit-testable without a real P2P
/// harness; the system around it only adds the event/command plumbing.
///
/// `known_pin` is this node's TOFU-pinned publisher for `tileset_id`, when
/// one exists; `ad_publisher_did` is the pin candidate fe-sync carried on the
/// event — the SAME provider-map entry `root_for` chose its root from,
/// never a different entry's claim (fe-sync's own invariant,
/// `fe-sync/src/AGENTS.md` §partial-fetch-p2p). The ad's DID is used only
/// when no local pin exists yet (critic tick 4: an advertiser can never
/// override an already-pinned publisher).
///
/// On success, returns the verified publisher (to record as the TOFU pin,
/// if not already known) and the exact chunk hashes the signed header names.
/// `None` for the hashes means a sealed MONOLITHIC archive, which signs no
/// per-chunk table (the whole-archive check runs later at install, S6) —
/// the caller should confirm with whatever the HashSeq itself already
/// resolved in that case, since there is nothing more for this layer to
/// check. An unsealed header, or anything that fails to parse or verify, is
/// `Err` — every network path requires a seal (PAS-P5 decision A2).
/// `Ok((verified_publisher_did, signed_chunk_hashes))` — see
/// [`verify_install_meta`]'s doc comment for what `None` chunk hashes means.
type VerifiedInstallMeta = Result<(String, Option<Vec<[u8; 32]>>), String>;

fn verify_install_meta(
    tileset_id: &str,
    meta_json: &str,
    ad_publisher_did: &str,
    known_pin: Option<&str>,
) -> VerifiedInstallMeta {
    let doc: fe_format::HeaderDoc =
        serde_json::from_str(meta_json).map_err(|e| format!("not a header document: {e}"))?;
    let pin = known_pin
        .map(str::to_string)
        .or_else(|| (!ad_publisher_did.is_empty()).then(|| ad_publisher_did.to_string()));
    // The same header check `FetchStrategy` runs on every transport (final
    // review): RequireSealed, signed hexon_id bound to `tileset_id` (the pin
    // is keyed by it, S8), layout derived from the signed coverage.
    let header = fe_hexon::partial::verify_fetched_header(tileset_id, doc, pin.as_deref())
        .map_err(|e| match e {
            fe_hexon::partial::PartialFetchError::Substituted { signed, .. } => format!(
                "header refused: it is signed for {signed:?}, not the requested {tileset_id:?}"
            ),
            other => format!("header refused: {other}"),
        })?;
    let publisher_did = header.publisher_did().to_string();
    let coverage = header
        .verified
        .coverage()
        .ok_or_else(|| "sealed header carries no signed coverage".to_string())?;
    let hashes = match coverage.chunk_index() {
        Some(index) => {
            let mut out = Vec::with_capacity(index.bundles.len());
            for bundle in &index.bundles {
                let hash = fe_runtime::blob_store::hash_from_hex(&bundle.blake3_hex)
                    .map_err(|e| format!("bad signed chunk hash: {e}"))?;
                out.push(hash);
            }
            Some(out)
        }
        None => None,
    };
    Ok((publisher_did, hashes))
}

/// One install-flow header verdict, computed off the main thread:
/// `Ok(verified publisher)` or `Err(refusal reason)`.
struct InstallMetaVerdict {
    tileset_id: String,
    outcome: Result<String, String>,
    /// Which pin-chain link the header was verified against.
    pin_source: PinSource,
}

/// Install-flow header verdicts from worker threads, applied by
/// `drive_streamed_tilesets` (TOFU pin, or the refusal shown in the Downloads tab).
#[derive(Resource)]
pub struct InstallMetaVerdicts {
    tx: crossbeam::channel::Sender<InstallMetaVerdict>,
    rx: crossbeam::channel::Receiver<InstallMetaVerdict>,
}

impl Default for InstallMetaVerdicts {
    fn default() -> Self {
        let (tx, rx) = crossbeam::channel::unbounded();
        Self { tx, rx }
    }
}

/// W2b F1: the streamed (not-installed) path's own meta-verification verdict,
/// computed off the main thread by `drive_streamed_tilesets` — a
/// `StreamedTileSource` ready to add to the composite, or a refusal reason.
/// Mirrors [`InstallMetaVerdict`]/[`InstallMetaVerdicts`] but carries the
/// built source itself (the streamed path has no `ConfirmTilesetDownload` to
/// send — it just resolves or backs off). See `src/AGENTS.md` §tileset-p2p.
struct StreamedMetaVerdict {
    tileset_id: String,
    /// The built source and its verified sealed publisher (RP-4b TOFU input).
    outcome: Result<(StreamedTileSource, Option<String>), String>,
    /// Which pin-chain link the header was verified against.
    pin_source: PinSource,
}

/// Streamed-path meta verdicts from worker threads, applied by
/// `drive_streamed_tilesets` (adds the source, if the petal still wants it —
/// or backs off, same as a failed resolution always did).
#[derive(Resource)]
pub struct StreamedMetaVerdicts {
    tx: crossbeam::channel::Sender<StreamedMetaVerdict>,
    rx: crossbeam::channel::Receiver<StreamedMetaVerdict>,
}

impl Default for StreamedMetaVerdicts {
    fn default() -> Self {
        let (tx, rx) = crossbeam::channel::unbounded();
        Self { tx, rx }
    }
}

/// Install-flow `TilesetMetaReceived` handling (PAS-P5 S7a), worker-thread
/// half: verify the signed header under `pin` (installed, then TOFU, then the
/// ad hint) and send `ConfirmTilesetDownload` on success or
/// `CancelTilesetDownload` on refusal, before any chunk is requested.
/// `resolved_chunk_hashes` is used verbatim for a sealed MONOLITHIC archive
/// (no signed per-chunk table exists). See `src/AGENTS.md` §tileset-p2p.
fn verify_install_meta_and_send(
    sender: &fe_sync::SyncCommandSender,
    tileset_id: &str,
    meta_json: &str,
    ad_publisher_did: &str,
    pin: Option<&str>,
    resolved_chunk_hashes: Vec<[u8; 32]>,
) -> Result<String, String> {
    match verify_install_meta(tileset_id, meta_json, ad_publisher_did, pin) {
        Ok((publisher_did, signed_hashes)) => {
            let expected_chunk_hashes = signed_hashes.unwrap_or(resolved_chunk_hashes);
            if let Err(e) = sender.send(fe_sync::SyncCommand::ConfirmTilesetDownload {
                tileset_id: tileset_id.to_string(),
                expected_chunk_hashes,
            }) {
                tracing::warn!(tileset_id, "ConfirmTilesetDownload send failed: {e}");
            }
            Ok(publisher_did)
        }
        Err(reason) => {
            tracing::warn!(
                tileset_id,
                reason,
                "Tileset header verification refused — cancelling the download"
            );
            send_cancel(sender, tileset_id);
            Err(reason)
        }
    }
}

/// `CancelTilesetDownload`, warning (never swallowing) a closed channel.
fn send_cancel(sender: &fe_sync::SyncCommandSender, tileset_id: &str) {
    if let Err(e) = sender.send(fe_sync::SyncCommand::CancelTilesetDownload {
        tileset_id: tileset_id.to_string(),
    }) {
        tracing::warn!(tileset_id, "CancelTilesetDownload send failed: {e}");
    }
}

/// Main-thread half: a verified publisher becomes this node's pin (a TOFU
/// pin never overwrites one; a petal-authored pin is never recorded —
/// [`record_publisher_pin`]); a refusal is shown on the download's tracker.
fn record_install_verdict(
    known: &mut KnownTilesetPublishers,
    downloads: Option<&mut TilesetDownloads>,
    tileset_id: &str,
    outcome: Result<String, String>,
    pin_source: PinSource,
) {
    match outcome {
        Ok(publisher_did) => {
            record_publisher_pin(known, tileset_id, publisher_did, pin_source);
        }
        Err(reason) => {
            if let Some(dl) = downloads.and_then(|d| d.0.get_mut(tileset_id)) {
                dl.status = DownloadStatus::Failed(format!("Integrity check failed: {reason}"));
            }
        }
    }
}

/// Both halves inline (tests drive the pin semantics without a thread).
#[cfg(test)]
fn verify_install_meta_and_confirm(
    known: &mut KnownTilesetPublishers,
    installed_pin: Option<String>,
    sender: &fe_sync::SyncCommandSenderRes,
    tileset_id: String,
    meta_json: &str,
    ad_publisher_did: &str,
    resolved_chunk_hashes: Vec<[u8; 32]>,
) {
    let (pin, pin_source) =
        resolve_publisher_pin(installed_pin, None, known.0.get(&tileset_id).cloned());
    let outcome = verify_install_meta_and_send(
        &sender.0,
        &tileset_id,
        meta_json,
        ad_publisher_did,
        pin.as_deref(),
        resolved_chunk_hashes,
    );
    record_install_verdict(known, None, &tileset_id, outcome, pin_source);
}

/// Shortest wait before a failed streamed bundle is requested again.
pub const STREAMED_RETRY_BASE: std::time::Duration = std::time::Duration::from_secs(2);
/// Longest wait between retries of a repeatedly failing streamed bundle.
pub const STREAMED_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(120);
/// Largest streamed bundle file decoded (the transfer's per-bundle cap).
pub const MAX_STREAMED_BUNDLE_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// How long a meta or bundle request may go unanswered before its in-flight
/// slot is freed into the retry backoff (W1 residual; see `src/AGENTS.md` §tileset-p2p).
pub const STREAMED_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
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

/// Free every meta/bundle slot sent before `now - STREAMED_REQUEST_TIMEOUT`
/// into its backoff (a late reply is then ignored like any unsolicited one);
/// returns how many expired.
pub(crate) fn expire_stalled_requests(
    state: &mut StreamedTilesetState,
    now: std::time::Instant,
) -> usize {
    let stalled = |sent_at: &std::time::Instant| {
        now.saturating_duration_since(*sent_at) >= STREAMED_REQUEST_TIMEOUT
    };
    let metas: Vec<String> = state
        .meta_requested
        .iter()
        .filter(|(_, sent_at)| stalled(sent_at))
        .map(|(id, _)| id.clone())
        .collect();
    let chunks: Vec<(String, u32)> = state
        .chunk_requested
        .iter()
        .filter(|(_, sent_at)| stalled(sent_at))
        .map(|(key, _)| key.clone())
        .collect();
    let verifying: Vec<String> = state
        .meta_verifying
        .iter()
        .filter(|(_, started_at)| stalled(started_at))
        .map(|(id, _)| id.clone())
        .collect();
    for tileset_id in &verifying {
        state.meta_verifying.remove(tileset_id);
        let (failures, delay) = note_failure(&mut state.meta_backoff, tileset_id.clone());
        tracing::warn!(tileset_id, failures, retry_in = ?delay, "streamed tileset meta verification never reported");
    }
    for tileset_id in &metas {
        state.meta_requested.remove(tileset_id);
        let (failures, delay) = note_failure(&mut state.meta_backoff, tileset_id.clone());
        tracing::warn!(tileset_id, failures, retry_in = ?delay, "streamed tileset meta request timed out");
    }
    for key in &chunks {
        state.chunk_requested.remove(key);
        let (failures, delay) = note_failure(&mut state.chunk_backoff, key.clone());
        tracing::warn!(
            tileset_id = key.0,
            chunk_seq = key.1,
            failures,
            retry_in = ?delay,
            "streamed bundle request timed out"
        );
    }
    metas.len() + verifying.len() + chunks.len()
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
#[allow(clippy::too_many_arguments)]
pub fn drive_streamed_tilesets(
    mut events: ResMut<fe_sync::StreamedTilesetEventBuffer>,
    mut state: ResMut<StreamedTilesetState>,
    mut tile_source: ResMut<ActiveTileSource>,
    mut known_publishers: ResMut<KnownTilesetPublishers>,
    sync_sender: Option<Res<fe_sync::SyncCommandSenderRes>>,
    bundle_tx: Option<Res<StreamedBundleResultSender>>,
    tileset_registry: Option<Res<SharedTilesetRegistry>>,
    mut downloads: Option<ResMut<TilesetDownloads>>,
    verdicts: Option<Res<InstallMetaVerdicts>>,
    streamed_verdicts: Option<Res<StreamedMetaVerdicts>>,
    active_terrain: Option<Res<fe_terrain::petal_binding::ActivePetalTerrain>>,
) {
    if let Some(verdicts) = verdicts.as_ref() {
        while let Ok(verdict) = verdicts.rx.try_recv() {
            record_install_verdict(
                &mut known_publishers,
                downloads.as_deref_mut(),
                &verdict.tileset_id,
                verdict.outcome,
                verdict.pin_source,
            );
        }
    }
    // W2b F1: apply streamed-path meta verdicts a worker thread finished
    // verifying since last frame — never on the main thread (see the
    // `TilesetMetaReceived` arm below, which only ever SPAWNS the check).
    if let Some(streamed_verdicts) = streamed_verdicts.as_ref() {
        while let Ok(verdict) = streamed_verdicts.rx.try_recv() {
            state.meta_verifying.remove(&verdict.tileset_id);
            // RP-4b T25: re-check the pin chain at apply time — a verdict
            // computed before a competing first-contact pin (or a newly
            // authored petal pin) landed is refused, never applied.
            let outcome = verdict.outcome.and_then(|(source, publisher)| {
                let petal_pin = petal_publisher_pin(active_terrain.as_deref(), &verdict.tileset_id);
                let known_pin = known_publishers.0.get(&verdict.tileset_id).cloned();
                if streamed_verdict_still_trusted(
                    verdict.pin_source,
                    publisher.as_deref(),
                    petal_pin.as_deref(),
                    known_pin.as_deref(),
                ) {
                    Ok((source, publisher))
                } else {
                    Err(format!(
                        "header publisher {publisher:?} is not the pinned publisher \
                         (petal pin {petal_pin:?}, recorded pin {known_pin:?})"
                    ))
                }
            });
            match outcome {
                Ok((source, publisher)) => {
                    // RP-4b step 3: the streamed path records its first-contact
                    // pin too (symmetric with the install flow); a petal-authored
                    // pin is never recorded (OF-5 pin poisoning).
                    if let Some(publisher_did) = publisher {
                        record_publisher_pin(
                            &mut known_publishers,
                            &verdict.tileset_id,
                            publisher_did,
                            verdict.pin_source,
                        );
                    }
                    state.meta_backoff.remove(&verdict.tileset_id);
                    // Re-check "still wanted" at apply time: the petal may
                    // have changed while verification ran off-thread.
                    if let Some(composite) = tile_source.composite.as_mut() {
                        if composite
                            .pending_streamed_uris()
                            .any(|id| id == verdict.tileset_id)
                        {
                            composite.add_streamed_source(source);
                        }
                    }
                }
                Err(err) => {
                    let (failures, delay) =
                        note_failure(&mut state.meta_backoff, verdict.tileset_id.clone());
                    tracing::warn!(
                        tileset_id = verdict.tileset_id,
                        error = %err,
                        failures,
                        retry_in = ?delay,
                        "streamed tileset header verification refused"
                    );
                }
            }
        }
    }
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
                    chunk_hashes,
                    ad_publisher_did,
                    ..
                } => {
                    // Only a reply to OUR (streamed) request adds a source
                    // here; the install flow's meta replies ride the same
                    // feed and are handled below instead (PAS-P5 S7a).
                    if state.meta_requested.remove(&tileset_id).is_none() {
                        // Only a download the Downloads tab tracks is an
                        // install flow; anything else (a streamed reply that
                        // arrived after its timeout) is ignored, never
                        // confirmed into a full download.
                        let tracked = downloads
                            .as_ref()
                            .is_some_and(|d| d.0.contains_key(&tileset_id));
                        // An empty root is fe-sync's own honest-failure reply.
                        if !tracked || root_hash.is_empty() {
                            continue;
                        }
                        let (Some(sender), Some(verdicts)) =
                            (sync_sender.as_ref(), verdicts.as_ref())
                        else {
                            tracing::warn!(
                                tileset_id,
                                "Install-flow meta reply with no sync thread or verdict channel"
                            );
                            continue;
                        };
                        let sender = sender.0.clone();
                        let verdict_tx = verdicts.tx.clone();
                        let registry = tileset_registry.as_ref().map(|r| Arc::clone(&r.0));
                        let known_pin = known_publishers.0.get(&tileset_id).cloned();
                        let petal_pin = petal_publisher_pin(active_terrain.as_deref(), &tileset_id);
                        // Off the main thread: the installed pin opens the
                        // store's archive and the header check hashes every
                        // header member.
                        std::thread::spawn(move || {
                            // Pin precedence (S8 + RP-4b): installed, then
                            // petal-authored, then TOFU, then the ad hint.
                            let (pin, pin_source) = resolve_publisher_pin(
                                registry.and_then(|r| r.store().installed_publisher(&tileset_id)),
                                petal_pin,
                                known_pin,
                            );
                            let outcome = match chunk_hashes
                                .iter()
                                .map(|h| fe_runtime::blob_store::hash_from_hex(h))
                                .collect::<anyhow::Result<Vec<_>>>()
                            {
                                Ok(resolved_chunk_hashes) => verify_install_meta_and_send(
                                    &sender,
                                    &tileset_id,
                                    &meta_json,
                                    &ad_publisher_did,
                                    pin.as_deref(),
                                    resolved_chunk_hashes,
                                ),
                                Err(e) => {
                                    send_cancel(&sender, &tileset_id);
                                    Err(format!("unparsable chunk hash in the meta reply: {e}"))
                                }
                            };
                            if verdict_tx
                                .send(InstallMetaVerdict {
                                    tileset_id,
                                    outcome,
                                    pin_source,
                                })
                                .is_err()
                            {
                                tracing::warn!("install-flow verdict channel closed");
                            }
                        });
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
                    let Some(composite) = tile_source.composite.as_ref() else {
                        continue;
                    };
                    // A stale reply (the petal no longer wants this uri) is dropped.
                    if !composite.pending_streamed_uris().any(|id| id == tileset_id) {
                        continue;
                    }
                    // W2b F1: the same shape as the install-flow arm above —
                    // parsing (header or legacy index) and, for a sealed
                    // header, its signature verification both move to a
                    // worker thread; this system returns before the
                    // verdict, which `drive_streamed_tilesets` applies on a
                    // later frame via `StreamedMetaVerdicts`.
                    let Some(verdict_tx) = streamed_verdicts.as_ref().map(|v| v.tx.clone()) else {
                        tracing::warn!(
                            tileset_id,
                            "streamed meta verification dropped — no verdict channel registered"
                        );
                        continue;
                    };
                    // Pin precedence (S8 + RP-4b, mirrors the install-flow
                    // arm above): installed, then petal-authored, then TOFU.
                    // With no pin at all, the verified header becomes this
                    // node's first-contact TOFU pin when its verdict applies.
                    // The installed pin opens (and hashes) the store's
                    // archive, so it is resolved on the worker thread too.
                    let registry = tileset_registry.as_ref().map(|r| Arc::clone(&r.0));
                    let known_pin = known_publishers.0.get(&tileset_id).cloned();
                    let petal_pin = petal_publisher_pin(active_terrain.as_deref(), &tileset_id);
                    state
                        .meta_verifying
                        .insert(tileset_id.clone(), std::time::Instant::now());
                    let tileset_id_for_thread = tileset_id.clone();
                    std::thread::spawn(move || {
                        let (pin, pin_source) = resolve_publisher_pin(
                            registry.and_then(|r| {
                                r.store().installed_publisher(&tileset_id_for_thread)
                            }),
                            petal_pin,
                            known_pin,
                        );
                        let outcome = verify_streamed_meta(
                            &tileset_id_for_thread,
                            &meta_json,
                            pin.as_deref(),
                        );
                        if verdict_tx
                            .send(StreamedMetaVerdict {
                                tileset_id: tileset_id_for_thread,
                                outcome,
                                pin_source,
                            })
                            .is_err()
                        {
                            tracing::warn!("streamed meta verdict channel closed");
                        }
                    });
                }
                fe_sync::SyncEvent::ChunkReceived {
                    tileset_id,
                    chunk_seq,
                    path,
                    ..
                } => {
                    let key = (tileset_id.clone(), chunk_seq);
                    if state.chunk_requested.remove(&key).is_none() {
                        // Not a bundle we asked for on the streamed path —
                        // fe-ui's install-flow owns its own completion
                        // through `TilesetEventBuffer`.
                        continue;
                    }
                    let Some(tx) = bundle_tx.as_ref().map(|t| t.0.clone()) else {
                        continue;
                    };
                    // PAS-P5: what the (possibly signed) index promises for this bundle.
                    let check = tile_source
                        .composite
                        .as_ref()
                        .and_then(|c| c.streamed_bundle_check(&tileset_id, chunk_seq));
                    state.decoding.insert(key);
                    std::thread::spawn(move || {
                        // S8b: peer bytes — size-capped before the read and
                        // decoded under the streamed-bundle zip-bomb budget,
                        // only after they match their index digest / seal.
                        let outcome = match read_streamed_bundle(&path)
                            .and_then(|bytes| {
                                check
                                    .ok_or_else(|| {
                                        anyhow::anyhow!(
                                            "bundle {chunk_seq} is not in the tracked index"
                                        )
                                    })?
                                    .verify(&bytes)?;
                                Ok(bytes)
                            })
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
                    if state.chunk_requested.remove(&key).is_none() {
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

    // A request nobody answered frees its slot into the backoff (W1 residual).
    expire_stalled_requests(&mut state, std::time::Instant::now());

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
                && !state.meta_requested.contains_key(*id)
                && !state.meta_verifying.contains_key(*id)
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
        state.meta_requested.insert(tileset_id, now);
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
        if state.chunk_requested.contains_key(&key) || state.decoding.contains(&key) {
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
        state.chunk_requested.insert(key, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_petal_authored_pin_never_poisons_the_global_tofu_map() {
        let mut known = KnownTilesetPublishers::default();
        // Editor on petal X verifies against their petal pin: no global write.
        record_publisher_pin(
            &mut known,
            "ts-1",
            "did:key:attacker".into(),
            PinSource::PetalAuthored,
        );
        assert!(
            known.0.is_empty(),
            "petal-scoped trust wrote node-global state"
        );
        // On petal Y (no pin anywhere) the first verified header becomes TOFU.
        record_publisher_pin(
            &mut known,
            "ts-1",
            "did:key:legit".into(),
            PinSource::FirstContact,
        );
        assert_eq!(
            known.0.get("ts-1").map(String::as_str),
            Some("did:key:legit")
        );
        // Neither a later petal pin nor a later first contact overrides it.
        record_publisher_pin(
            &mut known,
            "ts-1",
            "did:key:attacker".into(),
            PinSource::PetalAuthored,
        );
        record_publisher_pin(
            &mut known,
            "ts-1",
            "did:key:attacker".into(),
            PinSource::FirstContact,
        );
        assert_eq!(
            known.0.get("ts-1").map(String::as_str),
            Some("did:key:legit")
        );
    }

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
            integrity: None,
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
            // The real extent of tiles 10/0/0 and 10/0/1 (Web Mercator top-left corner):
            // a streamed source answers `covers` from these bounds.
            bounds: [84.9, -180.0, 85.051_128_78, -179.6],
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
            integrity: fe_format::IntegrityStatus::Unsealed,
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
        // Startup indexing, as the host does before serving (`load_all`).
        assert_eq!(registry.load_all(), vec!["victim".to_string()]);

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

    // -------------------------------------------------------------------------
    // PAS-P5 S7b: hosted "install from registry" production path — see
    // src/AGENTS.md §tileset-p2p.
    // -------------------------------------------------------------------------

    /// An in-process `fe-hexon-registry` on an ephemeral port, serving
    /// whatever `.hexon` files are placed in `dir` (mirrors
    /// `fe-hexon/tests/support/registry_server.rs`, not reusable as a lib
    /// from this crate). Runs on its own OS thread with its own tokio
    /// runtime — exactly like `install_from_registry`'s production
    /// background thread — so the SYNC tests below can block on the result
    /// channel without needing (or risking deadlocking) an async test runtime.
    fn spawn_test_registry(dir: &std::path::Path, token: Option<String>) -> std::net::SocketAddr {
        let dir = dir.to_path_buf();
        let (addr_tx, addr_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test registry runtime");
            rt.block_on(async move {
                let state = std::sync::Arc::new(fe_hexon_registry::RegistryState::new(
                    dir.clone(),
                    token,
                    false,
                    fe_hexon_registry::scan_dir(&dir),
                ));
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind ephemeral port");
                let addr = listener.local_addr().expect("local_addr");
                addr_tx.send(addr).expect("send bound addr");
                axum::serve(listener, fe_hexon_registry::build_router(state))
                    .await
                    .expect("test registry server");
            });
        });
        addr_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("test registry did not start")
    }

    /// Drains `rx` until either the named uri reaches a terminal
    /// `RegistryInstalled`/`RegistryFailed` outcome or `timeout` elapses;
    /// returns every outcome seen (in order) plus the terminal one, if any.
    fn collect_registry_outcomes(
        rx: &crossbeam::channel::Receiver<HexonAssembleOutcome>,
        timeout: std::time::Duration,
    ) -> Vec<HexonAssembleOutcome> {
        let deadline = std::time::Instant::now() + timeout;
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(outcome) => {
                    let terminal = matches!(
                        outcome,
                        HexonAssembleOutcome::RegistryInstalled { .. }
                            | HexonAssembleOutcome::RegistryFailed { .. }
                    );
                    seen.push(outcome);
                    if terminal {
                        break;
                    }
                }
                Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        seen
    }

    /// A fresh `TilesetRegistry` over a fresh temp-dir `HexonStore`.
    fn fresh_tileset_registry() -> (tempfile::TempDir, TilesetRegistry) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = fe_terrain::tiles::HexonStore::with_dir(tmp.path().join("hexons"))
            .expect("fresh hexon store");
        (tmp, TilesetRegistry::new(store))
    }

    /// Bevy `App` wired exactly as `drain_hexon_ops` needs it in production,
    /// pointed at `registry_url`.
    fn app_for_install_from_registry(
        tileset_registry: std::sync::Arc<TilesetRegistry>,
        registry_url: String,
        tx: crossbeam::channel::Sender<HexonAssembleOutcome>,
    ) -> App {
        let settings = AppSettings {
            hexon_registry_url: Some(registry_url),
            ..Default::default()
        };
        let mut app = App::new();
        app.init_resource::<UiManager>()
            .init_resource::<PendingHexonOps>()
            .insert_resource(TilesetDownloads::default())
            .insert_resource(SharedTilesetRegistry(tileset_registry))
            .insert_resource(HexonAssembleResultSender(tx))
            .insert_resource(settings)
            .add_systems(Update, drain_hexon_ops);
        app
    }

    /// W2b F2: `HexonOp::Install` (a local `.hexon` file picked via a file
    /// dialog) moves its read + `TilesetRegistry::install` to a worker
    /// thread, exactly like `HexonOp::AssembleDownload` already did — a
    /// sealed archive reports `Installed` and is actually installed (a real
    /// tile reads back through the registry); a tampered one reports
    /// `Failed` and installs nothing.
    ///
    /// W3b fixup (AC-30d): the name claimed "off the main thread" but never
    /// asserted it — `app.update()` draining the op and `recv_timeout`
    /// blocking for up to 10s both looked identical whether the install ran
    /// inline or on a worker. Each leg below now checks `rx.try_recv()`
    /// immediately after `app.update()` returns, before blocking on `recv`:
    /// a main-thread (synchronous) install would already have the outcome
    /// sitting in the channel at that point, since the send happens inside
    /// the same system call that drains the op. This is probabilistic for a
    /// fixture this small (the worker could in principle finish first), but
    /// a real regression back to an inline install reliably fails it: an
    /// inline install sends its outcome as part of draining the op, strictly
    /// before `app.update()` can return.
    #[test]
    fn a_local_hexon_install_runs_off_the_main_thread_and_reports_its_outcome() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[95u8; 32]);
        let hexon_id = "w2b-f2-local-install";
        let tile_bytes = vec![0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let unsealed = fe_format::HexonArchive::export_tileset(
            synthetic_tileset_manifest(hexon_id),
            synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
            &[("10/0/0".to_string(), tile_bytes.clone())],
            &[],
            None,
        )
        .expect("export unsealed archive");
        let sealed = fe_format::seal(&unsealed, &key).expect("seal archive");

        let mut tampered = sealed.clone();
        let pos = tampered
            .windows(tile_bytes.len())
            .position(|w| w == tile_bytes.as_slice())
            .expect("tile bytes must appear verbatim in a Stored zip entry");
        tampered[pos] ^= 0xFF;

        let tmp = tempfile::tempdir().expect("tempdir");
        let sealed_path = tmp.path().join("sealed.hexon");
        let tampered_path = tmp.path().join("tampered.hexon");
        std::fs::write(&sealed_path, &sealed).expect("write sealed fixture");
        std::fs::write(&tampered_path, &tampered).expect("write tampered fixture");

        let (_store_tmp, tileset_registry) = fresh_tileset_registry();
        let tileset_registry = std::sync::Arc::new(tileset_registry);
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut app = app_for_install_from_registry(tileset_registry.clone(), String::new(), tx);
        let recv = |rx: &crossbeam::channel::Receiver<HexonAssembleOutcome>| {
            rx.recv_timeout(std::time::Duration::from_secs(10))
                .expect("the worker thread must report an outcome")
        };

        // Tampered: refused off the main thread, nothing installed.
        app.world_mut()
            .resource_mut::<PendingHexonOps>()
            .0
            .push(HexonOp::Install(tampered_path));
        app.update(); // drains the op, spawning the background install thread
        assert!(
            rx.try_recv().is_err(),
            "the outcome must not already be in the channel right after app.update() returns"
        );
        match recv(&rx) {
            HexonAssembleOutcome::Failed { reason, .. } => {
                assert!(!reason.is_empty(), "{reason}");
            }
            other => panic!("expected Failed for a tampered local install, got {other:?}"),
        }
        assert!(
            tileset_registry.store().list_installed().is_empty(),
            "a tampered install must leave the store untouched"
        );

        // Sealed: installed for real, through the SAME registry drain_hexon_ops
        // reads from in production.
        app.world_mut()
            .resource_mut::<PendingHexonOps>()
            .0
            .push(HexonOp::Install(sealed_path));
        app.update();
        assert!(
            rx.try_recv().is_err(),
            "the outcome must not already be in the channel right after app.update() returns"
        );
        match recv(&rx) {
            HexonAssembleOutcome::Installed { tileset_id } => {
                assert_eq!(tileset_id, hexon_id);
            }
            other => panic!("expected Installed for a sealed local install, got {other:?}"),
        }
        assert_eq!(tileset_registry.store().list_installed().len(), 1);
        assert_eq!(
            tileset_registry.get_tile(hexon_id, 10, 0, 0).as_deref(),
            Some(tile_bytes.as_slice()),
            "the installed tile must read back byte-identical"
        );
    }

    /// W3b fixup (AC-30b): `HexonOp::Remove`/`SetSeeding` moved their
    /// `HexonStore` disk work off the Bevy main thread — same shape as F2's
    /// `Install` above. Proves the outcome arrives over the worker-result
    /// channel (`RemoveApplied`/`SetSeedingApplied`), not synchronously, and
    /// that the store actually reflects each op once applied.
    #[test]
    fn remove_and_seeding_ops_report_through_the_worker_channel() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[96u8; 32]);
        let hexon_id = "w3b-remove-seeding-test";
        let sealed = fe_format::seal(
            &fe_format::HexonArchive::export_tileset(
                synthetic_tileset_manifest(hexon_id),
                synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
                &[("10/0/0".to_string(), vec![1u8; 8])],
                &[],
                None,
            )
            .expect("export archive"),
            &key,
        )
        .expect("seal archive");

        let (_store_tmp, tileset_registry) = fresh_tileset_registry();
        tileset_registry.install(&sealed).expect("install fixture");
        let tileset_registry = std::sync::Arc::new(tileset_registry);
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut app = app_for_install_from_registry(tileset_registry.clone(), String::new(), tx);
        let recv = |rx: &crossbeam::channel::Receiver<HexonAssembleOutcome>| {
            rx.recv_timeout(std::time::Duration::from_secs(10))
                .expect("the worker thread must report an outcome")
        };

        // SetSeeding(true): the outcome arrives over the channel, not inline
        // inside `drain_hexon_ops` (no `sync_sender` is wired into this test
        // app, so `drain_hexon_ops` never touches it either — only the
        // worker-returned result matters here).
        app.world_mut()
            .resource_mut::<PendingHexonOps>()
            .0
            .push(HexonOp::SetSeeding(hexon_id.to_string(), true));
        app.update();
        match recv(&rx) {
            HexonAssembleOutcome::SetSeedingApplied {
                tileset_id,
                enabled,
                result,
            } => {
                assert_eq!(tileset_id, hexon_id);
                assert!(enabled);
                assert!(result.is_ok(), "{result:?}");
            }
            other => panic!("expected SetSeedingApplied, got {other:?}"),
        }
        assert!(
            tileset_registry
                .list_tilesets()
                .iter()
                .any(|t| t.tileset_id == hexon_id && t.seeding),
            "the store must reflect the enabled seeding flag"
        );

        // Remove: also over the channel, and the tileset is actually gone
        // from the store (not just reported as gone).
        app.world_mut()
            .resource_mut::<PendingHexonOps>()
            .0
            .push(HexonOp::Remove(hexon_id.to_string()));
        app.update();
        match recv(&rx) {
            HexonAssembleOutcome::RemoveApplied { tileset_id, result } => {
                assert_eq!(tileset_id, hexon_id);
                assert!(result.is_ok(), "{result:?}");
            }
            other => panic!("expected RemoveApplied, got {other:?}"),
        }
        assert!(
            tileset_registry.list_tilesets().is_empty(),
            "the store must reflect the removal"
        );
    }

    /// Installs a one-tile sealed tileset `hexon_id` into `registry`.
    fn install_sealed_fixture(registry: &TilesetRegistry, key_seed: u8, hexon_id: &str) {
        let key = ed25519_dalek::SigningKey::from_bytes(&[key_seed; 32]);
        let sealed = fe_format::seal(
            &fe_format::HexonArchive::export_tileset(
                synthetic_tileset_manifest(hexon_id),
                synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
                &[("10/0/0".to_string(), vec![1u8; 8])],
                &[],
                None,
            )
            .expect("export archive"),
            &key,
        )
        .expect("seal archive");
        registry.install(&sealed).expect("install fixture");
    }

    /// W3b review: Remove/SetSeeding run on ONE serial worker, so a burst of
    /// clicks (one per frame) lands in click order. A thread per op let a
    /// quick on→off toggle land off→on — the store left seeding and the ad
    /// re-published after the user turned it off — and let a Remove overtake
    /// an earlier toggle. Mutation check: back to a `std::thread::spawn` per
    /// op, 16 threads contend for `HexonStore::write_lock` (not FIFO), so the
    /// applied order almost surely differs from the click order.
    #[test]
    fn queued_store_mutations_apply_in_click_order() {
        let hexon_id = "w3b-click-order";
        let (_store_tmp, tileset_registry) = fresh_tileset_registry();
        install_sealed_fixture(&tileset_registry, 98, hexon_id);
        let tileset_registry = std::sync::Arc::new(tileset_registry);
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut app = app_for_install_from_registry(tileset_registry.clone(), String::new(), tx);

        let toggles: Vec<bool> = (0..16).map(|i| i % 2 == 0).collect();
        for &enabled in &toggles {
            app.world_mut()
                .resource_mut::<PendingHexonOps>()
                .0
                .push(HexonOp::SetSeeding(hexon_id.to_string(), enabled));
            app.update();
        }
        app.world_mut()
            .resource_mut::<PendingHexonOps>()
            .0
            .push(HexonOp::Remove(hexon_id.to_string()));
        app.update();

        let mut applied = Vec::new();
        for _ in &toggles {
            match rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("every toggle reports an outcome")
            {
                HexonAssembleOutcome::SetSeedingApplied {
                    enabled, result, ..
                } => {
                    assert!(result.is_ok(), "{result:?}");
                    applied.push(enabled);
                }
                other => panic!("expected SetSeedingApplied before the Remove, got {other:?}"),
            }
        }
        assert_eq!(applied, toggles, "toggles must apply in click order");
        match rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the remove reports an outcome")
        {
            HexonAssembleOutcome::RemoveApplied { result, .. } => {
                assert!(result.is_ok(), "{result:?}")
            }
            other => panic!("expected RemoveApplied last, got {other:?}"),
        }
        assert!(tileset_registry.list_tilesets().is_empty());
    }

    /// Runs `app` frames until `done(seen)` holds or `timeout` passes;
    /// returns every `SyncCommand` the frames sent.
    fn pump_sync_commands(
        app: &mut App,
        cmd_rx: &crossbeam::channel::Receiver<fe_sync::SyncCommand>,
        timeout: std::time::Duration,
        done: impl Fn(&[fe_sync::SyncCommand]) -> bool,
    ) -> Vec<fe_sync::SyncCommand> {
        let deadline = std::time::Instant::now() + timeout;
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            app.update();
            seen.extend(cmd_rx.try_iter());
            if done(&seen) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        seen
    }

    /// W3b review, full flow (`drain_hexon_ops` → worker →
    /// `drain_hexon_assemble_results` → sync thread): a seeding toggle's P2P
    /// side effect follows the store write that actually landed. Enable
    /// publishes, disable unpublishes, and an enable whose write FAILED (a
    /// corrupt `registry.json` no longer lists the tileset, though its
    /// `.hexon` is still bundleable) is never advertised. Mutation check:
    /// dropping the `result.is_err()` gate publishes on the last leg.
    #[test]
    fn a_seeding_enable_that_fails_to_persist_is_never_published() {
        let hexon_id = "w3b-fail-closed";
        let (_store_tmp, tileset_registry) = fresh_tileset_registry();
        install_sealed_fixture(&tileset_registry, 99, hexon_id);
        let registry_json = tileset_registry.store().base_dir().join("registry.json");
        let tileset_registry = std::sync::Arc::new(tileset_registry);
        let (tx, rx) = crossbeam::channel::unbounded();
        let (cmd_tx, cmd_rx) = crossbeam::channel::unbounded();
        let mut app = app_for_install_from_registry(tileset_registry, String::new(), tx);
        app.insert_resource(HexonAssembleResults(rx))
            .insert_resource(fe_sync::SyncCommandSenderRes(cmd_tx))
            .add_systems(Update, drain_hexon_assemble_results);
        let is_publish = |c: &fe_sync::SyncCommand| matches!(c, fe_sync::SyncCommand::PublishTileset { tileset_id, .. } if tileset_id == hexon_id);
        let is_unpublish = |c: &fe_sync::SyncCommand| matches!(c, fe_sync::SyncCommand::UnpublishTileset { tileset_id } if tileset_id == hexon_id);
        let click = |app: &mut App, enabled: bool| {
            app.world_mut()
                .resource_mut::<PendingHexonOps>()
                .0
                .push(HexonOp::SetSeeding(hexon_id.to_string(), enabled));
        };
        let wait = std::time::Duration::from_secs(10);

        click(&mut app, true);
        let seen = pump_sync_commands(&mut app, &cmd_rx, wait, |s| s.iter().any(is_publish));
        assert!(
            seen.iter().any(is_publish),
            "a persisted enable publishes: {seen:?}"
        );

        click(&mut app, false);
        let seen = pump_sync_commands(&mut app, &cmd_rx, wait, |s| s.iter().any(is_unpublish));
        assert!(
            seen.iter().any(is_unpublish),
            "a disable unpublishes: {seen:?}"
        );

        std::fs::write(&registry_json, b"{ not json").expect("corrupt registry.json");
        click(&mut app, true);
        let seen = pump_sync_commands(
            &mut app,
            &cmd_rx,
            std::time::Duration::from_millis(1500),
            |s| s.iter().any(is_publish),
        );
        assert!(
            !seen.iter().any(is_publish),
            "an enable that never persisted must not publish: {seen:?}"
        );
    }

    /// (plan S7b) `install_from_registry_installs_a_sealed_container_end_to_end`:
    /// a real in-process registry serves a sealed, THREE-chunk container
    /// (same fixture shape as `fe-terrain`'s
    /// `container_backed_source_serves_tiles_from_every_chunk`, just sealed);
    /// queuing `HexonOp::InstallFromRegistry` and driving it through
    /// `drain_hexon_ops` fetches the header, fetches every chunk (reporting
    /// progress 0/3 → 3/3), reassembles the sealed container and installs it
    /// — proven by reading a real tile back through the registry afterward,
    /// not just checking the result enum.
    #[test]
    fn install_from_registry_installs_a_sealed_container_end_to_end() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[91u8; 32]);
        let hexon_id = "s7b-container-test";

        let mut chunks = Vec::new();
        for seq in 0..3u32 {
            let tile_bytes = vec![(100 + seq) as u8; 16];
            let chunk_bytes = fe_format::HexonArchive::export_tileset(
                synthetic_tileset_manifest(hexon_id),
                synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
                &[(format!("10/{seq}/0"), tile_bytes)],
                &[],
                None,
            )
            .expect("export chunk archive");
            chunks.push(fe_format::ChunkInput {
                seq,
                tile_count: 1,
                zoom: 10,
                x_range: [seq, seq],
                y_range: [0, 0],
                hilbert_range: [seq as u64, seq as u64],
                archive_bytes: chunk_bytes,
            });
        }
        let mut outer_meta = synthetic_tileset(hexon_id).tileset_meta.unwrap();
        outer_meta.tile_count = 3;
        let unsealed = fe_format::HexonArchive::export_chunked(
            synthetic_tileset_manifest(hexon_id),
            &outer_meta,
            &chunks,
        )
        .expect("export container");
        let sealed = fe_format::seal(&unsealed, &key).expect("seal container");

        let registry_dir = tempfile::tempdir().expect("registry tempdir");
        std::fs::write(registry_dir.path().join("pkg.hexon"), &sealed).expect("write fixture");
        let addr = spawn_test_registry(registry_dir.path(), None);

        let (tmp, tileset_registry) = fresh_tileset_registry();
        let tileset_registry = std::sync::Arc::new(tileset_registry);
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut app =
            app_for_install_from_registry(tileset_registry.clone(), format!("http://{addr}"), tx);
        app.world_mut()
            .resource_mut::<PendingHexonOps>()
            .0
            .push(HexonOp::InstallFromRegistry {
                uri: hexon_id.to_string(),
            });
        app.update(); // drains the op, spawning the background fetch+install thread

        let outcomes = collect_registry_outcomes(&rx, std::time::Duration::from_secs(20));
        let progress: Vec<(u32, u32)> = outcomes
            .iter()
            .filter_map(|o| match o {
                HexonAssembleOutcome::RegistryProgress {
                    chunks_received,
                    total_chunks,
                    ..
                } => Some((*chunks_received, *total_chunks)),
                _ => None,
            })
            .collect();
        assert_eq!(
            progress,
            vec![(0, 3), (1, 3), (2, 3), (3, 3)],
            "progress must run header (0/3) through every chunk in order: {outcomes:?}"
        );
        match outcomes.last() {
            Some(HexonAssembleOutcome::RegistryInstalled {
                uri,
                hexon_id: installed_id,
            }) => {
                assert_eq!(uri, hexon_id);
                assert_eq!(installed_id, hexon_id);
            }
            other => panic!("expected RegistryInstalled, got {other:?}: {outcomes:?}"),
        }

        // The real, observable outcome: every chunk's tile reads back
        // through the registry exactly as written, through the SAME
        // `TilesetRegistry` `drain_hexon_ops` installed into (not a
        // freshly-reloaded one) — proves `registry.reload` ran.
        for seq in 0..3u32 {
            assert_eq!(
                tileset_registry.get_tile(hexon_id, 10, seq, 0).as_deref(),
                Some(vec![(100 + seq) as u8; 16].as_slice()),
                "chunk {seq}'s tile must read back byte-identical"
            );
        }
        assert!(
            tileset_registry
                .list_tilesets()
                .iter()
                .any(|t| t.tileset_id == hexon_id),
            "installed via the real store, not faked"
        );
        drop(tmp);
    }

    /// S8 (partial-fetch size cap): a sealed container whose chunks together
    /// exceed the caller's byte budget is abandoned mid-fetch and installs
    /// nothing; the same container installs under a budget that fits it.
    #[test]
    fn install_from_registry_stops_a_container_past_its_byte_budget() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[93u8; 32]);
        let hexon_id = "s8-budget-test";
        let chunks: Vec<fe_format::ChunkInput> = (0..3u32)
            .map(|seq| fe_format::ChunkInput {
                seq,
                tile_count: 1,
                zoom: 10,
                x_range: [seq, seq],
                y_range: [0, 0],
                hilbert_range: [u64::from(seq), u64::from(seq)],
                archive_bytes: fe_format::HexonArchive::export_tileset(
                    synthetic_tileset_manifest(hexon_id),
                    synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
                    &[(format!("10/{seq}/0"), vec![seq as u8; 4096])],
                    &[],
                    None,
                )
                .expect("export chunk archive"),
            })
            .collect();
        let all_chunks: u64 = chunks.iter().map(|c| c.archive_bytes.len() as u64).sum();
        let mut outer_meta = synthetic_tileset(hexon_id).tileset_meta.unwrap();
        outer_meta.tile_count = 3;
        let sealed = fe_format::seal(
            &fe_format::HexonArchive::export_chunked(
                synthetic_tileset_manifest(hexon_id),
                &outer_meta,
                &chunks,
            )
            .expect("export container"),
            &key,
        )
        .expect("seal container");
        let registry_dir = tempfile::tempdir().expect("registry tempdir");
        std::fs::write(registry_dir.path().join("pkg.hexon"), &sealed).expect("write fixture");
        let addr = spawn_test_registry(registry_dir.path(), None);

        let (_tmp, tileset_registry) = fresh_tileset_registry();
        let client: std::sync::Arc<dyn fe_hexon::partial::PartialHexonFetch> = std::sync::Arc::new(
            fe_hexon::remote::RemoteRegistryClient::new(format!("http://{addr}"), None),
        );
        let strategy = fe_hexon::partial::FetchStrategy::new(fe_hexon::p2p::P2pConfig::default());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let mut progress = Vec::new();
        let err = rt
            .block_on(fetch_and_install_from_registry(
                &strategy,
                client.clone(),
                tileset_registry.store(),
                hexon_id,
                all_chunks - 1,
                |received, total| progress.push((received, total)),
            ))
            .expect_err("three chunks cannot fit a budget one byte short of them");
        assert!(err.to_string().contains("exceeds"), "{err:#}");
        assert_eq!(progress, vec![(0, 3), (1, 3), (2, 3)], "stopped at chunk 2");
        assert!(tileset_registry.store().list_installed().is_empty());

        rt.block_on(fetch_and_install_from_registry(
            &strategy,
            client,
            tileset_registry.store(),
            hexon_id,
            all_chunks,
            |_, _| {},
        ))
        .expect("the same container fits a budget of exactly its chunks");
        assert_eq!(tileset_registry.store().list_installed().len(), 1);
    }

    /// W2b F4: a table test for the pure resolver — no process-env mutation
    /// (env vars are process-global and race across parallel test threads;
    /// this function takes the env value as a parameter instead, same shape
    /// as `fe_ui::settings::resolve_hexon_registry_url`'s env half).
    #[test]
    fn resolve_hexon_registry_token_table() {
        let cases: &[(Option<&str>, Option<&str>)] = &[
            (None, None),
            (Some(""), None),
            (Some("   "), None),
            (Some("s3cr3t"), Some("s3cr3t")),
            (Some("  s3cr3t  "), Some("s3cr3t")),
        ];
        for (env, want) in cases {
            assert_eq!(
                resolve_hexon_registry_token(*env).as_deref(),
                *want,
                "env={env:?}"
            );
        }
    }

    /// W2b F4: `FE_HEXON_REGISTRY_TOKEN` is resolved at the `drain_hexon_ops`
    /// seam and threaded through to `RemoteRegistryClient`, never read inside
    /// the pure fetch core itself — proven by driving
    /// `fetch_and_install_from_registry` directly against a token-gated
    /// in-process registry (`RegistryState`'s own `token` field, not the
    /// `FE_REGISTRY_TOKEN` env var — no process-env mutation needed here
    /// either): the right token installs; no token (or the wrong one) is
    /// refused and installs nothing.
    #[test]
    fn install_from_registry_requires_the_configured_bearer_token() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[94u8; 32]);
        let hexon_id = "w2b-f4-token-test";
        let sealed = fe_format::seal(
            &fe_format::HexonArchive::export_tileset(
                synthetic_tileset_manifest(hexon_id),
                synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
                &[("10/0/0".to_string(), vec![9u8; 8])],
                &[],
                None,
            )
            .expect("export unsealed archive"),
            &key,
        )
        .expect("seal archive");

        let registry_dir = tempfile::tempdir().expect("registry tempdir");
        std::fs::write(registry_dir.path().join("pkg.hexon"), &sealed).expect("write fixture");
        let addr = spawn_test_registry(registry_dir.path(), Some("s3cr3t".to_string()));
        let registry_url = format!("http://{addr}");

        let (_tmp, tileset_registry) = fresh_tileset_registry();
        let strategy = fe_hexon::partial::FetchStrategy::new(fe_hexon::p2p::P2pConfig::default());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let client_for =
            |token: Option<String>| -> std::sync::Arc<dyn fe_hexon::partial::PartialHexonFetch> {
                std::sync::Arc::new(fe_hexon::remote::RemoteRegistryClient::new(
                    registry_url.clone(),
                    token,
                ))
            };

        // No token at all: refused, nothing installed.
        let err = rt
            .block_on(fetch_and_install_from_registry(
                &strategy,
                client_for(None),
                tileset_registry.store(),
                hexon_id,
                fe_format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES,
                |_, _| {},
            ))
            .expect_err("a missing bearer token must be refused by a token-gated registry");
        assert!(err.to_string().contains("refused"), "{err:#}");
        assert!(tileset_registry.store().list_installed().is_empty());

        // The wrong token: also refused.
        let err = rt
            .block_on(fetch_and_install_from_registry(
                &strategy,
                client_for(resolve_hexon_registry_token(Some("not-it"))),
                tileset_registry.store(),
                hexon_id,
                fe_format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES,
                |_, _| {},
            ))
            .expect_err("the wrong bearer token must be refused");
        assert!(err.to_string().contains("refused"), "{err:#}");
        assert!(tileset_registry.store().list_installed().is_empty());

        // The configured token installs.
        rt.block_on(fetch_and_install_from_registry(
            &strategy,
            client_for(resolve_hexon_registry_token(Some("s3cr3t"))),
            tileset_registry.store(),
            hexon_id,
            fe_format::limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES,
            |_, _| {},
        ))
        .expect("the configured bearer token must install");
        assert_eq!(tileset_registry.store().list_installed().len(), 1);
    }

    /// W3b fixup (AC-30a): the test above drives `fetch_and_install_from_registry`
    /// directly, bypassing `install_from_registry` — the function that
    /// actually owns `RemoteRegistryClient::new(registry_url.to_string(),
    /// token)`. Reverting that call to a hardcoded `None` token left the test
    /// above green, because it never calls `install_from_registry` at all.
    /// This test drives the REAL seam instead: `install_from_registry`
    /// itself, the one `drain_hexon_ops`'s `InstallFromRegistry` arm calls in
    /// production. No env mutation — the token is passed as a plain
    /// parameter, same as every other test here.
    #[test]
    fn install_from_registry_threads_the_bearer_token_to_the_client() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[97u8; 32]);
        let hexon_id = "w3b-f4-seam-test";
        let sealed = fe_format::seal(
            &fe_format::HexonArchive::export_tileset(
                synthetic_tileset_manifest(hexon_id),
                synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
                &[("10/0/0".to_string(), vec![7u8; 8])],
                &[],
                None,
            )
            .expect("export unsealed archive"),
            &key,
        )
        .expect("seal archive");

        let registry_dir = tempfile::tempdir().expect("registry tempdir");
        std::fs::write(registry_dir.path().join("pkg.hexon"), &sealed).expect("write fixture");
        let addr = spawn_test_registry(registry_dir.path(), Some("s3cr3t".to_string()));
        let registry_url = format!("http://{addr}");

        // The configured token installs for real, through `install_from_registry`.
        // Mutation check: if `:541`'s `RemoteRegistryClient::new(..., token)`
        // is reverted to a hardcoded `None`, THIS call goes red (the
        // token-gated registry refuses an anonymous request).
        let (_tmp, tileset_registry) = fresh_tileset_registry();
        let (tx, rx) = crossbeam::channel::unbounded();
        install_from_registry(
            &tileset_registry,
            &registry_url,
            hexon_id,
            Some("s3cr3t".to_string()),
            &tx,
        );
        match collect_registry_outcomes(&rx, std::time::Duration::from_secs(20)).last() {
            Some(HexonAssembleOutcome::RegistryInstalled {
                hexon_id: installed,
                ..
            }) => {
                assert_eq!(installed, hexon_id);
            }
            other => panic!("expected RegistryInstalled with the configured token, got {other:?}"),
        }
        assert_eq!(tileset_registry.store().list_installed().len(), 1);

        // No token against the SAME token-gated registry: refused, nothing
        // installed — proves the seam actually threads what it's given, both
        // ways, not just "any token always works".
        let (_tmp2, tileset_registry2) = fresh_tileset_registry();
        let (tx2, rx2) = crossbeam::channel::unbounded();
        install_from_registry(&tileset_registry2, &registry_url, hexon_id, None, &tx2);
        match collect_registry_outcomes(&rx2, std::time::Duration::from_secs(20)).last() {
            Some(HexonAssembleOutcome::RegistryFailed { .. }) => {}
            other => panic!("expected RegistryFailed with no token, got {other:?}"),
        }
        assert!(tileset_registry2.store().list_installed().is_empty());
    }

    /// (plan S7b) `install_from_registry_refuses_a_tampered_source_and_installs_nothing`:
    /// the registry serves a SEALED monolithic archive whose header verifies
    /// cleanly, but whose tile bytes were tampered with after sealing (same
    /// attack shape as fe-hexon's own
    /// `strategy_falls_back_from_a_tampering_source_to_an_honest_one` fixture)
    /// — `FetchStrategy::fetch_chunk`'s whole-archive check must refuse it,
    /// and nothing may land in the store.
    #[test]
    fn install_from_registry_refuses_a_tampered_source_and_installs_nothing() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[92u8; 32]);
        let hexon_id = "s7b-tampered-test";
        let tile_bytes: Vec<u8> = vec![0xAB, 0xCD, 0xEF, 0x12, 0x34, 0x56, 0x78, 0x9A];
        let unsealed = fe_format::HexonArchive::export_tileset(
            synthetic_tileset_manifest(hexon_id),
            synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
            &[("10/0/0".to_string(), tile_bytes.clone())],
            &[],
            None,
        )
        .expect("export unsealed archive");
        let sealed = fe_format::seal(&unsealed, &key).expect("seal archive");

        // Flip the tile's bytes in place, post-seal, without touching
        // manifest.json or any header member — the header still verifies;
        // only the tile content itself (and so the whole-archive check) is
        // wrong. Stored (uncompressed) tiles put these bytes verbatim in the
        // zip, so a literal byte search finds them.
        let mut tampered = sealed.clone();
        let pos = tampered
            .windows(tile_bytes.len())
            .position(|w| w == tile_bytes.as_slice())
            .expect("tile bytes must appear verbatim in a Stored zip entry");
        tampered[pos] ^= 0xFF;
        assert_ne!(tampered, sealed, "tamper must actually change the bytes");

        let registry_dir = tempfile::tempdir().expect("registry tempdir");
        std::fs::write(registry_dir.path().join("pkg.hexon"), &tampered).expect("write fixture");
        let addr = spawn_test_registry(registry_dir.path(), None);

        let (_tmp, tileset_registry) = fresh_tileset_registry();
        let tileset_registry = std::sync::Arc::new(tileset_registry);
        let (tx, rx) = crossbeam::channel::unbounded();
        let mut app =
            app_for_install_from_registry(tileset_registry.clone(), format!("http://{addr}"), tx);
        app.world_mut()
            .resource_mut::<PendingHexonOps>()
            .0
            .push(HexonOp::InstallFromRegistry {
                uri: hexon_id.to_string(),
            });
        app.update();

        let outcomes = collect_registry_outcomes(&rx, std::time::Duration::from_secs(20));
        match outcomes.last() {
            Some(HexonAssembleOutcome::RegistryFailed { uri, .. }) => {
                assert_eq!(uri, hexon_id);
            }
            other => panic!("expected RegistryFailed, got {other:?}: {outcomes:?}"),
        }
        assert!(
            tileset_registry.list_tilesets().is_empty(),
            "a tampered source must install nothing: {:?}",
            tileset_registry.list_tilesets()
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
            .init_resource::<KnownTilesetPublishers>()
            .init_resource::<InstallMetaVerdicts>()
            .init_resource::<StreamedMetaVerdicts>()
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
            .init_resource::<KnownTilesetPublishers>()
            .init_resource::<InstallMetaVerdicts>()
            .init_resource::<StreamedMetaVerdicts>()
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
            .insert((tileset_id.to_string(), 0), std::time::Instant::now());
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
                .contains_key(&(tileset_id.to_string(), 0)),
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
            .init_resource::<KnownTilesetPublishers>()
            .init_resource::<InstallMetaVerdicts>()
            .init_resource::<StreamedMetaVerdicts>()
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

    /// W1 residual: a meta or bundle request nobody ever answers frees its
    /// in-flight slot after `STREAMED_REQUEST_TIMEOUT` (the clock is the one
    /// faked input) and is retried through the ordinary backoff, never pinned.
    #[test]
    fn a_never_answered_request_frees_its_slot_after_the_timeout_and_retries() {
        let tileset_id = "silent-tileset";
        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.mark_streamed_pending(tileset_id);
        let (mut app, cmd_rx) = streamed_app(Some(composite));
        app.world_mut()
            .resource_mut::<fe_sync::StreamedTilesetEventBuffer>()
            .events
            .push(fe_sync::SyncEvent::PeerTilesetAdvertisement {
                peer_id: "peer".into(),
                advertisements_json: format!(r#"[{{"tileset_id":"{tileset_id}"}}]"#),
            });
        app.world_mut()
            .resource_mut::<fe_terrain::tiles::TileBundleDemand>()
            .demand(tileset_id, 0);
        let counts = |rx: &crossbeam::channel::Receiver<fe_sync::SyncCommand>| {
            rx.try_iter().fold((0, 0), |(meta, chunk), cmd| match cmd {
                fe_sync::SyncCommand::RequestTilesetMeta { .. } => (meta + 1, chunk),
                fe_sync::SyncCommand::RequestChunk { .. } => (meta, chunk + 1),
                _ => (meta, chunk),
            })
        };
        app.update();
        assert_eq!(counts(&cmd_rx), (1, 1), "both requests go out");
        app.update();
        assert_eq!(
            counts(&cmd_rx),
            (0, 0),
            "unanswered requests hold their slots"
        );

        let key = (tileset_id.to_string(), 0);
        let later = std::time::Instant::now() + STREAMED_REQUEST_TIMEOUT;
        {
            let mut state = app.world_mut().resource_mut::<StreamedTilesetState>();
            assert_eq!(expire_stalled_requests(&mut state, later), 2);
            assert!(state.meta_requested.is_empty() && state.chunk_requested.is_empty());
            assert_eq!(state.meta_backoff.get(tileset_id).map(|b| b.0), Some(1));
            assert_eq!(state.chunk_backoff.get(&key).map(|b| b.0), Some(1));
        }
        app.update();
        assert_eq!(
            counts(&cmd_rx),
            (0, 0),
            "the freed slots wait out their backoff"
        );

        {
            let mut state = app.world_mut().resource_mut::<StreamedTilesetState>();
            let now = std::time::Instant::now();
            state.meta_backoff.get_mut(tileset_id).unwrap().1 = now;
            state.chunk_backoff.get_mut(&key).unwrap().1 = now;
        }
        app.update();
        assert_eq!(
            counts(&cmd_rx),
            (1, 1),
            "both are retried after the backoff"
        );
    }

    /// Final review: the streamed path shares the meta feed with the install
    /// flow. A meta reply it never asked for must not add a streamed source;
    /// the reply to its own request (after an advertisement) must — off the
    /// main thread (W2b F1), so this polls a few frames rather than asserting
    /// resolution after exactly one `app.update()`.
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
            ad_publisher_did: String::new(),
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while pending(&app) && std::time::Instant::now() < deadline {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!pending(&app), "our own reply resolves the streamed source");
    }

    /// Drive one streamed meta reply through `drive_streamed_tilesets` the way
    /// production does — an advertisement, OUR `RequestTilesetMeta`, then the
    /// reply (an unsolicited reply is dropped before any verification, so a
    /// test that skips the request proves nothing). Asserts the system
    /// returns before the worker thread's verdict and does not re-request the
    /// uri while it verifies, then polls until the verdict is applied
    /// (resolved, or refused into `meta_backoff`). Returns whether the uri is
    /// still pending, i.e. whether the reply was refused.
    fn drive_one_streamed_meta_reply(
        app: &mut App,
        cmd_rx: &crossbeam::channel::Receiver<fe_sync::SyncCommand>,
        tileset_id: &str,
        meta_json: String,
    ) -> bool {
        let meta_requests = |rx: &crossbeam::channel::Receiver<fe_sync::SyncCommand>| {
            rx.try_iter()
                .filter(|cmd| matches!(cmd, fe_sync::SyncCommand::RequestTilesetMeta { .. }))
                .count()
        };
        let pending = |app: &App| {
            app.world()
                .resource::<ActiveTileSource>()
                .composite
                .as_ref()
                .expect("composite")
                .pending_streamed_uris()
                .any(|id| id == tileset_id)
        };
        let refused = |app: &App| {
            app.world()
                .resource::<StreamedTilesetState>()
                .meta_backoff
                .contains_key(tileset_id)
        };
        let buffer = |app: &mut App, event| {
            app.world_mut()
                .resource_mut::<fe_sync::StreamedTilesetEventBuffer>()
                .events
                .push(event)
        };

        buffer(
            app,
            fe_sync::SyncEvent::PeerTilesetAdvertisement {
                peer_id: "peer".into(),
                advertisements_json: format!(r#"[{{"tileset_id":"{tileset_id}"}}]"#),
            },
        );
        app.update();
        assert_eq!(
            meta_requests(cmd_rx),
            1,
            "the advertised uri is requested once"
        );

        buffer(
            app,
            fe_sync::SyncEvent::TilesetMetaReceived {
                peer_id: String::new(),
                tileset_id: tileset_id.to_string(),
                meta_json,
                total_chunks: 1,
                approx_size_bytes: 0,
                root_hash: "ab".repeat(32),
                chunk_hashes: Vec::new(),
                index_path: std::path::PathBuf::new(),
                ad_publisher_did: String::new(),
            },
        );
        app.update();
        assert!(
            pending(app) && !refused(app),
            "the drive system must return before the worker thread's verdict lands"
        );
        assert_eq!(
            meta_requests(cmd_rx),
            0,
            "a uri whose reply is being verified off-thread is not re-requested"
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while pending(app) && !refused(app) && std::time::Instant::now() < deadline {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            !pending(app) || refused(app),
            "the worker thread's verdict must be applied"
        );
        pending(app)
    }

    /// W2b F1, through the real system (not just the unit-level
    /// `from_header_doc` check in fe-terrain): once a publisher is pinned for
    /// a tileset_id (TOFU via `KnownTilesetPublishers`, the same pin chain the
    /// install flow uses), a validly-signed header for the SAME tileset_id
    /// from a DIFFERENT publisher is refused off the main thread and never
    /// resolves into a streamed source.
    #[test]
    fn a_streamed_header_signed_by_a_publisher_other_than_the_known_pin_is_refused() {
        let owner = ed25519_dalek::SigningKey::from_bytes(&[81u8; 32]);
        let impostor = ed25519_dalek::SigningKey::from_bytes(&[82u8; 32]);
        let owner_did = fe_format::verifying_key_to_did(&owner.verifying_key());
        let tileset_id = "s7-pinned-tileset";

        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.mark_streamed_pending(tileset_id);
        let (mut app, cmd_rx) = streamed_app(Some(composite));
        app.world_mut()
            .resource_mut::<KnownTilesetPublishers>()
            .0
            .insert(tileset_id.to_string(), owner_did);

        let still_pending = drive_one_streamed_meta_reply(
            &mut app,
            &cmd_rx,
            tileset_id,
            sealed_monolithic_header_json(&impostor, tileset_id),
        );
        assert!(
            still_pending,
            "an impostor's header must never resolve into a streamed source"
        );
    }

    /// W2b review: the INSTALLED publisher (first in the pin chain, resolved
    /// on the worker thread because it opens the store's archive) pins the
    /// streamed path even with no TOFU entry — an impostor's header for an
    /// installed tileset_id is refused, the installed publisher's own header
    /// resolves.
    #[test]
    fn a_streamed_header_is_pinned_to_the_installed_publisher() {
        let owner = ed25519_dalek::SigningKey::from_bytes(&[85u8; 32]);
        let impostor = ed25519_dalek::SigningKey::from_bytes(&[86u8; 32]);
        let tileset_id = "s7-installed-pin";
        let installed = fe_format::seal(
            &fe_format::HexonArchive::export_tileset(
                synthetic_tileset_manifest(tileset_id),
                synthetic_tileset(tileset_id).tileset_meta.as_ref().unwrap(),
                &[("10/0/0".to_string(), vec![3u8; 8])],
                &[],
                None,
            )
            .expect("export unsealed archive"),
            &owner,
        )
        .expect("seal archive");
        let (_store_tmp, tileset_registry) = fresh_tileset_registry();
        tileset_registry
            .store()
            .install_tileset(&installed)
            .expect("install the owner's archive");
        let tileset_registry = std::sync::Arc::new(tileset_registry);

        let run = |key: &ed25519_dalek::SigningKey| {
            let mut composite = fe_terrain::tiles::CompositeTileSource::new();
            composite.mark_streamed_pending(tileset_id);
            let (mut app, cmd_rx) = streamed_app(Some(composite));
            app.insert_resource(SharedTilesetRegistry(tileset_registry.clone()));
            drive_one_streamed_meta_reply(
                &mut app,
                &cmd_rx,
                tileset_id,
                sealed_monolithic_header_json(key, tileset_id),
            )
        };
        assert!(
            run(&impostor),
            "an impostor's header must be refused against the installed publisher"
        );
        assert!(
            !run(&owner),
            "the installed publisher's own header resolves into a streamed source"
        );
    }

    /// W2b F1, through the real system: once a sealed publisher is pinned for
    /// a tileset_id, an unsealed (legacy plain-index) meta reply for the SAME
    /// id is refused off the main thread and never resolves.
    #[test]
    fn an_unsealed_streamed_meta_is_refused_once_a_sealed_publisher_is_known() {
        let owner = ed25519_dalek::SigningKey::from_bytes(&[84u8; 32]);
        let owner_did = fe_format::verifying_key_to_did(&owner.verifying_key());
        let tileset_id = "s7-unsealed-vs-pin";
        let bundles = fe_terrain::tiles::bundle_tileset(
            &synthetic_tileset(tileset_id),
            fe_terrain::tiles::DEFAULT_BUNDLE_BYTES,
        )
        .expect("bundle_tileset");
        let unsealed_meta_json = String::from_utf8(bundles.index_json.clone()).expect("utf8 index");

        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.mark_streamed_pending(tileset_id);
        let (mut app, cmd_rx) = streamed_app(Some(composite));
        app.world_mut()
            .resource_mut::<KnownTilesetPublishers>()
            .0
            .insert(tileset_id.to_string(), owner_did);

        let still_pending =
            drive_one_streamed_meta_reply(&mut app, &cmd_rx, tileset_id, unsealed_meta_json);
        assert!(
            still_pending,
            "an unsealed reply must never resolve into a streamed source once a \
             sealed publisher pin is known"
        );
    }

    /// RP-4b: the active petal's authored pin for `tileset_id`.
    fn petal_pinning(
        tileset_id: &str,
        publisher_did: &str,
    ) -> fe_terrain::petal_binding::ActivePetalTerrain {
        fe_terrain::petal_binding::ActivePetalTerrain {
            petal_id: Some("petal-pinned".into()),
            config: Some(fe_terrain::config::TerrainConfig {
                enabled: true,
                tileset_hexon_uris: vec![tileset_id.to_string()],
                tileset_publisher_dids: [(tileset_id.to_string(), publisher_did.to_string())]
                    .into(),
                ..Default::default()
            }),
            revision: 1,
        }
    }

    /// Re-mark `tileset_id` pending, as a petal re-assignment does.
    fn re_pend(app: &mut App, tileset_id: &str) {
        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.mark_streamed_pending(tileset_id);
        app.world_mut().resource_mut::<ActiveTileSource>().composite = Some(composite);
    }

    /// RP-4b (critic tick 5 acceptance test, T24): a never-installed tileset
    /// streamed for a petal whose Editor-authored terrain pins publisher A
    /// refuses a validly-signed header from publisher B — with nothing
    /// installed and no TOFU entry — and resolves A's own header.
    #[test]
    fn a_streamed_tileset_signed_by_a_publisher_other_than_the_petal_maps_pin_is_refused() {
        let owner = ed25519_dalek::SigningKey::from_bytes(&[91u8; 32]);
        let impostor = ed25519_dalek::SigningKey::from_bytes(&[92u8; 32]);
        let owner_did = fe_format::verifying_key_to_did(&owner.verifying_key());
        let tileset_id = "rp4b-petal-pinned";

        let run = |key: &ed25519_dalek::SigningKey| {
            let mut composite = fe_terrain::tiles::CompositeTileSource::new();
            composite.mark_streamed_pending(tileset_id);
            let (mut app, cmd_rx) = streamed_app(Some(composite));
            app.insert_resource(petal_pinning(tileset_id, &owner_did));
            let still_pending = drive_one_streamed_meta_reply(
                &mut app,
                &cmd_rx,
                tileset_id,
                sealed_monolithic_header_json(key, tileset_id),
            );
            let recorded = app
                .world()
                .resource::<KnownTilesetPublishers>()
                .0
                .get(tileset_id)
                .cloned();
            (still_pending, recorded)
        };
        let (impostor_pending, impostor_recorded) = run(&impostor);
        assert!(
            impostor_pending,
            "a header from a publisher other than the petal's pin must never resolve"
        );
        assert_eq!(impostor_recorded, None, "a refused header pins nothing");
        let (owner_pending, owner_recorded) = run(&owner);
        assert!(
            !owner_pending,
            "the petal-pinned publisher's header resolves"
        );
        // OF-5: petal pins apply at resolve time only and are never recorded into the
        // node-global TOFU map (another petal's Editor could otherwise poison it).
        assert_eq!(
            owner_recorded, None,
            "a petal-authored pin is never recorded"
        );
    }

    /// RP-4b step 3 (T25): with nothing installed or authored, the first
    /// streamed header that verifies becomes this node's first-contact TOFU
    /// pin, so a later header for the same id from a different publisher in
    /// the same session is refused.
    #[test]
    fn a_second_streamed_publisher_for_the_same_id_is_refused_in_session() {
        let first = ed25519_dalek::SigningKey::from_bytes(&[93u8; 32]);
        let second = ed25519_dalek::SigningKey::from_bytes(&[94u8; 32]);
        let first_did = fe_format::verifying_key_to_did(&first.verifying_key());
        let tileset_id = "rp4b-first-contact";

        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.mark_streamed_pending(tileset_id);
        let (mut app, cmd_rx) = streamed_app(Some(composite));
        assert!(
            !drive_one_streamed_meta_reply(
                &mut app,
                &cmd_rx,
                tileset_id,
                sealed_monolithic_header_json(&first, tileset_id),
            ),
            "first contact resolves"
        );
        assert_eq!(
            app.world()
                .resource::<KnownTilesetPublishers>()
                .0
                .get(tileset_id),
            Some(&first_did),
            "the streamed path records its first-contact pin"
        );

        re_pend(&mut app, tileset_id);
        assert!(
            drive_one_streamed_meta_reply(
                &mut app,
                &cmd_rx,
                tileset_id,
                sealed_monolithic_header_json(&second, tileset_id),
            ),
            "a second publisher's header for the same id must be refused"
        );
        assert_eq!(
            app.world()
                .resource::<KnownTilesetPublishers>()
                .0
                .get(tileset_id),
            Some(&first_did),
            "a refused header never moves the pin"
        );
    }

    /// RP-4b (AC-47 precedence, OF-5 semantics): a petal-authored pin OVERRIDES
    /// an earlier first-contact TOFU pin at resolve time — the petal's publisher
    /// resolves even though TOFU recorded someone else, and the old TOFU
    /// publisher cannot override it while the petal pin is active — but the
    /// recorded TOFU entry itself is left untouched (petal pins are never
    /// recorded).
    #[test]
    fn a_petal_authored_pin_overrides_an_earlier_first_contact_tofu_pin() {
        let tofu = ed25519_dalek::SigningKey::from_bytes(&[95u8; 32]);
        let tofu_did = fe_format::verifying_key_to_did(&tofu.verifying_key());
        let authored = ed25519_dalek::SigningKey::from_bytes(&[96u8; 32]);
        let authored_did = fe_format::verifying_key_to_did(&authored.verifying_key());
        let tileset_id = "rp4b-precedence";

        let mut composite = fe_terrain::tiles::CompositeTileSource::new();
        composite.mark_streamed_pending(tileset_id);
        let (mut app, cmd_rx) = streamed_app(Some(composite));
        assert!(
            !drive_one_streamed_meta_reply(
                &mut app,
                &cmd_rx,
                tileset_id,
                sealed_monolithic_header_json(&tofu, tileset_id),
            ),
            "first contact resolves and records a TOFU pin"
        );

        // An Editor now authors the petal's pin for a different publisher.
        app.insert_resource(petal_pinning(tileset_id, &authored_did));
        re_pend(&mut app, tileset_id);
        assert!(
            !drive_one_streamed_meta_reply(
                &mut app,
                &cmd_rx,
                tileset_id,
                sealed_monolithic_header_json(&authored, tileset_id),
            ),
            "the petal-authored publisher resolves over the earlier TOFU pin"
        );
        assert_eq!(
            app.world()
                .resource::<KnownTilesetPublishers>()
                .0
                .get(tileset_id),
            Some(&tofu_did),
            "OF-5: the petal-authored pin decides resolution but never rewrites the TOFU map"
        );

        re_pend(&mut app, tileset_id);
        assert!(
            drive_one_streamed_meta_reply(
                &mut app,
                &cmd_rx,
                tileset_id,
                sealed_monolithic_header_json(&tofu, tileset_id),
            ),
            "the old TOFU publisher never overrides the petal-authored pin"
        );
    }

    /// RP-4b pin chain + apply-time recheck, pure (real branching): the
    /// precedence table and the T25 race guard.
    #[test]
    fn the_publisher_pin_chain_prefers_installed_then_petal_then_tofu() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(
            resolve_publisher_pin(s("i"), s("p"), s("t")),
            (s("i"), PinSource::Installed)
        );
        assert_eq!(
            resolve_publisher_pin(None, s("p"), s("t")),
            (s("p"), PinSource::PetalAuthored)
        );
        assert_eq!(
            resolve_publisher_pin(None, None, s("t")),
            (s("t"), PinSource::Tofu)
        );
        assert_eq!(
            resolve_publisher_pin(None, None, None),
            (None, PinSource::FirstContact)
        );

        for (source, publisher, petal, known, trusted) in [
            (PinSource::Installed, Some("i"), Some("p"), Some("t"), true),
            (PinSource::FirstContact, Some("b"), None, Some("a"), false),
            (PinSource::FirstContact, Some("a"), None, None, true),
            (PinSource::Tofu, Some("t"), Some("p"), Some("t"), false),
            (
                PinSource::PetalAuthored,
                Some("p"),
                Some("p"),
                Some("t"),
                true,
            ),
            (PinSource::FirstContact, None, None, Some("a"), false),
        ] {
            assert_eq!(
                streamed_verdict_still_trusted(source, publisher, petal, known),
                trusted,
                "{source:?} {publisher:?} petal={petal:?} known={known:?}"
            );
        }
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

    // -------------------------------------------------------------------------
    // PAS-P5 S7a: install-flow header verification (`verify_install_meta`,
    // `verify_install_meta_and_confirm`) — see src/AGENTS.md §tileset-p2p.
    // -------------------------------------------------------------------------

    /// A minimal sealed MONOLITHIC header's JSON (HashSeq child 0 shape),
    /// signed by `key`.
    fn sealed_monolithic_header_json(key: &ed25519_dalek::SigningKey, hexon_id: &str) -> String {
        let unsealed = fe_format::HexonArchive::export_tileset(
            synthetic_tileset_manifest(hexon_id),
            synthetic_tileset(hexon_id).tileset_meta.as_ref().unwrap(),
            &[("10/0/0".to_string(), vec![1u8, 2, 3, 4, 5, 6, 7, 8])],
            &[],
            None,
        )
        .expect("export unsealed archive");
        let sealed = fe_format::seal(&unsealed, key).expect("seal archive");
        header_doc_json(&sealed)
    }

    /// `HeaderDoc` JSON (HashSeq child 0 shape) for whatever archive bytes
    /// are handed in, sealed or not — the shape `TilesetMetaReceived.meta_json`
    /// carries over the wire.
    fn header_doc_json(archive_bytes: &[u8]) -> String {
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        std::fs::write(tmp.path(), archive_bytes).expect("write archive");
        let index = fe_format::HexonArchiveIndex::open(tmp.path()).expect("open archive index");
        String::from_utf8(index.header_doc().to_json()).expect("HeaderDoc JSON is UTF-8")
    }

    /// A sealed CONTAINER's header JSON plus the exact signed per-chunk
    /// blake3 hashes its `chunks/index.json` names, in seq order.
    fn sealed_container_header_and_hashes(
        key: &ed25519_dalek::SigningKey,
        hexon_id: &str,
    ) -> (String, Vec<[u8; 32]>) {
        let tileset_meta = synthetic_tileset(hexon_id).tileset_meta.unwrap();
        let chunks = vec![
            fe_format::ChunkInput {
                seq: 0,
                tile_count: 1,
                zoom: 10,
                x_range: [0, 0],
                y_range: [0, 0],
                hilbert_range: [0, 0],
                archive_bytes: vec![1u8, 2, 3],
            },
            fe_format::ChunkInput {
                seq: 1,
                tile_count: 1,
                zoom: 10,
                x_range: [1, 1],
                y_range: [0, 0],
                hilbert_range: [1, 1],
                archive_bytes: vec![4u8, 5, 6, 7],
            },
        ];
        let expected_hashes: Vec<[u8; 32]> = chunks
            .iter()
            .map(|c| {
                fe_runtime::blob_store::hash_from_hex(&fe_format::integrity::digest_hex(
                    &c.archive_bytes,
                ))
                .expect("digest_hex produces a valid 32-byte hex hash")
            })
            .collect();
        let unsealed = fe_format::HexonArchive::export_chunked(
            synthetic_tileset_manifest(hexon_id),
            &tileset_meta,
            &chunks,
        )
        .expect("export container");
        let sealed = fe_format::seal(&unsealed, key).expect("seal container");
        (header_doc_json(&sealed), expected_hashes)
    }

    /// (critic-named flow) `seeder_with_an_unsigned_chunk_index_is_refused_before_any_chunk_request`:
    /// an UNSEALED header (the shape a legacy/tampered seeder would still
    /// serve as child 0) is refused outright — PAS-P5 decision A2, "every
    /// network path requires a seal". Nothing is ever confirmed for it.
    #[test]
    fn seeder_with_an_unsigned_chunk_index_is_refused_before_any_chunk_request() {
        let unsealed = fe_format::HexonArchive::export_tileset(
            synthetic_tileset_manifest("unsealed-seeder"),
            synthetic_tileset("unsealed-seeder")
                .tileset_meta
                .as_ref()
                .unwrap(),
            &[("10/0/0".to_string(), vec![1u8; 8])],
            &[],
            None,
        )
        .expect("export unsealed archive");
        let meta_json = header_doc_json(&unsealed);

        let err = verify_install_meta("unsealed-seeder", &meta_json, "", None)
            .expect_err("an unsealed header must never verify over the network");
        assert!(err.contains("unsealed") || err.contains("refused"), "{err}");
    }

    /// (critic-named flow) `chunk_not_in_the_signed_index_never_reaches_install`:
    /// the hashes `verify_install_meta` hands back for a sealed CONTAINER are
    /// EXACTLY the signed `chunks/index.json` set — nothing more, nothing
    /// fewer. `TilesetTransfer::confirm` (fe-sync) then refuses any download
    /// whose actually-resolved HashSeq disagrees with this exact list (see
    /// `fe-sync/tests/tileset_transfer_test.rs`
    /// `a_confirmation_disagreeing_with_the_resolved_hashseq_is_refused`), so
    /// a seeder-added chunk the signed index never named can never become a
    /// fetched, installed chunk — it would fail confirmation one layer down.
    #[test]
    fn chunk_not_in_the_signed_index_never_reaches_install() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[41u8; 32]);
        let (meta_json, expected) = sealed_container_header_and_hashes(&key, "container-tileset");

        let (publisher_did, hashes) =
            verify_install_meta("container-tileset", &meta_json, "", None)
                .expect("a sealed container with a valid signature verifies");
        assert_eq!(
            publisher_did,
            fe_format::verifying_key_to_did(&key.verifying_key())
        );
        assert_eq!(
            hashes.expect("a container signs a per-chunk table"),
            expected,
            "exactly the signed chunk hashes, in seq order — nothing a seeder \
             might append unsigned ever appears here"
        );
    }

    /// (critic-mandated) `a_header_signed_by_a_publisher_other_than_the_pinned_one_is_refused`:
    /// once a publisher is pinned (TOFU or caller-supplied), a different
    /// publisher's validly-signed header for the same tileset_id is refused,
    /// not silently substituted.
    #[test]
    fn a_header_signed_by_a_publisher_other_than_the_pinned_one_is_refused() {
        let key_a = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
        let key_b = ed25519_dalek::SigningKey::from_bytes(&[43u8; 32]);
        let pinned_did = fe_format::verifying_key_to_did(&key_a.verifying_key());
        let meta_from_b = sealed_monolithic_header_json(&key_b, "same-tileset-id");

        let err = verify_install_meta("same-tileset-id", &meta_from_b, "", Some(&pinned_did))
            .expect_err("a different publisher's header must be refused against a pin");
        assert!(err.contains("refused"), "{err}");

        // The honest publisher still verifies against its OWN pin.
        let meta_from_a = sealed_monolithic_header_json(&key_a, "same-tileset-id");
        let (publisher_did, hashes) =
            verify_install_meta("same-tileset-id", &meta_from_a, "", Some(&pinned_did))
                .expect("the pinned publisher's own header verifies");
        assert_eq!(publisher_did, pinned_did);
        assert!(
            hashes.is_none(),
            "a monolithic archive signs no per-chunk table"
        );
    }

    /// No local pin yet: the ad's `publisher_did` becomes the TOFU candidate,
    /// and a successful verification records it so a LATER header from a
    /// different publisher for the same tileset_id is then refused — without
    /// ever sending a second `ConfirmTilesetDownload` for the impostor.
    #[test]
    fn first_contact_tofu_pins_the_publisher_then_refuses_an_impostor() {
        let key_a = ed25519_dalek::SigningKey::from_bytes(&[44u8; 32]);
        let key_b = ed25519_dalek::SigningKey::from_bytes(&[45u8; 32]);
        let did_a = fe_format::verifying_key_to_did(&key_a.verifying_key());
        let meta_a = sealed_monolithic_header_json(&key_a, "tofu-tileset");
        let meta_b = sealed_monolithic_header_json(&key_b, "tofu-tileset");

        let mut known = KnownTilesetPublishers::default();
        let (cmd_tx, cmd_rx) = crossbeam::channel::unbounded();
        let sender = fe_sync::SyncCommandSenderRes(cmd_tx);

        // First contact: no local pin, no ad hint either — first-use trust.
        verify_install_meta_and_confirm(
            &mut known,
            None,
            &sender,
            "tofu-tileset".to_string(),
            &meta_a,
            "",
            vec![[0u8; 32]],
        );
        match cmd_rx.try_recv() {
            Ok(fe_sync::SyncCommand::ConfirmTilesetDownload { tileset_id, .. }) => {
                assert_eq!(tileset_id, "tofu-tileset");
            }
            other => panic!("expected ConfirmTilesetDownload, got {other:?}"),
        }
        assert_eq!(known.0.get("tofu-tileset"), Some(&did_a));

        // An impostor's validly-signed header for the SAME tileset_id, with
        // no ad pin override, is refused against the now-known pin.
        verify_install_meta_and_confirm(
            &mut known,
            None,
            &sender,
            "tofu-tileset".to_string(),
            &meta_b,
            "",
            vec![[0u8; 32]],
        );
        match cmd_rx.try_recv() {
            Ok(fe_sync::SyncCommand::CancelTilesetDownload { tileset_id }) => {
                assert_eq!(tileset_id, "tofu-tileset");
            }
            other => panic!("expected CancelTilesetDownload for the impostor, got {other:?}"),
        }
        assert_eq!(
            known.0.get("tofu-tileset"),
            Some(&did_a),
            "the pin is never overwritten by a refused header"
        );
    }

    /// S8: a validly-signed header for ANOTHER hexon served under the
    /// requested tileset_id is refused before anything is confirmed, and it
    /// pins nothing (the pin is keyed by tileset_id).
    #[test]
    fn a_header_signed_for_another_tileset_is_refused_and_pins_nothing() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[46u8; 32]);
        let other = sealed_monolithic_header_json(&key, "some-other-tileset");
        let err = verify_install_meta("wanted-tileset", &other, "", None)
            .expect_err("a header signed for another tileset must be refused");
        assert!(err.contains("not the requested"), "{err}");

        let mut known = KnownTilesetPublishers::default();
        let (cmd_tx, cmd_rx) = crossbeam::channel::unbounded();
        verify_install_meta_and_confirm(
            &mut known,
            None,
            &fe_sync::SyncCommandSenderRes(cmd_tx),
            "wanted-tileset".to_string(),
            &other,
            "",
            vec![[0u8; 32]],
        );
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(fe_sync::SyncCommand::CancelTilesetDownload { .. })
        ));
        assert!(known.0.is_empty(), "a refused header pins nothing");
    }

    /// S8 (pin provenance across a restart): with an installed sealed
    /// tileset and an EMPTY in-memory pin store (a fresh process), the
    /// advertiser's own DID hint cannot become the pin; the installed
    /// publisher is the pin, so the impostor is cancelled before any chunk.
    #[test]
    fn an_installed_tileset_pins_its_publisher_over_the_advertisers_hint() {
        let owner = ed25519_dalek::SigningKey::from_bytes(&[47u8; 32]);
        let impostor = ed25519_dalek::SigningKey::from_bytes(&[48u8; 32]);
        let impostor_did = fe_format::verifying_key_to_did(&impostor.verifying_key());

        let (_tmp, registry) = fresh_tileset_registry();
        let installed = fe_format::seal(
            &fe_format::HexonArchive::export_tileset(
                synthetic_tileset_manifest("restart-pinned"),
                synthetic_tileset("restart-pinned")
                    .tileset_meta
                    .as_ref()
                    .unwrap(),
                &[("10/0/0".to_string(), vec![5u8; 8])],
                &[],
                None,
            )
            .expect("export"),
            &owner,
        )
        .expect("seal");
        registry
            .store()
            .install_tileset(&installed)
            .expect("the owner installs first");
        let installed_pin = registry.store().installed_publisher("restart-pinned");
        assert_eq!(
            installed_pin.as_deref(),
            Some(fe_format::verifying_key_to_did(&owner.verifying_key()).as_str())
        );

        let mut known = KnownTilesetPublishers::default();
        let (cmd_tx, cmd_rx) = crossbeam::channel::unbounded();
        verify_install_meta_and_confirm(
            &mut known,
            installed_pin,
            &fe_sync::SyncCommandSenderRes(cmd_tx),
            "restart-pinned".to_string(),
            &sealed_monolithic_header_json(&impostor, "restart-pinned"),
            &impostor_did,
            vec![[0u8; 32]],
        );
        assert!(
            matches!(
                cmd_rx.try_recv(),
                Ok(fe_sync::SyncCommand::CancelTilesetDownload { .. })
            ),
            "the advertiser's self-chosen pin must not override the installed publisher"
        );
        assert!(known.0.is_empty());
    }

    /// Final review, through the real system: an install-flow meta reply the
    /// Downloads tab tracks is verified off the main thread, then confirmed
    /// (pinning its publisher) or cancelled (its tracker shows the failure);
    /// an untracked reply, such as a streamed request answered after its
    /// timeout, is never confirmed into a full download.
    #[test]
    fn install_flow_replies_are_verified_off_thread_and_untracked_ones_ignored() {
        let key = ed25519_dalek::SigningKey::from_bytes(&[49u8; 32]);
        let did = fe_format::verifying_key_to_did(&key.verifying_key());
        let (mut app, cmd_rx) = streamed_app(None);
        app.init_resource::<TilesetDownloads>();
        let track = |app: &mut App, id: &str| {
            app.world_mut().resource_mut::<TilesetDownloads>().0.insert(
                id.to_string(),
                DownloadProgress {
                    tileset_id: id.to_string(),
                    chunks_received: 0,
                    total_chunks: 0,
                    bytes_received: 0,
                    total_bytes_estimate: 0,
                    status: DownloadStatus::Queued,
                },
            );
        };
        let push_reply = |app: &mut App, id: &str, meta_json: String| {
            app.world_mut()
                .resource_mut::<fe_sync::StreamedTilesetEventBuffer>()
                .events
                .push(fe_sync::SyncEvent::TilesetMetaReceived {
                    peer_id: String::new(),
                    tileset_id: id.to_string(),
                    meta_json,
                    total_chunks: 1,
                    approx_size_bytes: 0,
                    root_hash: "ab".repeat(32),
                    chunk_hashes: vec!["cd".repeat(32)],
                    index_path: std::path::PathBuf::new(),
                    ad_publisher_did: String::new(),
                });
        };
        let settle = |app: &mut App, done: &dyn Fn(&App) -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !done(app) && std::time::Instant::now() < deadline {
                app.update();
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };

        push_reply(
            &mut app,
            "untracked",
            sealed_monolithic_header_json(&key, "untracked"),
        );
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(200));
        app.update();
        assert!(
            cmd_rx.try_recv().is_err(),
            "an untracked reply sends neither Confirm nor Cancel"
        );

        track(&mut app, "good");
        push_reply(
            &mut app,
            "good",
            sealed_monolithic_header_json(&key, "good"),
        );
        settle(&mut app, &|app| {
            app.world()
                .resource::<KnownTilesetPublishers>()
                .0
                .contains_key("good")
        });
        match cmd_rx.try_recv() {
            Ok(fe_sync::SyncCommand::ConfirmTilesetDownload { tileset_id, .. }) => {
                assert_eq!(tileset_id, "good")
            }
            other => panic!("expected ConfirmTilesetDownload, got {other:?}"),
        }
        assert_eq!(
            app.world()
                .resource::<KnownTilesetPublishers>()
                .0
                .get("good"),
            Some(&did)
        );

        track(&mut app, "bad");
        push_reply(
            &mut app,
            "bad",
            sealed_monolithic_header_json(&key, "someone-else"),
        );
        let failed = |app: &App| {
            matches!(
                app.world().resource::<TilesetDownloads>().0["bad"].status,
                DownloadStatus::Failed(_)
            )
        };
        settle(&mut app, &failed);
        assert!(failed(&app), "the refusal reaches the Downloads tab");
        match cmd_rx.try_recv() {
            Ok(fe_sync::SyncCommand::CancelTilesetDownload { tileset_id }) => {
                assert_eq!(tileset_id, "bad")
            }
            other => panic!("expected CancelTilesetDownload, got {other:?}"),
        }
        assert!(!app
            .world()
            .resource::<KnownTilesetPublishers>()
            .0
            .contains_key("bad"));
    }
}
