//! Mirrors the actual API listener address into the runtime resource.

use bevy::prelude::*;
use fe_runtime::app::RuntimeApiEndpoint;

#[derive(Resource)]
struct ListeningReceiver(crossbeam::channel::Receiver<std::net::SocketAddr>);

pub(crate) fn install(app: &mut App, receiver: crossbeam::channel::Receiver<std::net::SocketAddr>) {
    app.init_resource::<RuntimeApiEndpoint>();
    app.insert_resource(ListeningReceiver(receiver));
    app.add_systems(Update, receive_endpoint);
}

fn receive_endpoint(receiver: Res<ListeningReceiver>, mut endpoint: ResMut<RuntimeApiEndpoint>) {
    for address in receiver.0.try_iter() {
        if endpoint.0 != Some(address) {
            endpoint.0 = Some(address);
        }
    }
}
