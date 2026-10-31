//! Agent screenshot surface through the full router (fe-api/AGENTS.md
//! §screenshot): REST `GET /api/v1/debug/screenshot` and the MCP `screenshot`
//! tool are deny-by-default (operator opt-in → Owner → 1/s per DID), and an
//! admitted call crosses the API→Bevy bridge and returns the host's PNG bytes.
//! The Bevy side is a fake capture responder — renderer-free by design.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::Engine as _;
use fe_api::screenshot::ScreenshotSurface;
use fe_runtime::screenshot::{screenshot_bridge, ScreenshotRequestReceiver};
use fractalengine_test_harness::api::{body_bytes, ApiHarness};
use serde_json::json;

const SCREENSHOT: &str = "/api/v1/debug/screenshot";
/// Not a real PNG on purpose: the API must relay the host's bytes verbatim.
const FAKE_PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-frame";

/// What a call returned: status, content type, raw body.
struct Reply {
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }
}

async fn get_screenshot(h: &ApiHarness, token: &str) -> Reply {
    let resp = h
        .request(
            Request::builder()
                .method("GET")
                .uri(SCREENSHOT)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .expect("build request"),
        )
        .await;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Reply {
        status,
        content_type,
        body: body_bytes(resp).await,
    }
}

async fn mcp_screenshot(h: &ApiHarness, token: &str) -> serde_json::Value {
    let (status, body) = h
        .post_json(
            "/mcp",
            Some(token),
            &json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "screenshot", "arguments": {} }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["result"].clone()
}

/// A harness whose screenshot bridge is wired; `allowed` is the operator opt-in.
async fn harness(allowed: bool) -> (ApiHarness, ScreenshotRequestReceiver) {
    let (tx, rx) = screenshot_bridge();
    let surface =
        ScreenshotSurface::new(allowed, Some(tx)).with_reply_timeout(Duration::from_millis(300));
    let h = ApiHarness::spawn_with_screenshot(surface)
        .await
        .expect("spawn harness");
    (h, rx)
}

/// Stand in for the Bevy host: answer every capture with `FAKE_PNG`.
fn serve_captures(rx: ScreenshotRequestReceiver) {
    std::thread::spawn(move || {
        while let Ok(request) = rx.recv() {
            request.reply(Ok(FAKE_PNG.to_vec()));
        }
    });
}

async fn owner_token(h: &ApiHarness) -> String {
    let verse = h.seed_verse("Shot Verse").await.expect("seed verse");
    h.mint_token(&format!("VERSE#{verse}"), "owner")
}

#[tokio::test]
async fn screenshot_is_refused_unless_the_operator_opted_in() {
    let (h, rx) = harness(false).await;
    let owner = owner_token(&h).await;

    let rest = get_screenshot(&h, &owner).await;
    assert_eq!(rest.status, StatusCode::FORBIDDEN);
    assert_eq!(
        rest.json()["error"],
        "screenshot endpoint is disabled on this node"
    );

    let mcp = mcp_screenshot(&h, &owner).await;
    assert_eq!(mcp["isError"], true, "{mcp}");
    assert_eq!(
        mcp["content"][0]["text"],
        "screenshot endpoint is disabled on this node"
    );

    assert!(rx.try_recv().is_err(), "nothing may reach the render host");
}

#[tokio::test]
async fn screenshot_requires_owner_even_when_opted_in() {
    let (h, rx) = harness(true).await;
    let verse = h.seed_verse("Shot Verse").await.expect("seed verse");

    for role in ["viewer", "editor", "manager"] {
        let token = h.mint_token(&format!("VERSE#{verse}"), role);
        let rest = get_screenshot(&h, &token).await;
        assert_eq!(rest.status, StatusCode::FORBIDDEN, "{role}");
        assert_eq!(
            rest.json()["error"],
            "insufficient permissions (owner required)",
            "{role}"
        );
        let mcp = mcp_screenshot(&h, &token).await;
        assert_eq!(mcp["isError"], true, "{role}: {mcp}");
    }
    assert!(rx.try_recv().is_err(), "nothing may reach the render host");
}

/// The admitted flow: the request crosses the bridge, the host's PNG comes
/// back verbatim (REST body / MCP base64 image block), and a second capture
/// inside the same second is rate limited.
#[tokio::test]
async fn owner_capture_returns_the_host_png_then_is_rate_limited() {
    let (h, rx) = harness(true).await;
    serve_captures(rx);
    let owner = owner_token(&h).await;

    let rest = get_screenshot(&h, &owner).await;
    assert_eq!(rest.status, StatusCode::OK, "{:?}", rest.json());
    assert_eq!(rest.content_type, "image/png");
    assert_eq!(rest.body, FAKE_PNG);

    let limited = get_screenshot(&h, &owner).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        limited.json()["error"],
        "rate limit exceeded (1 screenshot/sec)"
    );

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let mcp = mcp_screenshot(&h, &owner).await;
    assert_ne!(mcp["isError"], true, "{mcp}");
    let image = &mcp["content"][0];
    assert_eq!(image["type"], "image");
    assert_eq!(image["mimeType"], "image/png");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(image["data"].as_str().expect("base64 data"))
        .expect("valid base64");
    assert_eq!(decoded, FAKE_PNG);
}

/// A host that accepts the request but never answers yields a timeout error
/// within the configured backstop — never a hung request.
#[tokio::test]
async fn silent_render_host_times_out_instead_of_hanging() {
    let (h, rx) = harness(true).await;
    let owner = owner_token(&h).await;
    let held = std::thread::spawn(move || {
        let request = rx.recv().expect("capture request arrives");
        std::thread::sleep(Duration::from_secs(2));
        drop(request);
    });

    let started = Instant::now();
    let rest = get_screenshot(&h, &owner).await;
    assert_eq!(
        rest.status,
        StatusCode::GATEWAY_TIMEOUT,
        "{:?}",
        rest.json()
    );
    assert!(
        rest.json()["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("screenshot timed out")),
        "{:?}",
        rest.json()
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the API must give up at its own timeout, not wait for the host"
    );
    held.join().expect("holder thread");
}
