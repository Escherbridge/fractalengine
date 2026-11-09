//! Portal bundle ingest through the full router (README §8; fe-api/AGENTS.md §portal-bundles):
//! upload stores one blob and one metadata-only asset row, the by-id route answers the file
//! index, the raw blob stays attachment + nosniff, the summary's admission follows governance for
//! the local identity only, refusals store nothing, repository nodes list.
//! `CreateAsset` is answered by the production `create_asset_reply`.

use std::io::Write as _;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use fe_api::portal_bundles::{
    set_local_admission_source, AdmissionQuery, AdmissionView, LocalAdmissionSource,
};
use fe_database::handlers::crud::{create_asset_reply, AssetRegistration};
use fe_runtime::app::PendingApiRequests;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fractalengine_test_harness::api::{body_bytes, body_json_lenient, ApiHarness};
use serde_json::{json, Value};

/// Governance as the binary adapts it: an accepted kill list naming one archive digest, under no package id.
struct ArchiveKillList {
    local_did: String,
    archive_digest: String,
}

impl LocalAdmissionSource for ArchiveKillList {
    fn local_did(&self) -> String {
        self.local_did.clone()
    }
    fn admission(&self, bundle: &AdmissionQuery<'_>) -> Option<AdmissionView> {
        Some(if bundle.archive_digest == self.archive_digest {
            AdmissionView { state: "revoked".into(), reason: Some("revocation_list".into()) }
        } else {
            AdmissionView { state: "static_sandbox".into(), reason: None }
        })
    }
}

/// Service the harness's API-to-DB channel like the DB thread (`CreateAsset` only).
fn serve_create_asset(h: &ApiHarness) {
    let rx = h.api_command_receiver();
    let db = (*h.db).clone();
    let blob_store = h.state.blob_store.clone().expect("harness blob store");
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("db thread runtime");
        let mut pending = PendingApiRequests::default();
        while let Ok(ApiCommand::DbRequest { cmd, reply_tx }) = rx.recv() {
            pending.enqueue_for(&cmd, reply_tx);
            let reply = match cmd {
                DbCommand::CreateAsset {
                    name,
                    content_type,
                    size_bytes,
                    content_hash,
                    owner_did,
                    petal_id,
                    correlation_id,
                } => rt.block_on(create_asset_reply(
                    &db,
                    &blob_store,
                    AssetRegistration {
                        name,
                        content_type,
                        size_bytes,
                        content_hash,
                        owner_did,
                        petal_id,
                    },
                    correlation_id,
                )),
                other => DbResult::Error(format!("not serviced by this test: {other:?}")),
            };
            pending.try_deliver(reply);
        }
    });
}

/// The `portal` section of a docs-site bundle; `extra_csp` adds keys to `csp` (vocabulary probes).
fn extension_json(extra_csp: Option<(&str, Value)>) -> Vec<u8> {
    let mut csp = json!({
        "connect": [], "img": [], "font": [], "media": [], "frame": [],
        "wasm": false, "inline_styles": false
    });
    if let Some((key, value)) = extra_csp {
        csp[key] = value;
    }
    serde_json::to_vec(&json!({
        "manifest_version": 2,
        "id": "org.example.pump-docs",
        "version": "1.0.0",
        "kind": "web-bundle",
        "portal": {
            "schema": "fe-portal-bundle/1",
            "entry": "web/index.html",
            "title": "Pump station operating procedures",
            "csp": csp,
            "navigation": [],
            "subscriptions": { "node": false, "readings": false, "queries": [] },
            "commands": {},
            "hooks": [],
            "streams": [],
            "ui_intents": []
        }
    }))
    .expect("extension json")
}

