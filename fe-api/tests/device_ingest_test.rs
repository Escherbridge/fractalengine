//! R5 device format + device payload REST surface through the full router
//! (CDL-SPEC10 A4, fe-api/AGENTS.md §device-ingest): register → multipart
//! ingest → fetch each blob with the declared MIME + nosniff, authz/limit
//! refusals write nothing, a non-GLB asset cannot be placed as a scene node,
//! and a reading joins to its payload. Mirrors `asset_ownership_test.rs`'s
//! in-process `ApiHarness` idiom; `CreateAsset` / `RegisterDeviceFormat` /
//! `InsertDevicePayloads` / `ListDeviceFormats` / `ListDevicePayloads` are
//! answered by the production handlers against the harness DB + blob store.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fe_database::handlers::crud::{create_asset_reply, AssetRegistration};
use fe_database::handlers::device_payload;
use fe_runtime::app::PendingApiRequests;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fractalengine_test_harness::api::{body_json_lenient, ApiHarness};

/// Service every DB-thread command this surface sends: `CreateAsset` (one
/// per ingested file, `upload::register_asset_as`) plus the four R5 device
/// commands, each against the production handler.
fn serve_device_commands(h: &ApiHarness) {
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
                DbCommand::RegisterDeviceFormat {
                    verse_id,
                    descriptor,
                    declared_by,
                    correlation_id,
                } => rt.block_on(async {
                    match device_payload::register_format(&db, &verse_id, &descriptor, &declared_by)
                        .await
                    {
                        Ok(()) => DbResult::DeviceFormatRegistered {
                            verse_id,
                            format_id: descriptor.format_id,
                            correlation_id,
                        },
                        Err(e) => match e.rejection() {
                            Some(reason) => DbResult::DeviceFormatRejected {
                                verse_id,
                                reason,
                                correlation_id,
                            },
                            None => DbResult::Error(format!("RegisterDeviceFormat failed: {e}")),
                        },
                    }
                }),
                DbCommand::ListDeviceFormats {
                    verse_id,
                    correlation_id,
                } => rt.block_on(async {
                    match device_payload::list_device_formats(&db, &verse_id).await {
                        Ok(formats) => DbResult::DeviceFormatsListed {
                            verse_id,
                            formats,
                            correlation_id,
                        },
                        Err(e) => DbResult::Error(format!("ListDeviceFormats failed: {e}")),
                    }
                }),
                DbCommand::InsertDevicePayloads {
                    petal_id,
                    verse_id,
                    source_did,
                    source_class,
                    payload,
                    files,
                    correlation_id,
                } => rt.block_on(async {
                    match device_payload::insert_device_payloads_with_files(
                        &db,
                        &petal_id,
                        verse_id.as_deref(),
                        &source_did,
                        source_class,
                        &payload,
                        &files,
                        &blob_store,
                        None,
                        "did:key:harness-db",
                    )
                    .await
                    {
                        Ok((payload_id, asset_ids)) => DbResult::DevicePayloadsInserted {
                            petal_id,
                            payload_id,
                            asset_ids,
                            correlation_id,
                        },
                        Err(e) => match e.rejection() {
                            Some(reason) => DbResult::DevicePayloadsRejected {
                                petal_id,
                                reason,
                                correlation_id,
                            },
                            None => DbResult::Error(format!("InsertDevicePayloads failed: {e}")),
                        },
                    }
                }),
                DbCommand::ListDevicePayloads {
                    petal_id,
                    node_id,
                    format_id,
                    from_ms,
                    to_ms,
                    correlation_id,
                } => rt.block_on(async {
                    match device_payload::list_device_payloads(
                        &db,
                        &petal_id,
                        node_id.as_deref(),
                        format_id.as_deref(),
                        from_ms,
                        to_ms,
                    )
                    .await
                    {
                        Ok(payloads) => DbResult::DevicePayloadsListed {
                            petal_id,
                            payloads,
                            correlation_id,
                        },
                        Err(e) => DbResult::Error(format!("ListDevicePayloads failed: {e}")),
                    }
                }),
                other => DbResult::Error(format!("not serviced by this test: {other:?}")),
            };
            pending.try_deliver(reply);
        }
    });
}

