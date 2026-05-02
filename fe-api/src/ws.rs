use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use tokio::sync::broadcast;

use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, SceneChange, TransformUpdate};

use crate::server::ApiState;

// ---------------------------------------------------------------------------
// Client → Server messages
// ---------------------------------------------------------------------------

/// Messages sent from the WebSocket client to the server.
///
/// Serialized with `serde(tag = "type")` so JSON looks like
/// `{"type": "scene_subscribe", "petal_id": "..."}`.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMsg {
    /// Authenticate with a Bearer API token.
    Auth { access_token: String },
    /// Subscribe to a named channel for a specific petal (e.g. transforms).
    Subscribe { channel: String, petal_id: String },
    /// Unsubscribe from a named channel.
    Unsubscribe { channel: String, petal_id: String },
    /// Push a real-time transform update (requires editor role).
    TransformUpdate {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Subscribe to scene graph changes for a petal. The server responds with a
    /// `SceneSnapshot` followed by incremental `SceneDelta` messages.
    SceneSubscribe {
        petal_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        last_known_version: Option<u64>,
    },
    /// Latency probe.
    Ping { timestamp_ms: u64 },
}

// ---------------------------------------------------------------------------
// Server → Client messages
// ---------------------------------------------------------------------------

/// Messages sent from the server to the WebSocket client.
#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerMsg {
    /// Server requests authentication (sent immediately after upgrade).
    AuthRequired {},
    /// Authentication succeeded.
    AuthOk {},
    /// Authentication failed.
    AuthInvalid { message: String },
    /// Channel subscription confirmed.
    Subscribed { channel: String, petal_id: String },
    /// Channel unsubscription confirmed.
    Unsubscribed { channel: String, petal_id: String },
    /// Real-time transform broadcast from another client.
    TransformUpdate {
        node_id: String,
        petal_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
        timestamp_ms: u64,
    },
    /// Full snapshot of all nodes in a petal (sent after `SceneSubscribe`).
    SceneSnapshot {
        petal_id: String,
        version: u64,
        nodes: Vec<crate::types::NodeDto>,
    },
    /// Incremental scene changes (node added/removed/renamed/transformed).
    SceneDelta {
        petal_id: String,
        version: u64,
        changes: Vec<fe_runtime::messages::SceneChange>,
    },
    /// Latency probe response.
    Pong { timestamp_ms: u64, server_timestamp_ms: u64 },
    /// A previously-optimistic transform failed to persist — client should
    /// revert the node to these last-known-good values.
    TransformRollback {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Error message.
    Error { code: String, message: String },
}

// ---------------------------------------------------------------------------
// Public handler entry point
// ---------------------------------------------------------------------------

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ApiState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

// ---------------------------------------------------------------------------
// Socket lifecycle
// ---------------------------------------------------------------------------

