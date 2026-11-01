//! agent_vision SEC-ADDENDUM through the full router, on BOTH doors (REST
//! and MCP): every capture and camera read carries the caller's token scope
//! to the host, a verse-B token is refused for an active-verse-A view with
//! the one uniform 404, and a foreign focus target is indistinguishable from
//! a nonexistent one. The host is a fake applying fe-policy's containment
//! rule (the real orchestrator's rule is flow-tested in fractalengine
//! `vision_bridge.rs`); what this suite proves is the forwarding + mapping.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fe_api::screenshot::ScreenshotSurface;
use fe_runtime::screenshot::{screenshot_bridge, ScreenshotRequestReceiver, SCREENSHOT_SCOPE_DENIED};
use fe_runtime::vision::{
    denied, vision_bridge, ActiveHierarchy, CameraState, CaptureMode, FocusTarget, PoseSource,
    ResolvedPose, ValidPoseSpec, VisionOp, VisionOutput, VisionRequestReceiver, WindowState,
};
use fractalengine_test_harness::api::{body_bytes, ApiHarness};
use serde_json::json;

const SCREENSHOT: &str = "/api/v1/debug/screenshot";
const CAMERA: &str = "/api/v1/debug/camera";
const DENIED_TEXT: &str = "not found or not permitted";
const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nscope-frame";

/// What the fake host knows: the open verse, node → verse, and every scope
/// it was asked on behalf of.
#[derive(Clone)]
struct Host {
    active_verse: String,
    node_verses: HashMap<String, String>,
    seen_scopes: Arc<Mutex<Vec<String>>>,
}

impl Host {
    fn covers(scope: &str, verse: &str) -> bool {
        fe_policy::scope_contains(scope, &format!("VERSE#{verse}"))
    }

    fn record(&self, scope: &str) {
        self.seen_scopes.lock().unwrap().push(scope.to_string());
    }

    fn pose(mode: CaptureMode) -> ResolvedPose {
        ResolvedPose {
            mode,
            position: [1.0, 2.0, 3.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_deg: 45.0,
            orbit: None,
            geo: None,
            world_scale: 1.0,
            width: 1280,
            height: 720,
            settle_frames_used: 0,
            settled: true,
            tiles_pending: 0,
            restored: true,
            source: PoseSource::Focus,
        }
    }

    fn serve_vision(&self, rx: VisionRequestReceiver) {
        let host = self.clone();
        std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                host.record(&request.caller_scope);
                let scope = request.caller_scope.clone();
                let result = if !Self::covers(&scope, &host.active_verse) {
                    Err(denied())
                } else {
                    match &request.op {
                        VisionOp::ReadCamera => Ok(VisionOutput::Camera(CameraState {
                            pose: Self::pose(CaptureMode::Window),
                            locked: false,
                            active: ActiveHierarchy {
                                verse_id: Some(host.active_verse.clone()),
                                verse_name: "A".into(),
                                fractal_id: None,
                                fractal_name: String::new(),
                                petal_id: None,
                            },
                            projection: None,
                            window: WindowState {
                                width: 1280,
                                height: 720,
                                minimized: false,
                            },
                        })),
                        VisionOp::Capture(spec) => match spec.pose() {
                            Some(ValidPoseSpec::Focus {
                                target: FocusTarget::Node(node_id),
                                ..
                            }) if !host
                                .node_verses
                                .get(node_id)
                                .is_some_and(|verse| Self::covers(&scope, verse)) =>
                            {
                                Err(denied())
                            }
                            _ => Ok(VisionOutput::Capture {
                                png: FAKE_PNG.to_vec(),
                                pose: Self::pose(spec.mode()),
                            }),
                        },
                    }
                };
                request.reply(result);
            }
        });
    }

    fn serve_plain(&self, rx: ScreenshotRequestReceiver) {
        let host = self.clone();
        std::thread::spawn(move || {
            while let Ok(request) = rx.recv() {
                host.record(&request.caller_scope);
                let result = if Self::covers(&request.caller_scope, &host.active_verse) {
                    Ok(FAKE_PNG.to_vec())
                } else {
                    Err(SCREENSHOT_SCOPE_DENIED.to_string())
                };
                request.reply(result);
            }
        });
    }
}

