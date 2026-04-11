use std::sync::{Arc, Mutex};

use bevy::asset::io::AssetSourceBuilder;
use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

use crate::bevy_blob_reader::OnMissCallback;
use crate::blob_store::BlobStoreHandle;
use crate::messages::{DbCommand, DbResult, NetworkCommand, NetworkEvent};

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
