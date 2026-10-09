//! Hexon/tileset round-trip through the real REST surface (F14/T2).
//!
//! Recon flagged this as zero-coverage: `fe-api/src/terrain.rs`'s hexon
//! tileset routes had never been driven through the harness. This builds a
//! real (tiny) `.hexon` `TerrainTileset` archive with `fe_format`'s own
//! exporter — the same function `fe-hexon`'s sample-builder support calls —
//! installs it via the authed multipart REST endpoint with a real petal
//! binding, then reads it back through `/api/v1/tilesets`, tileset meta, and
//! one elevation tile, and confirms a foreign-petal token is denied.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;

use fe_format::{ElevationEncoding, HexonArchive, HexonManifest, HexonType, TilesetMeta};
use fractalengine_test_harness::api::{body_json_lenient, ApiHarness};

/// A minimal, structurally valid `TerrainTileset` `.hexon` archive with one
/// elevation tile at z=5/x=10/y=12. The tile payload itself is never decoded
/// by the install/serve path (`TilesetRegistry::get_tile` returns the raw
/// stored bytes), so it is deliberately NOT a real PNG — the cheapest
/// archive that still round-trips through `fe_format::HexonArchive::import`.
fn build_tiny_hexon_tileset(hexon_id: &str) -> Vec<u8> {
    let now = chrono::Utc::now().to_rfc3339();
    let manifest = HexonManifest {
        schema_version: "1.0.0".to_string(),
        hexon_id: hexon_id.to_string(),
        hexon_type: HexonType::TerrainTileset,
        publisher_did: "did:key:z6MkFuzzTestPublisher".to_string(),
        publisher_name: None,
        version: "0.1.0".to_string(),
        build_id: None,
        name: "F14 T2 fuzz tileset".to_string(),
        description: None,
        tags: vec![],
        created_at: now.clone(),
        updated_at: now,
        source_peer_did: None,
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        signature: None,
    };
    let meta = TilesetMeta {
        bounds: [47.0, 8.0, 47.1, 8.1],
        min_zoom: 5,
        max_zoom: 5,
        tile_size: 256,
        elevation_encoding: ElevationEncoding::TerrainRgb,
        has_satellite: false,
        tile_count: 1,
        satellite_tile_count: 0,
        region_name: "F14 T2 test region".to_string(),
        parent_tileset: None,
        chunk_index: None,
        native_scale: None,
        ground_sample_distance_m: None,
        crs: None,
        scale_bounds: None,
    };
    HexonArchive::export_tileset(
        manifest,
        &meta,
        &[(
            "5/10/12".to_string(),
            b"not-a-real-png-but-stored-raw".to_vec(),
        )],
        &[],
        None,
    )
    .expect("export tiny tileset archive")
}

