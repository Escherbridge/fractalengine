use std::sync::{Arc, Mutex};

use bevy::prelude::IntoScheduleConfigs;
use fe_identity::NodeIdentity;
use fe_runtime::app::{
    ApiCommandSender, AppOptions, BevyHandles, LoggingOwnership,
    PendingApiRequests, TransformBroadcastSender,
};
use fe_runtime::channels::ChannelHandles;
use fe_runtime::messages::DbCommand;
use fe_runtime::PeerRegistry;
use fe_ui::plugin::LocalUserRole;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

mod asset_bridge;
mod extension_host;
mod api_endpoint;
mod gpx_bridge;
mod input_bridge;
mod native_accessibility;
mod native_window_witness;
mod plugin_bridge;
mod panic_log;
mod portal_host;
mod qa_a11y_timing;
mod qa_debug_bridge;
mod qa_dev_token;
mod qa_log;
mod qa_portal_bridge;
mod qa_invitation_bridge;
mod relay_serve_bridge;
mod render_wake;
mod screenshot_bridge;
mod terrain_bridge;
mod terminal_reset;
mod vision_bridge;
mod window_chrome;

/// SurrealKV database path, opened exactly once per process by the DB thread.
const DB_PATH: &str = "data/fractalengine.db";

fn main() {
    let qa_root = match fe_identity::file_fallback::qa_storage_root() {
        Ok(root) => root,
        Err(error) => {
            eprintln!("invalid QA storage root: {error}");
            std::process::exit(1);
        }
    };
    if let Some(root) = qa_root.as_ref() {
        fe_runtime::qa_debug::enable_qa_operator();
        for (name, suffix) in [
            ("FE_BLOB_DIR", "data/blobs"),
            ("FE_P2P_DIR", "data/p2p"),
            ("FE_HEXON_DIR", "hexons"),
            ("WEBVIEW2_USER_DATA_FOLDER", "webview"),
        ] {
            std::env::set_var(name, root.join(suffix));
        }
    }
    // Packaged build: run from the executable's folder so `data/` (database,
    // P2P store, panic log) lands beside the binary wherever it is launched
    // from. Before any relative path is touched. See fe-runtime src/AGENTS.md
    // §packaged-layout.
    if let Some(root) = qa_root.clone().or_else(fe_runtime::app::packaged_root) {
        if let Err(error) = std::env::set_current_dir(&root) {
            eprintln!("could not enter {}: {error}", root.display());
            std::process::exit(1);
        }
    }

    // Captures the payload of any panic (esp. in-egui-pass panics that
    // otherwise only surface as "Encountered a panic in system" + abort) to
    // `data/panic.log`. Installed first so it covers every subsequent line.
    panic_log::install();

    // Durability: default SurrealKV's fsync mode unless the operator overrode it.
    // Valid values are `never` | `every` | a duration >100ms; `"true"` is invalid
    // and would brick startup. See src/AGENTS.md §durability. Must run before any
    // datastore opens (the DB thread below).
    if std::env::var("SURREAL_DATASTORE_SYNC_DATA").is_err() {
        std::env::set_var("SURREAL_DATASTORE_SYNC_DATA", "every");
    }

    let qa_enabled = fe_api::input::input_allowed_from_env();
    if let Err(error) = fe_runtime::qa_web::install(qa_root.as_deref(), qa_enabled) {
        eprintln!("controlled QA web fixture unavailable: {error}");
    }
    let qa_dialogs = if qa_enabled {
        qa_root
            .as_ref()
            .map(|root| fe_ui::file_dialogs::QaFileDialogs::new(root.clone()).map(Arc::new))
            .transpose()
            .unwrap_or_else(|error| {
                eprintln!("could not configure QA dialogs: {error}");
                std::process::exit(1);
            })
    } else {
        None
    };
    let qa_api_port = if qa_root.is_some() {
        match std::env::var("FE_QA_API_PORT") {
            Ok(value) => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .unwrap_or_else(|| {
                    eprintln!("FE_QA_API_PORT must be a nonzero port");
                    std::process::exit(1);
                }),
            Err(_) => 8765,
        }
    } else {
        8765
    };
    let qa_logs = fe_runtime::qa_debug::QaLogRing::default();
    // `FE_LOG_FORMAT=json` swaps the text layer for one JSON object per line (fe-runtime/src/AGENTS.md §log-format).
    let log_format = fe_runtime::log_format::LogFormat::from_env();
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with((log_format == fe_runtime::log_format::LogFormat::Text).then(|| tracing_subscriber::fmt::layer()))
        .with((log_format == fe_runtime::log_format::LogFormat::Json).then(|| tracing_subscriber::fmt::layer().json()))
        .with(qa_enabled.then(|| qa_log::QaLogLayer(qa_logs.clone())))
        .init();
    if qa_enabled {
        tracing::warn!(
            "FE_API_INPUT_ALLOW=1: {}",
            fe_api::input::INPUT_GATE.enabled_warning
        );
    }

    // Application settings (D-78) load first: the relay spec must be registered
    // before the sync thread builds its endpoint. See src/AGENTS.md §app-settings.
    let (app_settings, settings_path) = fe_ui::settings::load_startup_settings();
    fe_sync::relay_config::set_settings_relay_spec(app_settings.relay.clone());
    // NET-30: auto-home and act-as-prober are read by the sync thread at startup, so register them here too.
    relay_serve_bridge::register_settings(&app_settings);

    let maintenance_jobs = fe_runtime::maintenance::HostJobs::install().expect("one host job owner");
    let mut maintenance_services = Vec::new();
    let ch = ChannelHandles::new();

    let net_thread = fe_network::spawn_network_thread(ch.net_cmd_rx, ch.net_evt_tx);
    maintenance_services.push(fe_runtime::service_lifecycle::OwnedService::new("network", Default::default(), net_thread));

    // P2P Mycelium Phase A+B: local content-addressed blob store, shared with
    // the DB thread and the Bevy blob:// asset source.
    let blob_store: fe_database::BlobStoreHandle = match fe_sync::FsBlobStore::open_default() {
        Ok(store) => std::sync::Arc::new(store),
        Err(error) => {
            tracing::error!(%error, "Could not open the local blob store");
            eprintln!("Fatal error: could not open the local blob store: {error}");
            std::process::exit(1);
        }
    };

    // Secret store: OS keychain on desktop.
    let secret_store: Arc<dyn fe_identity::SecretStore> = if let Some(root) = qa_root.as_ref() {
        match fe_identity::secret_store::RunLocalBackend::new(root.join("identity")) {
            Ok(store) => Arc::new(store),
            Err(error) => {
                eprintln!("could not initialize run-local secret storage: {error}");
                std::process::exit(1);
            }
        }
    } else {
        Arc::new(fe_identity::OsKeystoreBackend::new())
    };

    // Load or generate a persistent node keypair across three durability
    // tiers: OS keystore -> a key file under the per-user data directory ->
    // (only if BOTH fail) an in-process ephemeral key. A silent ephemeral
    // fallback here used to mint a DID that no longer matches a verse's
    // created_by, locking the local user out of their own verse as a mere
    // Viewer (F6 in-app lockout root cause; see src/AGENTS.md
    // §identity-tiers and fe-database/src/AGENTS.md §reconcile
    // "ClaimOrphanedVerse" for the explicit recovery path once that has
    // already happened once).
    //
    // Security follow-up: the file tier must live under a per-user OS data
    // directory (`%LOCALAPPDATA%\fractalengine\identity` on Windows), NOT
    // a path derived from `DB_PATH` — that path is CWD-relative and could
    // resolve somewhere unexpected depending on how the binary is launched.
    // `legacy_identity_dir` is the pre-this-fix location (still CWD-relative,
    // beside the DB/p2p stores) kept as a one-shot, read-only migration
    // source — see `fe_identity::file_fallback::load_or_generate_migrating`.
    let legacy_identity_dir = std::path::Path::new(DB_PATH)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("data"))
        .join("identity");
    let identity_fallback_dir = dirs::data_local_dir()
        .map(|d| d.join("fractalengine").join("identity"))
        .unwrap_or_else(|| {
            tracing::warn!(
                "could not resolve a per-user data directory; using the legacy \
                 CWD-relative identity directory instead"
            );
            legacy_identity_dir.clone()
        });
    let fe_identity::TieredKeypair {
        keypair: node_kp,
        source: identity_source,
    } = if let Some(root) = qa_root.as_ref() {
        let keypair = match fe_identity::file_fallback::load_or_generate(
            &root.join("identity"),
            "node_keypair",
        ) {
            Ok(keypair) => keypair,
            Err(error) => {
                eprintln!("could not load QA identity: {error}");
                std::process::exit(1);
            }
        };
        if let Err(error) =
            fe_identity::keychain::store_keypair(&secret_store, "node_keypair", &keypair)
        {
            eprintln!("could not initialize QA secret store: {error}");
            std::process::exit(1);
        }
        fe_identity::TieredKeypair {
            keypair,
            source: fe_identity::KeypairSource::FileFallback,
        }
    } else {
        fe_identity::load_or_generate_keypair_durable(
            &secret_store,
            "node_keypair",
            &identity_fallback_dir,
            &legacy_identity_dir,
        )
    };
    let identity_banner_message = match identity_source {
        fe_identity::KeypairSource::Keystore | fe_identity::KeypairSource::FileFallback => None,
        fe_identity::KeypairSource::Ephemeral => Some(
            "Your saved identity could not be loaded — running with a temporary identity; \
             existing content may appear read-only."
                .to_string(),
        ),
    };
    let iroh_secret = node_kp.to_iroh_secret();
    let local_did = node_kp.to_did_key();
    let api_verifying_key = node_kp.verifying_key();

    // A24/DEC-C9: dedicated share-URL signing key — a SEPARATE keystore slot,
    // not derived from the node identity seed, so independent rotation is
    // possible and a leaked share key can never be used to impersonate the
    // node. Persisted the same way as the node keypair; restarts keep
    // previously issued share links valid (fe-api/src/AGENTS.md §share).
    let share_signer = match fe_identity::load_or_generate_keypair(&secret_store, "share_signer") {
        Ok(kp) => Arc::new(kp),
        Err(e) => {
            tracing::warn!(
                "Could not load/store share signer in secret store, generating ephemeral: {e}"
            );
            Arc::new(fe_identity::NodeKeypair::generate())
        }
    };

    // Phase F: create a second keypair from the same seed for the DB thread.
    // NodeKeypair is not Clone, so we recreate from the same seed bytes.
    let db_keypair = fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes())
        .expect("recreate keypair from seed");
    // Opt-in agent-QA token file (src/AGENTS.md §qa-dev-token): unset = None,
    // and nothing below changes behaviour.
    let dev_token_out = fe_api::dev_token::dev_token_out_from_env();
    let maintenance_token_path = dev_token_out.clone();
    let dev_token_keypair = dev_token_out.as_ref().map(|_| {
        fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes())
            .expect("recreate keypair from seed")
    });
    // Relay adverts are checked against the iroh-docs author, which is the sync endpoint's key: the node seed.
    let advertisement_signer = Arc::new(
        fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes()).expect("recreate keypair from seed"),
    );

    // Phase E: replication channel (DB → sync bridge).
    // The DB thread emits ReplicationEvents; we bridge them to SyncCommand::WriteRowEntry
    // after the sync command sender is created below.
    let (repl_tx, repl_rx) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(256);
    // Clone for the API thread's emit seam (A11: the API-side ingestion path
    // writes directly on db_reader and needs its own replication sender).
    let repl_tx_for_api = repl_tx.clone();

    // Scene change broadcast: DB thread emits CUD deltas, API thread fans out to WS clients.
    let (entity_change_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::SceneChange>(256);

    // Node lifecycle seam (node_lifecycle_addressing_20260725 FR-6): the DB
    // thread emits create/promote/tombstone/reflow events onto `lifecycle_tx`;
    // the binary owns the receiver half (fe-database can't depend on fe-sync, so
    // it declares its own sender type and the binary bridges the two halves —
    // see fe-sync/src/AGENTS.md §lifecycle-forwarding).
    let (lifecycle_tx, lifecycle_rx) = fe_sync::lifecycle_channel(256);

    // Cloned for the seed-gate replay below — the original moves into the DB thread.
    let db_res_tx_replay = ch.db_res_tx.clone();
    // The DB thread is the store's only opener; `shared_db` is a clone of its
    // handle for the API reader. See AGENTS.md §entity-store-analytics.
    let fe_database::DbThread {
        join_handle: db_thread,
        shared_db,
    } = match fe_database::spawn_db_thread_shared(
        ch.db_cmd_rx,
        ch.db_res_tx,
        blob_store.clone(),
        Some(repl_tx),
        Some(db_keypair),
        Some(secret_store.clone()),
        Some(entity_change_tx.clone()),
        Some(DB_PATH.to_string()),
        Some(lifecycle_tx),
    ) {
        Ok(db_thread) => db_thread,
        Err(e) => {
            tracing::error!("Failed to start database thread: {}", e);
            eprintln!("Fatal error: Could not initialize database.\n{}", e);
            eprintln!("Please ensure the 'data/' directory is writable and the path is valid.");
            std::process::exit(1);
        }
    };

    // OF-1/S1b: co-locate the blob store with this install's own DB. The
    // pre-2026-10-14 default was per-USER while the DB is per-INSTALL, so one
    // machine's installs could share a blob store and one install's asset GC
    // could delete a blob another install's DB still referenced. Runs once,
    // right after the DB thread's shared handle is ready and before
    // `blob_store` is handed to anything else (the sync thread below,
    // `schedule_asset_gc`'s periodic Bevy system) — see
    // fe-database/src/handlers/blob_migration.rs, fe-sync/src/AGENTS.md
    // §blob-store. A migration failure is logged and never blocks startup:
    // the store is still usable, just not backfilled.
    if qa_root.is_none() {
        let legacy_root = fe_sync::FsBlobStore::legacy_default_path();
        let new_root = fe_sync::FsBlobStore::default_path();
        let migration_rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("blob migration runtime");
        match migration_rt.block_on(
            fe_database::handlers::blob_migration::migrate_legacy_blob_store(
                &shared_db,
                blob_store.as_ref(),
                &new_root,
                &legacy_root,
            ),
        ) {
            Ok(report) if report.copied > 0 => {
                tracing::info!(
                    copied = report.copied,
                    "Migrated blobs from the legacy per-user store"
                )
            }
            Ok(_) => {}
            Err(error) => {
                tracing::error!(%error, "Legacy blob-store migration failed — continuing with an unmigrated store")
            }
        }
    }

    // `lifecycle_rx` rides into `BevyHandles` below: fe-runtime pumps it into
    // `Messages<LifecycleEvent>` so in-app consumers (fe-ui PathReflow re-flow,
    // T5 reporting) observe create/promote/tombstone/reflow (FR-6). The op-log
    // stays the durable source of truth.

    // Gate seed and preserve its DB results; see AGENTS.md §entity-store-analytics.
    if ch.db_cmd_tx.send(DbCommand::Seed).is_err() {
        tracing::error!("Database command channel closed before seed");
        eprintln!("Fatal error: database command channel closed before seed.");
        std::process::exit(1);
    }
    let mut startup_db_results = Vec::new();
    loop {
        match ch
            .db_res_rx
            .recv_timeout(std::time::Duration::from_secs(30))
        {
            Ok(result @ fe_runtime::messages::DbResult::Seeded { .. }) => {
                startup_db_results.push(result);
                break;
            }
            Ok(fe_runtime::messages::DbResult::Error(error)) => {
                tracing::error!(%error, "Database seed failed");
                eprintln!("Fatal error: database seed failed: {error}");
                std::process::exit(1);
            }
            Ok(result) => startup_db_results.push(result),
            Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
                tracing::error!("Database seed did not complete within 30 seconds");
                eprintln!("Fatal error: database seed timed out during startup.");
                std::process::exit(1);
            }
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                tracing::error!("Database result channel closed during seed");
                eprintln!("Fatal error: database result channel closed during startup.");
                std::process::exit(1);
            }
        }
    }
    for result in startup_db_results {
        if db_res_tx_replay.send(result).is_err() {
            tracing::error!("Database result channel closed while replaying startup result");
            eprintln!("Fatal error: database result channel closed during startup.");
            std::process::exit(1);
        }
    }

    // ---- P2P Mycelium Phase D: sync thread (iroh endpoint) ----
    let (sync_cmd_tx, sync_cmd_rx) = crossbeam::channel::bounded(256);
    let (sync_evt_tx, sync_evt_rx) = crossbeam::channel::bounded(256);
    tracing::info!(
        node_id = %iroh_secret.public(),
        "Starting sync thread"
    );

    // Extension `blob.source`: empty (peers only) until the plugin bridge installs a source; the sync thread's scene
    // transfers and the `blob://` reader ask the same slot (src/AGENTS.md section extension-host).
    let blob_fallback = fe_runtime::bevy_blob_reader::BlobFallbackSlot::default();
    let sync_thread = fe_sync::sync_thread::spawn_sync_thread_with_sources(
        iroh_secret,
        blob_store.clone(),
        sync_cmd_rx,
        sync_evt_tx,
        local_did,
        // Inbound apply path (A4): replicated rows ride the DB thread's
        // command channel — the DB thread stays the single SurrealDB writer.
        Some(ch.db_cmd_tx.clone()),
        // Default data dir (FE_P2P_DIR env, `data/p2p`) — one DocsStack per
        // process for the desktop binary.
        None,
        // Production transport (never the sim lab's virtual one).
        None,
        blob_fallback.clone(),
    );

    maintenance_services.push(fe_runtime::service_lifecycle::OwnedService::new("sync", Default::default(), sync_thread));

    // NET-32: API diagnostics calls reach the sync thread through this bridge; it exits when the API drops the sender.
    let (network_diagnostics_tx, network_diagnostics_rx) =
        crossbeam::channel::bounded::<fe_runtime::net_diagnostics::NetworkDiagnosticsCall>(16);
    if let Err(error) =
        fe_sync::net_diagnostics::bridge_network_diagnostics(network_diagnostics_rx, sync_cmd_tx.clone())
    {
        tracing::warn!(%error, "network diagnostics bridge did not start; the diagnostics route answers 503");
    }

    // NET-30: desktop relay-capable mode; consent, feature, self-test and dial-back are checked off the main thread.
    if app_settings.serve_relay {
        spawn_desktop_relay_gate(app_settings.clone(), sync_cmd_tx.clone(), shared_db.clone());
    }

    // M2/F7 (A17): API→sync distributed-query bridge. The API thread's
    // distributed surfaces send `DistributedQueryCall`s; this bridge forwards
    // each one into `SyncCommand::SubmitComputeTask` on the sync thread
    // (request/response correlation + bounded concurrency live there). A
    // disconnected sync thread is shutdown (the bridge exits silently —
    // §backpressure's Disconnected-is-silent rule).
    let (distributed_call_tx, distributed_call_rx) =
        crossbeam::channel::bounded::<fe_runtime::distributed_query::DistributedQueryCall>(64);
    maintenance_services.push(fe_sync::distributed_query::bridge_distributed_queries_owned(
        distributed_call_rx,
        sync_cmd_tx.clone(),
    ));

    // Phase E: bridge replication events from DB thread to sync thread.
    // try_send + drop counter so a stalled sync thread never blocks this hop
    // (see fe-database/src/AGENTS.md §replication-backpressure).
    {
        let sync_tx_for_repl = sync_cmd_tx.clone();
        maintenance_services.push(fe_runtime::service_lifecycle::OwnedService::spawn("replication", move |stop| {
            let mut dropped: u64 = 0;
            while !stop.is_requested() {
                let evt = match repl_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(event) => event,
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => break,
                };
                match sync_tx_for_repl.try_send(fe_sync::SyncCommand::WriteRowEntry {
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
        }));
    }

    // Wire the on_miss callback: when BlobAssetReader can't find a blob
    // locally, it sends a FetchBlob command to the sync thread.
    let sync_cmd_for_miss = sync_cmd_tx.clone();
    let on_miss: fe_runtime::bevy_blob_reader::OnMissCallback = Arc::new(move |hash| {
        if let Err(error)=sync_cmd_for_miss.try_send(fe_sync::SyncCommand::FetchBlob {
            hash,verse_id:String::new(),
        }) {
            tracing::warn!(%error,"Asset-reader miss could not reach the admitted scene transfer queue");
        }
    });

    // Clone the blob-store handle for the API thread (asset endpoints) and the
    // asset-download bridge before it moves into BevyHandles below. All share one
    // content-addressed store.
    let blob_store_for_api = blob_store.clone();
    let blob_store_for_assets = blob_store.clone();
    let blob_store_for_extensions = blob_store.clone();
    let blob_store_for_portal = blob_store.clone();

    // S4 startup-seeding clone: `sync_cmd_tx` moves into `SyncCommandSenderRes`
    // just below, so the startup-publish loop (after `tileset_registry`
    // builds) needs its own handle taken before that move.
    let sync_cmd_tx_for_seeding = sync_cmd_tx.clone();

    // Frameless primary window: fe-ui's topbar is the title bar (src/AGENTS.md
    // §window-chrome).
    // QA-0119: every cross-thread queue a Bevy system drains wakes the reactive loop.
    let render_work = render_wake::PendingWork::default();
    render_work.watch(ch.db_res_rx.clone());
    render_work.watch(sync_evt_rx.clone());
    render_work.watch(ch.net_evt_rx.clone());
    render_work.watch(lifecycle_rx.clone());
    // QA-0161: the API command relay forwards to the DB thread off-frame (below).
    let db_cmd_tx_for_api = ch.db_cmd_tx.clone();
    let mut app = fe_runtime::app::build_app_with_options(
        BevyHandles {
            net_cmd_tx: ch.net_cmd_tx,
            net_evt_rx: ch.net_evt_rx,
            db_cmd_tx: ch.db_cmd_tx,
            db_res_rx: ch.db_res_rx,
            blob_store: Some(blob_store),
            on_blob_miss: Some(on_miss),
            lifecycle_rx: Some(lifecycle_rx),
            blob_fallback: blob_fallback.clone(),
        },
        window_chrome::primary_window(),
        AppOptions {
            close_when_requested: false,
            logging: LoggingOwnership::External,
        },
    );

    // The host thread always runs and stays inert; admitting third-party packages needs the
    // `plugin-host` feature AND FE_PLUGIN_HOST_ALLOW=1 (src/AGENTS.md §plugin-host).
    app.insert_resource(fe_plugin::PackageAdmission(
        cfg!(feature = "plugin-host") && fe_plugin::plugin_host_allowed_from_env(),
    ));
    // The bus judges callers against the node DID and feeds `host/node-changes`; the API channel is bound below.
    plugin_bridge::install_with(
        &mut app,
        plugin_bridge::ExtensionBridgeConfig {
            local_did: node_kp.to_did_key(),
            api_cmd_tx: None,
            scene_changes: Some(entity_change_tx.subscribe()),
            blob_fallback: Some(blob_fallback),
            // Read only, never generated here: the DB thread's device enrolment owns creating it (a second writer
            // could race it). Absent on a first run, so licensed builds wait for the next start (fail closed).
            device_key: fe_identity::load_device_keypair(
                &secret_store,
                fe_database::canon_log::device_enrolment::LOCAL_DEVICE_KEY_ACCOUNT,
            )
            .ok(),
            // The licensee a licence token request signs as (R-BIN-19); the same key as `local_did`.
            identity_key: Some(ed25519_dalek::SigningKey::from_bytes(&node_kp.seed_bytes())),
        },
    );
    // ingest.decode reads accepted payload files from the same content-addressed store.
    if let Some(slot) = app.world().get_resource::<plugin_bridge::ExtensionBlobSlot>() {
        slot.bind(blob_store_for_extensions);
    }
    app.add_systems(bevy::prelude::Startup, register_first_party_extension_ui);
    // The terrain package's slot commands run as editor actions (src/AGENTS.md section extension-host).
    app.add_plugins(extension_host::terrain_commands::TerrainCommandsPlugin);
    if let Some(results) = app.world().get_resource::<fe_plugin::PluginResultReceiver>() {
        // drain_plugin_results empties it every frame (src/AGENTS.md §render-loop).
        render_work.watch(results.0.lock().unwrap_or_else(|error| error.into_inner()).clone());
    }

    // GUI-only plugins (removed from fe-runtime so the headless relay can skip them)
    app.add_plugins(bevy_egui::EguiPlugin::default());
    // QA-0119: reactive, not continuous, in every build (src/AGENTS.md §render-loop).
    render_wake::install(&mut app, render_work.clone());
    app.add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default());

    // Insert sync resources into Bevy world
    app.insert_resource(fe_sync::SyncCommandSenderRes(sync_cmd_tx));
    app.insert_resource(fe_sync::SyncEventReceiverRes(Arc::new(Mutex::new(
        sync_evt_rx,
    ))));
    app.init_resource::<fe_sync::SyncStatus>();
    app.init_resource::<fe_sync::VersePeers>();
    // S7 (streamed tilesets): registering this makes `drain_sync_events`
    // duplicate advertisement/meta/chunk events for `terrain_bridge`'s
    // streamed-source consumer — see src/AGENTS.md §tileset-p2p.
    app.init_resource::<fe_sync::StreamedTilesetEventBuffer>();
    app.init_resource::<fe_sync::SceneAssetEventBuffer>();
    app.add_systems(bevy::prelude::Update, fe_sync::drain_sync_events);

    // Insert identity, peer, and secret store resources
    app.insert_resource(NodeIdentity::new(node_kp));
    app.insert_resource(PeerRegistry::default());
    app.insert_resource(LocalUserRole::default());
    app.insert_resource(fe_database::SecretStoreRes(secret_store));
    // Loud, dismissible startup warning when identity durability fell back
    // to the ephemeral tier (src/AGENTS.md §identity-tiers). `None` when
    // every launch used the keystore or the file tier, i.e. almost always.
    app.insert_resource(fe_ui::plugin::IdentityBanner {
        message: identity_banner_message,
    });

    // ---- API Gateway thread ----
    // Channel: API thread sends ApiCommand -> tx, Bevy drains from rx.
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(256);
    // The bus `host/query` and snapshot sources ride the same API channel (plugin_bridge §bus sources).
    if let Some(slot) = app.world().get_resource::<plugin_bridge::ExtensionApiSlot>() {
        slot.bind(api_cmd_tx.clone());
    }
    // Agent screenshot bridge: API thread -> Bevy capture (fe-api/AGENTS.md
    // §screenshot; the endpoint is on unless FE_API_SCREENSHOT_ALLOW=0).
    let (screenshot_tx, screenshot_rx) = fe_runtime::screenshot::screenshot_bridge();
    // Agent vision bridge: posed captures + camera read-back (src/AGENTS.md
    // §vision) — separate from the plain GET bridge above.
    let (vision_tx, vision_rx) = fe_runtime::vision::vision_bridge();
    // Agent input bridge: synthetic pointer/keyboard/scroll (src/AGENTS.md
    // §agent-input-surface; the endpoint is OFF unless FE_API_INPUT_ALLOW=1).
    let (input_tx, input_rx) = fe_runtime::input::input_bridge();
    let (qa_tx, qa_rx) = fe_runtime::qa_debug::qa_bridge();
    let (portal_fixture_tx, portal_fixture_rx) = fe_runtime::qa_portal::fixture_bridge();
    let (invitation_transfer_tx,invitation_transfer_rx)=fe_runtime::qa_invitation::invitation_bridge();
    render_work.watch(screenshot_rx.clone());
    render_work.watch(vision_rx.clone());
    render_work.watch(input_rx.clone());
    // QA-only bridges are drained only when QA is enabled; never probe an undrained queue.
    if qa_enabled {
        render_work.watch(qa_rx.clone());
        render_work.watch(portal_fixture_rx.clone());
        render_work.watch(invitation_transfer_rx.clone());
    }
    let (transform_broadcast_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::TransformUpdate>(1024);
    // Revocation broadcast: Bevy sends revoked JTIs, API thread updates its cache.
    let (revocation_tx, revocation_rx) = tokio::sync::broadcast::channel::<String>(64);

    // One shared cache; see AGENTS.md §entity-store-analytics.
    let entity_store = Arc::new(fe_entity_store::EntityStore::new());

    let (scene_change_tx_bevy, scene_change_rx_bevy) =
        crossbeam::channel::bounded::<fe_runtime::messages::SceneChange>(256);
    {
        let mut entity_change_rx = entity_change_tx.subscribe();
        maintenance_services.push(fe_runtime::service_lifecycle::OwnedService::spawn("scene-bridge", move |stop| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("scene change bridge runtime");
            rt.block_on(async move {
                tracing::info!("Scene change bridge started — feeding EntityStore");
                loop {
                    match tokio::select! { _ = stop.cancelled() => break, event = entity_change_rx.recv() => event } {
                        Ok(change) => match scene_change_tx_bevy.try_send(change) {
                            Ok(()) => {}
                            Err(crossbeam::channel::TrySendError::Full(_)) => {
                                tracing::warn!("Scene change bridge: channel full — dropping");
                            }
                            Err(crossbeam::channel::TrySendError::Disconnected(_)) => break,
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Scene change bridge lagged by {n}");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }));
    }

    // The dev-token writer resolves the local role per verse on its own clone.
    let dev_token_db = dev_token_out.as_ref().map(|_| shared_db.clone());
    // The portal resolver reads repository nodes on the same clone (src/AGENTS.md §portal-host).
    let portal_db = shared_db.clone();
    let (api_listening_tx, api_listening_rx) = crossbeam::channel::bounded(1);
    let (dev_listening_tx, dev_listening_rx) = crossbeam::channel::bounded(1);
    let (qa_listening_tx, qa_listening_rx) = crossbeam::channel::bounded(1);
    let (ui_listening_tx, ui_listening_rx) = crossbeam::channel::bounded(1);
    let notify_dev = dev_token_out.is_some();
    let notify_controls = qa_dialogs.is_some();
    {
        maintenance_services.push(fe_runtime::service_lifecycle::OwnedService::spawn("listener-fanout", move |stop| {
            let bound = loop {
                if stop.is_requested() { return; }
                match api_listening_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    Ok(bound) => break bound,
                    Err(crossbeam::channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam::channel::RecvTimeoutError::Disconnected) => return,
                }
            };
            {
                if ui_listening_tx.send(bound).is_err() {
                    tracing::warn!("UI listener notification was dropped");
                }
                if notify_dev && dev_listening_tx.send(bound).is_err() {
                    tracing::warn!("dev-token listener notification was dropped");
                }
                if notify_controls && qa_listening_tx.send(bound).is_err() {
                    tracing::warn!("QA-control listener notification was dropped");
                }
            }
        }));
    }
    api_endpoint::install(&mut app, ui_listening_rx);

    // Direct reader = clone of the DB thread's handle (never a second SurrealKV
    // open: the store lock refuses it, os error 33). Hydration failure leaves the
    // editor running with the analytics surface closed. See AGENTS.md §entity-store-analytics.
    let mut entity_store_for_api = Some(Arc::clone(&entity_store));
    let api_db_reader: Option<Arc<fe_api::db_reader::ReadDb>> = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("api_db_reader hydration runtime");
        match rt.block_on(fe_api::entity_store_bridge::hydrate_entity_store(
            &shared_db,
            &entity_store,
        )) {
            Ok(node_count) => {
                tracing::info!(
                    node_count,
                    "Hydrated EntityStore through the DB thread's shared SurrealKV handle"
                );
                Some(Arc::new(fe_api::db_reader::ReadDb::new(shared_db)))
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "Could not hydrate EntityStore; analytics API will remain unavailable"
                );
                entity_store_for_api = None;
                Some(Arc::new(fe_api::db_reader::ReadDb::new(shared_db)))
            }
        }
    };

    // ---- Terrain tileset registry (local .hexon store) ----
    let tileset_registry: Option<Arc<fe_terrain::tiles::TilesetRegistry>> =
        match fe_terrain::tiles::HexonStore::new() {
            Ok(store) => {
                let registry = fe_terrain::tiles::TilesetRegistry::new(store);
                let loaded = registry.load_all();
                tracing::info!("Tileset registry loaded {} tileset(s)", loaded.len());
                Some(Arc::new(registry))
            }
            Err(e) => {
                tracing::warn!("Hexon store unavailable — terrain tileset wiring skipped: {e}");
                None
            }
        };

    // Startup publish (S4 step 2): every tileset this peer was already
    // seeding before the restart gets re-bundled (cache-reused if its source
    // hash is unchanged) and re-published, so a peer that was offline sees
    // it advertised again without the user re-toggling anything. Spawned
    // (never blocks the GUI from opening) via the same shared helper
    // `drain_hexon_ops` uses at runtime.
    if let Some(ref registry) = tileset_registry {
        for installed in registry.get_seeding_tilesets() {
            terrain_bridge::spawn_bundle_and_publish(
                registry.clone(),
                sync_cmd_tx_for_seeding.clone(),
                installed.hexon_id,
            );
        }
    }

    // Scene hooks gate node writes from the GUI and every API door; installed before the API serves.
    if let Some(backend) = plugin_bridge::scene_hook_backend(&app) {
        match fe_runtime::scene_hook_gate::SceneHookGate::install(backend) {
            Ok(gate) => {
                app.insert_resource(gate);
            }
            Err(reason) => tracing::warn!(reason, "scene hook gate not installed"),
        }
    }

    maintenance_services.push(fe_api::spawn_api_service(fe_api::ApiConfig {
        bind_addr: format!("127.0.0.1:{qa_api_port}"),
        api_cmd_tx: api_cmd_tx.clone(),
        transform_broadcast_tx: transform_broadcast_tx.clone(),
        verifying_key: api_verifying_key,
        revocation_rx,
        blob_store: Some(blob_store_for_api),
        cors_origins: None, // defaults to localhost-only
        entity_change_tx: entity_change_tx.clone(),
        api_db_reader,
        entity_store: entity_store_for_api,
        tileset_registry: tileset_registry.clone(),
        hexon_registry: None,
        announcement_store: None,
        replication_tx: Some(repl_tx_for_api),
        // M2/F7 (A17): the API→sync distributed-query bridge's call sender —
        // every distributed surface (/api/v1/query's `distributed` mode, the
        // analytics endpoint's merged readings table, the MCP query_timeseries
        // tool) fans out through this seam. The bridge below forwards each
        // call into `SyncCommand::SubmitComputeTask` on the sync thread.
        distributed_tx: Some(distributed_call_tx),
        // F9/A20: never a sim lab — the process-global sim run lock / HLC
        // override would corrupt a live editing session (fe-sim §session).
        sim_control_tx: None,
        share_signer,
        // Raw-SQL elevated endpoint: off unless FE_API_ELEVATED_ALLOW=1 (read
        // once here; loud WARN when on) — fe-api/AGENTS.md §elevated-query.
        elevated_query_allowed: fe_api::elevated_query_allowed_from_env(),
        // Per-owner asset upload quota (default 2 GiB) — fe-api/AGENTS.md §asset-ingest.
        upload_quota_bytes: fe_api::upload_quota_bytes_from_env(),
        // Agent screenshot + vision surface: on unless FE_API_SCREENSHOT_ALLOW=0
        // (read once here; loud WARN when on) — fe-api/AGENTS.md §screenshot.
        screenshot: fe_api::screenshot::ScreenshotSurface::new(
            fe_api::screenshot::screenshot_allowed_from_env(),
            Some(screenshot_tx),
        )
        .with_vision(vision_tx),
        // Agent input surface: OFF unless FE_API_INPUT_ALLOW=1 (read once
        // here; loud WARN when on) — fe-api/AGENTS.md §agent-input-surface.
        input: fe_api::input::InputSurface::new(qa_enabled, Some(input_tx)).with_qa_debug(qa_tx).with_portal_fixture(portal_fixture_tx).with_invitation_transfer(invitation_transfer_tx),
        listening_tx: Some(api_listening_tx),
        extension_host: plugin_bridge::api_extension_host(&app),
        binding_host: plugin_bridge::api_binding_host(&app),
        import_hooks: plugin_bridge::import_hooks_handle(&app),
        // Usage log and metrics bind from env; relay adverts signed as the sync author (fe-api/AGENTS.md §usage-metering).
        network_ops: fe_api::network_ops::NetworkOps {
            advertisement_signer: Some(advertisement_signer),
            network_diagnostics: Some(network_diagnostics_tx),
            ..fe_api::network_ops::NetworkOps::from_env()
        },
    }));

    // Agent QA: write loopback-only API tokens for every local verse once the
    // API is listening, and rewrite on verse-set changes (src/AGENTS.md §qa-dev-token).
    // One client DID for this host's own tokens, shared with the portal host (R-API: one upload bucket, one `client`).
    let local_client_did = dev_token_out.as_ref().map(|_| fe_identity::new_client_did());
    if let (Some(path), Some(db), Some(keypair), Some(client_did)) =
        (dev_token_out, dev_token_db, dev_token_keypair, local_client_did.clone())
    {
        let (verse_ids_tx, verse_ids_rx) = crossbeam::channel::unbounded();
        let writer = fe_api::dev_token::DevTokenWriter {
            path,
            db,
            keypair,
            listening_rx: dev_listening_rx,
            verse_ids_rx,
        };
        match fe_api::dev_token::spawn_dev_token_service_as(writer, client_did) {
            Ok(service) => maintenance_services.push(service),
            Err(error) => tracing::error!(%error, "dev-token service could not start"),
        }
        qa_dev_token::install(&mut app, verse_ids_tx);
    }

    // Window chrome: taskbar/window icon + the title bar's brand mark.
    if let Some(icon) = window_chrome::brand_icon() {
        app.insert_resource(fe_ui::ui_shell::topbar::TopbarState::with_brand_icon(icon));
    }
    app.add_systems(bevy::prelude::Update, window_chrome::set_window_icon);
    app.add_systems(bevy::prelude::Update, window_chrome::sync_window_title);

    // Window captures: API bridge (agents) + F12 save-to-file (humans).
    screenshot_bridge::install(&mut app, screenshot_rx);
    app.init_resource::<screenshot_bridge::ScreenshotSaveResults>();
    app.add_systems(
        bevy::prelude::Update,
        (
            screenshot_bridge::capture_screenshot_on_f12,
            screenshot_bridge::surface_saved_screenshots,
        ),
    );
    // Agent vision: posed window/scene captures + camera read-back (src/AGENTS.md §vision).
    vision_bridge::install(&mut app, vision_rx);
    // Agent input: replay synthetic batches as winit-shaped messages ahead of
    // bevy_input + bevy_egui (src/AGENTS.md §agent-input-surface).
    input_bridge::install(&mut app, input_rx);
    native_accessibility::install(&mut app);
    qa_debug_bridge::install(
        &mut app,
        qa_enabled,
        qa_rx,
        qa_logs,
        qa_dialogs.clone(),
        qa_listening_rx,
    );
    qa_a11y_timing::install(&mut app, qa_enabled);
    qa_portal_bridge::install(&mut app, qa_enabled, portal_fixture_rx);
    qa_invitation_bridge::install(&mut app,qa_enabled,invitation_transfer_rx);

    // ---- Entity Store (in-memory hot cache) ----
    app.insert_resource(SharedEntityStore(entity_store));

    render_work.watch(scene_change_rx_bevy.clone());
    app.insert_resource(SceneChangeReceiver(scene_change_rx_bevy));

    app.insert_resource(fe_runtime::app::RevocationBroadcastSender(revocation_tx));
    // QA-0161: API commands reach the DB thread without waiting for a Bevy frame; replies
    // still return through the GUI's result dispatch (fe-runtime src/AGENTS.md §api-command-relay).
    let pending_api = PendingApiRequests::default();
    maintenance_services.push(fe_runtime::app::spawn_api_command_relay(
        api_cmd_rx,
        db_cmd_tx_for_api,
        pending_api.clone(),
    ));
    app.insert_resource(pending_api);
    app.insert_resource(ApiCommandSender(api_cmd_tx));
    app.insert_resource(TransformBroadcastSender(transform_broadcast_tx.clone()));

    // Bridge: tokio broadcast → crossbeam channel so Bevy can poll inbound
    // API transform updates without a tokio runtime.
    let (inbound_tx, inbound_rx) =
        crossbeam::channel::bounded::<fe_runtime::messages::TransformUpdate>(256);
    {
        let mut rx = transform_broadcast_tx.subscribe();
        maintenance_services.push(fe_runtime::service_lifecycle::OwnedService::spawn("transform-bridge", move |stop| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("inbound transform bridge runtime");
            rt.block_on(async move {
                tracing::info!("Transform bridge started — listening for broadcasts");
                loop {
                    match tokio::select! { _ = stop.cancelled() => break, event = rx.recv() => event } {
                        Ok(update) => {
                            tracing::debug!(
                                "Bridge recv: node={} pos=[{:.2},{:.2},{:.2}]",
                                update.node_id, update.position[0], update.position[1], update.position[2],
                            );
                            match inbound_tx.try_send(update) {
                                Ok(()) => {}
                                Err(crossbeam::channel::TrySendError::Full(_)) => {
                                    tracing::warn!("Bridge: inbound crossbeam channel full — dropping transform");
                                }
                                Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                                    tracing::error!("Bridge: inbound crossbeam channel disconnected");
                                    break;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("Bridge lagged by {n} transform broadcasts");
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
        }));
    }
    render_work.watch(inbound_rx.clone());
    app.insert_resource(fe_runtime::app::InboundTransformReceiver(inbound_rx));

    app.add_systems(
        bevy::prelude::Update,
        (
            fe_runtime::app::drain_api_commands,
            drain_scene_changes_to_store,
        ),
    );

    // Add 3D viewport (camera, grid, lighting, axis gizmo) and UI overlay
    app.add_plugins(fe_renderer::viewport::ViewportPlugin);
    // Persisted settings replace the plugin's defaults (init_resource keeps an
    // existing resource); SettingsPersistence enables the debounced save.
    app.insert_resource(app_settings);
    if let Some(path) = settings_path {
        app.insert_resource(fe_ui::settings::SettingsPersistence { path });
    }
    app.add_plugins(fe_ui::plugin::GardenerConsolePlugin);
    if qa_enabled {
        app.world_mut()
            .resource_mut::<fe_ui::actions::UiManager>()
            .file_dialogs = qa_dialogs
            .map(|dialogs| fe_ui::file_dialogs::FileDialogs(dialogs))
            .unwrap_or_else(fe_ui::file_dialogs::FileDialogs::disabled);
    }
    // Terrain runtime: petal map assignments + tileset registry bridge.
    app.add_plugins(fe_terrain::terrain_plugin::TerrainPlugin);
    // Splat view: synthesized terrain splats + Mesh/Splats/Hybrid view modes.
    app.add_plugins(fe_terrain::splat::SplatPlugin);
    if let Some(ref registry) = tileset_registry {
        app.insert_resource(fe_terrain::petal_binding::SharedTilesetRegistry(
            registry.clone(),
        ));
    }
    // S4: background assemble-result channel for `HexonOp::AssembleDownload`
    // (verify+assemble+install run off the Bevy main thread; only the
    // dialog refresh needs it — see `terrain_bridge::drain_hexon_assemble_results`).
    {
        let (assemble_tx, assemble_rx) = crossbeam::channel::unbounded();
        app.insert_resource(terrain_bridge::HexonAssembleResultSender(assemble_tx));
        app.insert_resource(terrain_bridge::HexonAssembleResults(assemble_rx));
    }
    // S7: background bundle-decode channel for the streamed (not-installed)
    // tileset path — mirrors the S4 assemble channel above. See
    // src/AGENTS.md §tileset-p2p.
    app.init_resource::<terrain_bridge::StreamedTilesetState>();
    // PAS-P5 S7a: TOFU publisher pins the install-flow two-phase download
    // gate learns from a verified header. See src/AGENTS.md §tileset-p2p.
    app.init_resource::<terrain_bridge::KnownTilesetPublishers>();
    app.init_resource::<terrain_bridge::InstallMetaVerdicts>();
    // W2b F1: the streamed (not-installed) path's own off-thread meta
    // verification verdicts (mirrors InstallMetaVerdicts above).
    app.init_resource::<terrain_bridge::StreamedMetaVerdicts>();
    app.init_resource::<fe_runtime::diag15m::ResidencyStats>();
    {
        let (streamed_tx, streamed_rx) = terrain_bridge::streamed_bundle_channel();
        app.insert_resource(streamed_tx);
        app.insert_resource(streamed_rx);
    }
    // QA-0119: off-thread terrain results wake the reactive loop like every other bridge.
    terrain_bridge::watch_results(app.world(), &render_work);
    app.add_systems(
        bevy::prelude::Update,
        (
            terrain_bridge::bridge_petal_terrain
                .before(fe_terrain::terrain_plugin::TerrainSystemSet::ApplyAssignments),
            terrain_bridge::drain_hexon_ops,
            terrain_bridge::drain_hexon_assemble_results,
            // Settings-section ruler toggles → fe-terrain RulerSettings.
            terrain_bridge::sync_ruler_settings,
            // S8: tile residency → fe-runtime ResidencyStats (Settings readout).
            terrain_bridge::mirror_tile_residency_stats,
            // S7: streamed-tileset event drain -> bundle decode -> demand.
            (
                terrain_bridge::drive_streamed_tilesets,
                terrain_bridge::drain_streamed_bundle_results,
                terrain_bridge::request_demanded_bundles,
            )
                .chain(),
        ),
    );
    // Same-pass shell geometry precedes the actual terrain HUD producer.
    terrain_bridge::install_ruler_hud(&mut app);
    // Global Offline tile mode overrides the petal's mode on the freshly built
    // composite before any chunk fetch. See src/AGENTS.md §tile-mode.
    app.add_systems(
        bevy::prelude::Update,
        terrain_bridge::apply_global_tile_mode
            .after(fe_terrain::petal_binding::apply_terrain_assignments)
            .before(fe_terrain::terrain_plugin::fetch_and_spawn_terrain_chunks),
    );
    // Mesh budget -> proposal-ghost allowance; PreUpdate so the terrain render
    // reads the watchdog's previous-frame count. See src/AGENTS.md §proposal-budget.
    app.add_systems(
        bevy::prelude::PreUpdate,
        terrain_bridge::sync_proposal_overlay_budget,
    );
    // Asset-download bridge: drains fe-ui's queued node-asset downloads, copying
    // resolved blobs into the user's downloads folder. See src/AGENTS.md §assets.
    app.insert_resource(asset_bridge::AssetBlobStore(blob_store_for_assets));
    app.add_systems(bevy::prelude::Update, asset_bridge::drain_asset_ops);
    // GPX import bridge: drains fe-ui's queued GPX imports into petal-bound
    // track + waypoint nodes. See src/AGENTS.md §gpx.
    app.init_resource::<gpx_bridge::PendingGpxImports>();
    // Path editor bridge: drains fe-ui's queued path edits (create/append/
    // remove/annotate/export/delete) and materializes persisted gpx_points
    // on petal load. See src/AGENTS.md §gpx and conductor/tracks/gpx_path_editor_20260711.
    app.init_resource::<gpx_bridge::PendingPathEdits>();
    app.add_systems(
        bevy::prelude::Update,
        (
            gpx_bridge::request_petal_gpx_materialization,
            gpx_bridge::advance_host_outcomes,
            gpx_bridge::drain_gpx_ops.run_if(gpx_bridge::path_write_access),
            gpx_bridge::advance_gpx_imports.run_if(gpx_bridge::path_write_access),
            gpx_bridge::drain_path_ops.run_if(gpx_bridge::path_write_access),
            gpx_bridge::advance_path_edits.run_if(gpx_bridge::path_projection_access),
            gpx_bridge::advance_path_materialization,
            // Tag rendered track ribbons selectable (bridges fe-terrain's
            // GpxTrackLine → fe-ui's SpawnedNodeMarker for the viewport picker).
            gpx_bridge::tag_track_lines_selectable.run_if(gpx_bridge::path_projection_access),
        ).chain().after(fe_ui::plugin::UiSet::PostSelection),
    );
    // WebView portal: inline wry overlay + petal portal lifecycle systems.
    app.add_plugins(fe_webview::plugin::WebViewPlugin);
    app.add_plugins(fe_webview::petal_portal::PetalPortalPlugin);
    // Portal bundles: resolver, bus broker and the Repository section's admission drains. After the extension
    // host's bus and governance, which it reads (src/AGENTS.md §portal-host).
    maintenance_services.push(portal_host::install(
        &mut app,
        portal_host::PortalHostConfig {
            db: portal_db,
            blobs: blob_store_for_portal,
            scene_changes: entity_change_tx.subscribe(),
            // The dev tokens' client DID: those tokens are this host's own for the summary read.
            local_client_did: local_client_did.clone(),
        },
        &render_work,
    ));

    terminal_reset::install(&mut app, maintenance_services, maintenance_jobs, db_thread, maintenance_token_path, qa_root);
    app.run();
}

// ---------------------------------------------------------------------------
// EntityStore bridge: scene change receiver + drain system
// ---------------------------------------------------------------------------

#[derive(bevy::prelude::Resource)]
struct SceneChangeReceiver(crossbeam::channel::Receiver<fe_runtime::messages::SceneChange>);

/// Arc-backed Bevy handle for the API and scene-change hot cache.
#[derive(bevy::prelude::Resource, Clone)]
struct SharedEntityStore(Arc<fe_entity_store::EntityStore>);

// ---------------------------------------------------------------------------
// Desktop relay-capable mode (NET-30): see src/AGENTS.md §desktop-relay
// ---------------------------------------------------------------------------

/// Self-test polls before giving up on a verdict (one every [`SELF_TEST_POLL_INTERVAL`]).
const SELF_TEST_POLL_ATTEMPTS: u32 = 24;
const SELF_TEST_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
/// Dial-back requests before the provisional relay is stopped (peers may open verses late).
#[cfg(feature = "relay-server")]
const DIALBACK_ATTEMPTS: u32 = 3;
#[cfg(feature = "relay-server")]
const DIALBACK_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(20);

/// Spawns the thread that gates, starts and confirms the desktop relay; it holds the relay for the process lifetime.
fn spawn_desktop_relay_gate(
    settings: fe_ui::settings::AppSettings,
    sync_command_sender: fe_sync::SyncCommandSender,
    db: fe_database::repo::Db,
) {
    let spawned = std::thread::Builder::new().name("desktop-relay-gate".into()).spawn(move || {
        // Multi-thread: the relay's listener and reconciler tasks must keep running after `maybe_start` returns.
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("desktop-relay")
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::warn!(%error, "desktop relay not started: runtime build failed");
                return;
            }
        };
        let feature_built = cfg!(feature = "relay-server");
        let verdict = if feature_built { wait_for_self_test_verdict(&sync_command_sender) } else { None };
        let hosted_verses = runtime.block_on(load_hosted_verse_ids(&db));
        // The dial-back probes the listening relay, so it can only run after a provisional start; a failed
        // confirmation stops the relay again (the pure gate still refuses everything before that point).
        let provisional = verdict.as_deref() == Some("public");
        let Some(handle) = relay_serve_bridge::maybe_start(
            runtime.handle(),
            &settings,
            verdict.as_deref(),
            provisional,
            hosted_verses,
        ) else {
            return;
        };
        if !confirm_dialback(&runtime) {
            tracing::warn!("desktop relay stopped: no verse peer confirmed it is reachable (dial-back)");
            drop(handle);
            return;
        }
        tracing::info!("desktop relay confirmed reachable by a verse peer; serving until exit");
        let _relay = handle;
        loop {
            std::thread::park();
        }
    });
    if let Err(error) = spawned {
        tracing::warn!(%error, "desktop relay gate thread did not start");
    }
}

/// Asks the sync thread for its reachability verdict until it leaves `pending`; `None` when it never does.
fn wait_for_self_test_verdict(sync_command_sender: &fe_sync::SyncCommandSender) -> Option<String> {
    for attempt in 0..SELF_TEST_POLL_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(SELF_TEST_POLL_INTERVAL);
        }
        let (reply, answer) = crossbeam::channel::bounded(1);
        if sync_command_sender
            .send(fe_sync::SyncCommand::GetNetworkDiagnostics { detail: false, reply })
            .is_err()
        {
            tracing::warn!("desktop relay gate: the sync thread is not accepting commands");
            return None;
        }
        match answer.recv_timeout(SELF_TEST_POLL_INTERVAL) {
            Ok(snapshot) if snapshot.node.reachability_verdict != "pending" => {
                return Some(snapshot.node.reachability_verdict);
            }
            Ok(_) | Err(_) => continue,
        }
    }
    None
}

/// Verse ids in the local store: the hosted set the embedded relay may serve (empty on a read error).
async fn load_hosted_verse_ids(db: &fe_database::repo::Db) -> std::collections::BTreeSet<String> {
    let read = async {
        let mut response = db.query("SELECT VALUE verse_id FROM verse").await?.check()?;
        anyhow::Ok(response.take::<Vec<String>>(0)?)
    };
    match read.await {
        Ok(verse_ids) => verse_ids.into_iter().collect(),
        Err(error) => {
            tracing::warn!(%error, "desktop relay gate could not list hosted verses");
            std::collections::BTreeSet::new()
        }
    }
}

/// Public `https://` URL peers dial for the desktop relay: the configured hostname plus a non-443 bind port.
#[cfg(feature = "relay-server")]
fn desktop_relay_public_url() -> Option<String> {
    use fe_sync::relay_server::{RELAY_SERVE_HOSTNAME_ENV, RELAY_SERVE_HTTPS_BIND_ENV};
    let hostname = std::env::var(RELAY_SERVE_HOSTNAME_ENV).ok()?.trim().to_owned();
    if hostname.is_empty() {
        return None;
    }
    let port = std::env::var(RELAY_SERVE_HTTPS_BIND_ENV)
        .ok()
        .and_then(|bind| bind.trim().parse::<std::net::SocketAddr>().ok())
        .map_or(443, |bind| bind.port());
    Some(if port == 443 { format!("https://{hostname}") } else { format!("https://{hostname}:{port}") })
}

/// True once a verse peer returns a signed dial-back record saying the relay URL answered.
#[cfg(feature = "relay-server")]
fn confirm_dialback(runtime: &tokio::runtime::Runtime) -> bool {
    let Some(url) = desktop_relay_public_url() else {
        tracing::warn!("desktop relay has no public hostname to confirm; set FE_RELAY_SERVE_HOSTNAME");
        return false;
    };
    for attempt in 0..DIALBACK_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(DIALBACK_RETRY_INTERVAL);
        }
        if let Some(record) = runtime.block_on(fe_sync::dialback_responder::request_dialback(&url)) {
            return record.ok;
        }
    }
    false
}

/// Without the relay server nothing is ever started, so there is nothing to confirm.
#[cfg(not(feature = "relay-server"))]
fn confirm_dialback(_runtime: &tokio::runtime::Runtime) -> bool {
    false
}

/// Startup system: the first-party terrain extension contributes its UI slots (compiled in, never a package; AC-2).
fn register_first_party_extension_ui(mut extensions: bevy::prelude::ResMut<fe_ui::plugin::UiExtensions>) {
    fe_terrain::extension::register_terrain_ui(&mut extensions.0);
}

#[cfg(test)]
mod extension_ui_tests {
    use super::*;
    use bevy::prelude::*;

    #[test]
    fn terrain_extension_appears_in_ui_registry_after_one_tick() {
        let mut app = App::new();
        app.init_resource::<fe_ui::plugin::UiExtensions>();
        app.add_systems(Startup, register_first_party_extension_ui);
        app.update();
        let registry = &app.world().resource::<fe_ui::plugin::UiExtensions>().0;
        assert_eq!(registry.total(), 3);
        let inspector = registry.get_by_slot(fe_sdk::UiSlot::InspectorSection);
        assert!(inspector.iter().any(|c| c.label == "Terrain Config"));
    }
}

/// Bevy system: drain scene change events from the bridge channel into the
/// `EntityStore` hot cache each frame (shared conversion in `fe_api::entity_store_bridge`).
fn drain_scene_changes_to_store(
    receiver: bevy::prelude::Res<SceneChangeReceiver>,
    store: bevy::prelude::Res<SharedEntityStore>,
    transforms: bevy::prelude::Res<TransformBroadcastSender>,
) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    while let Ok(change) = receiver.0.try_recv() {
        // QA-0087: a refused persist re-broadcasts the last-good transform, so
        // the GUI's optimistic preview (and every WS client) rolls back.
        if let fe_runtime::messages::SceneChange::TransformFailed { node_id, petal_id, position, rotation, scale } = &change {
            if transforms.0.send(fe_runtime::messages::TransformUpdate {
                node_id: node_id.clone(),
                petal_id: petal_id.clone(),
                position: *position,
                rotation: *rotation,
                scale: *scale,
                timestamp_ms: now_ms,
                source_did: String::new(),
            }).is_err() {
                tracing::debug!(node_id, "transform rollback had no subscribers");
            }
        }
        store.0.apply_scene_change(
            &fe_api::entity_store_bridge::runtime_scene_change_to_store(change),
            now_ms,
        );
    }
}
