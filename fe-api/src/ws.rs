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
    /// CUD operation over WS (requires editor role). The server replies with
    /// an `EntityCommandResult` echoing `request_id`.
    EntityCommand {
        request_id: String,
        command: crate::types::EntityCommand,
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
    Pong {
        timestamp_ms: u64,
        server_timestamp_ms: u64,
    },
    /// A previously-optimistic transform failed to persist — client should
    /// revert the node to these last-known-good values.
    TransformRollback {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 3],
        scale: [f32; 3],
    },
    /// Response to an `EntityCommand`, echoing its `request_id`.
    EntityCommandResult {
        request_id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
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
                match fe_identity::api_token::verify_api_token(&access_token, &state.verifying_key)
                {
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
    type TransformTriple = ([f32; 3], [f32; 3], [f32; 3]); // (position, rotation, scale)
    let mut transform_debounce: std::collections::HashMap<String, TransformTriple> =
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
                        let Some(scope) = resolve_petal_scope_ws(&state, &petal_id).await else {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "not_found".into(),
                                message: "could not resolve subscription scope".into(),
                            }).await;
                            continue;
                        };
                        if !fe_database::scope_contains(&claims.scope, &scope) {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "forbidden".into(),
                                message: "subscription outside token scope".into(),
                            }).await;
                            continue;
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
                        if !crate::types::is_valid_ulid(&node_id) {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "invalid_request".into(),
                                message: "invalid node_id".into(),
                            }).await;
                            continue;
                        }
                        let Some(scope) = crate::rest::resolve_node_scope(&state, &node_id).await else {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "not_found".into(),
                                message: "could not resolve transform target scope".into(),
                            }).await;
                            continue;
                        };
                        if !fe_database::scope_contains(&claims.scope, &scope) {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "forbidden".into(),
                                message: "transform target outside token scope".into(),
                            }).await;
                            continue;
                        }
                        let Some(petal_id) = petal_id_from_scope(&scope) else {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "not_found".into(),
                                message: "transform target has no petal scope".into(),
                            }).await;
                            continue;
                        };
                        let now = unix_now_ms();
                        // Optimistic broadcast — subscribers see the update immediately.
                        let _ = state.transform_broadcast_tx.send(TransformUpdate {
                            node_id: node_id.clone(),
                            petal_id,
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
                        let Some(scope) = resolve_petal_scope_ws(&state, &petal_id).await else {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "not_found".into(),
                                message: "could not resolve scene subscription scope".into(),
                            }).await;
                            continue;
                        };
                        if !fe_database::scope_contains(&claims.scope, &scope) {
                            send_msg(&mut socket, &WsServerMsg::Error {
                                code: "forbidden".into(),
                                message: "scene subscription outside token scope".into(),
                            }).await;
                            continue;
                        }
                        let next_version = scene_version + 1;
                        let snapshot = scene_snapshot_message(
                            petal_id.clone(),
                            next_version,
                            load_petal_nodes(&state, &petal_id).await,
                        );
                        let snapshot = match snapshot {
                            Ok(snapshot) => snapshot,
                            Err(error) => {
                                tracing::warn!(?error, %petal_id, "WS scene subscription snapshot failed");
                                subscribed_petals.remove(&petal_id);
                                send_resync_required(&mut socket).await;
                                continue;
                            }
                        };
                        subscribed_petals.insert(petal_id);
                        scene_version = next_version;
                        send_msg(&mut socket, &snapshot).await;
                    }
                    Some(WsClientMsg::EntityCommand { request_id, command }) => {
                        let msg = match handle_entity_command(&state, &claims, command).await {
                            Ok(data) => WsServerMsg::EntityCommandResult {
                                request_id, ok: true, data: Some(data), error: None,
                            },
                            Err(e) => WsServerMsg::EntityCommandResult {
                                request_id, ok: false, data: None, error: Some(e),
                            },
                        };
                        send_msg(&mut socket, &msg).await;
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
                        if let Err(error) = send_scene_snapshots(
                            &mut socket,
                            &state,
                            &subscribed_petals,
                            &mut scene_version,
                        )
                        .await
                        {
                            tracing::warn!(?error, "WS transform lag snapshot recovery failed");
                            subscribed_petals.clear();
                            send_resync_required(&mut socket).await;
                        }
                    }
                    Err(_) => break,
                }
            }

            // Outbound entity change deltas (scene graph CUD)
            change = entity_change_rx.recv() => {
                match change {
                    Ok(ref sc) if !subscribed_petals.is_empty() => {
                        // TransformFailed → send rollback instead of a delta
                        let petal_id = sc.petal_id();
                        if !subscribed_petals.contains(petal_id) {
                            continue;
                        }
                        if let SceneChange::TransformFailed { node_id, position, rotation, scale, .. } = sc {
                            send_msg(&mut socket, &WsServerMsg::TransformRollback {
                                node_id: node_id.clone(),
                                position: *position,
                                rotation: *rotation,
                                scale: *scale,
                            }).await;
                            continue;
                        }
                        scene_version += 1;
                        send_msg(&mut socket, &WsServerMsg::SceneDelta {
                            petal_id: petal_id.to_owned(),
                            version: scene_version,
                            changes: vec![sc.clone()],
                        }).await;
                    }
                    Ok(_) => {} // no subscriptions
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WS client lagged by {n} entity change updates");
                        if let Err(error) = send_scene_snapshots(
                            &mut socket,
                            &state,
                            &subscribed_petals,
                            &mut scene_version,
                        )
                        .await
                        {
                            tracing::warn!(?error, "WS scene lag snapshot recovery failed");
                            subscribed_petals.clear();
                            send_resync_required(&mut socket).await;
                        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SceneSnapshotLoadError {
    DirectQuery,
    DirectResult,
    MalformedNode,
    ChannelClosed,
    ReplyCancelled,
    TimedOut,
    DatabaseError,
    UnexpectedResult,
}

/// Load all nodes for a petal, distinguishing an empty petal from a read failure.
async fn load_petal_nodes(
    state: &ApiState,
    petal_id: &str,
) -> Result<Vec<crate::types::NodeDto>, SceneSnapshotLoadError> {
    if let Some(ref db) = state.db_reader {
        return load_petal_nodes_direct(db, petal_id).await;
    }

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::LoadNodesByPetal {
                petal_id: petal_id.to_string(),
            },
            reply_tx,
        })
        .map_err(|_| SceneSnapshotLoadError::ChannelClosed)?;

    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodesLoaded {
            petal_id: result_petal_id,
            nodes,
        })) if result_petal_id == petal_id => Ok(nodes
            .into_iter()
            .map(|node| crate::types::NodeDto {
                id: node.node_id,
                name: node.name,
                petal_id: node.petal_id,
                position: node.position,
                has_asset: node.has_asset,
                asset_path: node.asset_path,
                webpage_url: None,
            })
            .collect()),
        Ok(Ok(DbResult::Error(_))) => Err(SceneSnapshotLoadError::DatabaseError),
        Ok(Ok(_)) => Err(SceneSnapshotLoadError::UnexpectedResult),
        Ok(Err(_)) => Err(SceneSnapshotLoadError::ReplyCancelled),
        Err(_) => Err(SceneSnapshotLoadError::TimedOut),
    }
}

