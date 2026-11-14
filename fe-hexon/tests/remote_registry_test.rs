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

const PLUGIN_ID: &str = "org.example.fleet-sim";
const PLUGIN_EXTENSION_JSON: &[u8] =
    br#"{"id":"org.example.fleet-sim","version":"1.0.0","name":"Fleet Sim","contract":"fractalengine:plugin@2.0.0"}"#;

/// Publisher key of the sealed plugin fixture; the journey's trust root accepts exactly its did:key.
fn plugin_publisher_key() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[21u8; 32])
}

/// A sealed, signed `HexonType::Plugin` package (wasm component preamble + body).
fn plugin_fixture() -> Vec<u8> {
    let component = [&[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00][..], b"fleet-sim body"].concat();
    fe_hexon::publisher::PluginHexonBuilder::new(PLUGIN_EXTENSION_JSON, component)
        .build(&plugin_publisher_key())
        .expect("sealed plugin fixture")
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
    std::fs::write(dir.join(format!("{PLUGIN_ID}@1.0.0.hexon")), plugin_fixture()).expect("plugin fixture write failed");
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
    let plugin_bytes = client.download(PLUGIN_ID).await.unwrap();

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
        // AC-6: the Extensions source (anonymous, plugin packages only) lists exactly the sealed plugin, not the content fixtures.
        let publisher_did = fe_format::verifying_key_to_did(&plugin_publisher_key().verifying_key());
        let plugin_trust = SourceTrust::new(Arc::new(move |did: &str| did == publisher_did));
        let extensions = remote_extension_source("extensions", base.clone(), plugin_trust.clone()).unwrap();
        let plugin_listing = extensions.list(None).unwrap();
        assert_eq!(plugin_listing.entries.len(), 1);
        let plugin_entry = plugin_listing.entries[0].clone();
        assert_eq!(plugin_entry.package_id, PLUGIN_ID);
        assert_eq!(plugin_entry.version, "1.0.0");
        assert_eq!(plugin_entry.content_hash.len(), 64);
        // Header phase returns the committed extension.json before any download; fetch verifies against the listed hash.
        assert_eq!(extensions.header(&plugin_entry).unwrap(), PLUGIN_EXTENSION_JSON);
        let installed = extensions.fetch(&plugin_entry).unwrap();
        assert_eq!(installed.package.package_id, PLUGIN_ID);
        assert_eq!(installed.package.content_hash, plugin_entry.content_hash);
        assert_eq!(installed.bytes, plugin_bytes);
        // A renamed copy (signed id differs from the listed id) is refused on identity, via the uri check and via the seal reader.
        let renamed = RegistryEntry { package_id: "org.example.other".into(), ..plugin_entry.clone() };
        assert!(matches!(extensions.fetch(&renamed), Err(RegistryError::PackageInvalid(_))));
        let renamed_bytes = fe_hexon::registry_source::verify_fetched(plugin_bytes.clone(), &renamed, &plugin_trust);
        assert!(matches!(renamed_bytes, Err(RegistryError::PackageInvalid(reason)) if reason.contains("id-mismatch")));
        // The same bytes under a kill predicate that matches the hash are refused.
        let killed_hash = plugin_entry.content_hash.clone();
        let killing = plugin_trust.with_kill_list(Arc::new(move |_: &str, hash: &str| hash == killed_hash));
        let killed = remote_extension_source("extensions", base, killing).unwrap();
        assert!(matches!(killed.fetch(&plugin_entry), Err(RegistryError::Revoked { .. })));
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

    // Download progress against the real registry: monotonic, ends at (n, n), totals match the listing, bytes still verify.
    let progress_base = format!("http://{addr}");
    let fixture_dir = tmp.path().to_path_buf();
    let (events, listed_size, listed_url, tampered) = tokio::task::spawn_blocking(move || {
        let trust = SourceTrust {
            is_trust_root: Arc::new(|_| false),
            index_signer: Arc::new(|_| None),
            is_killed: fe_hexon::registry_source::no_kill_list(),
            allow_unsigned_dev: true,
        };
        let source = remote_source("progress registry", progress_base, None, trust).unwrap();
        let entry = source
            .list(Some("alpine"))
            .unwrap()
            .entries
            .into_iter()
            .find(|e| e.version == "1.2.0")
            .expect("alpine 1.2.0 listed");
        let mut events: Vec<(u64, u64)> = Vec::new();
        let fetched = source.fetch_hexon_with_progress(&entry, &mut |received, total| events.push((received, total)));
        assert!(matches!(fetched, Ok(FetchedHexon::Content(_))), "progress download must still verify");

        // One flipped byte inside manifest.json's stored data, on disk after indexing: verification must fail.
        let morning = source.list(Some("morning-run")).unwrap().entries.into_iter().next().expect("morning-run listed");
        let path = fixture_dir.join("morning-run@0.1.0.hexon");
        let mut bytes = std::fs::read(&path).unwrap();
        // The first occurrence of the name is its local file header: 30 fixed bytes, then name, extra, data.
        let name = fe_format::integrity::MANIFEST_PATH.as_bytes();
        let name_at = bytes.windows(name.len()).position(|window| window == name).expect("manifest member");
        let header_at = name_at - 30;
        let field = |offset: usize| u16::from_le_bytes([bytes[header_at + offset], bytes[header_at + offset + 1]]) as usize;
        let compressed = u32::from_le_bytes(bytes[header_at + 18..header_at + 22].try_into().unwrap()) as usize;
        let data_at = name_at + field(26) + field(28);
        assert!(compressed > 0, "manifest data length is in the local header");
        bytes[data_at + compressed / 2] ^= 0xff;
        std::fs::write(&path, bytes).unwrap();
        let mut tamper_events: Vec<(u64, u64)> = Vec::new();
        let tampered = source.fetch_hexon_with_progress(&morning, &mut |received, total| tamper_events.push((received, total)));
        assert!(tamper_events.last().is_some_and(|(received, total)| received == total), "progress completes before verification refuses");
        (events, entry.size, entry.url.clone(), tampered)
    })
    .await
    .unwrap();
    assert!(!events.is_empty());
    assert!(events.windows(2).all(|pair| pair[0].0 <= pair[1].0), "received never decreases: {events:?}");
    let (final_received, final_total) = *events.last().unwrap();
    assert_eq!(final_received, final_total, "last event is (n, n)");
    assert_eq!(final_received, listed_size, "n equals the downloaded length");
    assert!(events.iter().all(|(_, total)| *total == 0 || *total == listed_size), "totals are unknown or the listed size: {events:?}");
    assert!(tampered.is_err(), "a flipped byte must fail verification after progress completes");

    // Oversized Content-Length is refused before any byte or callback: cap below the file length.
    let cap = listed_size / 2;
    let mut capped_events: Vec<(u64, u64)> = Vec::new();
    let refused = client
        .download_capped_with_progress(&listed_url, cap, 0, &mut |received, total| capped_events.push((received, total)))
        .await;
    assert!(refused.is_err(), "a download larger than the cap is refused");
    assert!(capped_events.is_empty(), "the Content-Length check refuses before any progress event: {capped_events:?}");

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
    assert_eq!(hits.len(), 4);
    let bytes = authed.download("morning-run@0.1.0").await.unwrap();
    assert!(HexonArchive::import(&bytes).is_ok());
}
