//! Dynamic posed capture (`POST /api/v1/debug/screenshot`) + camera
//! read-back (`GET /api/v1/debug/camera`) through the full router — fe-api
//! AGENTS.md §screenshot, §camera-state. The Bevy side is a fake vision
//! responder — renderer-free by design (the `screenshot_test.rs` precedent).

use std::time::Duration;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use base64::Engine as _;
use fe_api::input::InputSurface;
use fe_api::operator_gate::OWNER_REQUIRED;
use fe_api::screenshot::{ScreenshotSurface, SCREENSHOT_GATE};
use fe_runtime::input::{
    camera_locked_by_capture, input_bridge, InputOp, InputOutput, InputRequestReceiver,
    CAMERA_LOCKED_BY_CAPTURE_CODE, NO_EFFECTIVE_INPUT_CODE,
};
use fe_runtime::screenshot::screenshot_bridge;
use fe_runtime::vision::{
    vision_bridge, ActiveHierarchy, CameraState, PoseSource, ProjectionState, ResolvedPose,
    ValidCaptureSpec, ValidPoseSpec, VisionFailure, VisionFailureClass, VisionOp, VisionOutput,
    VisionRequestReceiver, WindowState,
};
use fractalengine_test_harness::api::{body_bytes, ApiHarness};
use serde_json::json;

const SCREENSHOT: &str = "/api/v1/debug/screenshot";
const CAMERA: &str = "/api/v1/debug/camera";
/// The folded-in `agent_input_surface_20261031` route (I5 extends this
/// journey file rather than adding a new one — TEST POLICY 2026-11-01).
const INPUT: &str = "/api/v1/debug/input";
/// Not a real PNG on purpose: the API must relay the host's bytes verbatim.
const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-vision-frame";

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    bytes: Vec<u8>,
}

impl Reply {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.bytes).unwrap_or(serde_json::Value::Null)
    }

    /// Parse the `X-FE-Capture-Pose` header (present only on a 200 capture).
    fn pose_header(&self) -> ResolvedPose {
        let raw = self
            .headers
            .get("x-fe-capture-pose")
            .expect("pose header present")
            .to_str()
            .expect("ascii header value");
        serde_json::from_str(raw).expect("valid resolved-pose JSON")
    }
}

async fn post_raw(h: &ApiHarness, token: &str, raw_body: &str) -> Reply {
    let resp = h
        .request(
            Request::builder()
                .method("POST")
                .uri(SCREENSHOT)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(raw_body.to_string()))
                .expect("build request"),
        )
        .await;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = body_bytes(resp).await;
    Reply {
        status,
        headers,
        bytes,
    }
}

async fn post_screenshot(h: &ApiHarness, token: &str, body: &serde_json::Value) -> Reply {
    post_raw(h, token, &body.to_string()).await
}

