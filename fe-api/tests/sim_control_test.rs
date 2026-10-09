//! Integration tests for the F9/A20 sim control surface: `/api/v1/sim/*`
//! and the MCP `sim_*` tools, through the full router (real auth).
//!
//! The seam under test is `ApiState.sim_control_tx`: these tests own the
//! receiver and answer `SimControlCall`s the way the fe-sim bridge would
//! (a one-session state machine with canned payloads), asserting what the
//! surface forwarded and how it maps bridge outcomes. The REAL bridge +
//! session are proven in fe-sim (`control.rs` / `session.rs` tests).

use std::sync::{Arc, Mutex};

use axum::http::StatusCode;
use serde_json::json;

use fe_runtime::sim_control::{
    sim_control_channel, SimControlCallReceiver, SimControlCommand, SimControlError,
    SimControlErrorKind,
};
use fractalengine_test_harness::api::ApiHarness;

type Seen = Arc<Mutex<Vec<SimControlCommand>>>;

/// A stand-in bridge: one session at a time, canned payloads, every
/// command recorded. Exits when the harness (sender) drops.
fn spawn_bridge_emulator(rx: SimControlCallReceiver) -> Seen {
    let seen: Seen = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut running = false;
        while let Ok(call) = rx.recv() {
            log.lock().unwrap().push(call.cmd.clone());
            let reply = match call.cmd {
                SimControlCommand::Start { .. } if running => Err(SimControlError::new(
                    SimControlErrorKind::Conflict,
                    "a sim session is already running ('emu') — stop it first",
                )),
                SimControlCommand::Start { script } => {
                    running = true;
                    Ok(json!({ "started": script }))
                }
                SimControlCommand::Stop if running => {
                    running = false;
                    Ok(json!({ "ingested": 3 }))
                }
                SimControlCommand::Status => Ok(json!({ "running": running })),
                SimControlCommand::Step { n } if running => Ok(json!({ "executed": n })),
                SimControlCommand::InjectFault { event } if running => {
                    Ok(json!({ "at_ms": 0, "event": event }))
                }
                _ => Err(SimControlError::new(
                    SimControlErrorKind::NotRunning,
                    "no sim session is running — start one first",
                )),
            };
            let _ = call.reply_tx.send(reply);
        }
    });
    seen
}

async fn harness_with_bridge() -> (ApiHarness, Seen) {
    let (tx, rx) = sim_control_channel();
    let seen = spawn_bridge_emulator(rx);
    let h = ApiHarness::spawn_with_sim_control_tx(Some(tx))
        .await
        .expect("spawn harness");
    (h, seen)
}

/// Every verb as (method, path, body) — the REST inventory.
fn verbs() -> Vec<(&'static str, &'static str, serde_json::Value)> {
    vec![
        ("POST", "/api/v1/sim/start", json!({ "script": "default" })),
        ("GET", "/api/v1/sim/status", json!(null)),
        ("POST", "/api/v1/sim/step", json!({ "n": 3 })),
        (
            "POST",
            "/api/v1/sim/inject-fault",
            json!({ "event": { "kind": "peer_offline", "peer": "bob" } }),
        ),
        ("POST", "/api/v1/sim/stop", json!({})),
    ]
}

async fn call(
    h: &ApiHarness,
    method: &str,
    path: &str,
    token: &str,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    if method == "GET" {
        h.get(path, Some(token)).await
    } else {
        h.post_json(path, Some(token), body).await
    }
}

