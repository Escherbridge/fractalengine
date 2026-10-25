//! Asset ownership + upload quota through the full router (fe-api/AGENTS.md
//! §assets, §asset-ingest): an upload records its owner and anchor petal,
//! the anchor petal's scope can read it before (and after) it is placed,
//! other scopes cannot, legacy rows keep reference-only reach, and an upload
//! past the owner's quota is a 413 that writes neither a blob nor a row.
//! `CreateAsset` is answered by the production `create_asset_reply`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use fe_database::handlers::crud::{create_asset_reply, AssetRegistration};
use fe_runtime::app::PendingApiRequests;
use fe_runtime::blob_store::hash_from_hex;
use fe_runtime::messages::{ApiCommand, DbCommand, DbResult};
use fractalengine_test_harness::api::{body_bytes, body_json_lenient, ApiHarness};

/// Service the harness's API→DB channel like the DB thread: `CreateAsset`
/// is answered by the production handler against the harness DB + blob store.
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

/// A valid GLB header (version 2, declared length = actual) padded with `fill`.
fn glb(len: usize, fill: u8) -> Vec<u8> {
    let mut bytes = b"glTF".to_vec();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(len as u32).to_le_bytes());
    bytes.resize(len, fill);
    bytes
}

/// POST the GLB to `petal_id` via REST multipart; returns (status, envelope).
async fn upload(
    h: &ApiHarness,
    token: &str,
    petal_id: &str,
    bytes: &[u8],
) -> (StatusCode, serde_json::Value) {
    let boundary = "ASSETOWNERBOUNDARY";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"m.glb\"\r\n\
         Content-Type: model/gltf-binary\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/petals/{petal_id}/assets"))
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

/// GET a binary route; returns (status, body bytes).
async fn fetch(h: &ApiHarness, token: &str, uri: &str) -> (StatusCode, Vec<u8>) {
    let req = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("get request");
    let resp = h.request(req).await;
    let status = resp.status();
    (status, body_bytes(resp).await)
}

/// Seed verse → fractal → `petal_names.len()` petals; returns (verse, fractal, petals).
async fn seed_petals(h: &ApiHarness, petal_names: &[&str]) -> (String, String, Vec<String>) {
    let verse = h.seed_verse("Asset Verse").await.expect("seed verse");
    let fractal = h
        .seed_fractal(&verse, "Asset Fractal")
        .await
        .expect("fractal");
    let mut petals = Vec::new();
    for name in petal_names {
        petals.push(h.seed_petal(&fractal, name, None).await.expect("petal"));
    }
    (verse, fractal, petals)
}

/// Point an existing node at `asset_id` — the binding `place_asset` writes.
async fn reference_asset(h: &ApiHarness, petal_id: &str, asset_id: &str) {
    let node = h
        .seed_node(petal_id, "placed", [0.0, 0.0, 0.0], None)
        .await
        .expect("seed node");
    h.db.query("UPDATE node SET asset_id = $aid WHERE node_id = $nid")
        .bind(("aid", asset_id.to_string()))
        .bind(("nid", node))
        .await
        .expect("bind asset")
        .check()
        .expect("bind asset check");
}

async fn asset_rows(h: &ApiHarness) -> Vec<serde_json::Value> {
    let mut res =
        h.db.query("SELECT asset_id, owner_did, petal_id, size_bytes FROM asset")
            .await
            .expect("read assets");
    res.take(0).expect("asset rows")
}

