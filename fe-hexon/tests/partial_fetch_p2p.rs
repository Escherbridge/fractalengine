//! PAS-P5 A7 (P2P half): a peer serving sealed hexons over the PAS-P3
//! `BlobTransport` passes the same conformance suite as the hosted registry;
//! poisoned peers are refused; a fetch over real iroh is partial; and a
//! sealed container seeded through the P3P4 path keeps its signed chunks.

#![cfg(feature = "conformance")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use fe_format::{HeaderDoc, HexonArchiveIndex, Layout, TilesetBundleIndex};
use fe_hexon::p2p::{P2pConfig, PeerPriority};
use fe_hexon::partial::conformance::{self, ConformanceFixtures, FixtureHexon};
use fe_hexon::partial::{
    publish_to_transport, FetchStrategy, HexonLocator, P2pPartialFetch, PartialFetchError,
    SharedSource,
};
use fe_runtime::blob_store::hash_to_hex;
use fe_runtime::blob_transport::{
    decode_hash_seq, BlobTransport, InMemoryNetwork, InMemoryTransport,
};
use tempfile::TempDir;

/// Write a fixture's sealed bytes where `publish_to_transport` can open them.
fn write(dir: &Path, fixture: &FixtureHexon) -> PathBuf {
    let path = dir.join(fixture.file_name());
    std::fs::write(&path, &fixture.bytes).expect("fixture write failed");
    path
}

/// A P2P source fetching through `transport` from `peer`.
fn peer_source(
    transport: &InMemoryTransport,
    locator: &Arc<HexonLocator>,
    peer: &InMemoryTransport,
) -> SharedSource {
    Arc::new(P2pPartialFetch::new(
        Arc::new(transport.clone()),
        Arc::clone(locator),
        peer.provider_ref(),
        PeerPriority::Lan,
    ))
}

#[tokio::test]
async fn in_memory_p2p_source_passes_conformance() {
    let fixtures = ConformanceFixtures::build().expect("fixtures build");
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let dir = TempDir::new().unwrap();
    let locator = HexonLocator::new();
    for fixture in fixtures.all() {
        let published = publish_to_transport(&alice, &write(dir.path(), fixture))
            .await
            .expect("a sealed fixture publishes");
        assert_eq!(published.uri, fixture.uri());
        assert_eq!(published.layout, fixture.layout());
        locator.insert(&fixture.uri(), published.root, [alice.provider_ref()]);
    }

    let report = conformance::run(peer_source(&bob, &locator, &alice), &fixtures).await;

    assert!(report.passed(), "{report}");
    for name in [
        "conformance-container: chunk 2 bytes equal the signed bytes",
        "conformance-container: chunk 3 is out of range",
        "conformance-tileset: chunk 0 bytes equal the signed bytes",
        "conformance-model: member terrain/tracks/route.gpx bytes equal the archive's",
        "conformance-model: a pinned wrong publisher is refused",
        "unknown uri: the transport itself answers NotFound",
        "missing chunk: the transport itself answers NotFound or OutOfRange",
    ] {
        assert!(
            report.cases.iter().any(|case| case.name == name),
            "missing case {name}: {report}"
        );
    }
}