/// A fresh harness (own rate limiter) with verses A (open) and B, a node in
/// each, Owner tokens for both, and the fake host wired to both bridges.
struct World {
    h: ApiHarness,
    token_a: String,
    token_b: String,
    scope_b: String,
    host: Host,
}

async fn world() -> World {
    let (vision_tx, vision_rx) = vision_bridge();
    let (plain_tx, plain_rx) = screenshot_bridge();
    let surface = ScreenshotSurface::new(true, Some(plain_tx))
        .with_vision(vision_tx)
        .with_reply_timeout(Duration::from_millis(400));
    let h = ApiHarness::spawn_with_screenshot(surface)
        .await
        .expect("spawn harness");
    let verse_a = h.seed_verse("A").await.expect("seed A");
    let verse_b = h.seed_verse("B").await.expect("seed B");
    let host = Host {
        active_verse: verse_a.clone(),
        node_verses: HashMap::from([
            ("node-a".to_string(), verse_a.clone()),
            ("node-b".to_string(), verse_b.clone()),
        ]),
        seen_scopes: Arc::default(),
    };
    host.serve_vision(vision_rx);
    host.serve_plain(plain_rx);
    World {
        token_a: h.mint_token(&format!("VERSE#{verse_a}"), "owner"),
        token_b: h.mint_token(&format!("VERSE#{verse_b}"), "owner"),
        scope_b: format!("VERSE#{verse_b}"),
        h,
        host,
    }
}

/// The two doors a capture can come through.
#[derive(Debug, Clone, Copy)]
enum Door {
    Rest,
    Mcp,
}

/// `(status-or-200, error text or None)` — REST status + `error`, or MCP
/// (`isError` → 404-equivalent) + its text, normalised so both doors compare.
async fn capture(w: &World, door: Door, token: &str, args: serde_json::Value) -> (bool, String) {
    match door {
        Door::Rest => {
            let plain = args.as_object().is_some_and(|o| o.is_empty());
            let request = Request::builder()
                .method(if plain { "GET" } else { "POST" })
                .uri(SCREENSHOT)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(if plain {
                    Body::empty()
                } else {
                    Body::from(args.to_string())
                })
                .expect("build request");
            let resp = w.h.request(request).await;
            let status = resp.status();
            let bytes = body_bytes(resp).await;
            if status == StatusCode::OK {
                assert_eq!(bytes, FAKE_PNG);
                return (true, String::new());
            }
            assert_eq!(status, StatusCode::NOT_FOUND, "denials are 404");
            let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json error");
            (false, body["error"].as_str().unwrap_or_default().to_string())
        }
        Door::Mcp => mcp(w, token, "screenshot", args).await,
    }
}

