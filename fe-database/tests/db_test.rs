use fe_runtime::messages::{DbCommand, DbResult};
use std::time::{Duration, Instant};

#[test]
fn test_db_thread_isolation() {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "No ambient Tokio runtime should exist on the test (main) thread"
    );
}

#[test]
fn test_db_ping_pong_roundtrip() {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(256);
    let (res_tx, res_rx) = crossbeam::channel::bounded(256);

    let _handle = fe_database::spawn_db_thread(cmd_rx, res_tx);

    // Wait for Started — SurrealDB init may take a moment
    let started = res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("DB thread did not start");
    assert!(matches!(started, DbResult::Started));

    let t0 = Instant::now();
    cmd_tx.send(DbCommand::Ping).unwrap();
    let result = res_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("No Pong received");
    let elapsed = t0.elapsed();

    assert!(matches!(result, DbResult::Pong));
    assert!(
        elapsed.as_millis() < 5,
        "DB round-trip {}ms exceeded 5ms budget",
        elapsed.as_millis()
    );
}

#[test]
fn test_db_clean_shutdown() {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(256);
    let (res_tx, res_rx) = crossbeam::channel::bounded(256);

    let handle = fe_database::spawn_db_thread(cmd_rx, res_tx);

    res_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("Started");

    cmd_tx.send(DbCommand::Shutdown).unwrap();
    handle.join().expect("DB thread panicked");
}
