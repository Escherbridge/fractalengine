use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::prelude::{AppExit, MessageWriter, PluginGroup};
use fe_runtime::app::{
    ApiCommandReceiver, ApiCommandSender, BevyHandles, PendingApiRequests,
    RevocationBroadcastSender, TransformBroadcastSender,
};
use fe_runtime::channels::ChannelHandles;
use fe_runtime::messages::DbCommand;
use tracing_subscriber::EnvFilter;

mod replicas;

/// Bevy resource carrying the "shutdown requested" flag set by the signal
/// thread. A system polls it and raises `AppExit::Success` so `app.run()`
/// returns and the process exits 0 (A8) instead of the loop running forever.
#[derive(bevy::prelude::Resource)]
struct ShutdownRequested(Arc<AtomicBool>);

fn main() -> anyhow::Result<()> {
    // Durability: default SurrealKV's fsync mode unless the operator overrode it,
    // mirroring the GUI binary so both agree. Valid values are `never` | `every` |
    // a duration >100ms. Must run before the DB thread opens the datastore below.
    if std::env::var("SURREAL_DATASTORE_SYNC_DATA").is_err() {
        std::env::set_var("SURREAL_DATASTORE_SYNC_DATA", "every");
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting FractalEngine Relay (headless)");

    let ch = ChannelHandles::new();

    let _net_thread = fe_network::spawn_network_thread(ch.net_cmd_rx, ch.net_evt_tx);

    let blob_store: fe_database::BlobStoreHandle =
        Arc::new(fe_sync::FsBlobStore::open_default().expect("open blob store"));

    // Relay uses EnvBackend — secrets come from environment variables
    let secret_store: Arc<dyn fe_identity::SecretStore> = Arc::new(fe_identity::EnvBackend::new());

    let node_kp = match fe_identity::load_or_generate_keypair(&secret_store, "node_keypair") {
        Ok(kp) => kp,
        Err(e) => {
            tracing::warn!("Could not load/store keypair, generating ephemeral: {e}");
            fe_identity::NodeKeypair::generate()
        }
    };

    let iroh_secret = node_kp.to_iroh_secret();
    let local_did = node_kp.to_did_key();
    let api_verifying_key = node_kp.verifying_key();

    // A24/DEC-C9: dedicated share-URL signing key, independent of the node
    // identity seed — EnvBackend slot `FE_SECRET_FRACTALENGINE_SHARE_SIGNER`
    // (operator must export it for the key to survive a restart, same
    // caveat as the relay's node keypair; fe-api/src/AGENTS.md §share).
    let share_signer = match fe_identity::load_or_generate_keypair(&secret_store, "share_signer") {
        Ok(kp) => Arc::new(kp),
        Err(e) => {
            tracing::warn!("Could not load/store share signer, generating ephemeral: {e}");
            Arc::new(fe_identity::NodeKeypair::generate())
        }
    };

    let db_keypair = fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes())
        .expect("recreate keypair from seed");

    let (repl_tx, repl_rx) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(256);
    // Clone for the API thread's emit seam (A11: the API ingestion path writes
    // directly on `db_reader` and needs its own replication sender).
    let repl_tx_for_api = repl_tx.clone();

    let db_path = std::env::var("FE_DB_PATH").unwrap_or_else(|_| "data/fractalengine.db".into());

    // Cloned for the seed-gate replay below — the original moves into the DB thread.
    let db_res_tx_replay = ch.db_res_tx.clone();

    // Scene change broadcast: DB thread emits CUD deltas, API thread fans out to WS clients.
    let (entity_change_tx, entity_change_rx) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::SceneChange>(256);

    // F24: the relay's analytics hot cache. The `EntityStore` is subscribed to
    // the scene-change broadcast BEFORE the DB thread spawns (no change-event
    // gap), and the drain thread mirrors every DB-thread CUD delta into it —
    // the headless twin of the GUI's `drain_scene_changes_to_store` system.
    // Hydration from pre-startup rows happens later, when the optional API
    // read connection opens (see the api_db_reader block below).
    let entity_store: Arc<fe_entity_store::EntityStore> =
        Arc::new(fe_entity_store::EntityStore::new());
    let entity_store_for_drain = Arc::clone(&entity_store);
    std::thread::spawn(move || run_entity_store_drain(entity_change_rx, entity_store_for_drain));

    let _db_thread = fe_database::spawn_db_thread_with_sync(
        ch.db_cmd_rx,
        ch.db_res_tx,
        blob_store.clone(),
        Some(repl_tx),
        Some(db_keypair),
        Some(secret_store.clone()),
        Some(entity_change_tx.clone()),
        Some(db_path.clone()),
    );

    // A10 letter gap (F20 fold-in): the seed send was a bare `.ok()` — a
    // closed DB channel (dead DB thread) was silent. Mirror the GUI's
    // seed-gate: loud error + exit, never a quiet drop.
    if ch.db_cmd_tx.send(DbCommand::Seed).is_err() {
        tracing::error!("Database command channel closed before seed");
        std::process::exit(1);
    }

    // Seed barrier (same pattern as the GUI): SurrealKV takes an exclusive
    // file lock while the DB thread initialises, so a second read-only
    // connection opened mid-init fails (os error 33). Wait for the seed to
    // complete, collecting results and replaying them so Bevy still sees the
    // full `DbResult` stream.
    let mut startup_db_results = Vec::new();
    let seed_deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        match ch.db_res_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(result @ fe_runtime::messages::DbResult::Seeded { .. }) => {
                startup_db_results.push(result);
                break;
            }
            Ok(fe_runtime::messages::DbResult::Error(e)) => {
                tracing::error!("Database seed failed: {e}");
                break;
            }
            Ok(result) => startup_db_results.push(result),
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                if std::time::Instant::now() >= seed_deadline {
                    tracing::warn!("Database seed did not complete in time; continuing");
                    break;
                }
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                tracing::warn!("Database result channel closed during seed");
                break;
            }
        }
    }
    for result in startup_db_results {
        if db_res_tx_replay.send(result).is_err() {
            tracing::warn!("Database result channel closed while replaying startup result");
            break;
        }
    }

    // Sync thread
    let (sync_cmd_tx, sync_cmd_rx) = crossbeam::channel::bounded(256);
    let (sync_evt_tx, sync_evt_rx) = crossbeam::channel::bounded(256);
    tracing::info!(node_id = %iroh_secret.public(), "Starting sync thread");

    let _sync_thread = fe_sync::spawn_sync_thread(
        iroh_secret,
        blob_store.clone(),
        sync_cmd_rx,
        sync_evt_tx,
        local_did,
        // Inbound apply path (A4/A7): replicated rows ride the DB thread's
        // command channel — the DB thread stays the single SurrealDB writer, so
        // the relay is a fully applying replica (not drop-and-count only).
        Some(ch.db_cmd_tx.clone()),
        // Default data dir (FE_P2P_DIR env, `data/p2p`).
        None,
    );

    // M2/F7 (A17): API→sync distributed-query bridge — the relay answers
    // compute requests for shards it hosts (a seeder serves its shards), and
    // its own distributed surfaces fan out through the same seam. A
    // disconnected sync thread is shutdown (the bridge exits silently —
    // §backpressure's Disconnected-is-silent rule).
    let (distributed_call_tx, distributed_call_rx) =
        crossbeam::channel::bounded::<fe_runtime::distributed_query::DistributedQueryCall>(64);
    fe_sync::distributed_query::bridge_distributed_queries(
        distributed_call_rx,
        sync_cmd_tx.clone(),
    );

    // Replication bridge (A10): try_send + drop-and-warn, matching the GUI
    // pattern. A stalled sync thread must degrade to observable replication lag,
    // never block the DB→sync hop; a disconnected channel is shutdown (silent).
    {
        let sync_tx_for_repl = sync_cmd_tx.clone();
        std::thread::spawn(move || run_replication_bridge(repl_rx, &sync_tx_for_repl));
    }

    // Clone db_cmd_tx before it moves into BevyHandles — needed for graceful shutdown.
    let db_cmd_tx_for_shutdown = ch.db_cmd_tx.clone();
    let sync_cmd_tx_for_shutdown = sync_cmd_tx.clone();

    // Build headless Bevy app
    let mut app = bevy::app::App::new();
    app.add_plugins(
        bevy::MinimalPlugins.set(bevy::app::ScheduleRunnerPlugin::run_loop(
            Duration::from_millis(50),
        )),
    );

    fe_runtime::app::setup_core_systems(
        &mut app,
        BevyHandles {
            net_cmd_tx: ch.net_cmd_tx,
            net_evt_rx: ch.net_evt_rx,
            db_cmd_tx: ch.db_cmd_tx,
            db_res_rx: ch.db_res_rx,
            blob_store: None,
            on_blob_miss: None,
            // Headless relay has no in-app lifecycle consumers (fe-ui absent).
            lifecycle_rx: None,
        },
    );

    // Graceful shutdown (A8): SIGINT (and SIGTERM on unix) sends
    // `DbCommand::Shutdown` **and** `SyncCommand::Shutdown`, waits briefly for
    // the DB flush + sync teardown, then flips the flag the Bevy system below
    // turns into `AppExit::Success` — the process exits 0 instead of the
    // schedule loop running forever.
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    {
        let flag_for_shutdown = shutdown_flag.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(wait_for_shutdown_signal());
            tracing::info!("Received shutdown signal, shutting down gracefully...");
            if db_cmd_tx_for_shutdown.send(DbCommand::Shutdown).is_err() {
                tracing::warn!("DB command channel closed before shutdown");
            }
            if sync_cmd_tx_for_shutdown
                .send(fe_sync::SyncCommand::Shutdown)
                .is_err()
            {
                tracing::warn!("Sync command channel closed before shutdown");
            }
            // Brief grace period: the DB thread flushes its store on Shutdown
            // and the sync thread closes replicas/endpoint before Stopped.
            std::thread::sleep(Duration::from_millis(750));
            flag_for_shutdown.store(true, Ordering::SeqCst);
        });
    }

    // Sync resources
    app.insert_resource(fe_sync::SyncCommandSenderRes(sync_cmd_tx.clone()));
    app.insert_resource(fe_sync::SyncEventReceiverRes(Arc::new(Mutex::new(
        sync_evt_rx,
    ))));
    app.init_resource::<fe_sync::SyncStatus>();
    app.init_resource::<fe_sync::VersePeers>();
    // `drain_sync_events` writes tileset events into this buffer; without it the
    // first frame panics with "Resource does not exist" (the GUI inits it in
    // fe-ui's plugin, which the relay does not load).
    app.init_resource::<fe_sync::TilesetEventBuffer>();
    app.add_systems(bevy::prelude::Update, fe_sync::drain_sync_events);

    // Secret store resource (cloned for the startup replica scan below).
    app.insert_resource(fe_database::SecretStoreRes(secret_store.clone()));

    // API Gateway thread
    let bind_addr = std::env::var("FE_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8765".into());
    let cors_origins: Vec<String> = std::env::var("FE_CORS_ORIGINS")
        .map(|s| s.split(',').map(|o| o.trim().to_string()).collect())
        .unwrap_or_else(|_| vec!["*".to_string()]);

    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(256);
    let (transform_broadcast_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::TransformUpdate>(1024);
    let (revocation_tx, revocation_rx) = tokio::sync::broadcast::channel::<String>(64);

    // Open a second read-only SurrealKV connection for direct API reads.
    // SurrealKV supports concurrent readers; this bypasses the crossbeam channel.
    // Note: SurrealKV takes a per-handle file lock, so while the DB thread's
    // writer connection is alive this legitimately fails (Windows os error 33)
    // and the API falls back to the crossbeam channel — the same fallback the
    // GUI binary hits on this platform. A couple of retries absorb a transient
    // open during DB startup.
    // F24: the analytics hot cache rides `ApiConfig` unless hydration was
    // attempted and failed (fail-closed below). On platforms where the
    // SurrealKV per-handle lock keeps the API read connection closed
    // (Windows, os error 33), the analytics authz gate — which requires the
    // direct reader — is the honest first refusal; the store itself stays
    // wired and live via the drain thread.
    let mut entity_store_for_api: Option<Arc<fe_entity_store::EntityStore>> =
        Some(Arc::clone(&entity_store));

    let api_db_reader: Option<Arc<surrealdb::Surreal<surrealdb::engine::local::Db>>> = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("api_db_reader runtime");
        let mut opened = None;
        for attempt in 1..=4u32 {
            match rt.block_on(async {
                let db = surrealdb::Surreal::new::<surrealdb::engine::local::SurrealKv>(&db_path)
                    .await?;
                db.use_ns("fractalengine").use_db("fractalengine").await?;
                Ok::<_, surrealdb::Error>(db)
            }) {
                Ok(db) => {
                    opened = Some(Arc::new(db));
                    break;
                }
                Err(e) => {
                    if attempt < 4 {
                        tracing::debug!(attempt, "API read connection not ready yet: {e}");
                        std::thread::sleep(Duration::from_millis(250));
                    } else {
                        tracing::warn!(
                            "Could not open API read connection, falling back to channel: {e}"
                        );
                    }
                }
            }
        }
        if opened.is_some() {
            tracing::info!("Opened read-only SurrealKV connection for API gateway");
        }
        // F24: hydrate the analytics hot cache from the pre-startup rows the
        // same way the GUI binary does (the GUI's hydrate_entity_store,
        // extracted verbatim into fe_api::entity_store_bridge). A hydration
        // failure fails the analytics surface CLOSED (entity_store: None)
        // rather than serving a partial nodes table; the drain thread keeps
        // mirroring live scene changes either way.
        if let Some(ref db) = opened {
            match rt.block_on(fe_api::entity_store_bridge::hydrate_entity_store(
                db,
                &entity_store,
            )) {
                Ok(node_count) => tracing::info!(
                    node_count,
                    "Hydrated analytics EntityStore from pre-startup nodes"
                ),
                Err(e) => {
                    tracing::error!(
                        %e,
                        "EntityStore hydration failed — analytics surface stays unavailable"
                    );
                    entity_store_for_api = None;
                }
            }
        }
        opened
    };

    // Tileset registry: scan installed hexon tilesets and load into memory.
    let tileset_registry = match fe_terrain::tiles::HexonStore::new() {
        Ok(store) => {
            let registry = fe_terrain::tiles::TilesetRegistry::new(store);
            let loaded = registry.load_all();
            tracing::info!(count = loaded.len(), "Loaded hexon tilesets into registry");
            Some(Arc::new(registry))
        }
        Err(e) => {
            tracing::warn!("Could not initialize hexon store: {e}");
            None
        }
    };

    // F9/A20: the sim control seam exists only in a `sim-control` build
    // (fe-sim/src/AGENTS.md §session) AND with the runtime opt-in
    // `FE_SIM_ALLOW=1`; otherwise `None` and every /api/v1/sim/* call fails
    // closed.
    #[cfg(feature = "sim-control")]
    let sim_control_tx = spawn_sim_control();
    #[cfg(not(feature = "sim-control"))]
    let sim_control_tx: Option<fe_runtime::sim_control::SimControlCallSender> = None;

    let _api_thread = fe_api::spawn_api_thread(fe_api::ApiConfig {
        bind_addr,
        api_cmd_tx: api_cmd_tx.clone(),
        transform_broadcast_tx: transform_broadcast_tx.clone(),
        verifying_key: api_verifying_key,
        revocation_rx,
        // Shared content-addressed store — closes the asset-endpoint 503 gap
        // (the relay previously passed `None`).
        blob_store: Some(blob_store),
        cors_origins: Some(cors_origins),
        entity_change_tx,
        api_db_reader,
        // F24: the analytics hot cache (scene-change drain + startup
        // hydration, mirroring the GUI binary) — closes the relay's
        // `entity_store: None` gap that kept the merged analytics surface
        // dead on every platform.
        entity_store: entity_store_for_api,
        tileset_registry,
        hexon_registry: None,
        announcement_store: None,
        // A11: API-side ingestion emit seam (IoT readings ride db_reader).
        replication_tx: Some(repl_tx_for_api),
        // M2/F7 (A17): the API→sync distributed-query bridge's call sender —
        // the relay answers compute requests for shards it hosts (a seeder
        // serves its shards), and its own distributed surfaces fan out
        // through the same seam.
        distributed_tx: Some(distributed_call_tx),
        sim_control_tx,
        share_signer,
    });

    app.insert_resource(RevocationBroadcastSender(revocation_tx));
    app.insert_resource(ApiCommandReceiver(Arc::new(Mutex::new(api_cmd_rx))));
    app.insert_resource(ApiCommandSender(api_cmd_tx));
    app.insert_resource(TransformBroadcastSender(transform_broadcast_tx));
    app.init_resource::<PendingApiRequests>();
    app.add_systems(bevy::prelude::Update, fe_runtime::app::drain_api_commands);
    // Headless host: deliver DB replies to pending API requests (the GUI's
    // fe-ui dispatcher does this in the GUI; without it every channel-fallback
    // API request — `/ready`'s Ping, hierarchy reads, scope resolutions —
    // times out even though the DB thread answered).
    app.add_systems(
        bevy::prelude::Update,
        fe_runtime::app::deliver_pending_api_results,
    );

    // A7: react to VerseCreated by opening its replica, and open a replica for
    // every verse the relay hosts at startup (one LoadHierarchy round-trip per
    // process; each open also runs fe-sync's reconciliation pass). The
    // opened-set resource is shared by both systems so runtime-created verses
    // are never churned close+reopen by a later hierarchy reply.
    app.init_resource::<replicas::OpenedReplicas>();
    app.add_systems(
        bevy::prelude::Update,
        replicas::open_replica_on_verse_created,
    );
    app.add_systems(bevy::prelude::Update, replicas::startup_replica_scan);

    // A8: turn the signal thread's flag into a clean Bevy exit (exit code 0).
    app.insert_resource(ShutdownRequested(shutdown_flag));
    app.add_systems(bevy::prelude::Update, exit_on_shutdown_signal);

    tracing::info!("Relay ready -- entering headless event loop");
    let exit = app.run();
    if !exit.is_success() {
        tracing::warn!(?exit, "Relay exited with a non-success status");
    }
    Ok(())
}

