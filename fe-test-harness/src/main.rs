//! FractalEngine P2P Mycelium Test Harness
//!
//! A standalone binary that runs functional tests for the P2P Mycelium features
//! in fractalengine. Each scenario spawns isolated peer instances (each with its
//! own in-memory DB, blob store, sync thread, and identity) and tests the full
//! P2P flow headlessly (no Bevy/GPU required).

mod fixtures;
mod scenarios;

// `peer` moved into the library (F8/M3 — the sim lab builds SimPeer on the
// same in-process peer model). Re-exported here so the scenario modules'
// `crate::peer::TestPeer` imports keep resolving inside the bin crate.
pub use fractalengine_test_harness::peer;

use anyhow::Result;

/// Result of a single test scenario.
#[derive(Debug)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

impl TestResult {
    pub fn pass(name: &str) -> Self {
        Self {
            name: name.to_string(),
            passed: true,
            message: String::new(),
        }
    }

    pub fn fail(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            passed: false,
            message: message.to_string(),
        }
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Loopback-only by design: every peer dials every other peer over
    // 127.0.0.1, so the n0-hosted relay servers contribute nothing but a
    // nondeterministic dial path (observed: relay-routed gossip joins failing
    // TLS auth and timing out mid-scenario, silently killing one peer's
    // request/response path for the whole run). `RelayMode::Disabled` keeps
    // every scenario on direct loopback dials (see fe-sync/src/AGENTS.md
    // §relay-health for the production config this deliberately overrides).
    std::env::set_var("FE_SYNC_RELAY", "disabled");

    println!("\n=== FractalEngine P2P Mycelium Test Harness ===\n");

    type Scenario = (&'static str, fn() -> Result<TestResult>);
    let scenarios: Vec<Scenario> = vec![
        ("Blob Store Roundtrip", scenarios::blob_roundtrip::run),
        ("Legacy Migration", scenarios::migration::run),
        ("Invite Flow", scenarios::invite_flow::run),
        ("Verse Sync Infrastructure", scenarios::verse_sync::run),
        (
            "Two-Peer Blob Exchange",
            scenarios::two_peer_blob_exchange::run,
        ),
        ("Two-Peer Verse Join", scenarios::two_peer_verse_join::run),
        (
            "Two-Peer Sync Pipeline",
            scenarios::two_peer_sync_pipeline::run,
        ),
        (
            "Two-Peer Real Replica Sync (A2)",
            scenarios::two_peer_replica_sync::run,
        ),
        (
            "Two-Peer Tombstone Sync (A5)",
            scenarios::two_peer_tombstone_sync::run,
        ),
        (
            "Two-Peer Timeseries Union Sync (A12)",
            scenarios::two_peer_timeseries_sync::run,
        ),
        (
            "Two-Peer Shard Fabric (A13/A14)",
            scenarios::two_peer_shard_fabric::run,
        ),
        (
            "Distributed Query Fan-out (A15/A16)",
            scenarios::distributed_query::run,
        ),
        ("API Token Flow", scenarios::api_token_flow::run),
        (
            "API Token Edge Cases",
            scenarios::api_token_flow::run_edge_cases,
        ),
        (
            "API-DB-Sync Cross-Thread (F14/T4)",
            scenarios::api_db_sync_cross_thread::run,
        ),
    ];

    let mut passed = 0;
    let mut failed = 0;

    // Optional subset filter for iterating on one scenario: a
    // case-insensitive substring of the scenario name (empty = all).
    let only = std::env::var("FE_HARNESS_SCENARIO")
        .unwrap_or_default()
        .to_lowercase();

    for (name, runner) in &scenarios {
        if !only.is_empty() && !name.to_lowercase().contains(&only) {
            continue;
        }
        print!("  [{name}] ...");
        match runner() {
            Ok(r) if r.passed => {
                println!(" PASS");
                passed += 1;
            }
            Ok(r) => {
                println!(" FAIL: {}", r.message);
                failed += 1;
            }
            Err(e) => {
                println!(" ERROR: {e:#}");
                failed += 1;
            }
        }
    }

    println!("\n  Results: {passed} passed, {failed} failed\n");
    std::process::exit(if failed > 0 { 1 } else { 0 });
}
