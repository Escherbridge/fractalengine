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
    let _db_thread = fe_database::spawn_db_thread(ch.db_cmd_rx, ch.db_res_tx);

    // Send seed command so the DB populates initial data
    ch.db_cmd_tx.send(DbCommand::Seed).ok();

    let mut app = fe_runtime::app::build_app(BevyHandles {
        net_cmd_tx: ch.net_cmd_tx,
        net_evt_rx: ch.net_evt_rx,
        db_cmd_tx: ch.db_cmd_tx,
        db_res_rx: ch.db_res_rx,
    });

    // Add UI console and a camera so we can see the panels
    app.add_plugins(fe_ui::plugin::GardenerConsolePlugin);
    app.add_systems(bevy::prelude::Startup, setup_camera);

    app.run();
}

fn setup_camera(mut commands: bevy::prelude::Commands) {
    commands.spawn(bevy::prelude::Camera2d);
}
