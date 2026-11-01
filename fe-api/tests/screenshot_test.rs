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

/// Seed a verse another node created, granting visitors `default_access`.
async fn seed_foreign_verse(h: &ApiHarness, default_access: &str) -> String {
    let verse_id = ulid::Ulid::new().to_string();
    h.db.query(
        "CREATE verse CONTENT { verse_id: $vid, name: 'Visited', created_by: $did, \
         created_at: '2026-11-01T00:00:00Z', default_access: $default_access }",
    )
    .bind(("vid", verse_id.clone()))
    .bind(("did", fe_identity::new_client_did()))
    .bind(("default_access", default_access.to_string()))
    .await
    .expect("seed foreign verse")
    .check()
    .expect("foreign verse row");
    verse_id
}

/// Read the emitted token file and assert it is owner-only on disk
/// (independent oracle: `icacls` on Windows, the mode bits on unix).
fn read_owner_only_token_file(path: &std::path::Path) -> serde_json::Value {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("icacls")
            .arg(path)
            .output()
            .expect("run icacls");
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "icacls failed: {listing}");
        for broad in ["Everyone", "BUILTIN\\Users", "Authenticated Users", "(I)"] {
            assert!(
                !listing.contains(broad),
                "token file must carry only the protected owner ACE, found {broad}: {listing}"
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "token file must be owner-only");
    }
    serde_json::from_slice(&std::fs::read(path).expect("read token file")).expect("token JSON")
}

/// The agent's whole bootstrap-then-capture flow: the GUI's opt-in dev-token
/// emission (fe-api/AGENTS.md §qa-dev-token) refuses a non-loopback bind,
/// writes one Owner token per owned verse (never above the node's own role)
/// owner-only, rewrites the file when a new verse appears; then the agent
/// captures with the token READ FROM THE FILE: the request crosses the bridge,
/// the host's PNG comes back verbatim (REST body / MCP base64 image block),
/// and a second capture inside the same second is rate limited.
#[tokio::test]
async fn owner_capture_via_the_dev_token_file_returns_the_host_png_then_is_rate_limited() {
    use fe_api::dev_token::{emit_dev_tokens, DEV_TOKEN_TTL_SECS};

    let (h, rx) = harness(true).await;
    serve_captures(rx);
    let verse_a = h.seed_verse("Shot Verse").await.expect("seed verse");
    let visited = seed_foreign_verse(&h, "viewer").await;
    let shut_out = seed_foreign_verse(&h, "none").await;
    let client = fe_identity::new_client_did();
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("qa").join("dev-tokens.json");
    let mut verses = vec![verse_a.clone(), visited.clone(), shut_out.clone()];

    // 1. A non-loopback listener: refused before anything is minted or written.
    for bound in ["0.0.0.0:8765", "192.168.1.10:8765", "[::]:8765"] {
        let refused = emit_dev_tokens(
            &out,
            bound.parse().unwrap(),
            &h.db,
            &h.keypair,
            &client,
            &verses,
        )
        .await
        .expect_err(bound);
        assert!(refused.to_string().contains("not a loopback"), "{refused}");
        assert!(!out.exists(), "{bound}: nothing may be written");
    }

    // 2. Loopback: one token per verse at the node's own role, owner-only file.
    let loopback = "127.0.0.1:8765".parse().unwrap();
    let emission = emit_dev_tokens(&out, loopback, &h.db, &h.keypair, &client, &verses)
        .await
        .expect("emit");
    let doc = read_owner_only_token_file(&out);
    assert_eq!(doc["api_base"], "http://127.0.0.1:8765");
    assert_eq!(doc["client"], client.as_str());
    assert_eq!(doc["expires_at"], emission.expires_at.as_str());
    assert_eq!(doc["roles"][&verse_a], "owner");
    assert_eq!(
        doc["roles"][&visited], "viewer",
        "never above the node's role"
    );
    assert!(doc["tokens"].get(&shut_out).is_none(), "no role, no token");
    for (verse, role) in [(&verse_a, "owner"), (&visited, "viewer")] {
        let claims = fe_identity::verify_api_token(
            doc["tokens"][verse].as_str().expect("token"),
            &h.keypair.verifying_key(),
        )
        .expect("a valid token for the node's key");
        assert_eq!(claims.scope, format!("VERSE#{verse}"));
        assert_eq!(claims.max_role, role);
        assert_eq!(claims.sub, h.did());
        assert_eq!(claims.client.as_deref(), Some(client.as_str()));
        assert_eq!(claims.exp - claims.iat, DEV_TOKEN_TTL_SECS);
    }
    let first_owner_token = doc["tokens"][&verse_a].as_str().unwrap().to_string();

    // 3. A scenario creates a verse: the file is rewritten (atomically, still
    //    owner-only, no temp file left behind) and the new verse is drivable.
    let verse_b = h.seed_verse("Scenario Verse").await.expect("seed verse");
    verses.push(verse_b.clone());
    emit_dev_tokens(&out, loopback, &h.db, &h.keypair, &client, &verses)
        .await
        .expect("re-emit");
    let doc = read_owner_only_token_file(&out);
    assert_eq!(doc["roles"][&verse_b], "owner");
    assert_eq!(doc["tokens"].as_object().unwrap().len(), 3);
    assert_ne!(doc["tokens"][&verse_a], first_owner_token.as_str());
    let leftovers: Vec<_> = std::fs::read_dir(out.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("dev-tokens.json")]);

    // 4. The agent drives the GUI with tokens read from the file: the visited
    //    verse's Viewer token cannot capture; the Owner token can.
    let visitor = doc["tokens"][&visited].as_str().unwrap();
    assert_eq!(
        get_screenshot(&h, visitor).await.status,
        StatusCode::FORBIDDEN
    );
    let owner = doc["tokens"][&verse_a].as_str().unwrap().to_string();

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
