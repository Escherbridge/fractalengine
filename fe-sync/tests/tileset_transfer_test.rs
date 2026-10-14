//! PAS-P3 S2 flows: publish → resolve → fetch → verify → persist → resume over
//! `TilesetTransfer`, on the in-memory network and on real iroh loopback stacks.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use fe_runtime::blob_transport::{
    content_hash, BlobTransport, Hash32, InMemoryNetwork, InMemoryTransport, ProviderRef,
};
use fe_sync::endpoint::SyncEndpoint;
use fe_sync::tileset_transfer::{
    download_tag, TransferLimits, DOWNLOAD_TAG_PREFIX, LEDGER_FILE, MAX_HASH_SEQ_CHILDREN,
};
use fe_sync::{
    ChunkError, DocsStack, IrohBlobTransport, RelayConfig, TilesetTransfer, TransferError,
    TransferEvent,
};
use tempfile::TempDir;

const BUNDLE_BYTES: usize = 64 * 1024;

/// A synthetic published tileset: opaque index + pseudo-random bundles on disk.
struct SourceTileset {
    index_json: Bytes,
    bundle_paths: Vec<PathBuf>,
    bundles: Vec<Vec<u8>>,
}

impl SourceTileset {
    fn write(dir: &Path, tileset_id: &str, count: usize) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        let index_json = Bytes::from(format!(
            r#"{{"schema":1,"tileset_id":"{tileset_id}","bundles":{count}}}"#
        ));
        let mut bundles = Vec::new();
        let mut bundle_paths = Vec::new();
        for seq in 0..count {
            let mut bytes = vec![0u8; BUNDLE_BYTES];
            blake3::Hasher::new()
                .update(format!("{tileset_id}/{seq}").as_bytes())
                .finalize_xof()
                .fill(&mut bytes);
            let path = dir.join(format!("{seq}.bundle"));
            std::fs::write(&path, &bytes).unwrap();
            bundle_paths.push(path);
            bundles.push(bytes);
        }
        Self {
            index_json,
            bundle_paths,
            bundles,
        }
    }

    fn chunk_hash(&self, seq: usize) -> Hash32 {
        content_hash(&self.bundles[seq])
    }
}

fn service(transport: &InMemoryTransport, root: &Path) -> TilesetTransfer {
    TilesetTransfer::for_p2p_dir(Arc::new(transport.clone()), root)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(files_under(&path));
            }
            out.push(path);
        }
    }
    out
}

#[tokio::test]
async fn a_published_tileset_downloads_verified_and_complete() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "valley", 5);
    let publisher = service(&alice, alice_dir.path());
    let entry = publisher
        .publish("valley", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();
    assert_eq!(entry.total_chunks, 5);
    assert_eq!(entry.bytes, (5 * BUNDLE_BYTES) as u64);

    let fetcher = service(&bob, bob_dir.path());
    let meta = fetcher
        .request_meta(&[alice.provider_ref()], "valley", entry.root)
        .await
        .unwrap();
    assert_eq!(
        meta.index_json, source.index_json,
        "index passes through opaque"
    );
    assert_eq!(std::fs::read(&meta.index_path).unwrap(), source.index_json);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let report = fetcher.download("valley", Some(&tx)).await.unwrap();

    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.stored, vec![0, 1, 2, 3, 4]);
    let complete = report.complete.expect("download completes");
    assert_eq!(complete.bundle_paths.len(), 5);
    for (seq, path) in complete.bundle_paths.iter().enumerate() {
        let on_disk = std::fs::read(path).unwrap();
        assert_eq!(content_hash(&on_disk), meta.chunk_hashes[seq]);
        assert_eq!(on_disk, source.bundles[seq]);
    }
    let mut stored_events = 0;
    let mut saw_complete = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            TransferEvent::ChunkStored { .. } => stored_events += 1,
            TransferEvent::Complete(done) => saw_complete = done == complete,
            TransferEvent::ChunkFailed { .. } => panic!("no chunk should fail"),
        }
    }
    assert_eq!(stored_events, 5);
    assert!(saw_complete);
}