/// Build a `multipart/form-data` body with one file field — the shape
/// `install_hexon_tileset` reads via `multipart.next_field()`.
fn multipart_body(boundary: &str, filename: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

#[tokio::test]
async fn install_then_list_meta_and_tile_round_trip_with_petal_binding_authz() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store =
        fe_terrain::tiles::HexonStore::with_dir(tmp.path().join("tilesets")).expect("hexon store");
    let registry = std::sync::Arc::new(fe_terrain::tiles::TilesetRegistry::new(store));

    let h = ApiHarness::spawn_with_tileset_registry(registry)
        .await
        .expect("spawn harness with tileset registry");

    let hexon_id = format!("tileset-f14t2-{}", ulid::Ulid::new());
    let archive = build_tiny_hexon_tileset(&hexon_id);

    // Seed a petal whose terrain already binds this tileset id — REST
    // `install_hexon_tileset` requires exclusive prior binding before the
    // store write (fe-api/src/terrain.rs::require_exclusive_petal_tileset_binding).
    let verse_id = h.seed_verse("T2 Verse").await.expect("seed verse");
    let fractal_id = h
        .seed_fractal(&verse_id, "T2 Fractal")
        .await
        .expect("seed fractal");
    let petal_id = h
        .seed_petal(
            &fractal_id,
            "T2 Petal",
            Some(json!({ "tileset_hexon_uris": [hexon_id] })),
        )
        .await
        .expect("seed petal with terrain binding");
    let petal_scope = fe_database::build_scope(&verse_id, Some(&fractal_id), Some(&petal_id));

    let editor_token = h.mint_token(&petal_scope, "editor");
    let viewer_token = h.mint_token(&petal_scope, "viewer");

    // 1. Install via the real authed multipart REST endpoint.
    let boundary = "F14T2BOUNDARY";
    let body = multipart_body(boundary, "tileset.hexon", &archive);
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/hexons/tilesets/install?petal_id={petal_id}"
        ))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {editor_token}"))
        .body(Body::from(body))
        .expect("build multipart request");
    let resp = h.request(req).await;
    assert_eq!(resp.status(), StatusCode::OK, "install transport status");
    let installed = body_json_lenient(resp).await;
    assert_eq!(
        installed["ok"], true,
        "install must succeed with a validly bound archive: {installed}"
    );
    assert_eq!(installed["data"]["hexon_id"], hexon_id);

    // 2. List tilesets for the authorized petal — the installed tileset is visible.
    let (status, body) = h
        .get(
            &format!("/api/v1/tilesets?petal_id={petal_id}"),
            Some(&viewer_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = body["data"]
        .as_array()
        .expect("tileset list array")
        .iter()
        .filter_map(|t| t["tileset_id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec![hexon_id.as_str()],
        "exactly the bound tileset: {body}"
    );

    // 3. Fetch metadata through the authed route.
    let (status, body) = h
        .get(
            &format!("/api/v1/tilesets/{hexon_id}/meta?petal_id={petal_id}"),
            Some(&viewer_token),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["region_name"], "F14 T2 test region");
    assert_eq!(body["data"]["tile_count"], 1);

    // 4. Fetch the one elevation tile through the authed tile-serving route.
    let req = Request::builder()
        .method("GET")
        .uri(format!(
            "/api/v1/tiles/elevation/{hexon_id}/5/10/12.png?petal_id={petal_id}"
        ))
        .header("authorization", format!("Bearer {viewer_token}"))
        .body(Body::empty())
        .expect("build tile request");
    let resp = h.request(req).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "elevation tile transport status"
    );
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "image/png",
        "tile response declares image/png regardless of payload shape"
    );
    let tile_bytes = fractalengine_test_harness::api::body_bytes(resp).await;
    assert_eq!(
        tile_bytes, b"not-a-real-png-but-stored-raw",
        "the exact stored tile bytes come back unmodified"
    );

    // 5. Petal-binding authz: a token scoped to an UNRELATED petal is denied
    // on every one of the authed routes above, even though the tileset
    // genuinely exists in the local store.
    let foreign_verse = h.seed_verse("T2 Foreign Verse").await.expect("seed verse");
    let foreign_fractal = h
        .seed_fractal(&foreign_verse, "T2 Foreign Fractal")
        .await
        .expect("seed fractal");
    let foreign_petal = h
        .seed_petal(&foreign_fractal, "T2 Foreign Petal", None)
        .await
        .expect("seed foreign petal");
    let foreign_scope =
        fe_database::build_scope(&foreign_verse, Some(&foreign_fractal), Some(&foreign_petal));
    let foreign_token = h.mint_token(&foreign_scope, "editor");

    let (status, _) = h
        .get(
            &format!("/api/v1/tilesets?petal_id={petal_id}"),
            Some(&foreign_token),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a foreign-petal token must not list another petal's tilesets"
    );

    let (status, _) = h
        .get(
            &format!("/api/v1/tilesets/{hexon_id}/meta?petal_id={petal_id}"),
            Some(&foreign_token),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a foreign-petal token must not read another petal's tileset meta"
    );
}