/// Query a direct reader without converting database failures into empty snapshots.
async fn load_petal_nodes_direct(
    db: &surrealdb::Surreal<surrealdb::engine::local::Db>,
    petal_id: &str,
) -> Result<Vec<crate::types::NodeDto>, SceneSnapshotLoadError> {
    let mut result = db
        .query(
            "SELECT * FROM node WHERE petal_id = $pid AND tombstone = NONE ORDER BY created_at ASC",
        )
        .bind(("pid", petal_id.to_string()))
        .await
        .map_err(|_| SceneSnapshotLoadError::DirectQuery)?;
    let rows: Vec<serde_json::Value> = result
        .take(0)
        .map_err(|_| SceneSnapshotLoadError::DirectResult)?;

    rows.iter()
        .map(snapshot_node_from_row)
        .collect::<Result<Vec<_>, _>>()
}

/// Decode one direct-query node row for a scene snapshot.
fn snapshot_node_from_row(
    row: &serde_json::Value,
) -> Result<crate::types::NodeDto, SceneSnapshotLoadError> {
    let node_id = row
        .get("node_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(SceneSnapshotLoadError::MalformedNode)?;
    let name = row
        .get("display_name")
        .and_then(serde_json::Value::as_str)
        .ok_or(SceneSnapshotLoadError::MalformedNode)?;
    let petal_id = row
        .get("petal_id")
        .and_then(serde_json::Value::as_str)
        .ok_or(SceneSnapshotLoadError::MalformedNode)?;
    let coordinates = row
        .get("position")
        .and_then(|position| position.get("coordinates"))
        .and_then(serde_json::Value::as_array)
        .ok_or(SceneSnapshotLoadError::MalformedNode)?;
    let x = coordinates
        .first()
        .and_then(serde_json::Value::as_f64)
        .ok_or(SceneSnapshotLoadError::MalformedNode)? as f32;
    let z = coordinates
        .get(1)
        .and_then(serde_json::Value::as_f64)
        .ok_or(SceneSnapshotLoadError::MalformedNode)? as f32;
    let y = row
        .get("elevation")
        .and_then(serde_json::Value::as_f64)
        .ok_or(SceneSnapshotLoadError::MalformedNode)? as f32;

    Ok(crate::types::NodeDto {
        id: node_id.to_string(),
        name: name.to_string(),
        petal_id: petal_id.to_string(),
        position: [x, y, z],
        has_asset: row
            .get("asset_id")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        asset_path: row
            .get("asset_path")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        webpage_url: None,
    })
}

/// Send a fresh scene snapshot for each subscribed petal after broadcast lag.
async fn send_scene_snapshots(
    socket: &mut WebSocket,
    state: &ApiState,
    subscribed_petals: &HashSet<String>,
    scene_version: &mut u64,
) -> Result<(), SceneSnapshotLoadError> {
    let mut petal_ids: Vec<_> = subscribed_petals.iter().collect();
    petal_ids.sort_unstable();
    let mut snapshots = Vec::with_capacity(petal_ids.len());
    for petal_id in petal_ids {
        let nodes = load_petal_nodes(state, petal_id).await?;
        snapshots.push((petal_id.clone(), nodes));
    }
    for (petal_id, nodes) in snapshots {
        *scene_version += 1;
        send_msg(
            socket,
            &WsServerMsg::SceneSnapshot {
                petal_id,
                version: *scene_version,
                nodes,
            },
        )
        .await;
    }
    Ok(())
}

/// Require the client to resubscribe after an authoritative snapshot load fails.
async fn send_resync_required(socket: &mut WebSocket) {
    send_msg(
        socket,
        &WsServerMsg::Error {
            code: "resync_required".into(),
            message: "scene state is unavailable; resubscribe required".into(),
        },
    )
    .await;
}

/// Construct a snapshot only from a successful node load.
fn scene_snapshot_message(
    petal_id: String,
    version: u64,
    nodes: Result<Vec<crate::types::NodeDto>, SceneSnapshotLoadError>,
) -> Result<WsServerMsg, SceneSnapshotLoadError> {
    nodes.map(|nodes| WsServerMsg::SceneSnapshot {
        petal_id,
        version,
        nodes,
    })
}

/// Execute one `EntityCommand`: role + scope enforcement, then a `DbRequest`
/// round-trip. Ok(json) on success; Err(client-safe message) otherwise.
/// Scene deltas reach subscribers via the DB thread's entity-change broadcast.
async fn handle_entity_command(
    state: &ApiState,
    claims: &ApiClaims,
    command: crate::types::EntityCommand,
) -> Result<serde_json::Value, String> {
    use crate::types::EntityCommand as Ec;

    // All CUD ops require editor role — mirrors the REST/TransformUpdate policy.
    let role = fe_database::RoleLevel::from(claims.max_role.as_str());
    if !role.can_edit() {
        return Err("editor role required for entity commands".into());
    }

    // Resolve the target's scope and enforce it against the token scope.
    let scope = match &command {
        Ec::CreateNode { petal_id, .. } => resolve_petal_scope_ws(state, petal_id).await,
        Ec::DeleteNode { node_id }
        | Ec::SetNodeProperty { node_id, .. }
        | Ec::DeleteNodeProperty { node_id, .. } => {
            if !crate::types::is_valid_ulid(node_id) {
                return Err("invalid node_id".into());
            }
            crate::rest::resolve_node_scope(state, node_id).await
        }
    };
    let Some(scope) = scope else {
        return Err("could not resolve target scope".into());
    };
    if !fe_database::scope_contains(&claims.scope, &scope) {
        return Err("insufficient scope".into());
    }

    let db_cmd = match command {
        Ec::CreateNode {
            petal_id,
            name,
            position,
        } => DbCommand::CreateNode {
            petal_id,
            name,
            position: position.unwrap_or([0.0, 0.0, 0.0]),
            correlation_id: None,
        },
        Ec::DeleteNode { node_id } => DbCommand::DeleteNode { node_id },
        Ec::SetNodeProperty {
            node_id,
            key,
            value,
        } => DbCommand::SetNodeProperty {
            node_id,
            key,
            value,
        },
        Ec::DeleteNodeProperty { node_id, key } => DbCommand::DeleteNodeProperty { node_id, key },
    };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: db_cmd,
            reply_tx,
        })
        .map_err(|_| "internal channel closed".to_string())?;

    match tokio::time::timeout(std::time::Duration::from_secs(5), reply_rx).await {
        Ok(Ok(DbResult::NodeCreated {
            id, name, petal_id, ..
        })) => Ok(serde_json::json!({
            "node_id": id, "name": name, "petal_id": petal_id,
        })),
        Ok(Ok(DbResult::NodeDeleted { node_id, petal_id })) => Ok(serde_json::json!({
            "node_id": node_id, "petal_id": petal_id,
        })),
        Ok(Ok(DbResult::NodePropertySet { node_id, key })) => Ok(serde_json::json!({
            "node_id": node_id, "key": key,
        })),
        Ok(Ok(DbResult::NodePropertyDeleted { node_id, key })) => Ok(serde_json::json!({
            "node_id": node_id, "key": key,
        })),
        Ok(Ok(DbResult::Error(e))) => {
            tracing::error!("WS entity command failed: {e}");
            Err("operation failed".into())
        }
        Ok(Ok(_)) => Err("unexpected response".into()),
        Ok(Err(_)) => Err("request cancelled".into()),
        Err(_) => Err("request timed out".into()),
    }
}

