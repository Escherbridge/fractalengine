//! Terminal maintenance supervisor; see AGENTS.md §terminal-reset.

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use fe_runtime::{
    maintenance::{HostJobs, QuiescenceProof, ResetRequest, ResetRequestSender, TerminalMode},
    messages::{DbCommand, DbResult, NetworkCommand},
    service_lifecycle::OwnedService,
};
use std::{
    path::PathBuf,
    thread::JoinHandle,
    time::{Duration, Instant},
};

mod observation;
mod presentation;
use observation::drain_terminal_channels;
use presentation::{close_requests, render_terminal};

const DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
enum Phase {
    Running,
    Quiescing,
    Clearing,
    Cleared,
    Failed,
    Unconfirmed,
}

#[derive(Resource, serde::Serialize)]
struct Status {
    schema: &'static str,
    process_id: u32,
    phase: Phase,
    request_id: Option<String>,
    detail: String,
    cleared: Option<bool>,
    database_joined: bool,
    joined_services: Vec<&'static str>,
    remaining_jobs: Option<usize>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            schema: "fe-terminal-reset-status-v1",
            process_id: std::process::id(),
            phase: Phase::Running,
            request_id: None,
            detail: String::new(),
            cleared: None,
            database_joined: false,
            joined_services: Vec::new(),
            remaining_jobs: None,
        }
    }
}

#[derive(Resource)]
struct Supervisor {
    requests: crossbeam::channel::Receiver<ResetRequest>,
    services: Vec<OwnedService>,
    jobs: HostJobs,
    db_thread: Option<JoinHandle<()>>,
    db_tx: crossbeam::channel::Sender<DbCommand>,
    network: crossbeam::channel::Sender<NetworkCommand>,
    sync: fe_sync::SyncCommandSender,
    token_path: Option<PathBuf>,
    qa_root: Option<PathBuf>,
    developer_enabled: bool,
    started: Option<Instant>,
    sync_stop_sent: bool,
    network_stop_sent: bool,
    pending_clear: Option<DbCommand>,
    clear_sent: bool,
    receipt: Option<(bool, Option<String>)>,
    database_outcome_processed: bool,
}

pub fn install(
    app: &mut App,
    services: Vec<OwnedService>,
    jobs: HostJobs,
    db_thread: JoinHandle<()>,
    token_path: Option<PathBuf>,
    qa_root: Option<PathBuf>,
) {
    let (tx, requests) = crossbeam::channel::bounded(1);
    let world = app.world_mut();
    let db_tx = world
        .resource::<fe_runtime::app::DbCommandSender>()
        .0
        .clone();
    let network = world
        .resource::<fe_runtime::app::NetworkCommandSender>()
        .0
        .clone();
    let sync = world.resource::<fe_sync::SyncCommandSenderRes>().0.clone();
    world
        .resource_mut::<fe_ui::settings::SettingsSession>()
        .install_reset_host(ResetRequestSender(tx));
    world.insert_resource(Supervisor {
        requests,
        services,
        jobs,
        db_thread: Some(db_thread),
        db_tx,
        network,
        sync,
        token_path,
        qa_root,
        developer_enabled: fe_database::admin::DevelopmentResetPermit::capture().is_enabled(),
        started: None,
        sync_stop_sent: false,
        network_stop_sent: false,
        pending_clear: None,
        clear_sent: false,
        receipt: None,
        database_outcome_processed: false,
    });
    world.init_resource::<Status>();
    world.init_resource::<TerminalMode>();
    app.add_systems(First, tick);
}

fn tick(world: &mut World) {
    let Some(mut supervisor) = world.remove_resource::<Supervisor>() else {
        return;
    };
    let mut status = world.remove_resource::<Status>().unwrap_or_default();
    let previous = (status.phase, status.cleared, status.database_joined);
    if status.phase == Phase::Running {
        if let Ok(request) = supervisor.requests.try_recv() {
            if supervisor.developer_enabled
                && request.confirmation == "RESET"
                && valid_request_id(&request.request_id)
            {
                status.request_id = Some(request.request_id);
                status.phase = Phase::Quiescing;
                status.detail = "Stopping external writers and draining earlier work. The database has not been cleared.".into();
                supervisor.started = Some(Instant::now());
                if let Err(error) = freeze_editor(world, &mut supervisor) {
                    fail(&mut status, error, false);
                }
            }
        }
    }
    if status.phase != Phase::Running {
        drain_terminal_channels(world, &mut supervisor, &mut status);
        supervisor.send_stops();
        if status.phase == Phase::Quiescing {
            supervisor.poll_quiescence(world, &mut status);
        }
        supervisor.poll_database(world, &mut status);
        if matches!(status.phase, Phase::Quiescing | Phase::Clearing)
            && supervisor
                .started
                .is_some_and(|started| started.elapsed() >= DEADLINE)
        {
            let sent = supervisor.clear_sent;
            fail(
                &mut status,
                if sent {
                    "Reset outcome is unconfirmed. No retry will be sent; close and reopen manually."
                } else {
                    "Writers did not quiesce within the deadline. The reset was not sent; this process remains in maintenance mode."
                },
                sent,
            );
            supervisor.pending_clear = None;
        }
    }
    if (status.phase, status.cleared, status.database_joined) != previous {
        supervisor.write_receipt(&status);
    }
    world.insert_resource(status);
    world.insert_resource(supervisor);
}

