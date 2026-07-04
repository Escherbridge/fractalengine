//! In-process remote registry roundtrip: spawn fe-hexon-registry, drive it via RemoteRegistryClient.

#![cfg(feature = "remote")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fe_format::{AssetEntry, EntryKind, HexonArchive, HexonManifest, HexonType, License};
use fe_hexon::remote::RemoteRegistryClient;
use fe_hexon_registry::{build_router, scan_dir, RegistryState};

const ALPINE_V12_BLOB: &[u8] = b"alpine tile data v1.2.0 (newer)";

/// Minimal fixture builder (mirrors fe-hexon-registry/tests/support/fixtures.rs).
fn build_hexon(
    id: &str,
    version: &str,
    hexon_type: HexonType,
    name: &str,
    tags: &[&str],
    entry_id: &str,
    kind: EntryKind,
    blob: &[u8],
) -> Vec<u8> {
    let manifest = HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: id.into(),
        hexon_type,
        publisher_did: "did:key:z6MkTestPublisher".into(),
        publisher_name: None,
        version: version.into(),
        build_id: None,
        name: name.into(),
        description: Some(format!("{name} fixture")),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        created_at: "2026-07-01T00:00:00Z".into(),
        updated_at: "2026-07-01T00:00:00Z".into(),
        source_peer_did: None,
        approx_size_bytes: None,
        min_engine_version: None,
        homepage_url: None,
        dependencies: vec![],
        platforms: vec![],
        address: None,
        signature: None,
    };
    let hash = blake3::hash(blob).to_hex().to_string();
    let entry = AssetEntry {
        entry_id: entry_id.into(),
        kind,
        asset_hash: hash.clone(),
        format: "bin".into(),
        label: entry_id.into(),
        tags: vec![],
        description: None,
        is_placeable: false,
        is_private: false,
        auto_scale: false,
        preview_image: None,
        center: None,
        extents: None,
        sub_assets: None,
        address: None,
        metadata: None,
    };
    HexonArchive::export(
        manifest,
        vec![entry],
        vec![(hash, blob.to_vec())],
        Some(License::default()),
        None,
        None,
        None,
    )
    .expect("fixture export failed")
}

fn write_fixtures(dir: &Path) {
    let fixtures = [
        (
            "alpine-terrain@1.0.0.hexon",
            build_hexon(
                "alpine-terrain",
                "1.0.0",
                HexonType::Terrain,
                "Alpine Terrain Pack",
                &["terrain", "alps"],
                "tileset_a",
                EntryKind::TerrainTileset,
                b"alpine tile data v1.0.0",
            ),
        ),
        (
            "alpine-terrain@1.2.0.hexon",
            build_hexon(
                "alpine-terrain",
                "1.2.0",
                HexonType::Terrain,
                "Alpine Terrain Pack",
                &["terrain", "alps"],
                "tileset_a",
                EntryKind::TerrainTileset,
                ALPINE_V12_BLOB,
            ),
        ),
        (
            "morning-run@0.1.0.hexon",
            build_hexon(
                "morning-run",
                "0.1.0",
                HexonType::GpxCollection,
                "Morning Run",
                &["gpx", "running"],
                "morning_run",
                EntryKind::GpxTrack,
                b"<gpx/>",
            ),
        ),
    ];
    for (name, bytes) in fixtures {
        std::fs::write(dir.join(name), bytes).expect("fixture write failed");
    }
}

/// Serve the registry router on an ephemeral local port.
async fn spawn_registry(dir: &Path, token: Option<&str>) -> SocketAddr {
    let entries = scan_dir(dir);
    let state = Arc::new(RegistryState::new(
        dir.to_path_buf(),
        token.map(str::to_string),
        false,
        entries,
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn search_download_verify_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixtures(tmp.path());
    let addr = spawn_registry(tmp.path(), None).await;
    let client = RemoteRegistryClient::new(format!("http://{addr}"), None);

    // Search by q, tags, type.
    let hits = client.search(Some("alpine"), None, None).await.unwrap();
    assert_eq!(hits.len(), 2);
    let hits = client.search(None, Some("gpx,running"), None).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].hexon_id, "morning-run");
    let hits = client.search(None, None, Some("terrain")).await.unwrap();
    assert_eq!(hits.len(), 2);

    // Manifest: unversioned resolves to latest.
    let manifest = client.manifest("alpine-terrain").await.unwrap();
    assert_eq!(manifest["version"], "1.2.0");
    let manifest = client.manifest("alpine-terrain@1.0.0").await.unwrap();
    assert_eq!(manifest["version"], "1.0.0");

    // Download and verify through the archive importer (fetch-and-install path).
    let bytes = client.download("alpine-terrain").await.unwrap();
    let data = HexonArchive::import(&bytes).expect("downloaded hexon must import");
    assert_eq!(data.manifest.hexon_id, "alpine-terrain");
    assert_eq!(data.manifest.version, "1.2.0");
    assert_eq!(data.assets.len(), 1);
    assert_eq!(data.assets[0].1, ALPINE_V12_BLOB);

    // Missing package surfaces a registry error.
    assert!(client.manifest("no-such-hexon").await.is_err());
}

#[tokio::test]
async fn bearer_token_is_applied() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixtures(tmp.path());
    let addr = spawn_registry(tmp.path(), Some("s3cret")).await;

    let anon = RemoteRegistryClient::new(format!("http://{addr}"), None);
    assert!(anon.search(None, None, None).await.is_err());

    let authed = RemoteRegistryClient::new(format!("http://{addr}"), Some("s3cret".into()));
    let hits = authed.search(None, None, None).await.unwrap();
    assert_eq!(hits.len(), 3);
    let bytes = authed.download("morning-run@0.1.0").await.unwrap();
    assert!(HexonArchive::import(&bytes).is_ok());
}