#[tokio::test]
async fn a_tampered_bundle_is_rejected_and_never_persisted() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "ridge", 4);
    let entry = service(&alice, alice_dir.path())
        .publish("ridge", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();
    alice.tamper(source.chunk_hash(2), Bytes::from_static(b"forged bundle"));

    let fetcher = service(&bob, bob_dir.path());
    let first = fetcher
        .start_download(&[alice.provider_ref()], "ridge", entry.root, None)
        .await
        .unwrap();
    assert_eq!(
        first.failed,
        vec![(
            2,
            ChunkError::HashMismatch {
                expected: source.chunk_hash(2),
                actual: content_hash(b"forged bundle"),
            }
        )]
    );
    assert!(first.complete.is_none());
    let download_dir = fetcher.download_dir("ridge").unwrap();
    assert!(
        !download_dir.join("2.bundle").exists(),
        "tampered bytes never land"
    );
    assert!(
        files_under(&download_dir)
            .iter()
            .all(|p| !p.to_string_lossy().contains(".tmp-")),
        "no temp file is left behind"
    );

    alice.repair(source.chunk_hash(2));
    let retry = fetcher.download("ridge", None).await.unwrap();
    assert_eq!(
        retry.stored,
        vec![2],
        "only the rejected bundle is fetched again"
    );
    let complete = retry.complete.expect("retry completes");
    assert_eq!(
        std::fs::read(&complete.bundle_paths[2]).unwrap(),
        source.bundles[2]
    );
}

#[tokio::test]
async fn an_interrupted_download_resumes_after_restart_without_refetching() {
    let network = InMemoryNetwork::new();
    let alice = network.transport("alice");
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "coast", 4);
    let entry = service(&alice, alice_dir.path())
        .publish("coast", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();

    {
        // A full download is interrupted after bundles 0-1 (alice cannot
        // serve 2-3 yet), so its ledger is marked for resume.
        alice.withhold(source.chunk_hash(2));
        alice.withhold(source.chunk_hash(3));
        let before_restart = service(&network.transport("bob"), bob_dir.path());
        let first = before_restart
            .start_download(&[alice.provider_ref()], "coast", entry.root, None)
            .await
            .unwrap();
        assert_eq!(first.stored, vec![0, 1]);
        assert!(first.complete.is_none());
        alice.repair(source.chunk_hash(2));
        alice.repair(source.chunk_hash(3));
    }

    // Alice can no longer serve the root, the index or bundles 0-1, and the
    // restarted bob has an empty blob store: only bob's verified files can
    // supply them, so completion proves they were not re-fetched.
    for hash in [
        entry.root,
        content_hash(&source.index_json),
        source.chunk_hash(0),
        source.chunk_hash(1),
    ] {
        alice.withhold(hash);
    }
    let after_restart = service(&network.transport("bob-restarted"), bob_dir.path());
    let pending = after_restart.pending_downloads().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (
            pending[0].received,
            pending[0].total,
            pending[0].index_present,
            pending[0].full_download
        ),
        (2, 4, true, true)
    );

    let reports = after_restart.resume_all(None).await.unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].stored, vec![2, 3]);
    let complete = reports[0].complete.as_ref().expect("resume completes");
    for (seq, path) in complete.bundle_paths.iter().enumerate() {
        assert_eq!(std::fs::read(path).unwrap(), source.bundles[seq]);
    }
    assert!(
        after_restart.resume_all(None).await.unwrap().is_empty(),
        "a complete download is not resumed again"
    );
}