async fn get_camera(h: &ApiHarness, token: &str) -> Reply {
    let resp = h
        .request(
            Request::builder()
                .method("GET")
                .uri(CAMERA)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = body_bytes(resp).await;
    Reply {
        status,
        headers,
        bytes,
    }
}

/// POST to the folded-in input-injection route; same `Reply` shape as
/// `post_screenshot` (this door is JSON-only, never a raw PNG body).
async fn post_input(h: &ApiHarness, token: &str, body: &serde_json::Value) -> Reply {
    let resp = h
        .request(
            Request::builder()
                .method("POST")
                .uri(INPUT)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("build request"),
        )
        .await;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = body_bytes(resp).await;
    Reply {
        status,
        headers,
        bytes,
    }
}

async fn mcp_call(
    h: &ApiHarness,
    token: &str,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    let (status, body) = h
        .post_json(
            "/mcp",
            Some(token),
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": args }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"].clone()
}

async fn mcp_tool_names(h: &ApiHarness, token: &str) -> Vec<String> {
    let (status, body) = h
        .post_json(
            "/mcp",
            Some(token),
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

async fn owner_token(h: &ApiHarness) -> String {
    let verse = h.seed_verse("Vision Verse").await.expect("seed verse");
    h.mint_token(&format!("VERSE#{verse}"), "owner")
}

// ---------------------------------------------------------------------------
// Fake Bevy-side vision host
// ---------------------------------------------------------------------------

/// The pose kind a capture used (so a test can check the ValidCaptureSpec
/// that actually crossed the bridge, not just that *some* reply came back).
fn pose_source(pose: Option<&ValidPoseSpec>) -> PoseSource {
    match pose {
        Some(ValidPoseSpec::Camera { .. }) | None => PoseSource::Camera,
        Some(ValidPoseSpec::Orbit { .. }) => PoseSource::Orbit,
        Some(ValidPoseSpec::Geo { .. }) => PoseSource::Geo,
        Some(ValidPoseSpec::Tile { .. }) => PoseSource::Tile,
        Some(ValidPoseSpec::Focus { .. }) => PoseSource::Focus,
    }
}

/// Every field here is DERIVED from the `ValidCaptureSpec` the fake host
/// actually received, so a test asserting on the response header/text is
/// really asserting the spec round-tripped the bridge intact (AC-3).
fn resolved_pose_for(spec: &ValidCaptureSpec) -> ResolvedPose {
    ResolvedPose {
        mode: spec.mode(),
        position: [1.0, 2.0, 3.0],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        fov_deg: 45.0,
        orbit: None,
        geo: None,
        world_scale: 1.0,
        width: spec.width().unwrap_or(1280),
        height: spec.height().unwrap_or(720),
        settle_frames_used: spec.settle_frames(),
        settled: true,
        tiles_pending: 0,
        restored: spec.restore().unwrap_or(true),
        source: pose_source(spec.pose()),
    }
}

fn fake_camera_state() -> CameraState {
    CameraState {
        pose: resolved_pose_for(
            &fe_runtime::vision::CaptureSpec::default()
                .validate()
                .expect("bare window capture is valid"),
        ),
        locked: false,
        active: ActiveHierarchy {
            verse_id: Some("v1".into()),
            verse_name: "Vision Verse".into(),
            fractal_id: None,
            fractal_name: String::new(),
            petal_id: None,
        },
        projection: Some(ProjectionState {
            origin_lat: 10.0,
            origin_lon: 20.0,
            origin_ele_grounded: 5.0,
            world_scale: 1.0,
            tileset_bounds: None,
            zoom_range: None,
        }),
        window: WindowState {
            width: 1280,
            height: 720,
            minimized: false,
        },
    }
}

/// Stand in for the Bevy host: answers every `Capture` with [`FAKE_PNG`] + a
/// pose derived from the spec it received, every `ReadCamera` with a fake
/// state, and refuses a `tile` pose with `tile_outside_tileset` (the one
/// dynamic refusal this suite exercises). `delay` stalls every CAPTURE reply
/// (never `ReadCamera`) so a test can observe the single-flight slot while
/// the first job is still in flight.
fn serve_vision(rx: VisionRequestReceiver, delay: Duration) {
    std::thread::spawn(move || {
        while let Ok(request) = rx.recv() {
            let result = match &request.op {
                VisionOp::Capture(spec) => {
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    if matches!(spec.pose(), Some(ValidPoseSpec::Tile { .. })) {
                        Err(VisionFailure {
                            status_class: VisionFailureClass::Unresolvable,
                            code: "tile_outside_tileset",
                            message: "the requested tile has no coverage in any installed \
                                      tileset"
                                .to_string(),
                        })
                    } else {
                        Ok(VisionOutput::Capture {
                            png: FAKE_PNG.to_vec(),
                            pose: resolved_pose_for(spec),
                        })
                    }
                }
                VisionOp::ReadCamera => Ok(VisionOutput::Camera(fake_camera_state())),
            };
            request.reply(result);
        }
    });
}

/// Stand-in for the Bevy host's `drain_input_requests` (folded-in
/// `agent_input_surface_20261031`, I5). Every admitted `Inject` is answered
/// with the exact batch size/`wait_frames` it received plus `FAKE_PNG` when
/// `capture_after` was set — so a test asserting on the reply is really
/// asserting the `ValidInputSpec` round-tripped the bridge intact (AC-3).
/// `lock: true` instead answers every request with
/// [`camera_locked_by_capture`], simulating a posed vision capture holding
/// `CameraCaptureLock` (`§INPUT-ADDENDUM` item 4) without needing a second,
/// real capture job in flight.
fn serve_input(rx: InputRequestReceiver, lock: bool) {
    std::thread::spawn(move || {
        while let Ok(request) = rx.recv() {
            let InputOp::Inject(spec) = &request.op;
            let result = if lock {
                Err(camera_locked_by_capture())
            } else {
                Ok(InputOutput {
                    commands_applied: spec.commands().len() as u32,
                    wait_frames_used: spec.wait_frames(),
                    png: spec.capture_after().then(|| FAKE_PNG.to_vec()),
                })
            };
            request.reply(result);
        }
    });
}

/// A harness whose vision bridge is wired; `allowed` is the operator opt-in.
async fn harness(allowed: bool) -> (ApiHarness, VisionRequestReceiver) {
    let (tx, rx) = vision_bridge();
    let surface = ScreenshotSurface::new(allowed, None)
        .with_vision(tx)
        .with_reply_timeout(Duration::from_millis(400));
    let h = ApiHarness::spawn_with_screenshot(surface)
        .await
        .expect("spawn harness");
    (h, rx)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// AC-3: a valid posed POST reaches the bridge as the equivalent
/// `ValidCaptureSpec`, and the 200 carries the responder's PNG verbatim plus
/// a pose header that round-trips the spec's mode/settle/restore/source.
#[tokio::test]
async fn posed_capture_round_trips_png_and_pose_header() {
    let (h, rx) = harness(true).await;
    serve_vision(rx, Duration::ZERO);
    let owner = owner_token(&h).await;

    let body = json!({
        "pose": { "orbit": { "focus": [0.0, 0.0, 0.0], "yaw_deg": 10.0, "pitch_deg": 5.0, "distance": 20.0 } },
        "settle_frames": 7,
        "restore": false
    });
    let reply = post_screenshot(&h, &owner, &body).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
    assert_eq!(reply.bytes, FAKE_PNG);

    let pose = reply.pose_header();
    assert_eq!(pose.mode, fe_runtime::vision::CaptureMode::Window);
    assert_eq!(pose.source, PoseSource::Orbit);
    assert_eq!(pose.settle_frames_used, 7);
    assert!(!pose.restored);
}

/// AC-2: every row of the static validation table yields 400 with its code,
/// and the fake bridge receives nothing for any of them.
///
/// NOTE on `non_finite`: a JSON number large enough to overflow `f64` (e.g.
/// `1e999`) is rejected by `serde_json` itself ("number out of range") —
/// `CaptureSpec::validate()` never runs, so the row observably becomes
/// `malformed_body` over the wire, not `non_finite`. NaN/Infinity are not
/// expressible as JSON number literals either. `non_finite` is real and
/// load-bearing (it is what makes a NaN *constructed in Rust* structurally
/// unreachable past `ValidCaptureSpec`), but only fe-runtime's
/// `capture_spec_validation_table` unit test — which builds the `NAN`/
/// `INFINITY` values directly — can actually exercise it; no HTTP body can.
#[tokio::test]
async fn invalid_bodies_are_400_with_their_code_and_never_reach_the_bridge() {
    let (h, rx) = harness(true).await;
    let owner = owner_token(&h).await;

    let cases: &[(&str, &str)] = &[
        // fov_out_of_range: a per-pose-kind range check, reachable over JSON.
        (
            r#"{"pose":{"camera":{"position":[0.0,1.0,0.0],"target":[0.0,0.0,1.0],"fov_deg":200.0}}}"#,
            "fov_out_of_range",
        ),
        // pose_must_have_exactly_one_kind: zero pose keys.
        (r#"{"pose":{}}"#, "pose_must_have_exactly_one_kind"),
        // size_out_of_range: below SIZE_MIN.
        (r#"{"mode":"scene","width":10}"#, "size_out_of_range"),
        // size_requires_scene_mode: width in window mode.
        (r#"{"width":640}"#, "size_requires_scene_mode"),
        // malformed_body: not valid JSON at all.
        (r#"{not valid json"#, "malformed_body"),
    ];
    for (i, &(raw_body, expected_code)) in cases.iter().enumerate() {
        if i > 0 {
            // Each case shares the owner's 1/s capture rate budget; wait out
            // the window so the refusal under test is validation, not rate.
            tokio::time::sleep(Duration::from_millis(1100)).await;
        }
        let reply = post_raw(&h, &owner, raw_body).await;
        assert_eq!(
            reply.status,
            StatusCode::BAD_REQUEST,
            "{raw_body}: {:?}",
            reply.json()
        );
        assert_eq!(
            reply.json()["code"],
            expected_code,
            "{raw_body}: {:?}",
            reply.json()
        );
    }
    assert!(
        rx.try_recv().is_err(),
        "no invalid body may reach the render host"
    );
}

/// AC-4: a second posed capture while the first is in flight gets 409
/// WITHOUT touching the bridge — proven with two harnesses (distinct DIDs,
/// so the 1/s rate limiter cannot be what produces the refusal) sharing one
/// cloned `ScreenshotSurface` (so the single-flight slot and the bridge
/// really are shared).
#[tokio::test]
async fn second_posed_capture_while_one_is_in_flight_is_409() {
    let (tx, rx) = vision_bridge();
    let surface = ScreenshotSurface::new(true, None)
        .with_vision(tx)
        .with_reply_timeout(Duration::from_millis(500));
    serve_vision(rx, Duration::from_millis(250));

    let h1 = ApiHarness::spawn_with_screenshot(surface.clone())
        .await
        .expect("spawn h1");
    let h2 = ApiHarness::spawn_with_screenshot(surface)
        .await
        .expect("spawn h2");
    let owner_a = owner_token(&h1).await;
    let owner_b = owner_token(&h2).await;

    let body = json!({
        "pose": { "orbit": { "focus": [0.0, 0.0, 0.0], "yaw_deg": 0.0, "pitch_deg": 0.0, "distance": 10.0 } }
    });
    let (first, second) = tokio::join!(
        post_screenshot(&h1, &owner_a, &body),
        post_screenshot(&h2, &owner_b, &body),
    );

    assert_eq!(second.status, StatusCode::CONFLICT, "{:?}", second.json());
    assert_eq!(second.json()["code"], "capture_in_progress");
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json());
    assert_eq!(first.bytes, FAKE_PNG);
}

/// A dynamic (Bevy-side) refusal maps to 422 with its code.
#[tokio::test]
async fn host_refusal_maps_to_422_with_its_code() {
    let (h, rx) = harness(true).await;
    serve_vision(rx, Duration::ZERO);
    let owner = owner_token(&h).await;

    let body = json!({ "mode": "scene", "pose": { "tile": { "z": 5, "x": 1, "y": 1 } } });
    let reply = post_screenshot(&h, &owner, &body).await;
    assert_eq!(
        reply.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        reply.json()
    );
    assert_eq!(reply.json()["code"], "tile_outside_tileset");
}

/// `GET /camera` uses its own 10/s budget, independent of the screenshot
/// surface's 1/s capture budget — a capture then a camera read in the same
/// second both succeed.
#[tokio::test]
async fn camera_state_round_trips_and_does_not_spend_the_capture_budget() {
    let (h, rx) = harness(true).await;
    serve_vision(rx, Duration::ZERO);
    let owner = owner_token(&h).await;

    let capture = post_screenshot(&h, &owner, &json!({})).await;
    assert_eq!(capture.status, StatusCode::OK, "{:?}", capture.json());

    let camera = get_camera(&h, &owner).await;
    assert_eq!(camera.status, StatusCode::OK, "{:?}", camera.json());
    assert_eq!(camera.json()["data"]["locked"], false);
    assert_eq!(
        camera.json()["data"]["active"]["verse_name"],
        "Vision Verse"
    );
    assert_eq!(
        camera.json()["data"]["projection"]["origin_ele_grounded"],
        5.0
    );
}

/// Disabled surface refuses POST and GET /camera (and both MCP calls)
/// before role, rate limit or bridge — nothing reaches the render host.
#[tokio::test]
async fn disabled_surface_refuses_post_and_camera_before_the_bridge() {
    let (h, rx) = harness(false).await;
    let owner = owner_token(&h).await;

    let post = post_screenshot(&h, &owner, &json!({})).await;
    assert_eq!(post.status, StatusCode::FORBIDDEN);
    assert_eq!(
        post.json()["error"],
        "screenshot endpoint is disabled on this node"
    );

    let camera = get_camera(&h, &owner).await;
    assert_eq!(camera.status, StatusCode::FORBIDDEN);
    assert_eq!(
        camera.json()["error"],
        "camera state endpoint is disabled on this node"
    );

    let mcp_shot = mcp_call(&h, &owner, "screenshot", json!({})).await;
    assert_eq!(mcp_shot["isError"], true, "{mcp_shot}");
    let mcp_cam = mcp_call(&h, &owner, "get_camera", json!({})).await;
    assert_eq!(mcp_cam["isError"], true, "{mcp_cam}");

    assert!(rx.try_recv().is_err(), "nothing may reach the render host");
}

/// AC-9: a headless host (opted in but no window, e.g. a relay) reports "no
/// window" on all three routes and hides both MCP rows.
#[tokio::test]
async fn headless_host_reports_no_window_and_hides_both_mcp_rows() {
    let h = ApiHarness::spawn_with_screenshot(ScreenshotSurface::new(true, None))
        .await
        .expect("spawn harness");
    let owner = owner_token(&h).await;

    let post = post_screenshot(&h, &owner, &json!({})).await;
    assert_eq!(
        post.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{:?}",
        post.json()
    );
    assert!(post.json()["error"]
        .as_str()
        .is_some_and(|e| e.contains("no window")));

    let camera = get_camera(&h, &owner).await;
    assert_eq!(
        camera.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{:?}",
        camera.json()
    );

    let names = mcp_tool_names(&h, &owner).await;
    assert!(!names.contains(&"screenshot".to_string()), "{names:?}");
    assert!(!names.contains(&"get_camera".to_string()), "{names:?}");
}

/// MCP `screenshot` with a pose returns the image block plus a text block
/// carrying the resolved pose JSON.
#[tokio::test]
async fn mcp_screenshot_with_pose_returns_image_and_pose_text() {
    let (h, rx) = harness(true).await;
    serve_vision(rx, Duration::ZERO);
    let owner = owner_token(&h).await;

    let result = mcp_call(
        &h,
        &owner,
        "screenshot",
        json!({ "pose": { "camera": { "position": [0.0, 5.0, 0.0], "target": [0.0, 0.0, 5.0] } } }),
    )
    .await;
    assert_ne!(result["isError"], true, "{result}");
    let content = &result["content"];
    assert_eq!(content[0]["type"], "image");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(content[0]["data"].as_str().expect("base64 data"))
        .expect("valid base64");
    assert_eq!(decoded, FAKE_PNG);
    assert_eq!(content[1]["type"], "text");
    let pose: ResolvedPose = serde_json::from_str(content[1]["text"].as_str().expect("pose text"))
        .expect("valid pose json");
    assert_eq!(pose.source, PoseSource::Camera);
}

/// MCP `screenshot` with empty arguments still uses the plain legacy bridge
/// (GET-equivalence), leaving the vision bridge untouched.
#[tokio::test]
async fn mcp_screenshot_without_args_still_uses_the_plain_bridge() {
    const FAKE_LEGACY_PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-legacy-frame";
    let (vtx, vrx) = vision_bridge();
    let (stx, srx) = screenshot_bridge();
    let surface = ScreenshotSurface::new(true, Some(stx)).with_vision(vtx);
    let h = ApiHarness::spawn_with_screenshot(surface)
        .await
        .expect("spawn harness");
    std::thread::spawn(move || {
        while let Ok(request) = srx.recv() {
            request.reply(Ok(FAKE_LEGACY_PNG.to_vec()));
        }
    });
    let owner = owner_token(&h).await;

    let result = mcp_call(&h, &owner, "screenshot", json!({})).await;
    assert_ne!(result["isError"], true, "{result}");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(result["content"][0]["data"].as_str().expect("base64 data"))
        .expect("valid base64");
    assert_eq!(decoded, FAKE_LEGACY_PNG);
    assert!(
        vrx.try_recv().is_err(),
        "empty-args screenshot must not touch the vision bridge"
    );
}

// ---------------------------------------------------------------------------
// Folded-in agent_input_surface_20261031 (plan I5, amended): extends this
// SAME journey file rather than a new one (TEST POLICY 2026-11-01). AIS spec
// AC-1..AC-6 plus `§INPUT-ADDENDUM` item 4's camera-lock interaction with
// this track's `CameraCaptureLock`, as ONE journey test — the input surface
// has no prior fe-api full-router coverage, so it earns one new test rather
// than a scatter of point tests (mirrors the "one long journey" doctrine
// `fe-runtime::input` and `fe-api::input`'s own unit tests already use).
// ---------------------------------------------------------------------------

/// AC-1 (disabled) -> AC-2 (role floor) -> AC-3 (round trip + capture_after)
/// -> AC-4 (rate limit) -> AC-5 (headless) -> I5's camera-lock case, each in
/// its own harness because the gate posture under test differs per step.
/// Every assertion goes red if its own branch inverts: flipping `allowed`,
/// the token's role, the fake responder's `lock` flag, the bridge wiring, or
/// the sixth rate-limited call would each fail only its own step.
#[tokio::test]
async fn input_injection_disabled_role_round_trip_rate_limited_headless_and_camera_lock_journey() {
    for allowed in [false, true] {
        let (qa_tx, qa_rx) = fe_runtime::qa_debug::qa_bridge();
        let h = ApiHarness::spawn_with_input(InputSurface::new(allowed, None).with_qa_debug(qa_tx))
            .await
            .expect("QA harness");
        let verse = h.seed_verse("QA verse").await.unwrap();
        let scope = format!("VERSE#{verse}");
        let owner = h.mint_token(&scope, "owner");
        let foreign = h.mint_token("VERSE#foreign", "owner");
        let editor = h.mint_token(&scope, "editor");
        if allowed {
            std::thread::spawn(move || {
                let logs = fe_runtime::qa_debug::QaLogRing::default();
                logs.set_verse(Some(&verse));
                logs.push(Some(&verse), "warn", "qa", "Bearer confidential");
                while let Ok(request) = qa_rx.recv() {
                    let result = if request.caller_scope != scope {
                        Err(fe_runtime::qa_debug::QaFailure::denied())
                    } else {
                        Ok(match &request.op {
                            fe_runtime::qa_debug::QaOp::UiTree(_) => {
                                json!({"coordinate_space":"physical_pixels","nodes":[{"name":"Maps","bounds":{"x":800,"y":10,"w":55,"h":20}}]})
                            }
                            fe_runtime::qa_debug::QaOp::UiState => {
                                json!({"navigation":{"verse_id":verse},"selection":null})
                            }
                            fe_runtime::qa_debug::QaOp::Logs(filter) => logs.read(filter),
                            fe_runtime::qa_debug::QaOp::DialogAnswers(batch) => {
                                json!({"queued":batch.answers.len()})
                            }
                            fe_runtime::qa_debug::QaOp::Window(action) => {
                                json!({"accepted":true,"action":action})
                            }
                        })
                    };
                    request.reply(result);
                }
            });
        }
        for (path, tool) in [
            ("ui-tree?name=Maps&interactive=1", "ui_tree"),
            ("ui-state", "ui_state"),
            ("logs?level=warn", "ui_logs"),
        ] {
            for (token, status) in [
                (
                    &owner,
                    if allowed {
                        StatusCode::OK
                    } else {
                        StatusCode::FORBIDDEN
                    },
                ),
                (
                    &foreign,
                    if allowed {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::FORBIDDEN
                    },
                ),
                (&editor, StatusCode::FORBIDDEN),
            ] {
                let response = h
                    .request(
                        Request::builder()
                            .uri(format!("/api/v1/debug/{path}"))
                            .header("authorization", format!("Bearer {token}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await;
                assert_eq!(response.status(), status);
                let body: serde_json::Value =
                    serde_json::from_slice(&body_bytes(response).await).unwrap();
                if status == StatusCode::OK && tool == "ui_tree" {
                    assert_eq!(body["data"]["nodes"][0]["name"], "Maps");
                    assert!(body["data"]["nodes"][0]["bounds"]["w"].as_u64().unwrap() > 0);
                }
                assert!(!body.to_string().contains("confidential"));
                let result = mcp_call(&h, token, tool, json!({})).await;
                assert_eq!(
                    result["isError"].as_bool().unwrap_or(false),
                    status != StatusCode::OK,
                    "{result}"
                );
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        for (path, tool, args) in [
            (
                "dialog-answers",
                "dialog_answers",
                json!({"answers":[{"kind":"open","cancel":true}]}),
            ),
            (
                "window",
                "window",
                json!({"action":"size","width":1024,"height":640}),
            ),
        ] {
            for (token, status) in [
                (
                    &owner,
                    if allowed {
                        StatusCode::OK
                    } else {
                        StatusCode::FORBIDDEN
                    },
                ),
                (
                    &foreign,
                    if allowed {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::FORBIDDEN
                    },
                ),
                (&editor, StatusCode::FORBIDDEN),
            ] {
                let response = h
                    .request(
                        Request::builder()
                            .method("POST")
                            .uri(format!("/api/v1/debug/{path}"))
                            .header("authorization", format!("Bearer {token}"))
                            .header("content-type", "application/json")
                            .body(Body::from(args.to_string()))
                            .unwrap(),
                    )
                    .await;
                assert_eq!(response.status(), status);
                let result = mcp_call(&h, token, tool, args.clone()).await;
                assert_eq!(
                    result["isError"].as_bool().unwrap_or(false),
                    status != StatusCode::OK,
                    "{result}"
                );
            }
        }
        if allowed {
            let response = h
                .request(
                    Request::builder()
                        .method("POST")
                        .uri("/api/v1/debug/window")
                        .header("authorization", format!("Bearer {owner}"))
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"action":"size","width":0,"height":640}"#))
                        .unwrap(),
                )
                .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }
    let click =
        |x: f32, y: f32| json!({ "kind": "pointer_click", "x": x, "y": y, "button": "left" });

    // AC-1: disabled without the opt-in — refused before role, rate or the
    // bridge, on both doors.
    {
        let (tx, rx) = input_bridge();
        let h = ApiHarness::spawn_with_input(InputSurface::new(false, Some(tx)))
            .await
            .expect("spawn harness");
        let owner = owner_token(&h).await;

        let rest = post_input(&h, &owner, &json!({ "commands": [click(1.0, 1.0)] })).await;
        assert_eq!(rest.status, StatusCode::FORBIDDEN, "{:?}", rest.json());
        assert_eq!(
            rest.json()["error"],
            "input injection endpoint is disabled on this node"
        );

        let mcp = mcp_call(
            &h,
            &owner,
            "inject_input",
            json!({ "commands": [click(1.0, 1.0)] }),
        )
        .await;
        assert_eq!(mcp["isError"], true, "{mcp}");

        assert!(
            rx.try_recv().is_err(),
            "nothing may reach the render host while disabled"
        );
    }

    // AC-2: role floor — a below-Owner token is refused even with the env
    // on, on both doors, before the bridge.
    {
        let (tx, rx) = input_bridge();
        let h = ApiHarness::spawn_with_input(InputSurface::new(true, Some(tx)))
            .await
            .expect("spawn harness");
        let verse = h.seed_verse("Input Verse").await.expect("seed verse");
        let editor = h.mint_token(&format!("VERSE#{verse}"), "editor");

        let rest = post_input(&h, &editor, &json!({ "commands": [click(1.0, 1.0)] })).await;
        assert_eq!(rest.status, StatusCode::FORBIDDEN, "{:?}", rest.json());
        assert_eq!(rest.json()["error"], OWNER_REQUIRED);

        let mcp = mcp_call(
            &h,
            &editor,
            "inject_input",
            json!({ "commands": [click(1.0, 1.0)] }),
        )
        .await;
        assert_eq!(mcp["isError"], true, "{mcp}");

        assert!(
            rx.try_recv().is_err(),
            "nothing may reach the render host below Owner"
        );
    }

    // INPUT-ADDENDUM items 6 + 7, input on but the screenshot surface off
    // (`FE_API_SCREENSHOT_ALLOW=0`): an empty text entry is a typed 400
    // (it would plan zero frames — the panic=abort crash), and
    // `capture_after` is refused by the SCREENSHOT gate on both doors, all
    // before the bridge. The same host then still serves a plain batch.
    {
        let (tx, rx) = input_bridge();
        let h = ApiHarness::spawn_with_input(InputSurface::new(true, Some(tx)))
            .await
            .expect("spawn harness");
        let owner = owner_token(&h).await;

        let empty_text = json!({ "commands": [{ "kind": "text_entry", "text": "" }] });
        let rest = post_input(&h, &owner, &empty_text).await;
        assert_eq!(rest.status, StatusCode::BAD_REQUEST, "{:?}", rest.json());
        assert_eq!(rest.json()["code"], NO_EFFECTIVE_INPUT_CODE);
        assert_eq!(rest.json()["field"], "commands[0].text");

        let with_capture = json!({ "commands": [click(1.0, 1.0)], "capture_after": true });
        let rest = post_input(&h, &owner, &with_capture).await;
        assert_eq!(rest.status, StatusCode::FORBIDDEN, "{:?}", rest.json());
        assert_eq!(rest.json()["error"], SCREENSHOT_GATE.disabled_message);
        let mcp = mcp_call(&h, &owner, "inject_input", with_capture).await;
        assert_eq!(mcp["isError"], true, "{mcp}");
        assert_eq!(mcp["content"][0]["text"], SCREENSHOT_GATE.disabled_message);
        assert!(
            rx.try_recv().is_err(),
            "a refused capture_after never reaches the render host"
        );

        serve_input(rx, false);
        let plain = post_input(&h, &owner, &json!({ "commands": [click(1.0, 1.0)] })).await;
        assert_eq!(plain.status, StatusCode::OK, "{:?}", plain.json());
        assert!(plain.json().get("capture_after_png_base64").is_none());
    }

    // AC-3: the admitted batch crosses the bridge intact (the fake host
    // echoes its own size/wait_frames back), and `capture_after` attaches
    // FAKE_PNG as base64 in the JSON success body (this door is JSON-only,
    // unlike the posed-capture route's raw-PNG body). A GUI host's shape:
    // screenshots on (their default) beside the opted-in input surface.
    {
        let (tx, rx) = input_bridge();
        let h = ApiHarness::spawn_with_screenshot_and_input(
            ScreenshotSurface::new(true, None),
            InputSurface::new(true, Some(tx)).with_reply_timeout(Duration::from_millis(400)),
        )
        .await
        .expect("spawn harness");
        serve_input(rx, false);
        let owner = owner_token(&h).await;

        let body = json!({
            "commands": [click(12.0, 34.0), { "kind": "scroll", "dx": 0.0, "dy": -3.0 }],
            "wait_frames": 5,
            "capture_after": true,
        });
        let rest = post_input(&h, &owner, &body).await;
        assert_eq!(rest.status, StatusCode::OK, "{:?}", rest.json());
        let reply = rest.json();
        assert_eq!(reply["commands_applied"], 2);
        assert_eq!(reply["wait_frames_used"], 5);
        let png_b64 = reply["capture_after_png_base64"]
            .as_str()
            .expect("capture_after attaches a PNG");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(png_b64)
                .expect("valid base64"),
            FAKE_PNG
        );
    }

    // AC-4: a burst past the 5/s per-DID budget is rate limited on the 6th
    // call within the window — same token, same harness.
    {
        let (tx, rx) = input_bridge();
        let h = ApiHarness::spawn_with_input(InputSurface::new(true, Some(tx)))
            .await
            .expect("spawn harness");
        serve_input(rx, false);
        let owner = owner_token(&h).await;

        for i in 0..5 {
            let rest = post_input(&h, &owner, &json!({ "commands": [click(i as f32, 0.0)] })).await;
            assert_eq!(rest.status, StatusCode::OK, "call {i}: {:?}", rest.json());
        }
        let limited = post_input(&h, &owner, &json!({ "commands": [click(99.0, 0.0)] })).await;
        assert_eq!(
            limited.status,
            StatusCode::TOO_MANY_REQUESTS,
            "{:?}",
            limited.json()
        );
        assert_eq!(
            limited.json()["error"],
            "rate limit exceeded (5 input requests/sec)"
        );
    }

    // AC-5: headless (opted in, no window/bridge) — REST 503, MCP hides the
    // `inject_input` row entirely (mirrors `headless_host_reports_no_window_
    // and_hides_both_mcp_rows` above, for this surface).
    {
        let h = ApiHarness::spawn_with_input(InputSurface::new(true, None))
            .await
            .expect("spawn harness");
        let owner = owner_token(&h).await;

        let rest = post_input(&h, &owner, &json!({ "commands": [click(1.0, 1.0)] })).await;
        assert_eq!(
            rest.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{:?}",
            rest.json()
        );

        let names = mcp_tool_names(&h, &owner).await;
        assert!(!names.contains(&"inject_input".to_string()), "{names:?}");
    }

    // I5 / `§INPUT-ADDENDUM` item 4: refused 409 while a posed capture holds
    // `CameraCaptureLock` — the fake host answers every `Inject` with the
    // `camera_locked_by_capture` failure the real `drain_input_requests`
    // gives while the lock exists, and the API must map it to 409 on both
    // doors with the stable code.
    {
        let (tx, rx) = input_bridge();
        let h = ApiHarness::spawn_with_input(InputSurface::new(true, Some(tx)))
            .await
            .expect("spawn harness");
        serve_input(rx, true);
        let owner = owner_token(&h).await;

        let rest = post_input(&h, &owner, &json!({ "commands": [click(1.0, 1.0)] })).await;
        assert_eq!(rest.status, StatusCode::CONFLICT, "{:?}", rest.json());
        assert_eq!(rest.json()["code"], CAMERA_LOCKED_BY_CAPTURE_CODE);

        let mcp = mcp_call(
            &h,
            &owner,
            "inject_input",
            json!({ "commands": [click(2.0, 2.0)] }),
        )
        .await;
        assert_eq!(mcp["isError"], true, "{mcp}");
        assert_eq!(
            mcp["content"][0]["text"],
            format!(
                "{CAMERA_LOCKED_BY_CAPTURE_CODE}: {}",
                camera_locked_by_capture().message
            )
        );
    }
}