/// Resolve a petal_id to its full scope string.
///
/// Uses a direct DB query when `db_reader` is available, falling back to the
/// crossbeam channel otherwise.
async fn resolve_petal_scope_ws(state: &ApiState, petal_id: &str) -> Option<String> {
    if let Some(ref db) = state.db_reader {
        return crate::rest::direct_resolve_petal_scope(db, petal_id).await;
    }
    // Fallback: channel-based resolution
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .api_cmd_tx
        .send(ApiCommand::DbRequest {
            cmd: DbCommand::ResolvePetalScope {
                petal_id: petal_id.to_string(),
            },
            reply_tx,
        })
        .ok()?;
    match tokio::time::timeout(std::time::Duration::from_secs(3), reply_rx).await {
        Ok(Ok(DbResult::ScopeResolved { scope })) => scope,
        _ => None,
    }
}

/// Extract the petal component from a fully-resolved node scope.
fn petal_id_from_scope(scope: &str) -> Option<String> {
    fe_database::parse_scope(scope).ok()?.petal_id
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
    fn petal_id_from_scope_requires_a_petal_component() {
        let petal_scope = fe_database::build_scope("verse", Some("fractal"), Some("petal"));
        assert_eq!(petal_id_from_scope(&petal_scope), Some("petal".into()));

        let verse_scope = fe_database::build_scope("verse", None, None);
        assert_eq!(petal_id_from_scope(&verse_scope), None);
    }

    #[test]
    fn scene_snapshot_requires_a_successful_load() {
        let failed = scene_snapshot_message("p1".into(), 1, Err(SceneSnapshotLoadError::TimedOut));
        assert!(matches!(failed, Err(SceneSnapshotLoadError::TimedOut)));

        let empty_petal = scene_snapshot_message("p1".into(), 1, Ok(Vec::new())).unwrap();
        match empty_petal {
            WsServerMsg::SceneSnapshot {
                petal_id, nodes, ..
            } => {
                assert_eq!(petal_id, "p1");
                assert!(nodes.is_empty());
            }
            _ => panic!("expected scene snapshot"),
        }
    }

    #[test]
    fn direct_snapshot_row_rejects_malformed_coordinates() {
        let row = serde_json::json!({
            "node_id": "n1",
            "display_name": "Node",
            "petal_id": "p1",
            "position": { "coordinates": [1.0] },
            "elevation": 2.0,
        });

        assert!(matches!(
            snapshot_node_from_row(&row),
            Err(SceneSnapshotLoadError::MalformedNode)
        ));
    }

    #[test]
    fn direct_snapshot_row_decodes_a_valid_empty_asset_node() {
        let row = serde_json::json!({
            "node_id": "n1",
            "display_name": "Node",
            "petal_id": "p1",
            "position": { "coordinates": [1.0, 3.0] },
            "elevation": 2.0,
            "asset_id": null,
        });

        let node = snapshot_node_from_row(&row).unwrap();
        assert_eq!(node.id, "n1");
        assert_eq!(node.name, "Node");
        assert_eq!(node.petal_id, "p1");
        assert_eq!(node.position, [1.0, 2.0, 3.0]);
        assert!(!node.has_asset);
    }

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
            WsClientMsg::SceneSubscribe {
                petal_id,
                last_known_version,
            } => {
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
            WsClientMsg::SceneSubscribe {
                petal_id,
                last_known_version,
            } => {
                assert_eq!(petal_id, "p2");
                assert_eq!(last_known_version, None);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn entity_command_serde() {
        let json = r#"{"type":"entity_command","request_id":"r1",
            "command":{"op":"create_node","petal_id":"p1","name":"n","position":[1.0,2.0,3.0]}}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMsg::EntityCommand {
                request_id,
                command,
            } => {
                assert_eq!(request_id, "r1");
                match command {
                    crate::types::EntityCommand::CreateNode {
                        petal_id,
                        name,
                        position,
                    } => {
                        assert_eq!(petal_id, "p1");
                        assert_eq!(name, "n");
                        assert_eq!(position, Some([1.0, 2.0, 3.0]));
                    }
                    other => panic!("expected CreateNode, got {other:?}"),
                }
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn entity_command_position_optional() {
        let json = r#"{"type":"entity_command","request_id":"r2",
            "command":{"op":"create_node","petal_id":"p1","name":"n"}}"#;
        let msg: WsClientMsg = serde_json::from_str(json).unwrap();
        match msg {
            WsClientMsg::EntityCommand { command, .. } => match command {
                crate::types::EntityCommand::CreateNode { position, .. } => {
                    assert_eq!(position, None);
                }
                other => panic!("expected CreateNode, got {other:?}"),
            },
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn entity_command_delete_and_property_ops_serde() {
        let del: WsClientMsg = serde_json::from_str(
            r#"{"type":"entity_command","request_id":"r3",
                "command":{"op":"delete_node","node_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV"}}"#,
        )
        .unwrap();
        assert!(matches!(
            del,
            WsClientMsg::EntityCommand {
                command: crate::types::EntityCommand::DeleteNode { .. },
                ..
            }
        ));
        let set: WsClientMsg = serde_json::from_str(
            r#"{"type":"entity_command","request_id":"r4",
                "command":{"op":"set_node_property","node_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","key":"opacity","value":0.5}}"#,
        ).unwrap();
        assert!(matches!(
            set,
            WsClientMsg::EntityCommand {
                command: crate::types::EntityCommand::SetNodeProperty { .. },
                ..
            }
        ));
        let delp: WsClientMsg = serde_json::from_str(
            r#"{"type":"entity_command","request_id":"r5",
                "command":{"op":"delete_node_property","node_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","key":"opacity"}}"#,
        ).unwrap();
        assert!(matches!(
            delp,
            WsClientMsg::EntityCommand {
                command: crate::types::EntityCommand::DeleteNodeProperty { .. },
                ..
            }
        ));
    }

    #[test]
    fn entity_command_result_serde() {
        let ok = WsServerMsg::EntityCommandResult {
            request_id: "r1".into(),
            ok: true,
            data: Some(serde_json::json!({"node_id": "n1"})),
            error: None,
        };
        let json = serde_json::to_string(&ok).unwrap();
        assert!(json.contains("\"entity_command_result\""));
        assert!(json.contains("\"request_id\":\"r1\""));
        assert!(json.contains("\"node_id\":\"n1\""));
        assert!(
            !json.contains("\"error\""),
            "error field must be omitted when None"
        );

        let err = WsServerMsg::EntityCommandResult {
            request_id: "r2".into(),
            ok: false,
            data: None,
            error: Some("insufficient scope".into()),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"ok\":false"));
        assert!(json.contains("insufficient scope"));
        assert!(
            !json.contains("\"data\""),
            "data field must be omitted when None"
        );
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
            changes: vec![fe_runtime::messages::SceneChange::NodeRemoved {
                node_id: "n1".into(),
                petal_id: "p1".into(),
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"scene_delta\""));
        assert!(json.contains("\"node_removed\""));
    }
}
