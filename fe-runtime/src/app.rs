use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bevy::asset::io::AssetSourceBuilder;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

use crate::bevy_blob_reader::OnMissCallback;
use crate::blob_store::BlobStoreHandle;
use crate::messages::{
    ApiCommand, DbCommand, DbResult, NetworkCommand, NetworkEvent, TransformUpdate,
    VerseHierarchyData,
};

#[derive(Resource)]
pub struct NetworkCommandSender(pub crossbeam::channel::Sender<NetworkCommand>);

#[derive(Resource)]
pub struct DbCommandSender(pub crossbeam::channel::Sender<DbCommand>);

#[derive(Resource)]
pub struct NetworkEventReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<NetworkEvent>>>);

#[derive(Resource)]
pub struct DbResultReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<DbResult>>>);

pub struct BevyHandles {
    pub net_cmd_tx: crossbeam::channel::Sender<NetworkCommand>,
    pub net_evt_rx: crossbeam::channel::Receiver<NetworkEvent>,
    pub db_cmd_tx: crossbeam::channel::Sender<DbCommand>,
    pub db_res_rx: crossbeam::channel::Receiver<DbResult>,
    /// Shared blob store handle for the `blob://` Bevy asset source.
    pub blob_store: Option<BlobStoreHandle>,
    /// Optional callback fired when the blob asset reader encounters a cache
    /// miss.  Set by the sync layer (Phase D) to trigger peer fetch.
    pub on_blob_miss: Option<OnMissCallback>,
}

pub fn build_app(handles: BevyHandles) -> App {
    let mut app = App::new();

    // Phase B: register the "blob" asset source BEFORE DefaultPlugins so Bevy
    // knows how to load `blob://{hash}.glb` paths via BlobAssetReader.
    if let Some(blob_store) = handles.blob_store {
        let on_miss = handles.on_blob_miss;
        app.register_asset_source(
            "blob",
            AssetSourceBuilder::new(move || match on_miss.clone() {
                Some(cb) => Box::new(crate::bevy_blob_reader::BlobAssetReader::with_on_miss(
                    blob_store.clone(),
                    cb,
                )),
                None => Box::new(crate::bevy_blob_reader::BlobAssetReader::new(
                    blob_store.clone(),
                )),
            }),
        );
        tracing::info!("Registered 'blob' asset source (BlobAssetReader)");
    }

    // Override AssetPlugin's file_path so Bevy loads assets from the same
    // directory the GLB import writer uses (`fractalengine/assets` relative
    // to CWD). We resolve it to an ABSOLUTE path to avoid Bevy's normal
    // CARGO_MANIFEST_DIR prefixing (which would yield
    // `fractalengine/fractalengine/assets`) and to avoid the exe-relative
    // fallback path when launched standalone from `target/{debug,release}/`.
    let asset_root = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .join("fractalengine")
        .join("assets");
    tracing::info!("Bevy AssetPlugin file_path = {}", asset_root.display());
    app.add_plugins(DefaultPlugins.set(bevy::asset::AssetPlugin {
        file_path: asset_root.to_string_lossy().into_owned(),
        ..Default::default()
    }));
    // EguiPlugin must be added after DefaultPlugins so Assets<Shader> exists in the World.
    app.add_plugins(bevy_egui::EguiPlugin::default());
    app.add_plugins(FrameTimeDiagnosticsPlugin::default());
    app.add_message::<NetworkEvent>();
    app.add_message::<DbResult>();
    app.insert_resource(NetworkCommandSender(handles.net_cmd_tx));
    app.insert_resource(DbCommandSender(handles.db_cmd_tx));
    app.insert_resource(NetworkEventReceiver(Arc::new(Mutex::new(
        handles.net_evt_rx,
    ))));
    app.insert_resource(DbResultReceiver(Arc::new(Mutex::new(handles.db_res_rx))));
    app.add_systems(Update, (drain_network_events, drain_db_results));
    app
}

fn drain_network_events(
    receiver: Res<NetworkEventReceiver>,
    mut writer: MessageWriter<NetworkEvent>,
) {
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(evt) = rx.try_recv() {
            writer.write(evt);
        }
    }
}

fn drain_db_results(receiver: Res<DbResultReceiver>, mut writer: MessageWriter<DbResult>) {
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(result) = rx.try_recv() {
            writer.write(result);
        }
    }
}

// ---------------------------------------------------------------------------
// API Gateway resources and drain system
// ---------------------------------------------------------------------------

/// Bevy resource: the receiver end of the API command channel.
#[derive(Resource)]
pub struct ApiCommandReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<ApiCommand>>>);

/// Bevy resource: sender end that the API thread clones for its use.
#[derive(Resource)]
pub struct ApiCommandSender(pub crossbeam::channel::Sender<ApiCommand>);

/// Bevy resource: broadcast sender for real-time transform fan-out.
#[derive(Resource, Clone)]
pub struct TransformBroadcastSender(pub tokio::sync::broadcast::Sender<TransformUpdate>);

/// Bevy resource: pending API requests awaiting DB results.
#[derive(Resource, Default)]
pub struct PendingApiRequests {
    pending: HashMap<u64, tokio::sync::oneshot::Sender<DbResult>>,
    pending_hierarchy: Vec<tokio::sync::oneshot::Sender<Vec<VerseHierarchyData>>>,
    next_id: u64,
}

impl PendingApiRequests {
    /// Enqueue a DB request and return a correlation ID.
    pub fn enqueue(&mut self, reply_tx: tokio::sync::oneshot::Sender<DbResult>) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.pending.insert(id, reply_tx);
        id
    }

    /// Try to deliver a DbResult to the first pending request.
    /// Returns true if a pending request consumed the result.
    pub fn try_deliver(&mut self, result: DbResult) -> bool {
        // Simple FIFO: deliver to the oldest pending request.
        if let Some((&id, _)) = self.pending.iter().next() {
            if let Some(tx) = self.pending.remove(&id) {
                let _ = tx.send(result);
                return true;
            }
        }
        false
    }

    pub fn enqueue_hierarchy(
        &mut self,
        reply_tx: tokio::sync::oneshot::Sender<Vec<VerseHierarchyData>>,
    ) {
        self.pending_hierarchy.push(reply_tx);
    }

    pub fn deliver_hierarchy(&mut self, data: Vec<VerseHierarchyData>) {
        for tx in self.pending_hierarchy.drain(..) {
            let _ = tx.send(data.clone());
        }
    }
}

/// Bevy system: drain API commands and forward to the DB command channel.
pub fn drain_api_commands(
    api_rx: Option<Res<ApiCommandReceiver>>,
    db_tx: Res<DbCommandSender>,
    mut pending: ResMut<PendingApiRequests>,
) {
    let Some(api_rx) = api_rx else { return };
    let Ok(rx) = api_rx.0.lock() else { return };
    // Drain up to 64 commands per frame to avoid stalling the main loop.
    for _ in 0..64 {
        match rx.try_recv() {
            Ok(ApiCommand::DbRequest { cmd, reply_tx }) => {
                pending.enqueue(reply_tx);
                let _ = db_tx.0.send(cmd);
            }
            Ok(ApiCommand::GetHierarchy { reply_tx }) => {
                pending.enqueue_hierarchy(reply_tx);
                let _ = db_tx.0.send(DbCommand::LoadHierarchy);
            }
            Ok(ApiCommand::SyncForward { .. }) => {
                // Transform sync forwarding handled via broadcast channel
            }
            Err(_) => break,
        }
    }
}
