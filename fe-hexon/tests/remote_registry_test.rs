//! In-process remote registry roundtrip: spawn fe-hexon-registry, drive it via RemoteRegistryClient.

#![cfg(feature = "remote")]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use fe_format::{AssetEntry, EntryKind, HexonArchive, HexonManifest, HexonType, License};
use fe_hexon::registry_source::{FetchedHexon, RegistryEntry, RegistryError, RegistrySource, SourceTrust};
use fe_hexon::remote::{remote_extension_source, remote_source, RemoteRegistryClient};
use fe_hexon_registry::{build_router, scan_dir, RegistryState};

const ALPINE_V12_BLOB: &[u8] = b"alpine tile data v1.2.0 (newer)";

/// Minimal fixture builder (mirrors fe-hexon-registry/tests/support/fixtures.rs).
#[allow(clippy::too_many_arguments)] // test fixture — params mirror manifest fields
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
        integrity: None,
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

    // Publish leg: a HexonBuilder package is accepted, indexes on hexon_id, and downloads byte-identical.
    let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let published = fe_hexon::publisher::HexonBuilder::new(
        "published-model",
        fe_format::verifying_key_to_did(&signing_key.verifying_key()),
    )
    .name("Published Model")
    .hexon_type(HexonType::Model)
    .version("1.0.0")
    .tag("published")
    .add_entry_with_file("model_main", EntryKind::Model, "glb", "Main", b"published glb bytes")
    .build(&signing_key)
    .expect("builder output");
    client.publish(published.clone()).await.expect("registry accepts builder output");
    let hits = client.search(Some("published-model"), None, None).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].hexon_id, "published-model");
    assert_eq!(client.download("published-model").await.unwrap(), published);

    // Search by q, tags, type.
    let hits = client.search(Some("alpine"), None, None).await.unwrap();
    assert_eq!(hits.len(), 2);
    let hits = client
        .search(None, Some("gpx,running"), None)
        .await
        .unwrap();
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

    // Through the RegistrySource trait: the registry advertises protocol v1, so the listing carries each
    // content_hash and a content hexon (terrain tileset) verifies through `fetch_hexon` (R32).
    let base = format!("http://{addr}");
    let fetched = tokio::task::spawn_blocking(move || {
        let trust = SourceTrust {
            is_trust_root: Arc::new(|_| false),
            index_signer: Arc::new(|_| None),
            is_killed: fe_hexon::registry_source::no_kill_list(),
            allow_unsigned_dev: true,
        };
        let source = remote_source("test registry", base.clone(), None, trust.clone()).unwrap();
        let listing = source.list(Some("alpine")).unwrap();
        let entry = listing.entries.iter().find(|e| e.version == "1.2.0").expect("alpine 1.2.0 listed").clone();
        assert_eq!(entry.content_hash.len(), 64);
        // Unsealed fixtures have no attestation-able plugin header: consent renders after download.
        assert!(source.header(&entry).is_err());
        // A tileset is never verified as a plugin: `fetch` refuses it, `fetch_hexon` verifies it as content.
        assert!(matches!(source.fetch(&entry), Err(RegistryError::PackageInvalid(_))));
        // The Extensions source (anonymous, plugin packages only) does not offer these content fixtures.
        let extensions = remote_extension_source("extensions", base, trust).unwrap();
        assert!(extensions.list(None).unwrap().entries.is_empty());
        let wrong = RegistryEntry { content_hash: "0".repeat(64), ..entry.clone() };
        let refused = source.fetch_hexon(&wrong).unwrap_err();
        let accepted = source.fetch_hexon(&entry).unwrap();
        (refused, accepted)
    })
    .await
    .unwrap();
    assert!(matches!(fetched.0, RegistryError::ContentHashMismatch { .. }));
    match fetched.1 {
        FetchedHexon::Content(content) => {
            assert_eq!(content.hexon_id, "alpine-terrain");
            assert_eq!(content.version, "1.2.0");
        }
        other => panic!("expected a content hexon, got {other:?}"),
    }

    // A registry that advertises no protocol is still refused before any listing (fail closed).
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_addr = silent.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(silent, axum::Router::new()).await.unwrap();
    });
    let refusal = tokio::task::spawn_blocking(move || {
        let trust = SourceTrust {
            is_trust_root: Arc::new(|_| false),
            index_signer: Arc::new(|_| None),
            is_killed: fe_hexon::registry_source::no_kill_list(),
            allow_unsigned_dev: false,
        };
        remote_source("silent registry", format!("http://{silent_addr}"), None, trust).unwrap().list(None)
    })
    .await
    .unwrap();
    assert_eq!(refusal.unwrap_err(), RegistryError::ProtocolMismatch { found: None });
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
