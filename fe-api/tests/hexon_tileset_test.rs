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
    build_hexon_tileset(
        hexon_id,
        &[(
            "5/10/12".to_string(),
            b"not-a-real-png-but-stored-raw".to_vec(),
        )],
    )
}

/// A `TerrainTileset` archive carrying `tiles` (stored, uncompressed).
fn build_hexon_tileset(hexon_id: &str, tiles: &[(String, Vec<u8>)]) -> Vec<u8> {
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
        tile_count: 1,
        region_name: "F14 T2 test region".to_string(),
        crs: None, // deliberately legacy (no CRS) — differs from Default's Some("EPSG:4326")
        ..Default::default()
    };
    HexonArchive::export_tileset(manifest, &meta, tiles, &[], None).expect("export tileset archive")
}

/// A tileset archive of at least `min_bytes` (plus a few KiB of headers),
/// padded with incompressible (xorshift) tiles of at most 1 MiB.
fn build_padded_hexon_tileset(hexon_id: &str, min_bytes: usize) -> Vec<u8> {
    const MIB: usize = 1 << 20;
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    let tiles: Vec<(String, Vec<u8>)> = (0..min_bytes.div_ceil(MIB))
        .map(|i| {
            let len = MIB.min(min_bytes - i * MIB);
            let tile = (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect();
            (format!("5/10/{i}"), tile)
        })
        .collect();
    let archive = build_hexon_tileset(hexon_id, &tiles);
    assert!(archive.len() >= min_bytes, "padding must not compress away");
    archive
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
        StatusCode::NOT_FOUND,
        "a foreign-petal token must not list another petal's tilesets (404 = unknown petal)"
    );

    let (status, _) = h
        .get(
            &format!("/api/v1/tilesets/{hexon_id}/meta?petal_id={petal_id}"),
            Some(&foreign_token),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a foreign-petal token must not read another petal's tileset meta (404 = unknown petal)"
    );
}

/// Seed a fresh petal whose terrain binds exactly `hexon_id`; returns
/// `(petal_id, editor token for that petal)`.
async fn petal_bound_to(h: &ApiHarness, hexon_id: &str) -> (String, String) {
    let verse_id = h.seed_verse("Big Verse").await.expect("seed verse");
    let fractal_id = h
        .seed_fractal(&verse_id, "Big Fractal")
        .await
        .expect("seed fractal");
    let petal_id = h
        .seed_petal(
            &fractal_id,
            "Big Petal",
            Some(json!({ "tileset_hexon_uris": [hexon_id] })),
        )
        .await
        .expect("seed petal with terrain binding");
    let scope = fe_database::build_scope(&verse_id, Some(&fractal_id), Some(&petal_id));
    let token = h.mint_token(&scope, "editor");
    (petal_id, token)
}

/// Install `archive` through the REST multipart route; returns the JSON envelope.
async fn rest_install(
    h: &ApiHarness,
    petal_id: &str,
    token: &str,
    archive: &[u8],
) -> serde_json::Value {
    let boundary = "BIGTILESETBOUNDARY";
    let req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/hexons/tilesets/install?petal_id={petal_id}"
        ))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(multipart_body(boundary, "big.hexon", archive)))
        .expect("build multipart request");
    let resp = h.request(req).await;
    assert_eq!(resp.status(), StatusCode::OK, "install transport status");
    body_json_lenient(resp).await
}

/// FEAPI-TILESET-2MIB: a ~5 MiB tileset (past axum's 2 MiB default body
/// limit) installs over BOTH surfaces; past the 11 MiB archive cap REST refuses
/// with an explicit message.
#[tokio::test]
async fn five_mib_tileset_installs_via_rest_and_mcp() {
    use base64::Engine as _;
    let tmp = tempfile::tempdir().expect("tempdir");
    let store =
        fe_terrain::tiles::HexonStore::with_dir(tmp.path().join("tilesets")).expect("hexon store");
    let registry = std::sync::Arc::new(fe_terrain::tiles::TilesetRegistry::new(store));
    let h = ApiHarness::spawn_with_tileset_registry(registry)
        .await
        .expect("spawn harness with tileset registry");
    let five_mib = 5 << 20;

    // REST multipart.
    let rest_id = format!("tileset-big-rest-{}", ulid::Ulid::new());
    let (petal_id, token) = petal_bound_to(&h, &rest_id).await;
    let installed = rest_install(
        &h,
        &petal_id,
        &token,
        &build_padded_hexon_tileset(&rest_id, five_mib),
    )
    .await;
    assert_eq!(installed["ok"], true, "REST 5 MiB install: {installed}");
    assert_eq!(installed["data"]["hexon_id"], rest_id);
    assert!(installed["data"]["size_bytes"].as_u64().unwrap() >= five_mib as u64);

    // MCP install_tileset (base64 under the 16 MiB /mcp body).
    let mcp_id = format!("tileset-big-mcp-{}", ulid::Ulid::new());
    let (petal_id, token) = petal_bound_to(&h, &mcp_id).await;
    let archive = build_padded_hexon_tileset(&mcp_id, five_mib);
    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "install_tileset",
            "arguments": {
                "petal_id": petal_id,
                "data_base64": base64::engine::general_purpose::STANDARD.encode(&archive),
            },
        },
    });
    let (status, body) = h.post_json("/mcp", Some(&token), &call).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["result"]["isError"].is_null(),
        "MCP 5 MiB install: {body}"
    );
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .expect("tool text");
    let installed: serde_json::Value = serde_json::from_str(text).expect("tool JSON");
    assert_eq!(installed["hexon_id"], mcp_id);

    // Over the archive cap but inside the route's multipart slack: explicit refusal.
    let over_id = format!("tileset-over-{}", ulid::Ulid::new());
    let (petal_id, token) = petal_bound_to(&h, &over_id).await;
    let over = build_padded_hexon_tileset(
        &over_id,
        fe_api::terrain::TILESET_ARCHIVE_MAX_BYTES + (256 << 10),
    );
    let refused = rest_install(&h, &petal_id, &token, &over).await;
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("MiB limit"),
        "{refused}"
    );
}
