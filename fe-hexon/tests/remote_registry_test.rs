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

fn minimal_valid_glb() -> Vec<u8> {
    let mut json = br#"{"asset":{"version":"2.0"}}"#.to_vec();
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    let bin = [0u8; 4];
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let mut s1 = 1u32;
    let mut s2 = 0u32;
    for &b in data {
        s1 = (s1 + u32::from(b)) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    (s2 << 16) | s1
}

fn png_chunk(kind: &[u8; 4], data: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(kind);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32_ieee(&crc_in).to_be_bytes());
}

fn minimal_1x1_png() -> Vec<u8> {
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    // IHDR: 1x1, 8-bit RGBA (color_type=6)
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&1u32.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(b"IHDR", &ihdr, &mut out);
    // IDAT: zlib stored block over 1 filter byte (0) + RGBA pixel
    let raw = [0u8, 0x20, 0x80, 0xff, 0xff];
    let len = raw.len() as u16;
    let mut zlib = vec![0x78, 0x01, 0x01];
    zlib.extend_from_slice(&len.to_le_bytes());
    zlib.extend_from_slice(&(!len).to_le_bytes());
    zlib.extend_from_slice(&raw);
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());
    png_chunk(b"IDAT", &zlib, &mut out);
    png_chunk(b"IEND", &[], &mut out);
    out
}