fn valid_request_id(id: &str) -> bool {
    id.strip_prefix("ui-reset-")
        .is_some_and(|id| ulid::Ulid::from_string(id).is_ok())
}

fn fail(status: &mut Status, detail: &str, unconfirmed: bool) {
    status.phase = if unconfirmed {
        Phase::Unconfirmed
    } else {
        Phase::Failed
    };
    status.detail = detail.into();
}

fn freeze_editor(world: &mut World, supervisor: &mut Supervisor) -> Result<(), &'static str> {
    world.resource_mut::<TerminalMode>().enter();
    for service in &supervisor.services {
        service.request_stop();
    }
    {
        let mut schedules = world.resource_mut::<Schedules>();
        drop(schedules.remove(Update));
        let mut terminal_update = Schedule::new(Update);
        terminal_update.add_systems(close_requests);
        schedules.insert(terminal_update);
        drop(schedules.remove(EguiPrimaryContextPass));
        let mut terminal_ui = Schedule::new(EguiPrimaryContextPass);
        terminal_ui.add_systems(render_terminal);
        schedules.insert(terminal_ui);
    }
    world.insert_resource(fe_runtime::app::PendingApiRequests::default());
    if let Some(mut messages) = world.get_resource_mut::<Messages<DbResult>>() {
        messages.clear();
    }
    if let Some(mut messages) =
        world.get_resource_mut::<Messages<fe_runtime::messages::LifecycleEvent>>()
    {
        messages.clear();
    }
    if let Some(mut messages) =
        world.get_resource_mut::<Messages<fe_webview::ipc::BrowserCommand>>()
    {
        messages.clear();
    }
    if let Some(mut backend) =
        world.get_non_send_resource_mut::<fe_webview::plugin::WebViewBackendRes>()
    {
        use fe_webview::backend::WebViewBackend;
        if let Some(mut browser) = backend.backend.take() {
            browser.destroy();
        }
        backend.lost = false;
    }
    world.remove_resource::<crate::terrain_bridge::HexonAssembleResults>();
    world.remove_resource::<crate::terrain_bridge::StreamedBundleResults>();
    crate::terrain_bridge::discard_terminal_state(world);
    fe_ui::maintenance::clear_read_models(world);
    if let Some(mut pending) = world.get_resource_mut::<crate::gpx_bridge::PendingPathEdits>() {
        pending.retire_for_terminal_reset();
    }
    if let Some(mut pending) = world.get_resource_mut::<crate::gpx_bridge::PendingGpxImports>() {
        pending.retire_for_terminal_reset();
    }
    world.insert_resource(fe_terrain::iot::TrackRouteMap::default());
    world.insert_resource(fe_terrain::iot::TrackStyleMap::default());
    world.insert_resource(fe_runtime::app::RuntimeApiEndpoint::default());
    world.insert_resource(fe_runtime::PeerRegistry::default());
    world.insert_resource(fe_sync::SyncStatus::default());
    world.insert_resource(fe_sync::VersePeers::default());
    world.insert_resource(fe_sync::TilesetEventBuffer::default());
    world.insert_resource(fe_sync::StreamedTilesetEventBuffer::default());
    world.insert_resource(fe_renderer::scene_analysis::SceneAnalysisSnapshot::default());
    world.insert_resource(fe_terrain::petal_binding::ActivePetalTerrain::default());
    world.insert_resource(fe_terrain::petal_binding::ActiveTileSource::default());
    world.remove_resource::<fe_renderer::terrain_overlay::ProposalPreview>();
    let entities: Vec<Entity> = world
        .query_filtered::<Entity, Or<(With<Mesh3d>, With<fe_ui::plugin::SpawnedNodeMarker>)>>()
        .iter(world)
        .collect();
    for entity in entities {
        let _ = world.despawn(entity);
    }
    supervisor.jobs.freeze()?;
    for service in &supervisor.services {
        service.request_stop();
    }
    if let Some(contexts) = world.get_resource::<fe_plugin::scene_host::PluginContexts>() {
        contexts
            .shutdown()
            .map_err(|_| "Plugin leases could not be revoked; reset will not run")?;
    }
    let plugin = world
        .get_resource::<fe_plugin::PluginHostService>()
        .ok_or("Plugin service owner is unavailable")?
        .0
        .lock()
        .map_err(|_| "Plugin service owner is unavailable")?
        .take()
        .ok_or("Plugin service ownership was already consumed")?;
    plugin.request_stop();
    supervisor.services.push(plugin);
    Ok(())
}