async fn handle_socket(mut socket: WebSocket, state: Arc<ApiState>) {
    // 1. Send auth_required
    send_msg(&mut socket, &WsServerMsg::AuthRequired {}).await;

    // 2. Wait for auth message (5-second timeout)
    let auth_claims: Option<ApiClaims> = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv_msg(&mut socket),
    )
    .await
    {
        Ok(Some(WsClientMsg::Auth { access_token })) => {
            if access_token.is_empty() {
                send_msg(
                    &mut socket,
                    &WsServerMsg::AuthInvalid {
                        message: "empty token".into(),
                    },
                )
                .await;
                None
            } else {
                match fe_identity::api_token::verify_api_token(
                    &access_token,
                    &state.verifying_key,
                ) {
                    Ok(claims) => {
                        tracing::info!(
                            "WS authenticated: sub={} scope={} role={}",
                            claims.sub,
                            claims.scope,
                            claims.max_role
                        );
                        send_msg(&mut socket, &WsServerMsg::AuthOk {}).await;
                        Some(claims)
                    }
                    Err(e) => {
                        tracing::warn!("WS auth failed: {e}");
                        send_msg(
                            &mut socket,
                            &WsServerMsg::AuthInvalid {
                                message: "invalid token".into(),
                            },
                        )
                        .await;
                        None
                    }
                }
            }
        }
        _ => {
            send_msg(
                &mut socket,
                &WsServerMsg::AuthInvalid {
                    message: "auth timeout".into(),
                },
            )
            .await;
            None
        }
    };

    let Some(claims) = auth_claims else {
        return;
    };

    // 3. Enter command loop.
    //
    // The WebSocket is used unsplit. tokio::select! picks exactly one branch
    // per iteration, so recv_msg and socket.send never race on the same &mut.
    let mut subscribed_petals: HashSet<String> = HashSet::new();
    let mut transform_rx = state.transform_broadcast_tx.subscribe();
    let mut entity_change_rx = state.entity_change_tx.subscribe();
    let mut scene_version: u64 = 0;

    // Debounce map: last transform update per node, flushed to DB every 200ms.
    let mut transform_debounce: std::collections::HashMap<String, ([f32; 3], [f32; 3], [f32; 3])> =
        std::collections::HashMap::new();
    let mut debounce_interval = tokio::time::interval(std::time::Duration::from_millis(200));
    debounce_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Incoming client message
            msg = recv_msg(&mut socket) => {
                match msg {
                    Some(WsClientMsg::Subscribe { channel, petal_id }) => {
                        // Resolve petal scope and enforce against token scope
                        if let Some(scope) = resolve_petal_scope_ws(&state, &petal_id).await {
                            if !fe_database::scope_contains(&claims.scope, &scope) {
                                send_msg(&mut socket, &WsServerMsg::Error {
                                    code: "forbidden".into(),
                                    message: "subscription outside token scope".into(),
                                }).await;
                                continue;
                            }
                        }
                        subscribed_petals.insert(petal_id.clone());
                        let resp = WsServerMsg::Subscribed { channel, petal_id };
                        send_msg(&mut socket, &resp).await;
                    }
                    Some(WsClientMsg::Unsubscribe { channel, petal_id }) => {
                        subscribed_petals.remove(&petal_id);
                        let resp = WsServerMsg::Unsubscribed { channel, petal_id };
                        send_msg(&mut socket, &resp).await;
                    }
                    Some(WsClientMsg::Ping { timestamp_ms }) => {
                        let now = unix_now_ms();
                        let resp = WsServerMsg::Pong {
                            timestamp_ms,
                            server_timestamp_ms: now,
                        };
                        send_msg(&mut socket, &resp).await;
                    }
                    Some(WsClientMsg::TransformUpdate {
                        node_id,
                        position,
                        rotation,
                        scale,
                    }) => {
                        // Enforce editor role — viewers cannot push transforms.
                        let role = fe_database::RoleLevel::from(claims.max_role.as_str());
                        if !role.can_edit() {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "forbidden".into(),
                                message: "editor role required for transform updates".into(),
                            }).await;
                            continue;
                        }
                        let now = unix_now_ms();
                        // Optimistic broadcast — subscribers see the update immediately.
                        let _ = state.transform_broadcast_tx.send(TransformUpdate {
                            node_id: node_id.clone(),
                            petal_id: String::new(),
                            position,
                            rotation,
                            scale,
                            timestamp_ms: now,
                            source_did: claims.sub.clone(),
                        });
                        // Store for debounced DB persist (200ms batching).
                        // The debounce_interval branch drains this map and sends
                        // TransformPersist commands so the DB isn't flooded by
                        // high-frequency WS streams.
                        transform_debounce.insert(node_id, (position, rotation, scale));
                    }
                    Some(WsClientMsg::SceneSubscribe { petal_id, .. }) => {
                        // Resolve petal scope and enforce against token scope
                        if let Some(scope) = resolve_petal_scope_ws(&state, &petal_id).await {
                            if !fe_database::scope_contains(&claims.scope, &scope) {
                                send_msg(&mut socket, &WsServerMsg::Error {
                                    code: "forbidden".into(),
                                    message: "scene subscription outside token scope".into(),
                                }).await;
                                continue;
                            }
                        }
                        subscribed_petals.insert(petal_id.clone());

                        // Query current nodes for this petal from DB
                        let nodes = load_petal_nodes(&state, &petal_id).await;
                        scene_version += 1;
                        send_msg(&mut socket, &WsServerMsg::SceneSnapshot {
                            petal_id,
                            version: scene_version,
                            nodes,
                        }).await;
                    }
                    Some(WsClientMsg::Auth { .. }) => {
                        // Already authenticated; ignore re-auth
                    }
                    None => break, // socket closed or error
                }
            }

            // Outbound transform broadcasts
            update = transform_rx.recv() => {
                match update {
                    Ok(tu) if !subscribed_petals.is_empty()
                        && subscribed_petals.contains(&tu.petal_id) =>
                    {
                        let msg = WsServerMsg::TransformUpdate {
                            node_id: tu.node_id,
                            petal_id: tu.petal_id,
                            position: tu.position,
                            rotation: tu.rotation,
                            scale: tu.scale,
                            timestamp_ms: tu.timestamp_ms,
                        };
                        send_msg(&mut socket, &msg).await;
                    }
                    Ok(_) => {} // filtered out by subscription
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS client lagged by {n} transform updates");
                    }
                    Err(_) => break,
                }
            }

            // Outbound entity change deltas (scene graph CUD)
            change = entity_change_rx.recv() => {
                match change {
                    Ok(ref sc) if !subscribed_petals.is_empty() => {
                        // TransformFailed → send rollback instead of a delta
                        if let SceneChange::TransformFailed { node_id, position, rotation, scale } = sc {
                            send_msg(&mut socket, &WsServerMsg::TransformRollback {
                                node_id: node_id.clone(),
                                position: *position,
                                rotation: *rotation,
                                scale: *scale,
                            }).await;
                            continue;
                        }
                        // Extract the petal_id from the change to filter by subscription
                        let petal_id = match sc {
                            SceneChange::NodeAdded { node } => Some(node.petal_id.clone()),
                            // NodeRemoved/NodeRenamed/NodeTransform don't carry petal_id;
                            // broadcast to all subscribed petals (clients filter locally).
                            _ => None,
                        };
                        let should_send = match &petal_id {
                            Some(pid) => subscribed_petals.contains(pid),
                            None => true, // broadcast to all subscribers
                        };
                        if should_send {
                            scene_version += 1;
                            let pid = petal_id.unwrap_or_default();
                            send_msg(&mut socket, &WsServerMsg::SceneDelta {
                                petal_id: pid,
                                version: scene_version,
                                changes: vec![sc.clone()],
                            }).await;
                        }
                    }
                    Ok(_) => {} // no subscriptions
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS client lagged by {n} entity change updates");
                    }
                    Err(_) => break,
                }
            }

            // Debounce flush: persist batched transform updates to DB every 200ms.
            _ = debounce_interval.tick() => {
                for (node_id, (position, rotation, scale)) in transform_debounce.drain() {
                    let _ = state.api_cmd_tx.send(ApiCommand::TransformPersist {
                        node_id,
                        position,
                        rotation,
                        scale,
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serialize `msg` to JSON and send it as a Text frame on `socket`.
async fn send_msg(socket: &mut WebSocket, msg: &WsServerMsg) {
    if let Ok(json) = serde_json::to_string(msg) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}

/// Read one text frame from `socket` and deserialize it as a [`WsClientMsg`].
/// Returns `None` on close, error, or deserialization failure.
async fn recv_msg(socket: &mut WebSocket) -> Option<WsClientMsg> {
    loop {
        match socket.recv().await? {
            Ok(Message::Text(text)) => {
                return serde_json::from_str(text.as_str()).ok();
            }
            Ok(Message::Close(_)) => return None,
            Ok(_) => continue, // ping/pong/binary — skip
            Err(_) => return None,
        }
    }
}

/// Load all nodes for a petal via the API command channel.
///
/// Returns `NodeDto` list from `fe_runtime::messages` (the scene streaming DTO,
/// not the REST API DTO). Falls back to an empty vec on timeout or error.
async fn load_petal_nodes(
    state: &ApiState,
    petal_id: &str,
) -> Vec<crate::types::NodeDto> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    if state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::LoadNodesByPetal {
                petal_id: petal_id.to_string(),
            },
            reply_tx,
        })
        .is_err()
    {
        return vec![];
    }
    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodesLoaded { nodes, .. })) => {
            nodes
                .into_iter()
                .map(|n| crate::types::NodeDto {
                    id: n.node_id,
                    name: n.name,
                    petal_id: n.petal_id,
                    position: n.position,
                    has_asset: n.has_asset,
                    asset_path: n.asset_path,
                    webpage_url: None,
                })
                .collect()
        }
        _ => vec![],
    }
}

