use std::sync::{Arc, Mutex};

use fe_runtime::app::BevyHandles;
use fe_runtime::channels::ChannelHandles;
use fe_runtime::messages::DbCommand;
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
    // Load or generate a persistent node keypair. The 32-byte seed is stored
    // in the OS keyring so the node identity survives across launches,
    // preserving P2P reconnection and invite verification.
    let node_kp = match load_or_generate_keypair() {
        Ok(kp) => kp,
        Err(e) => {
            tracing::warn!("Could not load/store keypair in keyring, generating ephemeral: {e}");
            fe_identity::NodeKeypair::generate()
        }
    };
    let iroh_secret = node_kp.to_iroh_secret();

    // Phase F: create a second keypair from the same seed for the DB thread.
    // NodeKeypair is not Clone, so we recreate from the same seed bytes.
    let db_keypair = fe_identity::NodeKeypair::from_bytes(&node_kp.seed_bytes())
        .expect("recreate keypair from seed");

    // Phase E: replication channel (DB → sync bridge).
    // The DB thread emits ReplicationEvents; we bridge them to SyncCommand::WriteRowEntry
    // after the sync command sender is created below.
    let (repl_tx, repl_rx) = crossbeam::channel::bounded::<fe_database::ReplicationEvent>(256);

    let _db_thread = fe_database::spawn_db_thread_with_sync(
        ch.db_cmd_rx,
        ch.db_res_tx,
        blob_store.clone(),
        Some(repl_tx),
        Some(db_keypair),
    );

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
        fe_sync::spawn_sync_thread(iroh_secret, blob_store.clone(), sync_cmd_rx, sync_evt_tx);

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

    // Insert sync resources into Bevy world
    app.insert_resource(fe_sync::SyncCommandSenderRes(sync_cmd_tx));
    app.insert_resource(fe_sync::SyncEventReceiverRes(Arc::new(Mutex::new(
        sync_evt_rx,
    ))));
    app.init_resource::<fe_sync::SyncStatus>();
    app.init_resource::<fe_sync::VersePeers>();
    app.add_systems(bevy::prelude::Update, fe_sync::drain_sync_events);

    // Add 3D viewport (camera, grid, lighting, axis gizmo) and UI overlay
    app.add_plugins(fe_renderer::viewport::ViewportPlugin);
    app.add_plugins(fe_ui::plugin::GardenerConsolePlugin);
    // WebView portal: inline wry overlay + petal portal lifecycle systems.
    app.add_plugins(fe_webview::plugin::WebViewPlugin);
    app.add_plugins(fe_webview::petal_portal::PetalPortalPlugin);

    app.run();
}

/// Load a node keypair from the OS keyring, or generate a new one and store it.
///
/// The 32-byte seed is stored as a 64-char hex string under the service
/// `"fractalengine"` with username `"node_keypair"`.
fn load_or_generate_keypair() -> anyhow::Result<fe_identity::NodeKeypair> {
    let entry = keyring::Entry::new("fractalengine", "node_keypair")
        .map_err(|e| anyhow::anyhow!("keyring entry creation failed: {e}"))?;

    match entry.get_password() {
        Ok(seed_hex) => {
            let seed_bytes = hex::decode(&seed_hex)
                .map_err(|e| anyhow::anyhow!("invalid keypair hex in keyring: {e}"))?;
            let seed_array: [u8; 32] = seed_bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("keyring keypair seed is not 32 bytes"))?;
            let kp = fe_identity::NodeKeypair::from_bytes(&seed_array)?;
            tracing::info!("Loaded node keypair from keyring");
            Ok(kp)
        }
        Err(keyring::Error::NoEntry) => {
            let kp = fe_identity::NodeKeypair::generate();
            let seed_hex = hex::encode(kp.seed_bytes());
            entry
                .set_password(&seed_hex)
                .map_err(|e| anyhow::anyhow!("keyring set_password failed: {e}"))?;
            tracing::info!("Generated and stored new node keypair in keyring");
            Ok(kp)
        }
        Err(e) => Err(anyhow::anyhow!("keyring get_password failed: {e}")),
    }
}