/// Seed verse -> fractal -> one petal -> one anchor node; returns
/// (verse_id, fractal_id, petal_id, node_id).
async fn seed_chain(h: &ApiHarness) -> (String, String, String, String) {
    let verse = h.seed_verse("Device Verse").await.expect("verse");
    let fractal = h
        .seed_fractal(&verse, "Device Fractal")
        .await
        .expect("fractal");
    let petal = h
        .seed_petal(&fractal, "Device Petal", None)
        .await
        .expect("petal");
    let node = h
        .seed_node(&petal, "anchor", [0.0, 0.0, 0.0], None)
        .await
        .expect("node");
    (verse, fractal, petal, node)
}

/// The verse-level scope a format declaration needs: formats are verse-wide,
/// so a petal-scoped Manager token does not cover them (see
/// `petal_scoped_manager_cannot_declare_a_verse_wide_format`).
fn verse_scope(verse_id: &str) -> String {
    fe_database::build_scope(verse_id, None, None)
}

async fn register_format(
    h: &ApiHarness,
    token: &str,
    verse_id: &str,
    format_id: &str,
    mime: &str,
) -> (StatusCode, serde_json::Value) {
    h.post_json(
        &format!("/api/v1/verses/{verse_id}/device-formats"),
        Some(token),
        &serde_json::json!({ "format_id": format_id, "mime": mime }),
    )
    .await
}

/// POST a multipart device-payload ingest with `format_id`/`node_id` text
/// fields plus one ordered `file` part per entry of `files`.
async fn post_payload(
    h: &ApiHarness,
    token: &str,
    petal_id: &str,
    format_id: &str,
    node_id: &str,
    files: &[&[u8]],
) -> (StatusCode, serde_json::Value) {
    post_payload_with_id(h, token, petal_id, format_id, node_id, None, files).await
}

/// [`post_payload`] plus an optional client `payload_id` text field.
async fn post_payload_with_id(
    h: &ApiHarness,
    token: &str,
    petal_id: &str,
    format_id: &str,
    node_id: &str,
    payload_id: Option<&str>,
    files: &[&[u8]],
) -> (StatusCode, serde_json::Value) {
    let boundary = "DEVICEPAYLOADBOUNDARY";
    let mut body = Vec::new();
    let mut push_text = |name: &str, value: &str| {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    };
    push_text("format_id", format_id);
    push_text("node_id", node_id);
    if let Some(payload_id) = payload_id {
        push_text("payload_id", payload_id);
    }
    for (i, bytes) in files.iter().enumerate() {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f{i}.bin\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/petals/{petal_id}/iot/payloads"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(body))
        .expect("payload request");
    let resp = h.request(req).await;
    let status = resp.status();
    (status, body_json_lenient(resp).await)
}

/// GET a binary route; returns (status, headers, body bytes).
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
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("read body")
        .to_vec();
    (status, headers, body)
}

