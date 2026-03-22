use fe_runtime::messages::{NetworkCommand, NetworkEvent};
use std::time::{Duration, Instant};

#[test]
fn test_thread_isolation_no_ambient_tokio() {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "No ambient Tokio runtime on the Bevy (main) thread"
    );
}

#[test]
fn test_network_round_trip() {
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
        elapsed.as_millis() < 50,
        "Network round-trip {}ms > 50ms",
        elapsed.as_millis()
    );

    cmd_tx.send(NetworkCommand::Shutdown).unwrap();
    _handle.join().expect("Network thread panicked");
}

// DB round-trip and shutdown tested in fe-database/tests/db_test.rs
// Duplicating here causes SurrealDB LOCK file conflicts since
// spawn_db_thread hardcodes the path. Parameterized path deferred.