#[tokio::test]
async fn poisoned_peer_store_is_refused() {
    let fixtures = ConformanceFixtures::build().expect("fixtures build");
    let fixture = &fixtures.container;
    let network = InMemoryNetwork::new();
    let (mallory, carol, bob) = (
        network.transport("mallory"),
        network.transport("carol"),
        network.transport("bob"),
    );
    let dir = TempDir::new().unwrap();
    let path = write(dir.path(), fixture);
    let published = publish_to_transport(&mallory, &path).await.unwrap();
    assert_eq!(
        publish_to_transport(&carol, &path).await.unwrap().root,
        published.root,
        "one sealed hexon, one root, whoever seeds it"
    );
    let locator = HexonLocator::new();
    locator.insert(
        &fixture.uri(),
        published.root,
        [mallory.provider_ref(), carol.provider_ref()],
    );
    // Mallory's store maps chunk 1's hash to other bytes.
    mallory.tamper(published.children[2], Bytes::from_static(b"forged chunk"));

    let poisoned = peer_source(&bob, &locator, &mallory);
    let honest = peer_source(&bob, &locator, &carol);
    match poisoned.fetch_chunk(&fixture.uri(), 1).await {
        Err(PartialFetchError::Integrity(_)) => {}
        other => panic!("the transport itself must refuse forged bytes, got {other:?}"),
    }

    let strategy = FetchStrategy::new(P2pConfig::default());
    let sources = vec![Arc::clone(&poisoned), Arc::clone(&honest)];
    let header = strategy
        .fetch_header(&fixture.uri(), &sources, Some(&fixtures.publisher_did))
        .await
        .expect("mallory's header child is untouched");
    let chunk = strategy
        .fetch_chunk(&header.value, 1, &sources)
        .await
        .expect("carol serves the signed chunk");
    assert_eq!(chunk.value.as_ref(), fixture.chunks[1].as_slice());
    assert_eq!(chunk.served_by, honest.source());
    assert_eq!(
        chunk.demoted().cloned().collect::<Vec<_>>(),
        vec![poisoned.source()]
    );

    // A forged header child fails before anything else is fetched (fresh fetcher).
    mallory.tamper(published.children[0], Bytes::from_static(b"{\"v\":1}"));
    let dave = network.transport("dave");
    let forged_header = peer_source(&dave, &locator, &mallory);
    let failure = FetchStrategy::new(P2pConfig::default())
        .fetch_header(&fixture.uri(), &[forged_header], None)
        .await
        .expect_err("a forged header is refused");
    assert!(failure.error.is_integrity(), "{failure:?}");
}