#[tokio::test]
async fn unplaced_upload_is_readable_from_its_anchor_petal_only() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (verse, fractal, petals) = seed_petals(&h, &["Home"]).await;
    let (other_verse, other_fractal, other) = seed_petals(&h, &["Elsewhere"]).await;
    let home_scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let other_scope = fe_database::build_scope(&other_verse, Some(&other_fractal), Some(&other[0]));
    let editor = h.mint_token(&home_scope, "editor");

    let bytes = glb(64, 1);
    let (status, body) = upload(&h, &editor, &petals[0], &bytes).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let asset_id = body["data"]["asset_id"]
        .as_str()
        .expect("asset_id")
        .to_string();
    let hash = body["data"]["content_hash"]
        .as_str()
        .expect("hash")
        .to_string();

    // The row records who uploaded it and where.
    let rows = asset_rows(&h).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["owner_did"], h.did().as_str());
    assert_eq!(rows[0]["petal_id"], petals[0].as_str());

    // Not yet placed, yet the anchor petal's scope reads it by id and by hash.
    let viewer = h.mint_token(&home_scope, "viewer");
    for uri in [
        format!("/api/v1/assets/by-id/{asset_id}"),
        format!("/api/v1/assets/{hash}"),
    ] {
        let (status, got) = fetch(&h, &viewer, &uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(got, bytes, "{uri}");
    }

    // Another tenant's scope sees one uniform 404.
    let outsider = h.mint_token(&other_scope, "viewer");
    for uri in [
        format!("/api/v1/assets/by-id/{asset_id}"),
        format!("/api/v1/assets/{hash}"),
    ] {
        let (status, _) = fetch(&h, &outsider, &uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn owned_asset_placed_elsewhere_stays_readable_from_its_anchor_petal() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (verse, fractal, petals) = seed_petals(&h, &["Anchor", "Sibling"]).await;
    let anchor_scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let sibling_scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[1]));
    let editor = h.mint_token(&anchor_scope, "editor");

    let (status, body) = upload(&h, &editor, &petals[0], &glb(48, 2)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let asset_id = body["data"]["asset_id"].as_str().unwrap().to_string();
    // Placed only in the sibling petal — no referencing node in the anchor petal.
    reference_asset(&h, &petals[1], &asset_id).await;

    let uri = format!("/api/v1/assets/by-id/{asset_id}");
    let anchor_viewer = h.mint_token(&anchor_scope, "viewer");
    assert_eq!(fetch(&h, &anchor_viewer, &uri).await.0, StatusCode::OK);
    let sibling_viewer = h.mint_token(&sibling_scope, "viewer");
    assert_eq!(fetch(&h, &sibling_viewer, &uri).await.0, StatusCode::OK);
}

#[tokio::test]
async fn legacy_unowned_asset_keeps_reference_only_reach() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    let (verse, fractal, petals) = seed_petals(&h, &["Legacy"]).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let viewer = h.mint_token(&scope, "viewer");

    // A pre-ownership row: no owner_did / petal_id columns set.
    let bytes = glb(32, 3);
    let blob_store = h.state.blob_store.clone().unwrap();
    let hash = fe_runtime::blob_store::hash_to_hex(&blob_store.add_blob(&bytes).unwrap());
    let asset_id = ulid::Ulid::new().to_string();
    h.db.query(
        "CREATE asset CONTENT { asset_id: $aid, name: 'legacy.glb', \
         content_type: 'model/gltf-binary', size_bytes: 32, content_hash: $hash, \
         created_at: '2025-01-01T00:00:00Z' }",
    )
    .bind(("aid", asset_id.clone()))
    .bind(("hash", hash))
    .await
    .unwrap()
    .check()
    .unwrap();

    let uri = format!("/api/v1/assets/by-id/{asset_id}");
    assert_eq!(fetch(&h, &viewer, &uri).await.0, StatusCode::NOT_FOUND);
    reference_asset(&h, &petals[0], &asset_id).await;
    let (status, got) = fetch(&h, &viewer, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(got, bytes);
}

#[tokio::test]
async fn upload_past_the_owner_quota_is_413_and_writes_nothing() {
    let h = ApiHarness::spawn_with_upload_quota(1000)
        .await
        .expect("spawn harness");
    serve_create_asset(&h);
    let (verse, fractal, petals) = seed_petals(&h, &["Quota"]).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let editor = h.mint_token(&scope, "editor");

    let (status, body) = upload(&h, &editor, &petals[0], &glb(600, 4)).await;
    assert_eq!(status, StatusCode::CREATED, "first upload fits: {body}");

    let second = glb(600, 5);
    let (status, body) = upload(&h, &editor, &petals[0], &second).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains("quota"),
        "{body}"
    );
    assert_eq!(
        asset_rows(&h).await.len(),
        1,
        "no row for the refused upload"
    );
    let second_hash = hash_from_hex(&hex::encode(blake3::hash(&second).as_bytes())).unwrap();
    assert!(
        !h.state.blob_store.as_ref().unwrap().has_blob(&second_hash),
        "no blob for the refused upload"
    );
}

