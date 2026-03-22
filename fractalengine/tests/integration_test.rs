use fe_runtime::messages::{DbCommand, DbResult, NetworkCommand, NetworkEvent};
use std::time::{Duration, Instant};

#[test]
fn test_thread_isolation_no_ambient_tokio() {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "No ambient Tokio runtime on the Bevy (main) thread"
    );
}

#[test]
fn test_network_round_trip_under_2ms() {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(256);
    let (evt_tx, evt_rx) = crossbeam::channel::bounded(256);

    let _handle = fe_network::spawn_network_thread(cmd_rx, evt_tx);
    evt_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("Network thread did not send Started");

    let t0 = Instant::now();
    cmd_tx.send(NetworkCommand::Ping).unwrap();
    let evt = evt_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("No Pong");
    let elapsed = t0.elapsed();

    assert!(matches!(evt, NetworkEvent::Pong));
    assert!(
        elapsed.as_millis() < 2,
        "Network round-trip {}ms > 2ms",
        elapsed.as_millis()
    );
}

#[test]
fn test_db_round_trip_under_5ms() {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(256);
    let (res_tx, res_rx) = crossbeam::channel::bounded(256);

    let _handle = fe_database::spawn_db_thread(cmd_rx, res_tx);
    res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("DB thread did not send Started");

    let t0 = Instant::now();
    cmd_tx.send(DbCommand::Ping).unwrap();
    let result = res_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("No Pong");
    let elapsed = t0.elapsed();

    assert!(matches!(result, DbResult::Pong));
    assert!(
        elapsed.as_millis() < 5,
        "DB round-trip {}ms > 5ms",
        elapsed.as_millis()
    );
}

#[test]
fn test_clean_shutdown_within_1s() {
    let (net_cmd_tx, net_cmd_rx) = crossbeam::channel::bounded(256);
    let (net_evt_tx, net_evt_rx) = crossbeam::channel::bounded(256);
    let (db_cmd_tx, db_cmd_rx) = crossbeam::channel::bounded(256);
    let (db_res_tx, db_res_rx) = crossbeam::channel::bounded(256);

    let net_handle = fe_network::spawn_network_thread(net_cmd_rx, net_evt_tx);
    let db_handle = fe_database::spawn_db_thread(db_cmd_rx, db_res_tx);

    net_evt_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("Network Started");
    db_res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("DB Started");

    let t0 = Instant::now();
    net_cmd_tx.send(NetworkCommand::Shutdown).unwrap();
    db_cmd_tx.send(DbCommand::Shutdown).unwrap();

    net_handle.join().expect("Network thread panicked");
    db_handle.join().expect("DB thread panicked");

    assert!(
        t0.elapsed() < Duration::from_secs(1),
        "Threads did not shut down within 1s"
    );
}

// DEFERRED TO WAVE 6 — requires headless Bevy runner
// #[test]
// fn test_frame_budget_under_20ms() { ... }