/// Spawn the fe-sim control bridge (sim-control builds only, AND the runtime
/// opt-in `FE_SIM_ALLOW=1` — DEC-C13); sessions get scratch dirs under
/// `FE_SIM_WORK_DIR` (default: `<tmp>/fe-relay-sim`). Without the opt-in the
/// bridge is not spawned and `/api/v1/sim/*` keeps its not-configured 503.
#[cfg(feature = "sim-control")]
fn spawn_sim_control() -> Option<fe_runtime::sim_control::SimControlCallSender> {
    if std::env::var("FE_SIM_ALLOW").as_deref() != Ok("1") {
        tracing::warn!(
            "sim-control build WITHOUT FE_SIM_ALLOW=1 — sim bridge NOT spawned (/api/v1/sim/* \
             stays 503). A sim session overrides this process's HLC clock source process-wide; \
             set FE_SIM_ALLOW=1 only on a dedicated lab relay that serves no production verses"
        );
        return None;
    }
    let work_root = std::env::var_os("FE_SIM_WORK_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("fe-relay-sim"));
    let (tx, rx) = fe_runtime::sim_control::sim_control_channel();
    match fe_sim::control::spawn_sim_control_bridge(rx, work_root.clone()) {
        Ok(_handle) => {
            tracing::warn!(
                work_root = %work_root.display(),
                "SIM LAB relay (FE_SIM_ALLOW=1): /api/v1/sim/* is live — a running sim session \
                 OVERRIDES THIS PROCESS'S HLC CLOCK process-wide (every DB thread here stamps sim \
                 time while it runs). Lab relays must not serve production verses"
            );
            Some(tx)
        }
        Err(e) => {
            tracing::error!("sim control bridge failed to spawn — sim surface disabled: {e}");
            None
        }
    }
}

/// DB→sync replication bridge (A10): forward `ReplicationEvent`s from the DB
/// thread to the sync thread with **`try_send` + drop-and-warn**, never a
/// blocking `send`.
///
/// A stalled sync thread must degrade to observable replication lag, never
/// block the DB thread's outbound hop; a disconnected sync channel is
/// shutdown, not backpressure, so it ends the bridge silently. Each dropped
/// event warns with the running total — the bridge's drop counter.
fn run_replication_bridge(
    repl_rx: crossbeam::channel::Receiver<fe_database::ReplicationEvent>,
    sync_tx: &crossbeam::channel::Sender<fe_sync::SyncCommand>,
) {
    let mut dropped: u64 = 0;
    while let Ok(evt) = repl_rx.recv() {
        match sync_tx.try_send(fe_sync::SyncCommand::WriteRowEntry {
            verse_id: evt.verse_id,
            table: evt.table,
            record_id: evt.record_id,
            content_hash: evt.content_hash,
        }) {
            Ok(()) => {}
            Err(crossbeam::channel::TrySendError::Full(_)) => {
                dropped += 1;
                tracing::warn!(
                    dropped_total = dropped,
                    "DB→sync replication bridge full — dropping event"
                );
            }
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => break,
        }
    }
}

/// F24: mirror DB-thread `SceneChange` events into the analytics hot cache.
///
/// Headless twin of the GUI's `drain_scene_changes_to_store` Bevy system: the
/// relay has no fe-ui pass, so a dedicated thread applies each broadcast
/// change directly through the shared conversion (fe-api
/// `entity_store_bridge`) — the store is a concurrent (papaya) map, safe to
/// update off the Bevy loop. Lag on the bounded broadcast degrades to a
/// warned cache gap (the documented eventual-consistency posture of the
/// analytics cache); a closed channel is shutdown, so the drain exits.
fn run_entity_store_drain(
    mut rx: tokio::sync::broadcast::Receiver<fe_runtime::messages::SceneChange>,
    store: Arc<fe_entity_store::EntityStore>,
) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("entity store drain runtime");
    rt.block_on(async move {
        loop {
            match rx.recv().await {
                Ok(change) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    store.apply_scene_change(
                        &fe_api::entity_store_bridge::runtime_scene_change_to_store(change),
                        now_ms,
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(
                        skipped = n,
                        "entity store drain lagged — analytics cache gap (eventual consistency)"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Wait for a shutdown trigger: Ctrl+C on every platform, SIGTERM on unix, or
/// the optional `FE_SHUTDOWN_AFTER_SECS` timer.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                    _ = shutdown_timer() => {}
                }
            }
            Err(e) => {
                tracing::warn!("could not install SIGTERM handler: {e}");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = shutdown_timer() => {}
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = shutdown_timer() => {}
        }
    }
}

/// CI/ops affordance: `FE_SHUTDOWN_AFTER_SECS=N` requests the same graceful
/// shutdown a signal would, after N seconds — used by smoke tests and one-shot
/// container runs (a windowed/headless process cannot always receive SIGINT).
/// Unset → never fires.
async fn shutdown_timer() {
    match std::env::var("FE_SHUTDOWN_AFTER_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(secs) => {
            tracing::info!(
                secs,
                "FE_SHUTDOWN_AFTER_SECS set — scheduling graceful shutdown"
            );
            tokio::time::sleep(Duration::from_secs(secs)).await;
        }
        None => std::future::pending::<()>().await,
    }
}

/// Bevy system: raise `AppExit::Success` once the signal thread requests
/// shutdown (A8). Idempotent — writing the exit message repeatedly is harmless.
fn exit_on_shutdown_signal(
    flag: bevy::prelude::Res<ShutdownRequested>,
    mut exit: MessageWriter<AppExit>,
) {
    if flag.0.load(Ordering::SeqCst) {
        tracing::info!("Shutdown requested — exiting relay");
        exit.write(AppExit::Success);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;

    /// A8: once the shutdown flag is set, the system raises `AppExit::Success`
    /// (exit code 0), which makes `app.run()` return.
    #[test]
    fn shutdown_flag_raises_app_exit_success() {
        let mut app = App::new();
        // `App::new()` already registers `Messages<AppExit>`.
        app.insert_resource(ShutdownRequested(Arc::new(AtomicBool::new(false))));
        app.add_systems(Update, exit_on_shutdown_signal);

        app.update();
        assert!(
            app.should_exit().is_none(),
            "no exit while the flag is clear"
        );

        app.world()
            .resource::<ShutdownRequested>()
            .0
            .store(true, Ordering::SeqCst);
        app.update();
        assert_eq!(
            app.should_exit(),
            Some(AppExit::Success),
            "flag set → AppExit::Success (exit code 0)"
        );
    }

    /// A10 (bridge backpressure): a full sync channel drops the event (with a
    /// running-total warn) instead of blocking the DB→sync hop, and a
    /// disconnected sync channel ends the bridge. The bridge must return
    /// promptly in both cases — a stalled consumer is replication lag, never
    /// a frozen DB thread.
    #[test]
    fn replication_bridge_drops_and_warns_on_full_channel() {
        use fe_database::ReplicationEvent;

        let (repl_tx, repl_rx) = crossbeam::channel::bounded::<ReplicationEvent>(8);
        // A sync channel with room for one command, already holding it.
        let (sync_tx, sync_rx) = crossbeam::channel::bounded::<fe_sync::SyncCommand>(1);
        sync_tx
            .send(fe_sync::SyncCommand::Shutdown)
            .expect("prime the sync channel full");

        let bridge = std::thread::spawn(move || {
            run_replication_bridge(repl_rx, &sync_tx);
        });

        let evt = ReplicationEvent {
            verse_id: "01BRIDGEVERSE00000000000000".into(),
            table: "node".into(),
            record_id: "01BRIDGENODE000000000000000".into(),
            content_hash: [0u8; 32],
            // Not forwarded by the bridge (WriteRowEntry carries no petal
            // scope), matching the GUI binary's bridge.
            petal_id: None,
        };
        let start = std::time::Instant::now();
        for _ in 0..4 {
            // try_send on the bridge side: each of these hits a Full channel
            // and must be dropped, not block.
            repl_tx.send(evt.clone()).expect("bridge alive");
        }
        drop(repl_tx);
        // Bridge ends when the replication channel disconnects; it must not
        // block on the full sync channel.
        bridge.join().expect("bridge thread");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "bridge returned promptly despite a full sync channel"
        );
        // The single primed command is all the sync channel ever received —
        // every replication event was dropped, none blocked the bridge.
        assert!(matches!(
            sync_rx.try_recv(),
            Ok(fe_sync::SyncCommand::Shutdown)
        ));
        assert!(sync_rx.try_recv().is_err(), "no replication event queued");
    }

    /// F24: the analytics-cache drain mirrors a DB-thread scene change into
    /// the `EntityStore` (READ-BACK from the store), and exits cleanly when
    /// the broadcast closes (no leaked thread at shutdown).
    #[test]
    fn entity_store_drain_mirrors_scene_changes_and_exits_on_close() {
        let (tx, rx) = tokio::sync::broadcast::channel::<fe_runtime::messages::SceneChange>(16);
        let store = Arc::new(fe_entity_store::EntityStore::new());
        let store_handle = Arc::clone(&store);
        let drain = std::thread::spawn(move || run_entity_store_drain(rx, store_handle));

        tx.send(fe_runtime::messages::SceneChange::NodeAdded {
            node: fe_runtime::messages::NodeDto {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "anchor".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                has_asset: false,
                asset_path: None,
            },
        })
        .expect("broadcast subscriber alive");

        // The drain applies asynchronously — poll for the mirrored row.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while store.get("n1").is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "drain never mirrored the NodeAdded change"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let snapshot = store.get("n1").expect("mirrored");
        assert_eq!(snapshot.petal_id, "p1");
        assert_eq!(snapshot.position, [1.0, 2.0, 3.0]);

        // Closing the broadcast (all senders dropped) is shutdown — the
        // drain thread exits instead of lingering.
        drop(tx);
        drain.join().expect("drain exits on closed broadcast");
    }
}
