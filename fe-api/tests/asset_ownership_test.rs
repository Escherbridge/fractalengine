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