#[tokio::test]
async fn cancel_discards_partial_state() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "delta", 3);
    let entry = service(&alice, alice_dir.path())
        .publish("delta", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();

    let fetcher = service(&bob, bob_dir.path());
    fetcher
        .request_meta(&[alice.provider_ref()], "delta", entry.root)
        .await
        .unwrap();
    fetcher.fetch_chunk("delta", 0).await.unwrap();
    let download_dir = fetcher.download_dir("delta").unwrap();
    assert!(download_dir.join(LEDGER_FILE).is_file());
    assert_eq!(
        bob.list_tags(DOWNLOAD_TAG_PREFIX).await.unwrap(),
        vec![(download_tag("delta"), entry.root)]
    );

    fetcher.cancel("delta").await.unwrap();
    assert!(!download_dir.exists(), "ledger and files are gone");
    assert!(bob.list_tags(DOWNLOAD_TAG_PREFIX).await.unwrap().is_empty());
    assert!(fetcher.pending_downloads().await.unwrap().is_empty());
    assert_eq!(
        fetcher.fetch_chunk("delta", 0).await,
        Err(TransferError::UnknownDownload("delta".to_string()))
    );

    let fresh = fetcher
        .start_download(&[alice.provider_ref()], "delta", entry.root, None)
        .await
        .unwrap();
    assert_eq!(
        fresh.stored,
        vec![0, 1, 2],
        "a fresh request starts at zero"
    );
    assert!(fresh.complete.is_some());
}

#[tokio::test]
async fn a_tileset_id_that_escapes_the_download_dir_is_refused() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let sandbox = TempDir::new().unwrap();
    let p2p_dir = sandbox.path().join("p2p");
    let fetcher = service(&bob, &p2p_dir);
    let bundle = sandbox.path().join("0.bundle");

    for id in ["../x", "a/b", "", "..", "a\\b", "C:x"] {
        let unsafe_id = Err(TransferError::UnsafeTilesetId(id.to_string()));
        assert_eq!(
            fetcher
                .request_meta(&[alice.provider_ref()], id, [7; 32])
                .await
                .map(|_| ()),
            unsafe_id,
            "request_meta({id:?})"
        );
        assert_eq!(fetcher.fetch_chunk(id, 0).await.map(|_| ()), unsafe_id);
        assert_eq!(fetcher.download(id, None).await.map(|_| ()), unsafe_id);
        assert_eq!(fetcher.cancel(id).await, unsafe_id);
        assert_eq!(
            fetcher
                .publish(id, Bytes::from_static(b"{}"), std::slice::from_ref(&bundle))
                .await
                .map(|_| ()),
            unsafe_id
        );
    }
    assert!(
        files_under(sandbox.path()).is_empty(),
        "nothing was created anywhere: {:?}",
        files_under(sandbox.path())
    );
    assert!(
        bob.list_tags("").await.unwrap().is_empty(),
        "no tag was written"
    );
}

/// S8b: a provider serving bundles over the service's per-bundle cap gets
/// every bundle refused, and nothing of them is written to disk.
#[tokio::test]
async fn an_oversized_bundle_is_refused_and_never_persisted() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "bloat", 2);
    let entry = service(&alice, alice_dir.path())
        .publish("bloat", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();

    let fetcher = service(&bob, bob_dir.path()).with_limits(TransferLimits {
        max_chunk_bytes: BUNDLE_BYTES - 1,
        ..TransferLimits::default()
    });
    let report = fetcher
        .start_download(&[alice.provider_ref()], "bloat", entry.root, None)
        .await
        .unwrap();
    assert_eq!(report.failed.len(), 2, "{:?}", report.failed);
    assert!(report.stored.is_empty());
    assert!(report.complete.is_none());
    let download_dir = fetcher.download_dir("bloat").unwrap();
    assert!(
        files_under(&download_dir)
            .iter()
            .all(|p| p.extension().is_none_or(|ext| ext != "bundle")),
        "no over-cap bundle reached disk"
    );
    assert!(!bob.has_local(&source.chunk_hash(0)));
}