/// Owner happy path: each verb round-trips through the seam, the command
/// arrives exactly as sent, the payload comes back in the envelope, and a
/// bridge Conflict maps to 409.
#[tokio::test]
async fn owner_drives_every_verb_through_the_seam() {
    let (h, seen) = harness_with_bridge().await;
    let owner = h.mint_token("VERSE#lab", "owner");

    for (method, path, body) in verbs() {
        let (status, resp) = call(&h, method, path, &owner, &body).await;
        assert_eq!(status, StatusCode::OK, "{path}: {resp}");
        assert_eq!(resp["ok"], true, "{path}: {resp}");
        match path {
            "/api/v1/sim/start" => assert_eq!(resp["data"]["started"], "default"),
            "/api/v1/sim/status" => assert_eq!(resp["data"]["running"], true),
            "/api/v1/sim/step" => assert_eq!(resp["data"]["executed"], 3),
            "/api/v1/sim/inject-fault" => assert_eq!(resp["data"]["event"]["peer"], "bob"),
            _ => assert_eq!(resp["data"]["ingested"], 3),
        }
    }
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            SimControlCommand::Start {
                script: json!("default")
            },
            SimControlCommand::Status,
            SimControlCommand::Step { n: 3 },
            SimControlCommand::InjectFault {
                event: json!({ "kind": "peer_offline", "peer": "bob" })
            },
            SimControlCommand::Stop,
        ],
        "each verb forwards exactly its command"
    );

    // Bridge-typed failures keep their meaning: double start → 409.
    let start = json!({ "script": "default" });
    call(&h, "POST", "/api/v1/sim/start", &owner, &start).await;
    let (status, resp) = call(&h, "POST", "/api/v1/sim/start", &owner, &start).await;
    assert_eq!(status, StatusCode::CONFLICT, "{resp}");
    assert_eq!(resp["ok"], false);
    assert!(resp["error"].as_str().unwrap().contains("already running"));

    // Out-of-range step is rejected at the API — it never reaches the seam.
    let before = seen.lock().unwrap().len();
    let (status, _) = call(&h, "POST", "/api/v1/sim/step", &owner, &json!({ "n": 0 })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        seen.lock().unwrap().len(),
        before,
        "rejected before the seam"
    );
}

/// Non-owners are refused (403) on every verb and nothing reaches the seam.
#[tokio::test]
async fn non_owner_is_forbidden_before_the_seam() {
    let (h, seen) = harness_with_bridge().await;
    let manager = h.mint_token("VERSE#lab", "manager");
    for (method, path, body) in verbs() {
        let (status, resp) = call(&h, method, path, &manager, &body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {resp}");
        assert_eq!(resp["error"], "insufficient permissions", "{path}");
    }
    assert!(
        seen.lock().unwrap().is_empty(),
        "the guard runs before the seam"
    );
}

/// A host without a sim bridge fails closed on every verb, even for owners.
#[tokio::test]
async fn unconfigured_host_fails_closed() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let owner = h.mint_token("VERSE#lab", "owner");
    for (method, path, body) in verbs() {
        let (status, resp) = call(&h, method, path, &owner, &body).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{path}: {resp}");
        assert_eq!(resp["ok"], false);
        assert!(
            resp["error"]
                .as_str()
                .unwrap()
                .contains("sim control not configured"),
            "{path}: {resp}"
        );
    }
}

/// MCP rides the same dispatch fns: an owner's `sim_step` reaches the seam
/// with its `n`; a viewer's `sim_start` is refused and never reaches it.
#[tokio::test]
async fn mcp_sim_tools_share_the_guarded_dispatch() {
    let (h, seen) = harness_with_bridge().await;
    let tool = |name: &str, args: serde_json::Value| {
        json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": { "name": name, "arguments": args } })
    };

    let viewer = h.mint_token("VERSE#lab", "viewer");
    let (_, resp) = h
        .post_json(
            "/mcp",
            Some(&viewer),
            &tool("sim_start", json!({ "script": "default" })),
        )
        .await;
    assert_eq!(resp["result"]["isError"], true, "{resp}");
    assert_eq!(
        resp["result"]["content"][0]["text"],
        "insufficient permissions"
    );
    assert!(seen.lock().unwrap().is_empty(), "authz before the seam");

    let owner = h.mint_token("VERSE#lab", "owner");
    let (_, resp) = h
        .post_json(
            "/mcp",
            Some(&owner),
            &tool("sim_start", json!({ "script": "sharded_query" })),
        )
        .await;
    assert!(resp["result"]["isError"].is_null(), "{resp}");
    let (_, resp) = h
        .post_json("/mcp", Some(&owner), &tool("sim_step", json!({ "n": 4 })))
        .await;
    assert!(resp["result"]["isError"].is_null(), "{resp}");
    let payload: serde_json::Value =
        serde_json::from_str(resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["executed"], 4);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            SimControlCommand::Start {
                script: json!("sharded_query")
            },
            SimControlCommand::Step { n: 4 },
        ]
    );
}
