//! Live `/ws` integration test (F14/T1).
//!
//! `ApiHarness::request`'s `tower::oneshot` dispatch (used by every other
//! fe-api integration test) cannot perform a WebSocket upgrade — there is no
//! real TCP connection for the protocol switch to ride. This suite instead
//! binds a real `TcpListener`, serves the harness's router over it with
//! `axum::serve`, and drives the handshake with `tokio-tungstenite` (already
//! in the dependency graph at this version via axum's own `ws` feature — see
//! Cargo.lock). Everything downstream of the handshake — auth, subscribe,
//! and the scene broadcast — therefore exercises the exact code in
//! `fe-api/src/ws.rs`, not a serde round-trip of its message types.

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use fractalengine_test_harness::api::ApiHarness;

/// Serve `h`'s router on a real loopback socket; returns the bound address.
/// The server task is detached — it dies with the test process, and each
/// test binds an ephemeral port so runs never collide.
async fn spawn_live_server(h: &ApiHarness) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    let router = h.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    addr
}

/// Read one text frame and parse it as JSON.
async fn next_json(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> serde_json::Value {
    loop {
        match ws
            .next()
            .await
            .expect("stream ended early")
            .expect("ws frame")
        {
            WsMessage::Text(t) => return serde_json::from_str(&t).expect("json frame"),
            WsMessage::Ping(_) | WsMessage::Pong(_) => continue,
            other => panic!("unexpected frame: {other:?}"),
        }
    }
}

#[tokio::test]
async fn ws_handshake_auth_subscribe_and_scene_broadcast_end_to_end() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let chain = h.seed_hierarchy().await.expect("seed hierarchy");
    let token = h.mint_token(&chain.verse_scope(), "editor");
    let addr = spawn_live_server(&h).await;

    let (mut ws, _resp) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("ws connect");

    // 1. Real upgrade handshake succeeded; server greets with auth_required.
    let greeting = next_json(&mut ws).await;
    assert_eq!(greeting["type"], "auth_required");

    // 2. Authenticate with a real minted token.
    ws.send(WsMessage::Text(
        json!({ "type": "auth", "access_token": token })
            .to_string()
            .into(),
    ))
    .await
    .expect("send auth");
    let ok = next_json(&mut ws).await;
    assert_eq!(ok["type"], "auth_ok");

    // 3. Subscribe to the seeded petal's scene graph.
    ws.send(WsMessage::Text(
        json!({
            "type": "scene_subscribe",
            "petal_id": chain.petal_id,
            "last_known_version": null
        })
        .to_string()
        .into(),
    ))
    .await
    .expect("send scene_subscribe");
    let snapshot = next_json(&mut ws).await;
    assert_eq!(snapshot["type"], "scene_snapshot");
    assert_eq!(snapshot["petal_id"], chain.petal_id);
    assert_eq!(snapshot["version"], 1);
    assert_eq!(
        snapshot["nodes"],
        json!([]),
        "freshly seeded petal has no nodes yet"
    );

    // 4. Trigger a real scene broadcast through the SAME channel the
    // entity-store bridge feeds in production
    // (fe-api/src/entity_store_bridge.rs) — the cheapest real path to an
    // end-to-end delta without standing up the entity store itself.
    let node = fe_runtime::messages::NodeDto {
        node_id: "n-broadcast".into(),
        petal_id: chain.petal_id.clone(),
        name: "live-broadcast".into(),
        position: [1.0, 2.0, 3.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        has_asset: false,
        asset_path: None,
    };
    h.state
        .entity_change_tx
        .send(fe_runtime::messages::SceneChange::NodeAdded { node })
        .expect("broadcast to entity_change_tx");

    // 5. The subscribed client observes the delta end-to-end.
    let delta = next_json(&mut ws).await;
    assert_eq!(delta["type"], "scene_delta");
    assert_eq!(delta["petal_id"], chain.petal_id);
    assert_eq!(delta["version"], 2);
    assert_eq!(delta["changes"][0]["op"], "node_added");
    assert_eq!(delta["changes"][0]["node"]["node_id"], "n-broadcast");

    let _ = ws.close(None).await;
}

#[tokio::test]
async fn ws_rejects_a_bad_token_and_closes() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let addr = spawn_live_server(&h).await;

    let (mut ws, _resp) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .expect("ws connect");

    let greeting = next_json(&mut ws).await;
    assert_eq!(greeting["type"], "auth_required");

    ws.send(WsMessage::Text(
        json!({ "type": "auth", "access_token": "not-a-real-token" })
            .to_string()
            .into(),
    ))
    .await
    .expect("send bad auth");

    let invalid = next_json(&mut ws).await;
    assert_eq!(invalid["type"], "auth_invalid");

    // The server drops the connection after a failed auth — no command loop
    // is ever entered for an unauthenticated socket. `handle_socket` simply
    // returns on bad auth rather than sending a WS close frame first, so the
    // client observes either a clean stream end, an explicit close frame, or
    // (what tungstenite actually reports for this abrupt drop) a
    // `Protocol(ResetWithoutClosingHandshake)` error — any of which is a
    // closed connection. What must NOT happen is a further protocol frame.
    match ws.next().await {
        None => {}
        Some(Ok(WsMessage::Close(_))) => {}
        Some(Err(_)) => {}
        other => panic!("expected the socket to close after bad auth, got {other:?}"),
    }
}
