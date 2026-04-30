use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use tokio::sync::broadcast;

use fe_identity::api_token::ApiClaims;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult, TransformUpdate};

use crate::server::ApiState;

// ---------------------------------------------------------------------------
// Client → Server messages
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientMsg {
    Auth { access_token: String },
    Subscribe { channel: String, petal_id: String },
    Unsubscribe { channel: String, petal_id: String },
    TransformUpdate {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    Ping { timestamp_ms: u64 },
}

// ---------------------------------------------------------------------------
// Server → Client messages
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerMsg {
    AuthRequired {},
    AuthOk {},
    AuthInvalid { message: String },
    Subscribed { channel: String, petal_id: String },
    Unsubscribed { channel: String, petal_id: String },
    TransformUpdate {
        node_id: String,
        petal_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
        timestamp_ms: u64,
    },
    Pong { timestamp_ms: u64, server_timestamp_ms: u64 },
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
                        let _ = state.transform_broadcast_tx.send(TransformUpdate {
                            node_id,
                            petal_id: String::new(),
                            position,
                            rotation,
                            scale,
                            timestamp_ms: now,
                            source_did: claims.sub.clone(),
                        });
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