/// An unsealed web-bundle archive holding `web_files` plus the header members.
fn unsealed_bundle(web_files: &[(&str, &[u8])], extension: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    let manifest = json!({
        "schema_version": "1.0.0",
        "hexon_id": "org.example.pump-docs",
        "hexon_type": "plugin",
        "publisher_did": "did:key:z6Mkfixture",
        "version": "1.0.0",
        "name": "Pump docs",
        "created_at": "2026-11-08T00:00:00Z",
        "updated_at": "2026-11-08T00:00:00Z"
    });
    let header: [(&str, Vec<u8>); 3] = [
        ("manifest.json", serde_json::to_vec(&manifest).expect("manifest")),
        ("entries.json", br#"{"entries":[]}"#.to_vec()),
        ("plugin/extension.json", extension.to_vec()),
    ];
    for (name, bytes) in &header {
        zip.start_file(*name, options).expect("start header");
        zip.write_all(bytes).expect("write header");
    }
    for (name, bytes) in web_files {
        zip.start_file(*name, options).expect("start member");
        zip.write_all(bytes).expect("write member");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// The docs-site fixture: an entry page and one stylesheet.
fn docs_bundle() -> Vec<u8> {
    unsealed_bundle(
        &[
            ("web/index.html", b"<!doctype html><title>docs</title>"),
            ("web/site.css", b"body{margin:0}"),
        ],
        &extension_json(None),
    )
}

/// POST `bytes` as the `file` part to the bundle route; returns (status, envelope).
async fn upload_bundle(
    h: &ApiHarness,
    token: &str,
    petal_id: &str,
    bytes: &[u8],
) -> (StatusCode, Value) {
    let boundary = "PORTALBUNDLEBOUNDARY";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"docs.hexon\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/petals/{petal_id}/portal-bundles"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .expect("upload request");
    let resp = h.request(req).await;
    let status = resp.status();
    (status, body_json_lenient(resp).await)
}

/// GET a route; returns (status, headers, body bytes).
async fn fetch(
    h: &ApiHarness,
    token: &str,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("get request");
    let resp = h.request(req).await;
    let status = resp.status();
    let headers = resp.headers().clone();
    (status, headers, body_bytes(resp).await)
}

async fn asset_count(h: &ApiHarness) -> usize {
    let mut res = h
        .db
        .query("SELECT asset_id FROM asset")
        .await
        .expect("read assets");
    let rows: Vec<Value> = res.take(0).expect("asset rows");
    rows.len()
}

/// Seed one petal and mint editor / viewer tokens at it; also returns the petal scope.
async fn seed_petal_with_tokens(h: &ApiHarness) -> (String, String, String, String) {
    let verse = h.seed_verse("Bundle Verse").await.expect("verse");
    let fractal = h.seed_fractal(&verse, "Bundle Fractal").await.expect("fractal");
    let petal = h.seed_petal(&fractal, "Docs", None).await.expect("petal");
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
    let (editor, viewer) = (h.mint_token(&scope, "editor"), h.mint_token(&scope, "viewer"));
    (petal, editor, viewer, scope)
}

/// The summary read's `admission` block for `token`, or `None` when absent.
async fn summary_admission(h: &ApiHarness, token: &str, asset_id: &str) -> Option<Value> {
    let (status, _, got) = fetch(h, token, &format!("/api/v1/portal-bundles/{asset_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let summary: Value = serde_json::from_slice(&got).expect("summary json");
    summary["data"].get("admission").cloned()
}

#[tokio::test]
async fn bundle_upload_lists_members_and_keeps_the_raw_blob_an_attachment() {
    // fe-runtime mirrors the MIME (it cannot depend on fe-format); the two copies must never drift.
    assert_eq!(
        fe_format::portal_bundle::PORTAL_BUNDLE_MIME,
        fe_runtime::scene_asset::PORTAL_BUNDLE_MIME
    );
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (petal, editor, viewer, scope) = seed_petal_with_tokens(&h).await;

    let bytes = docs_bundle();
    let (status, body) = upload_bundle(&h, &editor, &petal, &bytes).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["package_id"], "org.example.pump-docs");
    assert_eq!(data["entry"], "web/index.html");
    assert_eq!(data["sealed_by"], Value::Null, "an unsealed bundle has no publisher");
    let asset_id = data["asset_id"].as_str().expect("asset_id").to_string();
    let blob_hash = data["blob_hash"].as_str().expect("blob_hash").to_string();
    assert_eq!(
        data["content_hash"],
        blake3::hash(&bytes).to_hex().to_string().as_str(),
        "unsealed content_hash is the archive digest"
    );

    // The by-id route answers the file index (not 501) and lists the archive's member set.
    let (status, _, got) = fetch(&h, &viewer, &format!("/api/v1/assets/by-id/{asset_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let index: Value = serde_json::from_slice(&got).expect("index json");
    let mut listed: Vec<String> = index["data"]["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|m| m["path"].as_str().expect("path").to_string())
        .collect();
    listed.sort();
    assert!(listed.contains(&"web/index.html".to_string()), "{listed:?}");
    assert!(listed.contains(&"web/site.css".to_string()), "{listed:?}");
    let css = index["data"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["path"] == "web/site.css")
        .expect("css member");
    assert_eq!(css["mime"], "text/css; charset=utf-8");
    assert_eq!(css["blake3"], Value::Null, "unsealed members carry no committed digest");

    // The summary read has no members and, for a remote token, no admission block.
    let (status, _, got) = fetch(&h, &viewer, &format!("/api/v1/portal-bundles/{asset_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let summary: Value = serde_json::from_slice(&got).expect("summary json");
    assert!(summary["data"].get("members").is_none(), "{summary}");
    assert!(summary["data"].get("admission").is_none(), "{summary}");

    // Governance answers (README §8.5): the summary carries its verdict for the host's own client identity only,
    // judged on the stored archive's digest (a kill list matches by hash under any package id).
    let local_did = fe_identity::new_client_did();
    set_local_admission_source(Arc::new(ArchiveKillList {
        local_did: local_did.clone(),
        archive_digest: blob_hash.clone(),
    }));
    let local = h.mint_client_token(&scope, "viewer", &local_did);
    assert_eq!(
        summary_admission(&h, &local, &asset_id).await,
        Some(json!({ "state": "revoked", "reason": "revocation_list" }))
    );
    // A kill-listed build is not exported to render elsewhere (static-export.md §2): 409 revoked, no bytes.
    let listed = seed_repository(&h, &petal, &asset_id, None).await;
    let (status, _, got) =
        fetch(&h, &viewer, &format!("/api/v1/nodes/{listed}/portal/export.zip")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let refused: Value = serde_json::from_slice(&got).expect("refusal json");
    assert_eq!(refused["error"], "revoked", "{refused}");
    for (row, token) in [
        ("a node-level token: every accepted token's sub is this node", viewer.clone()),
        ("another API client", h.mint_client_token(&scope, "viewer", &fe_identity::new_client_did())),
    ] {
        assert_eq!(summary_admission(&h, &token, &asset_id).await, None, "{row}");
    }

    // The raw blob by hash is never served inline.
    let (status, headers, got) = fetch(&h, &viewer, &format!("/api/v1/assets/{blob_hash}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, bytes);
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert!(headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("attachment"));
}

#[tokio::test]
async fn sealed_bundle_reports_its_publisher_and_committed_member_digests() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (petal, editor, viewer, _) = seed_petal_with_tokens(&h).await;

    let key = SigningKey::from_bytes(&[7u8; 32]);
    let sealed = fe_format::integrity::seal(&docs_bundle(), &key).expect("seal");
    let (status, body) = upload_bundle(&h, &editor, &petal, &sealed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let asset_id = body["data"]["asset_id"].as_str().expect("asset_id").to_string();
    let publisher = body["data"]["sealed_by"].as_str().expect("sealed_by");
    assert!(publisher.starts_with("did:key:"), "{publisher}");
    assert_ne!(
        body["data"]["content_hash"], body["data"]["blob_hash"],
        "a sealed bundle's content_hash is the package hash, not the blob hash"
    );

    let (_, _, got) = fetch(&h, &viewer, &format!("/api/v1/assets/by-id/{asset_id}")).await;
    let index: Value = serde_json::from_slice(&got).expect("index json");
    let html = index["data"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["path"] == "web/index.html")
        .expect("html member");
    assert_eq!(
        html["blake3"],
        blake3::hash(b"<!doctype html><title>docs</title>").to_hex().to_string().as_str()
    );
}

#[tokio::test]
async fn refused_bundles_and_denied_callers_store_nothing() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (petal, editor, viewer, _) = seed_petal_with_tokens(&h).await;
    let outsider_verse = h.seed_verse("Other Verse").await.expect("verse");
    let outsider = h.mint_token(&fe_database::build_scope(&outsider_verse, None, None), "editor");

    let reserved_member = unsealed_bundle(
        &[
            ("web/index.html", b"<!doctype html>"),
            ("web/__fe/portal-v1.js", b"alert(1)"),
        ],
        &extension_json(None),
    );
    let remote_script = unsealed_bundle(
        &[("web/index.html", b"<!doctype html>")],
        &extension_json(Some(("script", json!(["https://cdn.example.net"])))),
    );
    let no_html_entry = unsealed_bundle(&[("web/site.css", b"a{}")], &extension_json(None));

    let rows: [(&str, Vec<u8>, &str); 4] = [
        ("not an archive", b"plain bytes".to_vec(), "integrity"),
        ("reserved __fe/ member", reserved_member, "member_reserved"),
        ("remote script origin", remote_script, "vocabulary_unknown"),
        ("entry names no member", no_html_entry, "entry_missing"),
    ];
    for (label, bytes, class) in rows {
        let (status, body) = upload_bundle(&h, &editor, &petal, &bytes).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{label}: {body}");
        assert_eq!(body["error"], class, "{label}: {body}");
    }
    assert_eq!(asset_count(&h).await, 0, "a refused bundle registers no asset row");

    let (status, _) = upload_bundle(&h, &viewer, &petal, &docs_bundle()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a viewer cannot upload");
    let (status, _) = upload_bundle(&h, &outsider, &petal, &docs_bundle()).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a foreign scope sees the uniform 404");
    assert_eq!(asset_count(&h).await, 0);
}

#[tokio::test]
async fn repository_node_lists_by_kind_and_carries_the_bundle_reference() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (petal, editor, viewer, _) = seed_petal_with_tokens(&h).await;

    let (_, body) = upload_bundle(&h, &editor, &petal, &docs_bundle()).await;
    let asset_id = body["data"]["asset_id"].as_str().expect("asset_id").to_string();
    let node = h
        .seed_node(
            &petal,
            "Pump docs",
            [0.0, 0.0, 0.0],
            Some(json!({ "node_kind": "repository", "portal.bundle_asset": asset_id })),
        )
        .await
        .expect("repository node");

    let (status, _, got) =
        fetch(&h, &viewer, &format!("/api/v1/petals/{petal}/nodes?kind=repository")).await;
    assert_eq!(status, StatusCode::OK);
    let listed: Value = serde_json::from_slice(&got).expect("list json");
    let rows = listed["data"].as_array().expect("rows");
    assert_eq!(rows.len(), 1, "{listed}");
    assert_eq!(rows[0]["node_id"], node.as_str());
    assert_eq!(rows[0]["properties"]["portal.bundle_asset"], asset_id.as_str());
}

/// Seed a repository node over `asset_id`, with optional `portal.feeds`.
async fn seed_repository(h: &ApiHarness, petal: &str, asset_id: &str, feeds: Option<Value>) -> String {
    let mut properties = json!({ "node_kind": "repository", "portal.bundle_asset": asset_id });
    if let Some(feeds) = feeds {
        properties["portal.feeds"] = feeds;
    }
    h.seed_node(petal, "Pump docs", [0.0, 0.0, 0.0], Some(properties))
        .await
        .expect("repository node")
}

/// Static export (static-export.md, R17): a viewer downloads the web members at the archive root plus
/// `fe-portal.json` and `__fe/feeds.json`; only reachable https share feeds survive; no page API ships;
/// nodes that are not exportable repositories answer the uniform 404.
#[tokio::test]
async fn static_export_ships_members_and_reachable_feeds_without_a_page_api() {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;

    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (petal, editor, viewer, _) = seed_petal_with_tokens(&h).await;
    let outsider_verse = h.seed_verse("Other Verse").await.expect("verse");
    let outsider = h.mint_token(&fe_database::build_scope(&outsider_verse, None, None), "viewer");

    // A distinct build, so a process-wide kill list set by another journey never names it.
    let bytes = unsealed_bundle(
        &[
            ("web/index.html", b"<!doctype html><title>dash</title>"),
            ("web/js/app.js", b"fetch('x')"),
        ],
        &extension_json(None),
    );
    let (status, body) = upload_bundle(&h, &editor, &petal, &bytes).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let asset_id = body["data"]["asset_id"].as_str().expect("asset_id").to_string();

    let token = format!("{}.c2ln", URL_SAFE_NO_PAD.encode(br#"{"exp":1900000000}"#));
    let node = seed_repository(
        &h,
        &petal,
        &asset_id,
        Some(json!({
            "flow": format!("https://relay.example.net/api/v1/shared/{token}"),
            "local": format!("http://127.0.0.1:8765/api/v1/shared/{token}"),
            "Bad Name": format!("https://relay.example.net/api/v1/shared/{token}"),
        })),
    )
    .await;

    let (status, headers, got) =
        fetch(&h, &viewer, &format!("/api/v1/nodes/{node}/portal/export.zip")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "application/zip");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(
        headers.get("content-disposition").unwrap(),
        "attachment; filename=\"org.example.pump-docs-1.0.0.zip\""
    );

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(got)).expect("export is a zip");
    let mut names: Vec<String> = archive.file_names().map(str::to_string).collect();
    names.sort();
    assert_eq!(
        names,
        ["__fe/feeds.json", "fe-portal.json", "index.html", "js/app.js"],
        "web/ prefix stripped, no __fe/portal-v1.js"
    );
    let mut read = |name: &str| -> Value {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name(name).expect("member"), &mut text).expect("utf8");
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    let descriptor = read("fe-portal.json");
    assert_eq!(descriptor["schema"], "fe-portal-export/1");
    assert_eq!(descriptor["entry"], "index.html");
    assert_eq!(descriptor["publisher_did"], Value::Null, "unsealed build");
    assert_eq!(descriptor["feeds_dropped"], json!(["Bad Name", "local"]));
    let csp = descriptor["csp"].as_str().expect("csp");
    assert!(
        csp.contains("connect-src 'self' https://relay.example.net/api/v1/shared/;"),
        "{csp}"
    );
    let feeds = read("__fe/feeds.json");
    assert_eq!(
        feeds,
        json!({ "v": 1, "feeds": { "flow": {
            "url": format!("https://relay.example.net/api/v1/shared/{token}"),
            "expires_at": "2030-03-17T17:46:40Z",
        } } })
    );

    // Denials collapse to the uniform 404: a plain node, an unknown id, a foreign scope.
    let plain = h
        .seed_node(&petal, "Plain", [0.0, 0.0, 0.0], None)
        .await
        .expect("plain node");
    let unknown = ulid::Ulid::new().to_string();
    for (row, token, id) in [
        ("not a repository node", &viewer, plain.as_str()),
        ("unknown node", &viewer, unknown.as_str()),
        ("foreign scope", &outsider, node.as_str()),
    ] {
        let (status, _, _) = fetch(&h, token, &format!("/api/v1/nodes/{id}/portal/export.zip")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{row}");
    }
}
