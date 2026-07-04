//! In-process router tests via tower::ServiceExt::oneshot.

#[path = "support/fixtures.rs"]
mod fixtures;

use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use fe_format::{EntryKind, HexonArchive, HexonType};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

use fe_hexon_registry::{build_router, scan_dir, RegistryState};

fn router_for(dir: &Path, token: Option<&str>, readonly: bool) -> Router {
    let entries = scan_dir(dir);
    build_router(Arc::new(RegistryState::new(
        dir.to_path_buf(),
        token.map(str::to_string),
        readonly,
        entries,
    )))
}

async fn get(router: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    let resp = router
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, body)
}

fn json_data(body: &[u8]) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_slice(body).expect("body not JSON");
    assert_eq!(v["ok"], true, "expected ok envelope, got: {v}");
    v["data"].clone()
}

#[tokio::test]
async fn health_reports_indexed_count() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    let (status, body) = get(&router, "/health").await;
    assert_eq!(status, StatusCode::OK);
    let data = json_data(&body);
    assert_eq!(data["status"], "ok");
    assert_eq!(data["indexed"], 3);
}

#[tokio::test]
async fn search_filters_q_tags_and_type() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    // q: substring on id/name/description
    let (status, body) = get(&router, "/api/v1/hexons/search?q=alpine").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_data(&body).as_array().unwrap().len(), 2);

    // tags: all must match
    let (_, body) = get(&router, "/api/v1/hexons/search?tags=gpx,running").await;
    let hits = json_data(&body);
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["hexon_id"], "morning-run");

    // tags: partial mismatch excludes
    let (_, body) = get(&router, "/api/v1/hexons/search?tags=gpx,alps").await;
    assert_eq!(json_data(&body).as_array().unwrap().len(), 0);

    // type filter
    let (_, body) = get(&router, "/api/v1/hexons/search?type=terrain").await;
    assert_eq!(json_data(&body).as_array().unwrap().len(), 2);

    // combined
    let (_, body) = get(&router, "/api/v1/hexons/search?q=run&type=gpx_collection").await;
    let hits = json_data(&body);
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["version"], "0.1.0");
}

#[tokio::test]
async fn manifest_versioned_and_unversioned() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    let (status, body) = get(&router, "/api/v1/hexons/alpine-terrain@1.0.0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_data(&body)["version"], "1.0.0");

    // Unversioned resolves to latest semver.
    let (status, body) = get(&router, "/api/v1/hexons/alpine-terrain").await;
    assert_eq!(status, StatusCode::OK);
    let data = json_data(&body);
    assert_eq!(data["version"], "1.2.0");
    assert_eq!(data["hexon_type"], "terrain");

    // Percent-encoded @ also resolves.
    let (status, body) = get(&router, "/api/v1/hexons/alpine-terrain%401.0.0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_data(&body)["version"], "1.0.0");

    let (status, _) = get(&router, "/api/v1/hexons/no-such-hexon").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn entries_returns_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    let (status, body) = get(&router, "/api/v1/hexons/morning-run/entries").await;
    assert_eq!(status, StatusCode::OK);
    let data = json_data(&body);
    let entries = data["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["entry_id"], "morning_run");
    assert_eq!(entries[0]["kind"], "gpx_track");
}

#[tokio::test]
async fn asset_streams_blob_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/hexons/morning-run@0.1.0/entries/morning_run/asset")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let len: usize = resp
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .expect("missing content-length");
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(len, fixtures::GPX_BLOB.len());
    assert_eq!(body.as_ref(), fixtures::GPX_BLOB);

    // Unknown entry -> 404
    let (status, _) = get(&router, "/api/v1/hexons/morning-run/entries/nope/asset").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn download_roundtrips_through_import() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/hexons/alpine-terrain@1.2.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/x-hexon+zip"
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let data = HexonArchive::import(&bytes).expect("downloaded hexon must import");
    assert_eq!(data.manifest.hexon_id, "alpine-terrain");
    assert_eq!(data.manifest.version, "1.2.0");
    assert_eq!(data.assets.len(), 1);
    assert_eq!(data.assets[0].1, fixtures::ALPINE_V12_BLOB);
}

async fn post_publish(router: &Router, bytes: Vec<u8>, token: Option<&str>) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method("POST").uri("/api/v1/hexons/publish");
    if let Some(t) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = router
        .clone()
        .oneshot(builder.body(Body::from(bytes)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, body)
}

fn night_sky_hexon() -> Vec<u8> {
    fixtures::build_hexon(
        "night-sky",
        "2.0.0",
        HexonType::Skybox,
        "Night Sky",
        "Starfield skybox",
        &["skybox", "night"],
        "starfield",
        EntryKind::Skybox,
        b"hdr starfield bytes",
    )
}

#[tokio::test]
async fn publish_accepts_and_indexes() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    let (status, body) = post_publish(&router, night_sky_hexon(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_data(&body)["hexon_id"], "night-sky");
    assert!(tmp.path().join("night-sky@2.0.0.hexon").exists());

    // Immediately searchable after reindex.
    let (_, body) = get(&router, "/api/v1/hexons/search?type=skybox").await;
    assert_eq!(json_data(&body).as_array().unwrap().len(), 1);
    let (status, body) = get(&router, "/api/v1/hexons/night-sky").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json_data(&body)["version"], "2.0.0");
}

#[tokio::test]
async fn publish_rejects_duplicate() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, false);

    let (status, _) = post_publish(&router, night_sky_hexon(), None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = post_publish(&router, night_sky_hexon(), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn publish_rejects_readonly() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), None, true);

    let (status, _) = post_publish(&router, night_sky_hexon(), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn publish_rejects_garbage_body() {
    let tmp = tempfile::tempdir().unwrap();
    let router = router_for(tmp.path(), None, false);

    let (status, _) = post_publish(&router, b"not a zip".to_vec(), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn token_gates_api_when_set() {
    let tmp = tempfile::tempdir().unwrap();
    fixtures::write_fixture_set(tmp.path());
    let router = router_for(tmp.path(), Some("s3cret"), false);

    // Publish without / with wrong / with correct token.
    let (status, _) = post_publish(&router, night_sky_hexon(), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post_publish(&router, night_sky_hexon(), Some("wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = post_publish(&router, night_sky_hexon(), Some("s3cret")).await;
    assert_eq!(status, StatusCode::OK);

    // Reads are gated too when a token is configured.
    let (status, _) = get(&router, "/api/v1/hexons/search").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let resp = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/hexons/search")
                .header(header::AUTHORIZATION, "Bearer s3cret")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Health stays public.
    let (status, _) = get(&router, "/health").await;
    assert_eq!(status, StatusCode::OK);
}