/// S8b: a download stops once its bundles pass the total byte cap, never completes, and says why.
#[tokio::test]
async fn a_download_over_its_total_byte_cap_stops_without_completing() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "sprawl", 8);
    let entry = service(&alice, alice_dir.path())
        .publish("sprawl", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();

    let cap = (BUNDLE_BYTES * 2) as u64;
    let fetcher = service(&bob, bob_dir.path()).with_limits(TransferLimits {
        max_download_bytes: cap,
        ..TransferLimits::default()
    });
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    fetcher
        .request_meta(&[alice.provider_ref()], "sprawl", entry.root)
        .await
        .unwrap();
    match fetcher.download("sprawl", Some(&tx)).await {
        Err(TransferError::DownloadTooLarge {
            bytes,
            cap: reported,
        }) => {
            assert_eq!(reported, cap);
            assert!(bytes > cap);
        }
        other => panic!("expected DownloadTooLarge, got {other:?}"),
    }
    while let Ok(event) = rx.try_recv() {
        assert!(
            !matches!(event, TransferEvent::Complete(_)),
            "an over-cap download never reports completion"
        );
    }
    let stored = files_under(&fetcher.download_dir("sprawl").unwrap())
        .iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "bundle"))
        .count();
    assert!(
        stored < 8,
        "the download stopped early ({stored} bundles stored)"
    );
}

/// S8b: bundles fetched one at a time for streaming (`fetch_chunk`, the
/// path a peer-replicated petal can trigger) stay a cache — a restart never
/// grows them into a full download, so nothing more is fetched or installed.
#[tokio::test]
async fn a_streamed_bundle_cache_is_not_resumed_as_a_full_download() {
    let network = InMemoryNetwork::new();
    let alice = network.transport("alice");
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "cache", 4);
    let entry = service(&alice, alice_dir.path())
        .publish("cache", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();
    {
        let streamer = service(&network.transport("bob"), bob_dir.path());
        streamer
            .request_meta(&[alice.provider_ref()], "cache", entry.root)
            .await
            .unwrap();
        streamer.fetch_chunk("cache", 0).await.unwrap();
    }

    let restarted = service(&network.transport("bob-restarted"), bob_dir.path());
    let pending = restarted.pending_downloads().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].full_download);
    assert!(restarted.resume_all(None).await.unwrap().is_empty());
    let bundles = files_under(&restarted.download_dir("cache").unwrap())
        .iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "bundle"))
        .count();
    assert_eq!(bundles, 1, "only the streamed bundle is on disk");
}

/// S8b resume-ledger tampering: a `download.json` whose bundle list no
/// longer hashes to its root is skipped on restart, so nothing is fetched
/// for it and no provider it names is dialed.
#[tokio::test]
async fn a_tampered_ledger_is_never_resumed() {
    let network = InMemoryNetwork::new();
    let alice = network.transport("alice");
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "mesa", 3);
    let entry = service(&alice, alice_dir.path())
        .publish("mesa", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();
    let ledger_path = {
        let before = service(&network.transport("bob"), bob_dir.path());
        before
            .request_meta(&[alice.provider_ref()], "mesa", entry.root)
            .await
            .unwrap();
        before.download_dir("mesa").unwrap().join(LEDGER_FILE)
    };

    let mut ledger: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&ledger_path).unwrap()).unwrap();
    ledger["chunk_hashes"][1] = serde_json::json!(hex::encode([0xAB; 32]));
    ledger["provider_candidates"] = serde_json::json!(["mallory"]);
    std::fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

    let restarted = service(&network.transport("bob-restarted"), bob_dir.path());
    assert!(
        restarted.pending_downloads().await.unwrap().is_empty(),
        "a ledger that no longer hashes to its root is not trusted"
    );
    assert!(restarted.resume_all(None).await.unwrap().is_empty());
    assert!(
        files_under(&restarted.download_dir("mesa").unwrap())
            .iter()
            .all(|p| p.extension().is_none_or(|ext| ext != "bundle")),
        "nothing was fetched for the tampered ledger"
    );
}