/// MCP-TEST-UPLOAD (mcp_scene_primitives Task 2.2): every refused REST upload
/// answers its distinct status + text and writes neither a row nor a blob.
/// The 201 + fetch happy row is `unplaced_upload_is_readable_from_its_anchor_petal_only`.
#[tokio::test]
async fn refused_uploads_write_nothing() {
    enum Host {
        Default,
        NoBlobStore,
    }
    let rows = [
        (
            "viewer on the anchor petal",
            Host::Default,
            "viewer",
            false,
            StatusCode::FORBIDDEN,
            "insufficient permissions",
        ),
        (
            "editor of a foreign verse",
            Host::Default,
            "editor",
            true,
            StatusCode::NOT_FOUND,
            fe_api::rest::NOT_FOUND_OR_DENIED,
        ),
        (
            "host without a blob store",
            Host::NoBlobStore,
            "editor",
            false,
            StatusCode::SERVICE_UNAVAILABLE,
            "asset upload unavailable: no blob store configured",
        ),
    ];
    for (label, host, role, foreign_token, status_want, error_want) in rows {
        let h = match host {
            Host::Default => ApiHarness::spawn().await,
            Host::NoBlobStore => ApiHarness::spawn_without_blob_store().await,
        }
        .expect("spawn harness");
        if h.state.blob_store.is_some() {
            serve_create_asset(&h);
        }
        let (verse, fractal, petals) = seed_petals(&h, &["Target"]).await;
        let (other_verse, ..) = seed_petals(&h, &["Elsewhere"]).await;
        let scope = if foreign_token {
            format!("VERSE#{other_verse}")
        } else {
            fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]))
        };
        let token = h.mint_token(&scope, role);

        let bytes = glb(64, 9);
        let (status, body) = upload(&h, &token, &petals[0], &bytes).await;
        assert_eq!(status, status_want, "{label}: {body}");
        assert_eq!(body["error"], error_want, "{label}");
        assert!(asset_rows(&h).await.is_empty(), "{label}: no asset row");
        if let Some(store) = h.state.blob_store.as_ref() {
            let hash = hash_from_hex(&hex::encode(blake3::hash(&bytes).as_bytes())).unwrap();
            assert!(!store.has_blob(&hash), "{label}: no blob");
        }
    }
}

/// A GLB whose one image lives in the BIN chunk, declared as `mime_type`.
fn glb_with_embedded_image(mime_type: &str, image: &[u8]) -> Vec<u8> {
    let mut json = format!(
        r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":{len}}}],"bufferViews":[{{"buffer":0,"byteLength":{len}}}],"images":[{{"bufferView":0,"mimeType":"{mime_type}"}}]}}"#,
        len = image.len()
    )
    .into_bytes();
    json.resize(json.len().div_ceil(4) * 4, b' ');
    let mut bin = image.to_vec();
    bin.resize(bin.len().div_ceil(4) * 4, 0);
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut bytes = b"glTF".to_vec();
    bytes.extend_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&(total as u32).to_le_bytes());
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"JSON");
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    bytes.extend_from_slice(b"BIN\0");
    bytes.extend_from_slice(&bin);
    bytes
}

/// RUSTSEC-2026-0344 interim guard (fe-runtime src/AGENTS.md §ktx2-guard): an
/// upload embedding a KTX2 texture is a 422 that writes neither a blob nor a
/// row, while the same GLB with a PNG texture is accepted.
#[tokio::test]
async fn ktx2_texture_upload_is_refused_and_writes_nothing() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (verse, fractal, petals) = seed_petals(&h, &["Textures"]).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let editor = h.mint_token(&scope, "editor");

    let mut ktx2_image = vec![
        0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
    ];
    ktx2_image.resize(48, 0);
    let refused = glb_with_embedded_image("image/ktx2", &ktx2_image);
    let (status, body) = upload(&h, &editor, &petals[0], &refused).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("RUSTSEC-2026-0344"),
        "{body}"
    );
    assert!(
        asset_rows(&h).await.is_empty(),
        "no row for the refused upload"
    );
    let refused_hash = hash_from_hex(&hex::encode(blake3::hash(&refused).as_bytes())).unwrap();
    assert!(
        !h.state.blob_store.as_ref().unwrap().has_blob(&refused_hash),
        "no blob for the refused upload"
    );

    let accepted = glb_with_embedded_image("image/png", &[0x89; 48]);
    let (status, body) = upload(&h, &editor, &petals[0], &accepted).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a PNG-textured GLB is accepted: {body}"
    );
}