/// Bind a hermetic loopback endpoint; `None` where UDP binding is unavailable.
async fn loopback_endpoint(seed: u8) -> Option<fe_sync::endpoint::SyncEndpoint> {
    match fe_sync::endpoint::SyncEndpoint::new(
        iroh::SecretKey::from_bytes(&[seed; 32]),
        &fe_sync::RelayConfig::Disabled,
    )
    .await
    {
        Ok(endpoint) => Some(endpoint),
        Err(e) => {
            eprintln!("endpoint bind failed (sandboxed env?) — skipping: {e}");
            None
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn iroh_loopback_fetches_one_chunk_without_the_rest() {
    let (Some(alice_ep), Some(bob_ep)) = (loopback_endpoint(71).await, loopback_endpoint(72).await)
    else {
        return;
    };
    let fixtures = ConformanceFixtures::build().expect("fixtures build");
    let fixture = &fixtures.container;
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let alice_stack =
        fe_sync::DocsStack::spawn(alice_ep.inner().clone(), alice_dir.path().join("p2p"))
            .await
            .expect("alice stack");
    let bob_stack = fe_sync::DocsStack::spawn(bob_ep.inner().clone(), bob_dir.path().join("p2p"))
        .await
        .expect("bob stack");
    let alice_transport = fe_sync::IrohBlobTransport::from_stack(&alice_stack);
    let published = publish_to_transport(&alice_transport, &write(alice_dir.path(), fixture))
        .await
        .expect("publish over iroh");
    assert_eq!(published.children.len(), 1 + fixture.chunks.len());

    let provider = fe_sync::IrohBlobTransport::provider_ref(&alice_ep.node_addr().await).unwrap();
    let locator = HexonLocator::new();
    locator.insert(&fixture.uri(), published.root, [provider.clone()]);
    let bob_transport: Arc<dyn BlobTransport> =
        Arc::new(fe_sync::IrohBlobTransport::from_stack(&bob_stack));
    let source: SharedSource = Arc::new(P2pPartialFetch::new(
        Arc::clone(&bob_transport),
        locator,
        provider,
        PeerPriority::Lan,
    ));
    let strategy = FetchStrategy::new(P2pConfig::default());
    let header = strategy
        .fetch_header(
            &fixture.uri(),
            std::slice::from_ref(&source),
            Some(&fixtures.publisher_did),
        )
        .await
        .expect("header over iroh");
    let chunk = strategy
        .fetch_chunk(&header.value, 1, std::slice::from_ref(&source))
        .await
        .expect("chunk 1 over iroh");
    assert_eq!(chunk.value.as_ref(), fixture.chunks[1].as_slice());

    let has_blob = |hash| {
        let transport = Arc::clone(&bob_transport);
        async move { transport.read_local(hash).await.unwrap().is_some() }
    };
    assert!(has_blob(published.children[2]).await, "chunk 1 is local");
    assert!(
        !has_blob(published.children[1]).await,
        "chunk 0 was never fetched"
    );
    assert!(
        !has_blob(published.children[3]).await,
        "chunk 2 was never fetched"
    );

    drop((source, bob_transport, alice_transport));
    alice_stack.shutdown().await;
    bob_stack.shutdown().await;
    alice_ep.shutdown().await;
    bob_ep.shutdown().await;
}

/// The children a P3P4 `PublishTileset` of `(index_path, bundle_paths)` stores.
async fn seeded_children(
    transport: &InMemoryTransport,
    dir: &Path,
    tileset_id: &str,
    index_path: &Path,
    bundle_paths: &[PathBuf],
) -> (
    fe_runtime::blob_transport::Hash32,
    Vec<fe_runtime::blob_transport::Hash32>,
) {
    let transfer = fe_sync::TilesetTransfer::for_p2p_dir(Arc::new(transport.clone()), dir);
    let entry = transfer
        .publish_files(tileset_id, index_path, bundle_paths)
        .await
        .expect("seeding publish");
    let seq = transport.read_local(entry.root).await.unwrap().unwrap();
    (entry.root, decode_hash_seq(&seq).unwrap())
}

/// Critic (§3.5 seeder rule, owner-confirmed R7): the GUI/relay seeding path
/// re-seeds a sealed container from its existing signed chunks — never
/// re-bundled — and a sealed monolithic archive whole; either way child 0 is
/// the `HeaderDoc`, and the root equals `publish_to_transport`'s, so a P2P
/// partial fetch works against any seeder.
#[tokio::test]
async fn a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling() {
    let fixtures = ConformanceFixtures::build().expect("fixtures build");
    let network = InMemoryNetwork::new();
    let (seeder, publisher, bob) = (
        network.transport("seeder"),
        network.transport("publisher"),
        network.transport("bob"),
    );
    let tmp = TempDir::new().unwrap();
    let store = fe_terrain::tiles::HexonStore::with_dir(tmp.path().join("hexons")).unwrap();

    for fixture in [&fixtures.container, &fixtures.tileset] {
        let installed = store.install_tileset(&fixture.bytes).expect("install");
        assert!(
            installed.verified,
            "{} installs as sealed",
            fixture.hexon_id
        );
        let (index_path, bundle_paths) =
            fe_terrain::tiles::bundle_for_seeding(&store, &fixture.hexon_id).expect("seed");
        let (root, children) = seeded_children(
            &seeder,
            &tmp.path().join("p2p"),
            &fixture.hexon_id,
            &index_path,
            &bundle_paths,
        )
        .await;

        let installed_path = store.base_dir().join(format!("{}.hexon", fixture.hexon_id));
        let reader = HexonArchiveIndex::open(&installed_path).unwrap();
        let header: HeaderDoc =
            serde_json::from_slice(&seeder.read_local(children[0]).await.unwrap().unwrap())
                .expect("child 0 is the HeaderDoc");
        assert_eq!(header.to_json(), reader.header_doc().to_json());
        match fixture.layout() {
            Layout::Container { .. } => {
                let signed: TilesetBundleIndex = serde_json::from_str(
                    &header.header_members[fe_format::integrity::CHUNK_INDEX_PATH],
                )
                .unwrap();
                let signed_hashes: Vec<String> = signed
                    .bundles
                    .iter()
                    .map(|b| b.blake3_hex.clone())
                    .collect();
                let seeded: Vec<String> = children[1..].iter().map(hash_to_hex).collect();
                assert_eq!(seeded, signed_hashes, "the signed chunks, never re-bundled");
            }
            Layout::Monolithic => {
                assert_eq!(children.len(), 2, "[HeaderDoc, whole archive]");
                assert_eq!(
                    seeder
                        .read_local(children[1])
                        .await
                        .unwrap()
                        .unwrap()
                        .as_ref(),
                    fixture.bytes.as_slice()
                );
            }
        }
        let direct = publish_to_transport(&publisher, &installed_path)
            .await
            .unwrap();
        assert_eq!(root, direct.root, "one root for every seeder");

        // A partial fetch against the seeder works.
        let locator = HexonLocator::new();
        locator.insert(&fixture.hexon_id, root, [seeder.provider_ref()]);
        let source = peer_source(&bob, &locator, &seeder);
        let strategy = FetchStrategy::new(P2pConfig::default());
        let fetched = strategy
            .fetch_header(
                &fixture.hexon_id,
                std::slice::from_ref(&source),
                Some(&fixtures.publisher_did),
            )
            .await
            .expect("header from the seeder");
        let first = strategy
            .fetch_chunk(&fetched.value, 0, std::slice::from_ref(&source))
            .await
            .expect("chunk 0 from the seeder");
        let expected = fixture.chunks.first().unwrap_or(&fixture.bytes);
        assert_eq!(first.value.as_ref(), expected.as_slice());
    }
}

/// The receiving half of the seeder rule: a sealed seed downloaded over the
/// P3P4 transfer installs with its seal intact (re-seedable) and streams
/// with every bundle checked against the signed root; a chunk file swapped
/// on disk after the download is refused and the store is left untouched.
#[tokio::test]
async fn a_sealed_seed_downloads_installs_sealed_and_streams_checked_bundles() {
    let fixtures = ConformanceFixtures::build().expect("fixtures build");
    let network = InMemoryNetwork::new();
    let (seeder, bob) = (network.transport("seeder"), network.transport("bob"));
    let tmp = TempDir::new().unwrap();
    let seed_store = fe_terrain::tiles::HexonStore::with_dir(tmp.path().join("seed")).unwrap();

    for fixture in [&fixtures.container, &fixtures.tileset] {
        let id = fixture.hexon_id.as_str();
        seed_store.install_tileset(&fixture.bytes).unwrap();
        let (index_path, bundle_paths) =
            fe_terrain::tiles::bundle_for_seeding(&seed_store, id).unwrap();
        let entry = fe_sync::TilesetTransfer::for_p2p_dir(
            Arc::new(seeder.clone()),
            &tmp.path().join("seed-p2p"),
        )
        .publish_files(id, &index_path, &bundle_paths)
        .await
        .unwrap();

        let fetcher = fe_sync::TilesetTransfer::for_p2p_dir(
            Arc::new(bob.clone()),
            &tmp.path().join("bob-p2p"),
        );
        let report = fetcher
            .start_download(&[seeder.provider_ref()], id, entry.root, None)
            .await
            .unwrap();
        let done = report.complete.expect("the seed downloads");
        let child0 = std::fs::read(&done.index_path).unwrap();

        // Streamed: the source is built from the signed header and checks bundles.
        let streamed = fe_terrain::tiles::StreamedTileSource::from_meta_json(
            id.to_string(),
            std::str::from_utf8(&child0).unwrap(),
        )
        .expect("a sealed child 0 becomes a streamed source");
        let first = std::fs::read(&done.bundle_paths[0]).unwrap();
        let check = streamed.bundle_check(0).expect("bundle 0 is indexed");
        check
            .verify(&first)
            .expect("the downloaded bundle passes its check");
        assert!(check.verify(b"forged bundle").is_err());

        // Install: a swapped chunk file is refused and nothing is written.
        let bob_store =
            fe_terrain::tiles::HexonStore::with_dir(tmp.path().join(format!("bob-{id}"))).unwrap();
        let original = std::fs::read(&done.bundle_paths[0]).unwrap();
        std::fs::write(&done.bundle_paths[0], b"swapped after download").unwrap();
        assert!(fe_terrain::tiles::assemble_bundles_from_index_json(
            &bob_store,
            id,
            &child0,
            &done.bundle_paths
        )
        .is_err());
        assert!(bob_store.list_installed().is_empty(), "nothing installed");
        std::fs::write(&done.bundle_paths[0], original).unwrap();
        assert!(
            fe_terrain::tiles::assemble_bundles_from_index_json(
                &bob_store,
                "another-tileset",
                &child0,
                &done.bundle_paths
            )
            .is_err(),
            "a header naming another tileset is refused"
        );

        let installed = fe_terrain::tiles::assemble_bundles_from_index_json(
            &bob_store,
            id,
            &child0,
            &done.bundle_paths,
        )
        .expect("the verified download installs");
        assert!(installed.verified, "installed with the publisher's seal");
        let stored = std::fs::read(bob_store.base_dir().join(format!("{id}.hexon"))).unwrap();
        fe_format::verify_archive(
            &stored,
            &fe_format::IntegrityPolicy::RequireSealed {
                pin: Some(fixtures.publisher_did.clone()),
            },
        )
        .expect("the stored file is re-seedable with its seal intact");
    }
}
