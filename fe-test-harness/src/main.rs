//! FractalEngine P2P Mycelium Test Harness
//!
//! A standalone binary that runs functional tests for the P2P Mycelium features
//! in fractalengine. Each scenario spawns isolated peer instances (each with its
//! own in-memory DB, blob store, sync thread, and identity) and tests the full
//! P2P flow headlessly (no Bevy/GPU required).

mod fixtures;
mod peer;
mod scenarios;

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

    println!("\n=== FractalEngine P2P Mycelium Test Harness ===\n");

    let scenarios: Vec<(&str, fn() -> Result<TestResult>)> = vec![
        ("Blob Store Roundtrip", scenarios::blob_roundtrip::run),
        ("Legacy Migration", scenarios::migration::run),
        ("Invite Flow", scenarios::invite_flow::run),
        (
            "Verse Sync Infrastructure",
            scenarios::verse_sync::run,
        ),
        (
            "Two-Peer Blob Exchange",
            scenarios::two_peer_blob_exchange::run,
        ),
        (
            "Two-Peer Verse Join",
            scenarios::two_peer_verse_join::run,
        ),
        (
            "Two-Peer Sync Pipeline",
            scenarios::two_peer_sync_pipeline::run,
        ),
        (
            "API Token Flow",
            scenarios::api_token_flow::run,
        ),
        (
            "API Token Edge Cases",
            scenarios::api_token_flow::run_edge_cases,
        ),
    ];

    let mut passed = 0;
    let mut failed = 0;

    for (name, runner) in &scenarios {
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
