//! Guarded QA diagnostics over the host bridge; see AGENTS.md §qa-debug.

use crate::bridge_client::round_trip;
use crate::operator_gate::OperatorGate;
use crate::server::ApiState;
use axum::extract::{Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use fe_identity::api_token::ApiClaims;
use fe_runtime::qa_debug::{DialogAnswerBatch, LogFilter, QaOp, TreeFilter, WindowAction};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

pub const QA_GATE: OperatorGate = OperatorGate {
    opt_in_env: crate::input::INPUT_ALLOW_ENV,
    endpoint: "QA diagnostics endpoint",
    enabled_warning: "QA UI tree, state and redacted log reads are enabled",
    disabled_message: "QA diagnostics endpoint is disabled on this node",
    rate_key_prefix: "qa-read",
    rate_per_sec: 20,
    rate_label: "20 QA reads/sec",
    default_on: false,
};

#[derive(Clone, Copy)]
pub enum QaKind {
    Tree,
    State,
    Logs,
    DialogAnswers,
    Window,
}

fn parse(kind: QaKind, args: Value) -> Result<QaOp, String> {
    match kind {
        QaKind::Tree => {
            let filter: TreeFilter = serde_json::from_value(args).map_err(|e| e.to_string())?;
            filter.validate()?;
            Ok(QaOp::UiTree(filter))
        }
        QaKind::State => {
            if args.as_object().is_none_or(|o| !o.is_empty()) {
                return Err("ui_state takes no arguments".into());
            }
            Ok(QaOp::UiState)
        }
        QaKind::Logs => {
            let filter: LogFilter = serde_json::from_value(args).map_err(|e| e.to_string())?;
            filter.validate()?;
            Ok(QaOp::Logs(filter))
        }
        QaKind::DialogAnswers => {
            let batch: DialogAnswerBatch =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            batch.validate()?;
            Ok(QaOp::DialogAnswers(batch))
        }
        QaKind::Window => {
            let action: WindowAction = serde_json::from_value(args).map_err(|e| e.to_string())?;
            action.validate()?;
            Ok(QaOp::Window(action))
        }
    }
}

fn qa_response(outcome: Result<Value, (StatusCode, String, String)>) -> Response {
    match outcome {
        Ok(data) => Json(json!({"ok":true,"data":data})).into_response(),
        Err((status, code, message)) => (
            status,
            Json(json!({"ok":false,"code":code,"error":message})),
        )
            .into_response(),
    }
}

pub async fn run(
    state: &ApiState,
    claims: &ApiClaims,
    kind: QaKind,
    args: Value,
) -> Result<Value, (StatusCode, String, String)> {
    QA_GATE
        .admit(state.input.allowed, state, claims)
        .await
        .map_err(|e| (e.status(), "qa_gate_refused".into(), e.message()))?;
    admitted_run(state, claims, kind, args).await
}

async fn admitted_run(
    state: &ApiState,
    claims: &ApiClaims,
    kind: QaKind,
    args: Value,
) -> Result<Value, (StatusCode, String, String)> {
    let op =
        parse(kind, args).map_err(|e| (StatusCode::BAD_REQUEST, "invalid_filter".into(), e))?;
    let bridge = state.input.qa_debug.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "qa_unavailable".into(),
        "QA diagnostics unavailable: this host has no window".into(),
    ))?;
    let reply = round_trip(
        bridge,
        &claims.scope,
        op,
        state.input.reply_timeout,
        "qa-debug",
    )
    .await
    .map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "qa_bridge_unavailable".into(),
            "QA host unavailable or busy".into(),
        )
    })?;
    reply.map_err(|e| {
        (
            if e.code == "not_found_or_denied" {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::CONFLICT
            },
            e.code.into(),
            e.message.into(),
        )
    })
}

async fn get(state: Arc<ApiState>, claims: ApiClaims, uri: Uri, kind: QaKind) -> Response {
    let preflight = QA_GATE.admit(state.input.allowed, &state, &claims).await;
    let outcome = match preflight {
        Err(e) => Err((e.status(), "qa_gate_refused".into(), e.message())),
        Ok(()) => match Query::<HashMap<String, String>>::try_from_uri(&uri) {
            Err(_) => Err((
                StatusCode::BAD_REQUEST,
                "invalid_filter".into(),
                "malformed query string".into(),
            )),
            Ok(Query(values)) => {
                let mut args = serde_json::Map::new();
                let mut invalid = None;
                for (key, value) in values {
                    let parsed = match key.as_str() {
                        "interactive" => match value.as_str() {
                            "1" | "true" => json!(true),
                            "0" | "false" => json!(false),
                            _ => {
                                invalid = Some("interactive must be 1/0 or true/false");
                                Value::Null
                            }
                        },
                        "since" => match value.parse::<u64>() {
                            Ok(v) => json!(v),
                            Err(_) => {
                                invalid = Some("since must be an unsigned sequence number");
                                Value::Null
                            }
                        },
                        _ => json!(value),
                    };
                    args.insert(key, parsed);
                }
                if let Some(message) = invalid {
                    Err((
                        StatusCode::BAD_REQUEST,
                        "invalid_filter".into(),
                        message.into(),
                    ))
                } else {
                    admitted_run(&state, &claims, kind, Value::Object(args)).await
                }
            }
        },
    };
    qa_response(outcome)
}

pub async fn ui_tree(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
) -> Response {
    get(state, claims, uri, QaKind::Tree).await
}
pub async fn ui_state(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
) -> Response {
    get(state, claims, uri, QaKind::State).await
}
pub async fn logs(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    uri: Uri,
) -> Response {
    get(state, claims, uri, QaKind::Logs).await
}

async fn post(
    state: Arc<ApiState>,
    claims: ApiClaims,
    body: axum::body::Bytes,
    kind: QaKind,
) -> Response {
    let outcome = match QA_GATE.admit(state.input.allowed, &state, &claims).await {
        Err(error) => Err((error.status(), "qa_gate_refused".into(), error.message())),
        Ok(()) => match serde_json::from_slice::<Value>(&body) {
            Err(_) => Err((
                StatusCode::BAD_REQUEST,
                "malformed_body".into(),
                "invalid QA request JSON".into(),
            )),
            Ok(args) => {
                let args = if matches!(kind, QaKind::DialogAnswers) && args.is_array() {
                    json!({"answers":args})
                } else {
                    args
                };
                admitted_run(&state, &claims, kind, args).await
            }
        },
    };
    qa_response(outcome)
}
pub async fn dialog_answers(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    post(state, claims, body, QaKind::DialogAnswers).await
}
pub async fn window(
    State(state): State<Arc<ApiState>>,
    Extension(claims): Extension<ApiClaims>,
    body: axum::body::Bytes,
) -> Response {
    post(state, claims, body, QaKind::Window).await
}
