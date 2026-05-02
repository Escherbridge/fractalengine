use std::sync::{Arc, Mutex};

use fe_identity::NodeIdentity;
use fe_runtime::app::{
    ApiCommandReceiver, ApiCommandSender, BevyHandles, PendingApiRequests,
    TransformBroadcastSender,
};
use fe_runtime::channels::ChannelHandles;
use fe_runtime::messages::DbCommand;
use fe_runtime::PeerRegistry;
use fe_ui::plugin::LocalUserRole;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let ch = ChannelHandles::new();

    let _net_thread = fe_network::spawn_network_thread(ch.net_cmd_rx, ch.net_evt_tx);

    // P2P Mycelium Phase A+B: local content-addressed blob store, shared with
    // the DB thread and the Bevy blob:// asset source.
    let blob_store: fe_database::BlobStoreHandle =
        std::sync::Arc::new(fe_sync::FsBlobStore::open_default().expect("open blob store"));

    // Secret store: OS keychain on desktop.
    let secret_store: Arc<dyn fe_identity::SecretStore> =
        Arc::new(fe_identity::OsKeystoreBackend::new());

    // Load or generate a persistent node keypair. The 32-byte seed is stored
    // in the secret store so the node identity survives across launches,
    // preserving P2P reconnection and invite verification.
    let node_kp = match load_or_generate_keypair(&secret_store) {
        Ok(kp) => kp,
        Err(e) => {
            tracing::warn!("Could not load/store keypair in secret store, generating ephemeral: {e}");
            fe_identity::NodeKeypair::generate()
        }
    };
    let iroh_secret = node_kp.to_iroh_secret();
    let local_did = node_kp.to_did_key();
    let api_verifying_key = node_kp.verifying_key();

    // Phase F: create a second keypair from the same seed for the DB thread.
    // NodeKeypair is not Clone, so we recreate from the same seed bytes.
    let db_keypair = fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes())
        .expect("recreate keypair from seed");

    // Phase E: replication channel (DB → sync bridge).
    // The DB thread emits ReplicationEvents; we bridge them to SyncCommand::WriteRowEntry
    // after the sync command sender is created below.
    let (repl_tx, repl_rx) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(256);

    // Scene change broadcast: DB thread emits CUD deltas, API thread fans out to WS clients.
    let (entity_change_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::SceneChange>(256);

    let _db_thread = match fe_database::spawn_db_thread_with_sync(
        ch.db_cmd_rx,
        ch.db_res_tx,
        blob_store.clone(),
        Some(repl_tx),
        Some(db_keypair),
        Some(secret_store.clone()),
        Some(entity_change_tx.clone()),
    ) {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!("Failed to start database thread: {}", e);
            eprintln!("Fatal error: Could not initialize database.\n{}", e);
            eprintln!("Please ensure the 'data/' directory is writable and the path is valid.");
            std::process::exit(1);
        }
    };

    // Send seed command so the DB populates initial data
    ch.db_cmd_tx.send(DbCommand::Seed).ok();

    // ---- P2P Mycelium Phase D: sync thread (iroh endpoint) ----
    let (sync_cmd_tx, sync_cmd_rx) = crossbeam::channel::bounded(256);
    let (sync_evt_tx, sync_evt_rx) = crossbeam::channel::bounded(256);
    tracing::info!(
        node_id = %iroh_secret.public(),
        "Starting sync thread"
    );

    let _sync_thread =
        fe_sync::spawn_sync_thread(iroh_secret, blob_store.clone(), sync_cmd_rx, sync_evt_tx, local_did);

    // Phase E: bridge replication events from DB thread to sync thread.
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

    // Wire the on_miss callback: when BlobAssetReader can't find a blob
    // locally, it sends a FetchBlob command to the sync thread.
    let sync_cmd_for_miss = sync_cmd_tx.clone();
    let on_miss: fe_runtime::bevy_blob_reader::OnMissCallback = Arc::new(move |hash| {
        // We don't know the verse_id at the asset-reader level, so use a
        // placeholder. Phase F will route through VersePeers instead.
        sync_cmd_for_miss
            .send(fe_sync::SyncCommand::FetchBlob {
                hash,
                verse_id: String::new(),
            })
            .ok();
    });

    let mut app = fe_runtime::app::build_app(BevyHandles {
        net_cmd_tx: ch.net_cmd_tx,
        net_evt_rx: ch.net_evt_rx,
        db_cmd_tx: ch.db_cmd_tx,
        db_res_rx: ch.db_res_rx,
        blob_store: Some(blob_store),
        on_blob_miss: Some(on_miss),
    });

    // GUI-only plugins (removed from fe-runtime so the headless relay can skip them)
    app.add_plugins(bevy_egui::EguiPlugin::default());
    app.add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default());

    // Insert sync resources into Bevy world
    app.insert_resource(fe_sync::SyncCommandSenderRes(sync_cmd_tx));
    app.insert_resource(fe_sync::SyncEventReceiverRes(Arc::new(Mutex::new(
        sync_evt_rx,
    ))));
    app.init_resource::<fe_sync::SyncStatus>();
    app.init_resource::<fe_sync::VersePeers>();
    app.add_systems(bevy::prelude::Update, fe_sync::drain_sync_events);

    // Insert identity, peer, and secret store resources
    app.insert_resource(NodeIdentity::new(node_kp));
    app.insert_resource(PeerRegistry::default());
    app.insert_resource(LocalUserRole::default());
    app.insert_resource(fe_database::SecretStoreRes(secret_store));

    // ---- API Gateway thread ----
    // Channel: API thread sends ApiCommand -> tx, Bevy drains from rx.
    let (api_cmd_tx, api_cmd_rx) = crossbeam::channel::bounded(256);
    let (transform_broadcast_tx, _) =
        tokio::sync::broadcast::channel::<fe_runtime::messages::TransformUpdate>(1024);
    // Revocation broadcast: Bevy sends revoked JTIs, API thread updates its cache.
    let (revocation_tx, revocation_rx) = tokio::sync::broadcast::channel::<String>(64);

    let _api_thread = fe_api::spawn_api_thread(fe_api::ApiConfig {
        bind_addr: "127.0.0.1:8765".to_string(),
        api_cmd_tx: api_cmd_tx.clone(),
        transform_broadcast_tx: transform_broadcast_tx.clone(),
        verifying_key: api_verifying_key,
        revocation_rx,
        blob_store: None,
        cors_origins: None, // defaults to localhost-only
        entity_change_tx: entity_change_tx.clone(),
    });

    app.insert_resource(fe_runtime::app::RevocationBroadcastSender(revocation_tx));
    app.insert_resource(ApiCommandReceiver(Arc::new(Mutex::new(api_cmd_rx))));
    app.insert_resource(ApiCommandSender(api_cmd_tx));
    app.insert_resource(TransformBroadcastSender(transform_broadcast_tx.clone()));

    // Bridge: tokio broadcast → crossbeam channel so Bevy can poll inbound
    // API transform updates without a tokio runtime.
    let (inbound_tx, inbound_rx) = crossbeam::channel::bounded::<fe_runtime::messages::TransformUpdate>(256);
    {
        let mut rx = transform_broadcast_tx.subscribe();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("inbound transform bridge runtime");
            rt.block_on(async move {
                tracing::info!("Transform bridge started — listening for broadcasts");
                loop {
                    match rx.recv().await {
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
        });
    }
    app.insert_resource(fe_runtime::app::InboundTransformReceiver(inbound_rx));

    app.init_resource::<PendingApiRequests>();
    app.add_systems(
        bevy::prelude::Update,
        fe_runtime::app::drain_api_commands,
    );

    // Add 3D viewport (camera, grid, lighting, axis gizmo) and UI overlay
    app.add_plugins(fe_renderer::viewport::ViewportPlugin);
    app.add_plugins(fe_ui::plugin::GardenerConsolePlugin);
    // WebView portal: inline wry overlay + petal portal lifecycle systems.
    app.add_plugins(fe_webview::plugin::WebViewPlugin);
    app.add_plugins(fe_webview::petal_portal::PetalPortalPlugin);

    app.run();
}

/// Load a node keypair from the secret store, or generate a new one and store it.
///
/// The 32-byte seed is stored as a 64-char hex string under the service
/// `"fractalengine"` with account `"node_keypair"`.
fn load_or_generate_keypair(
    store: &Arc<dyn fe_identity::SecretStore>,
) -> anyhow::Result<fe_identity::NodeKeypair> {
    match store.get("fractalengine", "node_keypair") {
        Ok(Some(seed_hex)) => {
            let seed_bytes = hex::decode(&seed_hex)
                .map_err(|e| anyhow::anyhow!("invalid keypair hex in secret store: {e}"))?;
            let seed_array: [u8; 32] = seed_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("secret store keypair seed is not 32 bytes"))?;
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
            tracing::info!("Generated and stored new node keypair in secret store");
            Ok(kp)
        }
        Err(e) => Err(anyhow::anyhow!("secret store get failed: {e}")),
    }
}