// ---------------------------------------------------------------------------
// TOKEN-CLIENT-CLAIM (W6-7a): upload ownership, quota and unplaced reach key
// on the token's `client` claim, or on `sub` (the node) for a legacy token —
// fe-api/AGENTS.md §assets "owner key". The client is never an authority.
// ---------------------------------------------------------------------------

/// Call an MCP tool through the full router (`POST /mcp`, Bearer auth).
async fn mcp_call(
    h: &ApiHarness,
    token: &str,
    tool: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let (status, body) = h
        .post_json(
            "/mcp",
            Some(token),
            &serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": arguments },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{tool}: {body}");
    body
}

/// The text of an MCP tool response that must be an error.
fn mcp_error_text(resp: &serde_json::Value) -> String {
    assert_eq!(resp["result"]["isError"], true, "expected a tool error: {resp}");
    resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Upload `bytes` and return (asset_id, content_hash); panics unless 201.
async fn upload_ok(h: &ApiHarness, token: &str, petal_id: &str, bytes: &[u8]) -> (String, String) {
    let (status, body) = upload(h, token, petal_id, bytes).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (
        body["data"]["asset_id"].as_str().expect("asset_id").to_string(),
        body["data"]["content_hash"].as_str().expect("hash").to_string(),
    )
}

/// The `owner_did` recorded on `asset_id`'s row.
async fn owner_of(h: &ApiHarness, asset_id: &str) -> serde_json::Value {
    asset_rows(h)
        .await
        .into_iter()
        .find(|row| row["asset_id"] == asset_id)
        .map(|row| row["owner_did"].clone())
        .expect("asset row")
}

#[tokio::test]
async fn two_clients_of_one_node_have_separate_quota_buckets() {
    let h = ApiHarness::spawn_with_upload_quota(1000)
        .await
        .expect("spawn harness");
    serve_create_asset(&h);
    let (verse, fractal, petals) = seed_petals(&h, &["Clients"]).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let client_a = fe_identity::new_client_did();
    let client_b = fe_identity::new_client_did();
    let token_a = h.mint_client_token(&scope, "editor", &client_a);
    let token_b = h.mint_client_token(&scope, "editor", &client_b);

    let (first_a, _) = upload_ok(&h, &token_a, &petals[0], &glb(600, 1)).await;
    assert_eq!(owner_of(&h, &first_a).await, client_a.as_str());

    // Client A's bucket is full …
    let (status, body) = upload(&h, &token_a, &petals[0], &glb(600, 2)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");

    // … but client B of the same node has its own.
    let (first_b, _) = upload_ok(&h, &token_b, &petals[0], &glb(600, 3)).await;
    assert_eq!(owner_of(&h, &first_b).await, client_b.as_str());
    assert_eq!(asset_rows(&h).await.len(), 2, "only the refused upload is missing");
}

#[tokio::test]
async fn a_legacy_token_uploads_into_the_node_level_bucket() {
    let h = ApiHarness::spawn_with_upload_quota(1000)
        .await
        .expect("spawn harness");
    serve_create_asset(&h);
    let (verse, fractal, petals) = seed_petals(&h, &["Legacy bucket"]).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let legacy_one = h.mint_token(&scope, "editor");
    let legacy_two = h.mint_token(&scope, "editor");

    let (asset_id, _) = upload_ok(&h, &legacy_one, &petals[0], &glb(600, 4)).await;
    assert_eq!(
        owner_of(&h, &asset_id).await,
        h.did().as_str(),
        "a legacy token's upload is owned by the node (sub)"
    );

    // A second legacy token of the same node shares that bucket.
    let (status, body) = upload(&h, &legacy_two, &petals[0], &glb(600, 5)).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");

    // A client token is never charged to the node-level bucket.
    let client = fe_identity::new_client_did();
    let client_token = h.mint_client_token(&scope, "editor", &client);
    let (client_asset, _) = upload_ok(&h, &client_token, &petals[0], &glb(600, 6)).await;
    assert_eq!(owner_of(&h, &client_asset).await, client.as_str());
}

#[tokio::test]
async fn unplaced_upload_reach_follows_the_owner_key() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (verse, fractal, petals) = seed_petals(&h, &["Shared petal"]).await;
    let scope = fe_database::build_scope(&verse, Some(&fractal), Some(&petals[0]));
    let token_a = h.mint_client_token(&scope, "editor", &fe_identity::new_client_did());
    let token_b = h.mint_client_token(&scope, "editor", &fe_identity::new_client_did());
    let legacy = h.mint_token(&scope, "editor");

    let a_bytes = glb(64, 7);
    let (a_asset, a_hash) = upload_ok(&h, &token_a, &petals[0], &a_bytes).await;
    let node_bytes = glb(64, 8);
    let (node_asset, node_hash) = upload_ok(&h, &legacy, &petals[0], &node_bytes).await;

    let (a, b, l) = (token_a.as_str(), token_b.as_str(), legacy.as_str());
    let a_up = (a_asset.as_str(), a_hash.as_str());
    let node_up = (node_asset.as_str(), node_hash.as_str());
    let rows = [
        ("client A reads its own unplaced upload", a, a_up, StatusCode::OK),
        ("client B, same petal, reads A's unplaced upload", b, a_up, StatusCode::NOT_FOUND),
        ("a legacy token reads a node-level upload", l, node_up, StatusCode::OK),
        ("a legacy token reads client A's unplaced upload", l, a_up, StatusCode::NOT_FOUND),
        ("client B reads a node-level upload (shared)", b, node_up, StatusCode::OK),
    ];
    for (label, token, (asset_id, hash), want) in rows {
        for uri in [
            format!("/api/v1/assets/by-id/{asset_id}"),
            format!("/api/v1/assets/{hash}"),
        ] {
            let (status, got) = fetch(&h, token, &uri).await;
            assert_eq!(status, want, "{label}: {uri}");
            if want == StatusCode::OK {
                let expected = if asset_id == a_asset { &a_bytes } else { &node_bytes };
                assert_eq!(&got, expected, "{label}: {uri}");
            }
        }
    }

    // Client B cannot place A's unplaced upload over MCP: the uniform text.
    let resp = mcp_call(
        &h,
        &token_b,
        "place_asset",
        serde_json::json!({ "petal_id": petals[0], "asset_id": a_asset, "name": "Taken" }),
    )
    .await;
    assert_eq!(mcp_error_text(&resp), fe_api::upload::UNKNOWN_ASSET);

    // Once A places it, B reaches it by reference like any placed asset.
    reference_asset(&h, &petals[0], &a_asset).await;
    let (status, got) = fetch(&h, &token_b, &format!("/api/v1/assets/by-id/{a_asset}")).await;
    assert_eq!(status, StatusCode::OK, "placed: reference reach");
    assert_eq!(got, a_bytes);
}

/// Threat TC-1: a `client` claim naming a verse Manager's DID grants nothing —
/// the token stays exactly as strong as its `max_role`, and the client only
/// becomes the upload owner key.
#[tokio::test]
async fn a_client_claim_naming_a_manager_did_gains_no_role() {
    let h = ApiHarness::spawn().await.expect("spawn harness");
    serve_create_asset(&h);
    let (verse, _fractal, petals) = seed_petals(&h, &["Escalation"]).await;
    let manager_did = fe_identity::NodeKeypair::generate().to_did_key();
    h.db.query("CREATE role CONTENT { peer_did: $did, scope: $scope, role: 'manager' }")
        .bind(("did", manager_did.clone()))
        .bind(("scope", format!("VERSE#{verse}")))
        .await
        .expect("seed manager role")
        .check()
        .expect("seed manager role check");
    let token = h.mint_client_token(&format!("VERSE#{verse}"), "editor", &manager_did);

    // REST Manager-only door: refused on role.
    let (status, body) = h
        .post_json(
            &format!("/api/v1/verses/{verse}/device-formats"),
            Some(&token),
            &serde_json::json!({ "format_id": "acme.thermo/frame@1", "mime": "application/octet-stream" }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "insufficient permissions");

    // MCP Manager-only tools: refused on role.
    for (tool, args) in [
        (
            "register_device_format",
            serde_json::json!({ "verse_id": verse, "format_id": "acme.thermo/frame@1",
                                "mime": "application/octet-stream" }),
        ),
        ("create_verse", serde_json::json!({ "name": "Usurped" })),
    ] {
        let resp = mcp_call(&h, &token, tool, args).await;
        assert_eq!(mcp_error_text(&resp), "insufficient permissions", "{tool}");
    }

    // The token is still a working editor token; the client only keys ownership.
    let (asset_id, _) = upload_ok(&h, &token, &petals[0], &glb(64, 9)).await;
    assert_eq!(owner_of(&h, &asset_id).await, manager_did.as_str());
}