#[tokio::test]
async fn register_format_ingest_multipart_payload_then_fetch_each_blob() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_device_commands(&h);
    let (verse, fractal, petal, node) = seed_chain(&h).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
    let manager = h.mint_token(&verse_scope(&verse), "manager");
    let editor = h.mint_token(&scope, "editor");
    let viewer = h.mint_token(&scope, "viewer");

    let (status, body) = register_format(
        &h,
        &manager,
        &verse,
        "acme.thermo/frame@1",
        "application/octet-stream",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let file_a = b"frame bytes A".to_vec();
    let file_b = b"frame bytes B, longer".to_vec();
    let (status, body) = post_payload(
        &h,
        &editor,
        &petal,
        "acme.thermo/frame@1",
        &node,
        &[&file_a, &file_b],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let payload_id = body["payload_id"].as_str().expect("payload_id").to_string();
    let asset_ids: Vec<String> = body["asset_ids"]
        .as_array()
        .expect("asset_ids")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(asset_ids.len(), 2, "{body}");

    for (asset_id, want_bytes) in asset_ids.iter().zip([&file_a, &file_b]) {
        let (status, headers, got) =
            fetch(&h, &viewer, &format!("/api/v1/assets/by-id/{asset_id}")).await;
        assert_eq!(status, StatusCode::OK, "{asset_id}");
        assert_eq!(got, *want_bytes, "{asset_id}");
        assert_eq!(
            headers.get("content-type").unwrap(),
            "application/octet-stream",
            "{asset_id}: descriptor MIME is the registered content_type"
        );
        assert_eq!(
            headers.get("x-content-type-options").unwrap(),
            "nosniff",
            "{asset_id}"
        );
        assert!(
            headers
                .get("content-disposition")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("attachment"),
            "{asset_id}"
        );
    }

    let (status, body) = h
        .get(
            &format!("/api/v1/petals/{petal}/iot/payloads"),
            Some(&viewer),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let payloads = body["payloads"].as_array().expect("payloads");
    assert_eq!(payloads.len(), 1, "{body}");
    assert_eq!(payloads[0]["payload_id"], payload_id, "{body}");
    assert_eq!(payloads[0]["asset_ids"], serde_json::json!(asset_ids));
}

/// Every refused device-payload ingest/register-format call answers its own
/// distinct status and writes nothing further (table-driven: role, scope,
/// per-route limits, unknown format, malformed id). The happy row is
/// `register_format_ingest_multipart_payload_then_fetch_each_blob`.
/// R5 flow: a device token (client X) ingests a payload; a dashboard viewer
/// token of the SAME petal but another client (Y) reads every blob by id —
/// the payload's petal confers reference reach, so a never-placeable device
/// asset is not locked to its uploading client. A viewer scoped to another
/// petal of the same verse still gets the uniform 404.
#[tokio::test]
async fn another_clients_viewer_reads_a_device_payloads_blobs_through_its_petal() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_device_commands(&h);
    let (verse, fractal, petal, node) = seed_chain(&h).await;
    let other_petal = h
        .seed_petal(&fractal, "Other Petal", None)
        .await
        .expect("other petal");
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
    let manager = h.mint_token(&verse_scope(&verse), "manager");
    let device_client = fe_identity::NodeKeypair::generate().to_did_key();
    let dashboard_client = fe_identity::NodeKeypair::generate().to_did_key();
    let device = h.mint_client_token(&scope, "editor", &device_client);
    let dashboard = h.mint_client_token(&scope, "viewer", &dashboard_client);
    let outsider = h.mint_token(
        &fe_database::build_scope(&verse, Some(&fractal), Some(&other_petal)),
        "viewer",
    );

    let (status, body) = register_format(
        &h,
        &manager,
        &verse,
        "acme.thermo/frame@1",
        "application/octet-stream",
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let file_a = b"client X frame".to_vec();
    let (status, body) = post_payload(
        &h,
        &device,
        &petal,
        "acme.thermo/frame@1",
        &node,
        &[&file_a],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let asset_id = body["asset_ids"][0].as_str().expect("asset id").to_string();
    let path = format!("/api/v1/assets/by-id/{asset_id}");

    let (status, _, got) = fetch(&h, &dashboard, &path).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "another client of the petal reads it"
    );
    assert_eq!(got, file_a);

    let (status, _, _) = fetch(&h, &outsider, &path).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a sibling petal's viewer does not"
    );
}

#[tokio::test]
async fn device_routes_authz_and_limits() {
    // -- Viewer ingest -> 403 ------------------------------------------------
    {
        let h = ApiHarness::spawn().await.expect("spawn harness");
        serve_device_commands(&h);
        let (verse, fractal, petal, node) = seed_chain(&h).await;
        let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
        let manager = h.mint_token(&verse_scope(&verse), "manager");
        let viewer = h.mint_token(&scope, "viewer");
        register_format(&h, &manager, &verse, "acme.x/y@1", "text/plain").await;

        let (status, body) = post_payload(&h, &viewer, &petal, "acme.x/y@1", &node, &[b"x"]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "viewer ingest: {body}");
    }

    // -- Editor register-format -> 403 --------------------------------------
    {
        let h = ApiHarness::spawn().await.expect("spawn harness");
        serve_device_commands(&h);
        let (verse, fractal, petal, _node) = seed_chain(&h).await;
        let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
        let editor = h.mint_token(&scope, "editor");

        let (status, body) = register_format(&h, &editor, &verse, "acme.x/y@1", "text/plain").await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "editor register-format: {body}"
        );
    }

    // -- Foreign petal -> 404, existence-oracle-identical text --------------
    {
        let h = ApiHarness::spawn().await.expect("spawn harness");
        serve_device_commands(&h);
        let (verse, _fractal, petal, node) = seed_chain(&h).await;
        let (other_verse, ..) = seed_chain(&h).await;
        let manager = h.mint_token(&verse_scope(&verse), "manager");
        register_format(&h, &manager, &verse, "acme.x/y@1", "text/plain").await;
        let foreign = h.mint_token(&format!("VERSE#{other_verse}"), "editor");

        let (status, body) = post_payload(&h, &foreign, &petal, "acme.x/y@1", &node, &[b"x"]).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
        assert_eq!(body["error"], fe_api::rest::NOT_FOUND_OR_DENIED, "{body}");
    }

    // -- 33 files -> 413 ------------------------------------------------------
    {
        let h = ApiHarness::spawn().await.expect("spawn harness");
        serve_device_commands(&h);
        let (verse, fractal, petal, node) = seed_chain(&h).await;
        let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
        let manager = h.mint_token(&verse_scope(&verse), "manager");
        let editor = h.mint_token(&scope, "editor");
        register_format(&h, &manager, &verse, "acme.x/y@1", "text/plain").await;

        let one = b"x".to_vec();
        let files: Vec<&[u8]> = (0..33).map(|_| one.as_slice()).collect();
        let (status, body) = post_payload(&h, &editor, &petal, "acme.x/y@1", &node, &files).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    }

    // -- Over owner quota -> 413 ----------------------------------------------
    {
        let h = ApiHarness::spawn_with_upload_quota(100)
            .await
            .expect("spawn harness");
        serve_device_commands(&h);
        let (verse, fractal, petal, node) = seed_chain(&h).await;
        let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
        let manager = h.mint_token(&verse_scope(&verse), "manager");
        let editor = h.mint_token(&scope, "editor");
        register_format(&h, &manager, &verse, "acme.x/y@1", "text/plain").await;

        let big = vec![0u8; 500];
        let (status, body) = post_payload(&h, &editor, &petal, "acme.x/y@1", &node, &[&big]).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
        assert!(
            body["error"].as_str().unwrap_or_default().contains("quota"),
            "{body}"
        );
    }

    // -- Unknown format -> 422 -------------------------------------------------
    {
        let h = ApiHarness::spawn().await.expect("spawn harness");
        serve_device_commands(&h);
        let (verse, fractal, petal, node) = seed_chain(&h).await;
        let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
        let editor = h.mint_token(&scope, "editor");

        let (status, body) =
            post_payload(&h, &editor, &petal, "never.declared/fmt@1", &node, &[b"x"]).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    }

    // -- Malformed ULID -> 400 -------------------------------------------------
    {
        let h = ApiHarness::spawn().await.expect("spawn harness");
        serve_device_commands(&h);
        let (verse, fractal, petal, node) = seed_chain(&h).await;
        let editor = h.mint_token(
            &fe_database::build_scope(&verse, Some(&fractal), Some(&petal)),
            "editor",
        );

        let (status, body) =
            post_payload(&h, &editor, "not-a-ulid", "acme.x/y@1", &node, &[b"x"]).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}

/// A device-payload asset (declared non-GLB MIME) cannot become a scene
/// node — `upload::place_asset_core`'s shared GLB-only guard, exercised
/// through REST `create_node`'s `asset_id` path.
#[tokio::test]
async fn device_asset_cannot_be_placed_as_a_scene_node() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_device_commands(&h);
    let (verse, fractal, petal, node) = seed_chain(&h).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
    let manager = h.mint_token(&verse_scope(&verse), "manager");
    let editor = h.mint_token(&scope, "editor");

    register_format(
        &h,
        &manager,
        &verse,
        "acme.x/y@1",
        "application/octet-stream",
    )
    .await;
    let (status, body) =
        post_payload(&h, &editor, &petal, "acme.x/y@1", &node, &[b"opaque bytes"]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let asset_id = body["asset_ids"][0].as_str().expect("asset_id").to_string();

    let (status, body) = h
        .post_json(
            "/api/v1/nodes",
            Some(&editor),
            &serde_json::json!({
                "petal_id": petal,
                "name": "placed",
                "asset_id": asset_id,
            }),
        )
        .await;
    // Historical house shape for this route: HTTP 200 + `{ok:false, ...}` on
    // a post-authz failure (rest.rs::create_node_in_petal), distinct from
    // the 404 UNKNOWN_ASSET branch — the asset IS reachable, it is simply
    // not placeable.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false, "{body}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains("GLB"),
        "{body}"
    );
}