/// Bind a hermetic loopback endpoint; `None` where UDP binding is unavailable.
async fn loopback_endpoint(seed: u8) -> Option<SyncEndpoint> {
    match SyncEndpoint::new(
        iroh::SecretKey::from_bytes(&[seed; 32]),
        &RelayConfig::Disabled,
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
async fn iroh_transport_round_trips_a_hash_seq_on_loopback() {
    let (Some(alice_ep), Some(bob_ep)) = (loopback_endpoint(61).await, loopback_endpoint(62).await)
    else {
        return;
    };
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let alice_stack = DocsStack::spawn(alice_ep.inner().clone(), alice_dir.path().join("p2p"))
        .await
        .expect("alice stack");
    let bob_stack = DocsStack::spawn(bob_ep.inner().clone(), bob_dir.path().join("p2p"))
        .await
        .expect("bob stack");
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "summit", 3);

    let publisher = TilesetTransfer::for_p2p_dir(
        Arc::new(IrohBlobTransport::from_stack(&alice_stack)),
        alice_stack.data_dir(),
    );
    let entry = publisher
        .publish("summit", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();
    let provider = IrohBlobTransport::provider_ref(&alice_ep.node_addr().await).unwrap();

    let bob_transport = Arc::new(IrohBlobTransport::from_stack(&bob_stack));
    let children = bob_transport
        .fetch_hash_seq(&provider, entry.root, MAX_HASH_SEQ_CHILDREN)
        .await
        .expect("root over loopback");
    let mut expected = vec![content_hash(&source.index_json)];
    expected.extend((0..3).map(|seq| source.chunk_hash(seq)));
    assert_eq!(children, expected);
    assert_eq!(
        bob_transport
            .fetch(&provider, children[2], BUNDLE_BYTES as u64)
            .await
            .unwrap(),
        Bytes::from(source.bundles[1].clone())
    );
    // S8b: the BAO-verified size is asked first, so an over-cap blob is
    // refused before any of its content lands in bob's store.
    assert!(
        bob_transport
            .fetch(&provider, children[3], BUNDLE_BYTES as u64 - 1)
            .await
            .is_err(),
        "a blob over the cap is refused"
    );
    assert_eq!(
        bob_transport.local_size(children[3]).await.unwrap(),
        None,
        "the refused blob never entered the local store"
    );
    assert!(
        bob_transport
            .fetch(
                &ProviderRef::new("https://example.invalid/blob"),
                children[3],
                BUNDLE_BYTES as u64
            )
            .await
            .is_err(),
        "a provider that is not EndpointAddr JSON is never dialed"
    );

    let fetcher = TilesetTransfer::for_p2p_dir(bob_transport, bob_stack.data_dir());
    let report = fetcher
        .start_download(std::slice::from_ref(&provider), "summit", entry.root, None)
        .await
        .unwrap();
    let complete = report.complete.expect("download over iroh completes");
    for (seq, path) in complete.bundle_paths.iter().enumerate() {
        assert_eq!(std::fs::read(path).unwrap(), source.bundles[seq]);
    }

    drop((publisher, fetcher));
    alice_stack.shutdown().await;
    bob_stack.shutdown().await;
    alice_ep.shutdown().await;
    bob_ep.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn catalog_survives_a_stack_restart() {
    let dir = TempDir::new().unwrap();
    let p2p_dir = dir.path().join("p2p");
    let source = SourceTileset::write(&dir.path().join("bundles"), "plateau", 2);

    let Some(endpoint) = loopback_endpoint(63).await else {
        return;
    };
    let stack = DocsStack::spawn(endpoint.inner().clone(), &p2p_dir)
        .await
        .expect("first stack");
    let published = {
        let transfer =
            TilesetTransfer::for_p2p_dir(Arc::new(IrohBlobTransport::from_stack(&stack)), &p2p_dir);
        transfer
            .publish("plateau", source.index_json.clone(), &source.bundle_paths)
            .await
            .unwrap()
    };
    stack.shutdown().await;
    endpoint.shutdown().await;

    let Some(endpoint) = loopback_endpoint(63).await else {
        return;
    };
    let stack = DocsStack::spawn(endpoint.inner().clone(), &p2p_dir)
        .await
        .expect("respawned stack");
    let transfer =
        TilesetTransfer::for_p2p_dir(Arc::new(IrohBlobTransport::from_stack(&stack)), &p2p_dir);
    assert!(transfer.catalog().is_empty(), "a new service starts empty");
    let rebuilt = transfer.catalog_from_tags().await.unwrap();
    assert_eq!(rebuilt, vec![published.clone()]);
    assert_eq!(transfer.catalog_entry("plateau"), Some(published));

    drop(transfer);
    stack.shutdown().await;
    endpoint.shutdown().await;
}
