use fe_runtime::messages::{NetworkCommand, NetworkEvent};
use std::time::{Duration, Instant};

#[test]
fn test_network_thread_isolation() {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "No ambient Tokio runtime should exist on the test (main) thread"
    );
}

#[test]
fn test_network_ping_pong_roundtrip() {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(256);
    let (evt_tx, evt_rx) = crossbeam::channel::bounded(256);

    let _handle = fe_network::spawn_network_thread(cmd_rx, evt_tx);

    // Wait for Started
    let started = evt_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("Network thread did not start");
    assert!(matches!(started, NetworkEvent::Started));

    let t0 = Instant::now();
    cmd_tx.send(NetworkCommand::Ping).unwrap();
    let evt = evt_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("No Pong received");
    let elapsed = t0.elapsed();

    assert!(matches!(evt, NetworkEvent::Pong));
    assert!(
        elapsed.as_millis() < 2,
        "Network round-trip {}ms exceeded 2ms budget",
        elapsed.as_millis()
    );
}

#[test]
fn test_network_clean_shutdown() {
    let (cmd_tx, cmd_rx) = crossbeam::channel::bounded(256);
    let (evt_tx, evt_rx) = crossbeam::channel::bounded(256);

    let handle = fe_network::spawn_network_thread(cmd_rx, evt_tx);

    evt_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("Started");

    cmd_tx.send(NetworkCommand::Shutdown).unwrap();
    handle.join().expect("Network thread panicked");
}
