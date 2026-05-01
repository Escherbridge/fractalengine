use std::sync::{Arc, Mutex};
use std::time::Duration;

use fe_runtime::app::{
    ApiCommandReceiver, ApiCommandSender, BevyHandles, PendingApiRequests,
    RevocationBroadcastSender, TransformBroadcastSender,
};
use fe_runtime::channels::ChannelHandles;
use fe_runtime::messages::DbCommand;
use bevy::prelude::PluginGroup;
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting FractalEngine Relay (headless)");

    let ch = ChannelHandles::new();

    let _net_thread = fe_network::spawn_network_thread(ch.net_cmd_rx, ch.net_evt_tx);

    let blob_store: fe_database::BlobStoreHandle =
        Arc::new(fe_sync::FsBlobStore::open_default().expect("open blob store"));

    // Relay uses EnvBackend — secrets come from environment variables
    let secret_store: Arc<dyn fe_identity::SecretStore> =
        Arc::new(fe_identity::EnvBackend::new());

    let node_kp = match load_or_generate_keypair(&secret_store) {
        Ok(kp) => kp,
        Err(e) => {
            tracing::warn!("Could not load/store keypair, generating ephemeral: {e}");
            fe_identity::NodeKeypair::generate()
        }
    };

    let iroh_secret = node_kp.to_iroh_secret();
    let local_did = node_kp.to_did_key();
    let api_verifying_key = node_kp.verifying_key();

    let db_keypair = fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes())
        .expect("recreate keypair from seed");

    let (repl_tx, repl_rx) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(256);

    let _db_path = std::env::var("FE_DB_PATH").unwrap_or_else(|_| "data/fractalengine.db".into());

    // Scene change broadcast: DB thread emits CUD deltas, API thread fans out to WS clients.
    let (entity_change_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::SceneChange>(256);

    // NOTE: spawn_db_thread_with_sync uses the default db path internally.
    // Custom db_path support will come in Phase 4.
    let _db_thread = fe_database::spawn_db_thread_with_sync(
        ch.db_cmd_rx,
        ch.db_res_tx,
        blob_store.clone(),
        Some(repl_tx),
        Some(db_keypair),
        Some(secret_store.clone()),
        Some(entity_change_tx.clone()),
    );

    ch.db_cmd_tx.send(DbCommand::Seed).ok();

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
    );

    // Replication bridge
    {
        let sync_tx_for_repl = sync_cmd_tx.clone();
        std::thread::spawn(move || {
            while let Ok(evt) = repl_rx.recv() {
                sync_tx_for_repl
                    .send(fe_sync::SyncCommand::WriteRowEntry {
                        verse_id: evt.verse_id,
                        table: evt.table,
                        record_id: evt.record_id,
                        content_hash: evt.content_hash,
                    })
                    .ok();
            }
        });
    }

    // Clone db_cmd_tx before it moves into BevyHandles — needed for graceful shutdown.
    let db_cmd_tx_for_shutdown = ch.db_cmd_tx.clone();

    // Build headless Bevy app
    let mut app = bevy::app::App::new();
    app.add_plugins(bevy::MinimalPlugins.set(
        bevy::app::ScheduleRunnerPlugin::run_loop(Duration::from_millis(50)),
    ));

    fe_runtime::app::setup_core_systems(
        &mut app,
        BevyHandles {
            net_cmd_tx: ch.net_cmd_tx,
            net_evt_rx: ch.net_evt_rx,
            db_cmd_tx: ch.db_cmd_tx,
            db_res_rx: ch.db_res_rx,
            blob_store: None,
            on_blob_miss: None,
        },
    );

    // Graceful shutdown: listen for SIGINT/SIGTERM and send DbCommand::Shutdown.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            tokio::signal::ctrl_c().await.ok();
            tracing::info!("Received shutdown signal, shutting down gracefully...");
            db_cmd_tx_for_shutdown.send(DbCommand::Shutdown).ok();
        });
    });

    // Sync resources
    app.insert_resource(fe_sync::SyncCommandSenderRes(sync_cmd_tx));
    app.insert_resource(fe_sync::SyncEventReceiverRes(Arc::new(Mutex::new(
        sync_evt_rx,
    ))));
    app.init_resource::<fe_sync::SyncStatus>();
    app.init_resource::<fe_sync::VersePeers>();
    app.add_systems(bevy::prelude::Update, fe_sync::drain_sync_events);

    // Secret store resource
    app.insert_resource(fe_database::SecretStoreRes(secret_store));

    // API Gateway thread
    let bind_addr = std::env::var("FE_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8765".into());
    let cors_origins: Vec<String> = std::env::var("FE_CORS_ORIGINS")
        .map(|s| s.split(',').map(|o| o.trim().to_string()).collect())
        .unwrap_or_else(|_| vec!["*".to_string()]);

    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(256);
    let (transform_broadcast_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::TransformUpdate>(1024);
    let (revocation_tx, revocation_rx) = tokio::sync::broadcast::channel::<String>(64);

    let _api_thread = fe_api::spawn_api_thread(fe_api::ApiConfig {
        bind_addr,
        api_cmd_tx: api_cmd_tx.clone(),
        transform_broadcast_tx: transform_broadcast_tx.clone(),
        verifying_key: api_verifying_key,
        revocation_rx,
        blob_store: None,
        cors_origins: Some(cors_origins),
        entity_change_tx,
    });

    app.insert_resource(RevocationBroadcastSender(revocation_tx));
    app.insert_resource(ApiCommandReceiver(Arc::new(Mutex::new(api_cmd_rx))));
    app.insert_resource(ApiCommandSender(api_cmd_tx));
    app.insert_resource(TransformBroadcastSender(transform_broadcast_tx));
    app.init_resource::<PendingApiRequests>();
    app.add_systems(
        bevy::prelude::Update,
        fe_runtime::app::drain_api_commands,
    );

    tracing::info!("Relay ready -- entering headless event loop");
    app.run();
    Ok(())
}

/// Load a node keypair from the secret store, or generate and store a new one.
fn load_or_generate_keypair(
    store: &Arc<dyn fe_identity::SecretStore>,
) -> anyhow::Result<fe_identity::NodeKeypair> {
    match store.get("fractalengine", "node_keypair") {
        Ok(Some(seed_hex)) => {
            let seed_bytes = hex::decode(&seed_hex)
                .map_err(|e| anyhow::anyhow!("invalid keypair hex: {e}"))?;
            let seed_array: [u8; 32] = seed_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("keypair seed is not 32 bytes"))?;
            let kp = fe_identity::NodeKeypair::from_bytes(&seed_array)?;
            tracing::info!("Loaded node keypair from secret store");
            Ok(kp)
        }
        Ok(None) => {
            let kp = fe_identity::NodeKeypair::generate();
            let seed_hex = hex::encode(kp.seed_bytes());
            store
                .set("fractalengine", "node_keypair", &seed_hex)
                .map_err(|e| anyhow::anyhow!("secret store set failed: {e}"))?;
            tracing::info!("Generated and stored new node keypair");
            Ok(kp)
        }
        Err(e) => Err(anyhow::anyhow!("secret store get failed: {e}")),
    }
}