async fn mcp(w: &World, token: &str, tool: &str, args: serde_json::Value) -> (bool, String) {
    let (status, body) = w
        .h
        .post_json(
            "/mcp",
            Some(token),
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": args }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["result"];
    if result["isError"] == true {
        (false, result["content"][0]["text"].as_str().unwrap_or_default().to_string())
    } else {
        (true, String::new())
    }
}

async fn camera(w: &World, door: Door, token: &str) -> (bool, String) {
    match door {
        Door::Rest => {
            let resp = w
                .h
                .request(
                    Request::builder()
                        .uri(CAMERA)
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .expect("build request"),
                )
                .await;
            let status = resp.status();
            let body: serde_json::Value =
                serde_json::from_slice(&body_bytes(resp).await).unwrap_or_default();
            if status == StatusCode::OK {
                return (true, String::new());
            }
            assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
            (false, body["error"].as_str().unwrap_or_default().to_string())
        }
        Door::Mcp => mcp(w, token, "get_camera", json!({})).await,
    }
}

fn posed() -> serde_json::Value {
    json!({ "pose": { "orbit": { "focus": [0, 0, 0], "yaw_deg": 0, "pitch_deg": 10, "distance": 5 } } })
}

/// SEC-ADDENDUM 1+3: a verse-B Owner token never receives verse-A pixels or
/// the camera state — plain and posed, REST and MCP — and the host saw the
/// caller's scope verbatim on every request; the verse-A token is served.
#[tokio::test]
async fn a_verse_b_token_is_denied_every_capture_and_camera_read_on_both_doors() {
    for door in [Door::Rest, Door::Mcp] {
        for args in [json!({}), posed()] {
            let w = world().await;
            let (served, error) = capture(&w, door, &w.token_b, args.clone()).await;
            assert!(!served, "{door:?} {args}: verse-B token must be denied");
            assert_eq!(error, DENIED_TEXT, "{door:?} {args}");
            let (served, _) = camera(&w, door, &w.token_b).await;
            assert!(!served, "{door:?}: camera read denied too");

            let w2 = world().await;
            let (served, error) = capture(&w2, door, &w2.token_a, args.clone()).await;
            assert!(served, "{door:?} {args}: covered token served ({error})");
            assert!(camera(&w2, door, &w2.token_a).await.0);

            // Both requests reached the host carrying exactly the caller's scope.
            let seen = w.host.seen_scopes.lock().unwrap().clone();
            assert_eq!(seen, vec![w.scope_b.clone(); 2], "{door:?} {args}");
        }
    }
}

/// SEC-ADDENDUM 2 (no existence oracle): focusing a node in an uncovered
/// verse and focusing a node that does not exist answer IDENTICALLY, with or
/// without `navigate` — REST and MCP alike.
#[tokio::test]
async fn foreign_and_nonexistent_focus_targets_are_indistinguishable_on_both_doors() {
    for door in [Door::Rest, Door::Mcp] {
        for navigate in [false, true] {
            let mut answers = Vec::new();
            for target in ["node-b", "no-such-node"] {
                let w = world().await;
                let args = json!({ "pose": { "focus": { "node_id": target, "navigate": navigate } } });
                answers.push(capture(&w, door, &w.token_a, args).await);
            }
            assert_eq!(answers[0], (false, DENIED_TEXT.to_string()), "{door:?}");
            assert_eq!(answers[0], answers[1], "{door:?} navigate={navigate}: no oracle");
        }
        // A covered target in the open verse is served through the same path.
        let w = world().await;
        let args = json!({ "pose": { "focus": { "node_id": "node-a" } } });
        assert!(capture(&w, door, &w.token_a, args).await.0, "{door:?}");
    }
}

/// UI-ADDENDUM 1+3: `include_ui` picks the mode, a contradicting `mode` is a
/// typed 400, and the response metadata reports `include_ui`.
#[tokio::test]
async fn include_ui_selects_the_mode_and_is_reported_in_the_metadata() {
    for (include_ui, mode) in [(true, "window"), (false, "scene")] {
        let w = world().await;
        let resp = w
            .h
            .request(
                Request::builder()
                    .method("POST")
                    .uri(SCREENSHOT)
                    .header("authorization", format!("Bearer {}", w.token_a))
                    .header("content-type", "application/json")
                    .body(Body::from(json!({ "include_ui": include_ui }).to_string()))
                    .expect("build request"),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let meta: serde_json::Value = serde_json::from_str(
            resp.headers()["x-fe-capture-pose"].to_str().expect("ascii"),
        )
        .expect("pose json");
        assert_eq!(meta["include_ui"], include_ui);
        assert_eq!(meta["mode"], mode);
    }

    let w = world().await;
    let resp = w
        .h
        .request(
            Request::builder()
                .method("POST")
                .uri(SCREENSHOT)
                .header("authorization", format!("Bearer {}", w.token_a))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "include_ui": false, "mode": "window" }).to_string()))
                .expect("build request"),
        )
        .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = serde_json::from_slice(&body_bytes(resp).await).unwrap();
    assert_eq!(body["code"], "include_ui_conflicts_with_mode");
    assert!(w.host.seen_scopes.lock().unwrap().is_empty(), "a 400 never reaches the host");

    // The MCP schema says it in plain words.
    let (status, body) = w
        .h
        .post_json(
            "/mcp",
            Some(&w.token_a),
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let tool = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|t| t["name"] == "screenshot")
        .expect("screenshot tool listed")
        .clone();
    assert_eq!(
        tool["inputSchema"]["properties"]["include_ui"]["description"],
        fe_runtime::vision::INCLUDE_UI_DESCRIPTION
    );
}