impl Supervisor {
    fn send_stops(&mut self) {
        if !self.network_stop_sent {
            self.network_stop_sent = match self.network.try_send(NetworkCommand::Shutdown) {
                Ok(()) | Err(crossbeam::channel::TrySendError::Disconnected(_)) => true,
                Err(crossbeam::channel::TrySendError::Full(_)) => false,
            };
        }
        if !self.sync_stop_sent {
            self.sync_stop_sent = match self.sync.try_send(fe_sync::SyncCommand::Shutdown) {
                Ok(()) | Err(crossbeam::channel::TrySendError::Disconnected(_)) => true,
                Err(crossbeam::channel::TrySendError::Full(_)) => false,
            };
        }
    }

    fn poll_quiescence(&mut self, world: &mut World, status: &mut Status) {
        if self.pending_clear.is_none() {
            let proof = match QuiescenceProof::after_joined(&mut self.services, &self.jobs) {
                Ok(Some(proof)) => proof,
                Ok(None) => return,
                Err(error) => {
                    fail(status, &format!("Service shutdown could not be confirmed: {error}. The reset was not sent."), false);
                    return;
                }
            };
            status.joined_services = self.services.iter().map(OwnedService::name).collect();
            status.remaining_jobs = Some(0);
            if let Some(path) = &self.token_path {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => {
                        fail(
                            status,
                            "Dev-token publication could not be invalidated; reset was not sent",
                            false,
                        );
                        return;
                    }
                }
            }
            let Some(mut store) = world.get_resource_mut::<crate::SharedEntityStore>() else {
                fail(
                    status,
                    "Entity cache owner is unavailable; reset was not sent",
                    false,
                );
                return;
            };
            let Some(store) = std::sync::Arc::get_mut(&mut store.0) else {
                fail(
                    status,
                    "Entity cache still has readers; reset was not sent",
                    false,
                );
                return;
            };
            store.clear_for_terminal_reset();
            self.pending_clear = Some(DbCommand::TerminalDevelopmentReset {
                request_id: status.request_id.clone().unwrap_or_default(),
                confirmation: "RESET".into(),
                proof,
            });
        }
        let Some(command) = self.pending_clear.take() else {
            return;
        };
        match self.db_tx.try_send(command) {
            Ok(()) => {
                self.clear_sent = true;
                self.started = Some(Instant::now());
                status.phase = Phase::Clearing;
                status.detail = "All owned writers have joined. Waiting for the checked database transaction and worker exit.".into();
            }
            Err(crossbeam::channel::TrySendError::Full(command)) => {
                self.pending_clear = Some(command)
            }
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => fail(
                status,
                "Database worker is unavailable; reset was not sent",
                false,
            ),
        }
    }

    fn poll_database(&mut self, world: &mut World, status: &mut Status) {
        if !self.clear_sent || self.database_outcome_processed {
            return;
        }
        if let Some(thread) = &self.db_thread {
            if !thread.is_finished() {
                return;
            }
            let Some(thread) = self.db_thread.take() else {
                return;
            };
            if thread.join().is_err() {
                fail(
                    status,
                    "Database worker exited abnormally; reset outcome is unconfirmed",
                    true,
                );
                self.database_outcome_processed = true;
                return;
            }
            status.database_joined = true;
            world.insert_resource(crate::gpx_bridge::PendingPathEdits::default());
            world.insert_resource(crate::gpx_bridge::PendingGpxImports::default());
        }
        let Some((cleared, error)) = self.receipt.take() else {
            return;
        };
        self.database_outcome_processed = true;
        status.cleared = Some(cleared);
        if status.phase == Phase::Unconfirmed {
            status.detail = format!(
                "Late stored outcome: cleared={cleared}. {} Close and reopen manually.",
                error.unwrap_or_else(|| "The database worker has joined.".into())
            );
        } else if cleared && error.is_none() {
            status.phase = Phase::Cleared;
            status.detail = "Local database cleared and default content restored. All writers remain stopped. Close and reopen FractalEngine manually.".into();
        } else {
            fail(
                status,
                &error.unwrap_or_else(|| "Database reset was refused".into()),
                false,
            );
        }
    }

    fn write_receipt(&self, status: &Status) {
        let (Some(root), Some(id)) = (&self.qa_root, &status.request_id) else {
            return;
        };
        let path = root.join(format!(
            "terminal-reset-{id}-{:?}-{:?}-{}.json",
            status.phase, status.cleared, status.database_joined
        ));
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            file.write_all(serde_json::to_string_pretty(status)?.as_bytes())?;
            file.sync_all()
        })();
        if let Err(error) = result {
            tracing::warn!(%error, "Terminal maintenance receipt was not saved");
        }
    }
}