/// A device payload registered with `text/html` is still served as
/// `attachment` + `nosniff` — the stored-XSS regression test (CDL-SPEC10
/// A4 security review).
#[tokio::test]
async fn html_payload_is_served_as_attachment_with_nosniff() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_device_commands(&h);
    let (verse, fractal, petal, node) = seed_chain(&h).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
    let manager = h.mint_token(&verse_scope(&verse), "manager");
    let editor = h.mint_token(&scope, "editor");
    let viewer = h.mint_token(&scope, "viewer");

    register_format(&h, &manager, &verse, "acme.web/page@1", "text/html").await;
    let html = b"<script>alert(document.cookie)</script>".to_vec();
    let (status, body) =
        post_payload(&h, &editor, &petal, "acme.web/page@1", &node, &[&html]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let asset_id = body["asset_ids"][0].as_str().expect("asset_id").to_string();

    let (status, headers, got) =
        fetch(&h, &viewer, &format!("/api/v1/assets/by-id/{asset_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, html);
    assert_eq!(headers.get("content-type").unwrap(), "text/html");
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert!(headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("attachment"));

    // A portal bundle's HTML members are never served inline either: the raw archive blob stays
    // attachment + nosniff, and the by-id route answers a JSON file index, not the HTML.
    let bundle = html_bundle(&html);
    let boundary = "BUNDLEBOUNDARY";
    let mut multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"b.hexon\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    multipart.extend_from_slice(&bundle);
    multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/petals/{petal}/portal-bundles"))
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .header("authorization", format!("Bearer {editor}"))
        .body(Body::from(multipart))
        .expect("bundle upload request");
    let resp = h.request(req).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let uploaded = body_json_lenient(resp).await;
    let bundle_id = uploaded["data"]["asset_id"].as_str().expect("asset_id").to_string();
    let bundle_blob = uploaded["data"]["blob_hash"].as_str().expect("blob_hash").to_string();

    let (status, headers, got) = fetch(&h, &viewer, &format!("/api/v1/assets/{bundle_blob}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, bundle);
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert!(headers
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("attachment"));
    let (status, headers, got) =
        fetch(&h, &viewer, &format!("/api/v1/assets/by-id/{bundle_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert!(
        headers.get("content-type").unwrap().to_str().unwrap().starts_with("application/json"),
        "a bundle id answers its file index, never an inline HTML member"
    );
    assert!(!got.windows(8).any(|w| w == b"<script>"), "no member bytes in the index");
}

/// An unsealed web-bundle archive whose entry page is `html` (a stored-XSS probe).
fn html_bundle(html: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let extension = serde_json::json!({
        "manifest_version": 2, "id": "org.example.xss-probe", "version": "1.0.0",
        "kind": "web-bundle",
        "portal": {
            "schema": "fe-portal-bundle/1", "entry": "web/index.html", "title": "Probe",
            "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": [],
                     "wasm": false, "inline_styles": false },
            "navigation": [],
            "subscriptions": { "node": false, "readings": false, "queries": [] },
            "commands": {}, "hooks": [], "streams": [], "ui_intents": []
        }
    });
    let manifest = serde_json::json!({
        "schema_version": "1.0.0", "hexon_id": "org.example.xss-probe",
        "hexon_type": "plugin", "publisher_did": "did:key:z6Mkfixture", "version": "1.0.0",
        "name": "Probe", "created_at": "2026-11-08T00:00:00Z",
        "updated_at": "2026-11-08T00:00:00Z"
    });
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("entries.json", br#"{"entries":[]}"#.to_vec()),
        ("plugin/extension.json", serde_json::to_vec(&extension).unwrap()),
        ("web/index.html", html.to_vec()),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(&bytes).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// An IoT reading posted with `payload_id` set joins to the device payload it
/// names; an unknown `payload_id` rejects the whole batch (A3's
/// `insert_readings_with_replication` invariant, exercised over REST — no A4
/// code change was needed for this, only the flow test).
#[tokio::test]
async fn readings_posted_with_payload_id_join_to_the_payload() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_device_commands(&h);
    let (verse, fractal, petal, node) = seed_chain(&h).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petal));
    let manager = h.mint_token(&verse_scope(&verse), "manager");
    let editor = h.mint_token(&scope, "editor");

    register_format(
        &h,
        &manager,
        &verse,
        "acme.x/y@1",
        "application/octet-stream",
    )
    .await;
    let (status, body) = post_payload(&h, &editor, &petal, "acme.x/y@1", &node, &[b"bytes"]).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let payload_id = body["payload_id"].as_str().expect("payload_id").to_string();

    let (status, body) = h
        .post_json(
            &format!("/api/v1/petals/{petal}/iot/readings"),
            Some(&editor),
            &serde_json::json!({
                "readings": [{
                    "node_id": node,
                    "metric": "temperature_c",
                    "value": 21.5,
                    "payload_id": payload_id,
                }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["accepted"], 1, "{body}");

    // An unknown payload_id rejects the whole batch (422).
    let (status, body) = h
        .post_json(
            &format!("/api/v1/petals/{petal}/iot/readings"),
            Some(&editor),
            &serde_json::json!({
                "readings": [{
                    "node_id": node,
                    "metric": "temperature_c",
                    "value": 21.5,
                    "payload_id": "01ARZ3NDEKTSV4RRFFQ69G5FAV",
                }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
}

/// CDL-SPEC10 Phase A security (MANDATORY finding 2 + ULID validation): a
/// malformed client `payload_id` is a 400 before any byte is stored, and a
/// caller in another petal replaying a taken id gets a 409 whose body names
/// none of the existing payload's asset ids.
#[tokio::test]
async fn client_payload_id_is_validated_and_a_foreign_replay_learns_nothing() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_device_commands(&h);
    let (verse_a, fractal_a, petal_a, node_a) = seed_chain(&h).await;
    let (verse_b, fractal_b, petal_b, node_b) = seed_chain(&h).await;
    let scope_a = fe_database::build_scope(&verse_a, Some(&fractal_a), Some(&petal_a));
    let scope_b = fe_database::build_scope(&verse_b, Some(&fractal_b), Some(&petal_b));
    for verse in [&verse_a, &verse_b] {
        let manager = h.mint_token(&verse_scope(verse), "manager");
        register_format(
            &h,
            &manager,
            verse,
            "acme.x/y@1",
            "application/octet-stream",
        )
        .await;
    }
    let editor_a = h.mint_token(&scope_a, "editor");
    let editor_b = h.mint_token(&scope_b, "editor");

    // Malformed id -> 400, and no asset row was written for the file.
    let (status, body) = post_payload_with_id(
        &h,
        &editor_a,
        &petal_a,
        "acme.x/y@1",
        &node_a,
        Some("client-chosen-1"),
        &[b"bytes"],
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let mut res = h.db.query("SELECT asset_id FROM asset").await.unwrap();
    let assets: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    assert!(
        assets.is_empty(),
        "no byte stored before the id check: {assets:?}"
    );

    // Petal A ingests with a client id.
    let client_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let (status, body) = post_payload_with_id(
        &h,
        &editor_a,
        &petal_a,
        "acme.x/y@1",
        &node_a,
        Some(client_id),
        &[b"petal a secret"],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let victim_asset = body["asset_ids"][0].as_str().expect("asset id").to_string();

    // Petal B replays it: 409, nothing about petal A's payload in the body.
    let (status, body) = post_payload_with_id(
        &h,
        &editor_b,
        &petal_b,
        "acme.x/y@1",
        &node_b,
        Some(client_id),
        &[b"petal b bytes"],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.get("asset_ids").is_none(), "{body}");
    assert!(!body.to_string().contains(&victim_asset), "{body}");
}

/// A format is verse-wide and immutable per id, so only a token whose scope
/// covers the whole verse may declare one: a petal-scoped Manager gets the
/// oracle-identical 404 and nothing is written (otherwise it could squat a
/// format id, with the wrong MIME, for every other petal of the verse).
#[tokio::test]
async fn petal_scoped_manager_cannot_declare_a_verse_wide_format() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_device_commands(&h);
    let (verse, fractal, petal, _node) = seed_chain(&h).await;
    let petal_manager = h.mint_token(
        &fe_database::build_scope(&verse, Some(&fractal), Some(&petal)),
        "manager",
    );

    let (status, body) =
        register_format(&h, &petal_manager, &verse, "acme.x/y@1", "text/plain").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], fe_api::rest::NOT_FOUND_OR_DENIED, "{body}");
    let mut res =
        h.db.query("SELECT format_id FROM device_format")
            .await
            .unwrap();
    let rows: Vec<serde_json::Value> = res.take(0).unwrap_or_default();
    assert!(rows.is_empty(), "{rows:?}");
}