#[tokio::test]
async fn model_library_journey() {
    let reg_dir = tempfile::tempdir().unwrap();
    let entries = scan_dir(reg_dir.path());
    let state = Arc::new(
        RegistryState::new(reg_dir.path().to_path_buf(), None, false, entries)
            .with_publish_token(Some("pub-secret".into()))
            .with_max_publish_bytes(1024 * 1024),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });

    let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let publisher_did = fe_format::verifying_key_to_did(&signing_key.verifying_key());
    let glb_bytes = minimal_valid_glb();
    let glb_hash = blake3::hash(&glb_bytes).to_hex().to_string();
    let png_bytes = minimal_1x1_png();

    let license = License {
        license_type: fe_format::LicenseType::Attribution,
        spdx_id: Some("CC-BY-4.0".into()),
        attribution: Some("Infra Kit by QA Test Studio, CC BY 4.0".into()),
        ..License::default()
    };
    let manifest = HexonManifest {
        schema_version: "1.0.0".into(),
        hexon_id: "qa.infra-kit".into(),
        hexon_type: HexonType::Model,
        publisher_did: publisher_did.clone(),
        publisher_name: Some("QA Test Studio".into()),
        version: "1.0.0".into(),
        build_id: None,
        name: "QA Infra Kit".into(),
        description: Some("Minimal model package fixture".into()),
        tags: vec!["infrastructure".into()],
        created_at: "2026-11-15T00:00:00Z".into(),
        updated_at: "2026-11-15T00:00:00Z".into(),
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
    let entry = AssetEntry {
        entry_id: "pylon".into(),
        kind: EntryKind::Model,
        asset_hash: glb_hash.clone(),
        format: "glb".into(),
        label: "Pylon".into(),
        tags: vec!["pylon".into()],
        description: None,
        is_placeable: true,
        is_private: false,
        auto_scale: false,
        preview_image: Some("preview/pylon.png".into()),
        center: None,
        extents: None,
        sub_assets: None,
        address: None,
        metadata: None,
    };
    let exported = HexonArchive::export(
        manifest,
        vec![entry.clone()],
        vec![(glb_hash.clone(), glb_bytes.clone())],
        Some(license.clone()),
        None,
        None,
        None,
    )
    .expect("export unsigned model hexon");
    let sealed = fe_hexon::publisher::HexonBuilder::new("qa.infra-kit", publisher_did)
        .name("QA Infra Kit")
        .description("Minimal model package fixture")
        .hexon_type(HexonType::Model)
        .version("1.0.0")
        .tag("infrastructure")
        .license(fe_format::LicenseType::Attribution)
        .attribution("Infra Kit by QA Test Studio, CC BY 4.0")
        .add_entry(entry, glb_bytes)
        .preview("pylon.png", png_bytes.clone())
        .build(&signing_key)
        .expect("seal model hexon");

    // 1. Publish sealed model package with publish token.
    let anon_pub = RemoteRegistryClient::new(format!("http://{addr}"), None);
    assert!(anon_pub.publish(sealed.clone()).await.is_err());

    let pub_client = RemoteRegistryClient::new(format!("http://{addr}"), Some("pub-secret".into()));
    pub_client.publish(sealed.clone()).await.expect("publish sealed model");

    // 2. Search type=model surfaces licence & preview fields.
    let hits = anon_pub.search(None, None, Some("model")).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].hexon_id, "qa.infra-kit");
    assert_eq!(hits[0].license_type.as_deref(), Some("attribution"));
    assert_eq!(
        hits[0].attribution.as_deref(),
        Some("Infra Kit by QA Test Studio, CC BY 4.0")
    );
    assert_eq!(hits[0].preview.as_deref(), Some("preview/pylon.png"));
    assert_eq!(hits[0].preview_bytes, Some(png_bytes.len() as u64));

    // 3. Fetch header and preview member and decode PNG thumbnail.
    let fetcher = fe_hexon::partial::FetchStrategy::new(fe_hexon::p2p::P2pConfig::default());
    let sources: Vec<Arc<dyn fe_hexon::partial::PartialHexonFetch>> =
        vec![Arc::new(RemoteRegistryClient::new(format!("http://{addr}"), None))];
    let header_doc = fetcher.fetch_header("qa.infra-kit@1.0.0", &sources, None).await.unwrap();
    let parsed_manifest: HexonManifest = serde_json::from_str(&header_doc.value.doc.manifest).unwrap();
    assert_eq!(parsed_manifest.hexon_id, "qa.infra-kit");
    let preview_bytes = anon_pub
        .fetch_member_capped(
            "qa.infra-kit@1.0.0",
            "preview/pylon.png",
            fe_hexon::thumbnail::MAX_THUMBNAIL_BYTES as u64,
        )
        .await
        .unwrap();
    let decoded = fe_hexon::thumbnail::decode(&preview_bytes).expect("preview PNG decodes");
    assert_eq!((decoded.width, decoded.height), (1, 1));

    // 4. Install through ModelLibrary; list() shows it and packages_for_hashes names it.
    let downloaded = anon_pub.download("qa.infra-kit@1.0.0").await.unwrap();
    let lib_dir = tempfile::tempdir().unwrap();
    let blobs = fe_hexon::registry::FsBlobStore::new(lib_dir.path().join("blobs"));
    let library = fe_hexon::model_library::ModelLibrary::open(lib_dir.path().join("models"), blobs).unwrap();
    let installed = library.install(&downloaded).expect("model install succeeds");
    assert_eq!(installed.uri, "qa.infra-kit@1.0.0");
    assert_eq!(library.list().len(), 1);
    let credits = library.packages_for_hashes(std::slice::from_ref(&glb_hash));
    assert_eq!(credits.get(&glb_hash).map(Vec::len), Some(1));

    // 5. Unsigned and tampered model packages are refused with nothing extra stored.
    assert!(matches!(
        library.install(&exported),
        Err(fe_hexon::model_library::ModelLibraryError::Unsigned)
    ));
    let mut tampered = sealed.clone();
    let mid = tampered.len() / 2;
    tampered[mid] ^= 0xff;
    assert!(library.install(&tampered).is_err());
    assert_eq!(library.list().len(), 1);

    // 6. Over-cap publish gets HTTP 413.
    let small_dir = tempfile::tempdir().unwrap();
    let small_state = Arc::new(
        RegistryState::new(small_dir.path().to_path_buf(), None, false, vec![])
            .with_publish_token(Some("pub-secret".into()))
            .with_max_publish_bytes(64),
    );
    let small_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let small_addr = small_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(small_listener, build_router(small_state)).await.unwrap();
    });
    let resp = reqwest::Client::new()
        .post(format!("http://{small_addr}/api/v1/hexons/publish"))
        .bearer_auth("pub-secret")
        .header("content-type", "application/octet-stream")
        .body(sealed)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);
}
