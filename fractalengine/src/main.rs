use fe_runtime::app::BevyHandles;
use fe_runtime::channels::ChannelHandles;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let ch = ChannelHandles::new();

    let _net_thread = fe_network::spawn_network_thread(ch.net_cmd_rx, ch.net_evt_tx);
    let _db_thread = fe_database::spawn_db_thread(ch.db_cmd_rx, ch.db_res_tx);

    let mut app = fe_runtime::app::build_app(BevyHandles {
        net_cmd_tx: ch.net_cmd_tx,
        net_evt_rx: ch.net_evt_rx,
        db_cmd_tx: ch.db_cmd_tx,
        db_res_rx: ch.db_res_rx,
    });

    app.run();
}
