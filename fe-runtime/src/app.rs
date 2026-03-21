use std::sync::{Arc, Mutex};

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

use crate::messages::{DbCommand, DbResult, NetworkCommand, NetworkEvent};

#[derive(Resource)]
pub struct NetworkCommandSender(pub crossbeam::channel::Sender<NetworkCommand>);

#[derive(Resource)]
pub struct DbCommandSender(pub crossbeam::channel::Sender<DbCommand>);

#[derive(Resource)]
pub struct NetworkEventReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<NetworkEvent>>>);

#[derive(Resource)]
pub struct DbResultReceiver(pub Arc<Mutex<crossbeam::channel::Receiver<DbResult>>>);

/// Bevy-side channel handles — the Tx ends that Bevy sends commands on,
/// and the Rx ends that Bevy drains events from each frame.
/// Thread-side Rx/Tx ends are passed directly to spawn_network_thread /
/// spawn_db_thread in main.rs before build_app is called.
pub struct BevyHandles {
    pub net_cmd_tx: crossbeam::channel::Sender<NetworkCommand>,
    pub net_evt_rx: crossbeam::channel::Receiver<NetworkEvent>,
    pub db_cmd_tx: crossbeam::channel::Sender<DbCommand>,
    pub db_res_rx: crossbeam::channel::Receiver<DbResult>,
}

pub fn build_app(handles: BevyHandles) -> App {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    app.add_plugins(FrameTimeDiagnosticsPlugin);
    app.add_event::<NetworkEvent>();
    app.add_event::<DbResult>();
    app.insert_resource(NetworkCommandSender(handles.net_cmd_tx));
    app.insert_resource(DbCommandSender(handles.db_cmd_tx));
    app.insert_resource(NetworkEventReceiver(Arc::new(Mutex::new(handles.net_evt_rx))));
    app.insert_resource(DbResultReceiver(Arc::new(Mutex::new(handles.db_res_rx))));
    app.add_systems(Update, (drain_network_events, drain_db_results));
    app
}

fn drain_network_events(
    receiver: Res<NetworkEventReceiver>,
    mut writer: EventWriter<NetworkEvent>,
) {
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(evt) = rx.try_recv() {
            writer.write(evt);
        }
    }
}

fn drain_db_results(receiver: Res<DbResultReceiver>, mut writer: EventWriter<DbResult>) {
    if let Ok(rx) = receiver.0.lock() {
        while let Ok(result) = rx.try_recv() {
            writer.write(result);
        }
    }
}