/// Resolve a petal_id to its full scope string via the API command channel.
async fn resolve_petal_scope_ws(state: &ApiState, petal_id: &str) -> Option<String> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state.api_cmd_tx.send(ApiCommand::DbRequest {
        cmd: DbCommand::ResolvePetalScope { petal_id: petal_id.to_string() },
        reply_tx,
    }).ok()?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}

/// Current Unix time in milliseconds.
fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_subscribe_serde() {
        let msg = WsClientMsg::SceneSubscribe {
            petal_id: "p1".into(),
            last_known_version: Some(42),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"scene_subscribe\""));
        let roundtrip: WsClientMsg = serde_json::from_str(&json).unwrap();
        match roundtrip {
            WsClientMsg::SceneSubscribe { petal_id, last_known_version } => {
                assert_eq!(petal_id, "p1");
                assert_eq!(last_known_version, Some(42));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn scene_subscribe_without_version() {
        let json = r#"{"type":"scene_subscribe","petal_id":"p2"}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMsg::SceneSubscribe { petal_id, last_known_version } => {
                assert_eq!(petal_id, "p2");
                assert_eq!(last_known_version, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn scene_snapshot_serde() {
        let msg = WsServerMsg::SceneSnapshot {
            petal_id: "p1".into(),
            version: 1,
            nodes: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"scene_snapshot\""));
        assert!(json.contains("\"version\":1"));
    }

    #[test]
    fn scene_delta_serde() {
        let msg = WsServerMsg::SceneDelta {
            petal_id: "p1".into(),
            version: 2,
            changes: vec![
                fe_runtime::messages::SceneChange::NodeRemoved {
                    node_id: "n1".into(),
                },
            ],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"scene_delta\""));
        assert!(json.contains("\"node_removed\""));
    }
}
