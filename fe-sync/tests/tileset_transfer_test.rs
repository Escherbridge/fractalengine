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
        .request_download_meta(&[alice.provider_ref()], "valley", entry.root)
        .await
        .unwrap();
    assert_eq!(
        meta.index_json, source.index_json,
        "index passes through opaque"
    );
    assert_eq!(std::fs::read(&meta.index_path).unwrap(), source.index_json);

    // PAS-P5 S7a: no chunk before the host confirms the signed hashes it
    // read from the (here: trivially "verified") header.
    assert_eq!(
        fetcher.download("valley", None).await,
        Err(TransferError::AwaitingConfirmation("valley".to_string())),
        "an unconfirmed download fetches nothing"
    );
    fetcher.confirm("valley", &meta.chunk_hashes).await.unwrap();

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

    // Rate-cap leg (PAS-P5-P2P-POLICY): the same tileset under a download cap
    // takes at least bytes/rate; the lower bound is a floor, never an upper bound.
    let capped_dir = TempDir::new().unwrap();
    let rate: u64 = 1_000_000;
    let capped = service(&network.transport("carol"), capped_dir.path()).with_limits(
        TransferLimits {
            max_download_rate_bytes_per_sec: Some(rate),
            ..TransferLimits::default()
        },
    );
    let capped_meta = capped
        .request_download_meta(&[alice.provider_ref()], "valley", entry.root)
        .await
        .unwrap();
    capped
        .confirm("valley", &capped_meta.chunk_hashes)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let capped_report = capped.download("valley", None).await.unwrap();
    let elapsed = started.elapsed();
    assert!(capped_report.complete.is_some());
    let floor = std::time::Duration::from_nanos((entry.bytes as u128 * 1_000_000_000 / rate as u128) as u64);
    assert!(
        elapsed + std::time::Duration::from_millis(50) >= floor,
        "capped download took {elapsed:?}, floor {floor:?}"
    );
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
    let meta = fetcher
        .request_download_meta(&[alice.provider_ref()], "sprawl", entry.root)
        .await
        .unwrap();
    fetcher.confirm("sprawl", &meta.chunk_hashes).await.unwrap();
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

/// Final review: a full download that completed before a restart (the host
/// never got to install it) is handed back as a `CompletedDownload`; a
/// streamed-bundle cache is not, even when every bundle is on disk.
#[tokio::test]
async fn a_download_completed_before_a_restart_is_handed_back_for_install() {
    let network = InMemoryNetwork::new();
    let alice = network.transport("alice");
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let plain = SourceTileset::write(&alice_dir.path().join("plain"), "plain", 2);
    let cached = SourceTileset::write(&alice_dir.path().join("cached"), "cached", 1);
    let publisher = service(&alice, alice_dir.path());
    let plain_entry = publisher
        .publish("plain", plain.index_json.clone(), &plain.bundle_paths)
        .await
        .unwrap();
    let cached_entry = publisher
        .publish("cached", cached.index_json.clone(), &cached.bundle_paths)
        .await
        .unwrap();
    {
        let bob = service(&network.transport("bob"), bob_dir.path());
        let report = bob
            .start_download(&[alice.provider_ref()], "plain", plain_entry.root, None)
            .await
            .unwrap();
        assert!(report.complete.is_some());
        bob.request_meta(&[alice.provider_ref()], "cached", cached_entry.root)
            .await
            .unwrap();
        bob.fetch_chunk("cached", 0).await.unwrap();
    }

    let restarted = service(&network.transport("bob-restarted"), bob_dir.path());
    let pending = restarted.pending_downloads().await.unwrap();
    assert_eq!(pending.len(), 2, "both ledgers are listed: {pending:?}");
    let completed = restarted.completed_from(&pending);
    assert_eq!(completed.len(), 1, "only the full download: {completed:?}");
    assert_eq!(completed[0].tileset_id, "plain");
    assert_eq!(completed[0].root, plain_entry.root);
    for (seq, path) in completed[0].bundle_paths.iter().enumerate() {
        assert_eq!(std::fs::read(path).unwrap(), plain.bundles[seq]);
    }
}

/// Final review: `catalog_from_tags` runs at sync-thread start while startup
/// seeding may already be publishing. A tag snapshot that predates a publish
/// (modelled by deleting the fresh tag) must not erase the published entry.
#[tokio::test]
async fn a_tag_rebuild_never_drops_a_tileset_published_meanwhile() {
    let network = InMemoryNetwork::new();
    let alice = network.transport("alice");
    let alice_dir = TempDir::new().unwrap();
    let older = SourceTileset::write(&alice_dir.path().join("older"), "older", 1);
    let fresh = SourceTileset::write(&alice_dir.path().join("fresh"), "fresh", 1);
    let transfer = service(&alice, alice_dir.path());
    let older_entry = transfer
        .publish("older", older.index_json.clone(), &older.bundle_paths)
        .await
        .unwrap();
    let fresh_entry = transfer
        .publish("fresh", fresh.index_json.clone(), &fresh.bundle_paths)
        .await
        .unwrap();
    alice
        .delete_tag(&fe_sync::tileset_transfer::tileset_tag("fresh"))
        .await
        .unwrap();

    let rebuilt = transfer.catalog_from_tags().await.unwrap();
    assert_eq!(rebuilt, vec![older_entry.clone()]);
    assert_eq!(transfer.catalog_entry("older"), Some(older_entry));
    assert_eq!(
        transfer.catalog_entry("fresh"),
        Some(fresh_entry),
        "the concurrent publish keeps its catalog entry (it is still advertised)"
    );
}

/// AC-21(a): streamed-bundle caches share one disk budget across tilesets
/// and restarts. Past it the least-recently-used cache is evicted first; an
/// in-progress full download is never evicted, however old or large.
#[tokio::test]
async fn streamed_caches_past_the_disk_budget_evict_least_recently_used_first() {
    let network = InMemoryNetwork::new();
    let alice = network.transport("alice");
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let publisher = service(&alice, alice_dir.path());
    let mut roots = std::collections::HashMap::new();
    let mut sources = std::collections::HashMap::new();
    for id in ["full", "old", "mid", "new"] {
        let source = SourceTileset::write(&alice_dir.path().join(id), id, 2);
        let entry = publisher
            .publish(id, source.index_json.clone(), &source.bundle_paths)
            .await
            .unwrap();
        roots.insert(id, entry.root);
        sources.insert(id, source);
    }
    let tick = || tokio::time::sleep(std::time::Duration::from_millis(5));
    // Room for two one-bundle caches (plus ledger/index overhead), not three.
    let two_caches = (2 * BUNDLE_BYTES + 16 * 1024) as u64;
    let bob = service(&network.transport("bob"), bob_dir.path()).with_limits(TransferLimits {
        max_streamed_cache_bytes: two_caches,
        ..TransferLimits::default()
    });

    // An in-progress full download: bundle 1 is withheld, so it stays partial.
    alice.withhold(sources["full"].chunk_hash(1));
    let partial = bob
        .start_download(&[alice.provider_ref()], "full", roots["full"], None)
        .await
        .unwrap();
    assert!(partial.complete.is_none());
    tick().await;

    for id in ["old", "mid"] {
        bob.request_meta(&[alice.provider_ref()], id, roots[id])
            .await
            .unwrap();
        bob.fetch_chunk(id, 0).await.unwrap();
        tick().await;
    }
    // Re-reading "old" makes "mid" the least recently used.
    bob.fetch_chunk("old", 0).await.unwrap();
    tick().await;
    bob.request_meta(&[alice.provider_ref()], "new", roots["new"])
        .await
        .unwrap();
    bob.fetch_chunk("new", 0).await.unwrap();

    let present = |id: &str| bob.download_dir(id).unwrap().exists();
    assert!(!present("mid"), "the least-recently-used cache was evicted");
    assert!(
        present("old") && present("new"),
        "recently used caches stay"
    );
    assert!(
        present("full"),
        "an in-progress full download is never evicted"
    );
    assert!(bob.download_dir("full").unwrap().join("0.bundle").is_file());

    // Across a restart with a tighter budget, startup trimming keeps only the newest cache.
    let restarted =
        service(&network.transport("bob-restarted"), bob_dir.path()).with_limits(TransferLimits {
            max_streamed_cache_bytes: (BUNDLE_BYTES + 8 * 1024) as u64,
            ..TransferLimits::default()
        });
    assert_eq!(restarted.trim_streamed_caches().await.unwrap(), vec!["old"]);
    assert!(restarted.download_dir("new").unwrap().exists());
    assert!(restarted.download_dir("full").unwrap().exists());
    let pending = restarted.pending_downloads().await.unwrap();
    let ids: Vec<&str> = pending.iter().map(|p| p.tileset_id.as_str()).collect();
    assert_eq!(ids, vec!["full", "new"]);
}

/// PAS-P5 S7a (critic, mandatory): a download the host never confirmed is
/// never resumed — not on the first `resume_all`, and not across a restart
/// either, since confirmation is persisted in the ledger rather than kept
/// only in memory. No bundle ever reaches disk for it.
#[tokio::test]
async fn an_unconfirmed_download_never_fetches_a_chunk_even_across_a_restart() {
    let network = InMemoryNetwork::new();
    let alice = network.transport("alice");
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "pending-seal", 3);
    let entry = service(&alice, alice_dir.path())
        .publish(
            "pending-seal",
            source.index_json.clone(),
            &source.bundle_paths,
        )
        .await
        .unwrap();

    {
        let bob = service(&network.transport("bob"), bob_dir.path());
        bob.request_download_meta(&[alice.provider_ref()], "pending-seal", entry.root)
            .await
            .unwrap();
        // Never confirmed: the (simulated) host's header verification is
        // still pending, or was refused.
        assert_eq!(
            bob.download("pending-seal", None).await,
            Err(TransferError::AwaitingConfirmation(
                "pending-seal".to_string()
            ))
        );
        assert!(bob.resume_all(None).await.unwrap().is_empty());
    }

    let restarted = service(&network.transport("bob-restarted"), bob_dir.path());
    let pending = restarted.pending_downloads().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].confirmed, "confirmation never happened");
    assert!(
        restarted.resume_all(None).await.unwrap().is_empty(),
        "an unconfirmed download requests nothing, even across a restart"
    );
    let bundles = files_under(&restarted.download_dir("pending-seal").unwrap())
        .iter()
        .filter(|p| p.extension().is_some_and(|ext| ext == "bundle"))
        .count();
    assert_eq!(bundles, 0, "no chunk ever reached disk");
}

/// PAS-P5 S7a: `confirm` refuses chunk hashes that do not exactly match what
/// the HashSeq itself resolved (a seeder's chunk index naming more or fewer
/// bundles than the HashSeq actually carries) — the mismatch is caught
/// before any chunk fetch, not discovered partway through one.
#[tokio::test]
async fn a_confirmation_disagreeing_with_the_resolved_hashseq_is_refused() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "shrunk", 3);
    let entry = service(&alice, alice_dir.path())
        .publish("shrunk", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();

    let fetcher = service(&bob, bob_dir.path());
    let meta = fetcher
        .request_download_meta(&[alice.provider_ref()], "shrunk", entry.root)
        .await
        .unwrap();

    // A signed index naming only 2 of the 3 chunks the HashSeq actually
    // carries (as if a seeder appended an unsigned bundle) is refused
    // outright, never partially accepted.
    let short: Vec<Hash32> = meta.chunk_hashes[..2].to_vec();
    assert!(matches!(
        fetcher.confirm("shrunk", &short).await,
        Err(TransferError::BadIndex(_))
    ));
    assert!(!fetcher.is_confirmed("shrunk").await.unwrap());
    assert_eq!(
        fetcher.download("shrunk", None).await,
        Err(TransferError::AwaitingConfirmation("shrunk".to_string())),
        "the refused confirmation never unlocked a fetch"
    );

    // The real signed hashes (exactly what resolved) confirm cleanly.
    fetcher.confirm("shrunk", &meta.chunk_hashes).await.unwrap();
    assert!(fetcher.is_confirmed("shrunk").await.unwrap());
    let report = fetcher.download("shrunk", None).await.unwrap();
    assert!(report.complete.is_some());
}

/// Final review: a streamed (not-installed) cache can never be confirmed into
/// a full download. Before, any `ConfirmTilesetDownload` for an id with a
/// ledger confirmed it, so a late streamed meta reply the host mistook for an
/// install-flow reply turned the cache into a full download that then
/// auto-installed. Now only `request_download_meta` makes `confirm` succeed.
#[tokio::test]
async fn a_streamed_cache_is_never_confirmed_into_a_full_download() {
    let network = InMemoryNetwork::new();
    let (alice, bob) = (network.transport("alice"), network.transport("bob"));
    let (alice_dir, bob_dir) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let source = SourceTileset::write(&alice_dir.path().join("bundles"), "streamed", 2);
    let entry = service(&alice, alice_dir.path())
        .publish("streamed", source.index_json.clone(), &source.bundle_paths)
        .await
        .unwrap();

    let fetcher = service(&bob, bob_dir.path());
    let meta = fetcher
        .request_meta(&[alice.provider_ref()], "streamed", entry.root)
        .await
        .unwrap();
    fetcher.fetch_chunk("streamed", 0).await.unwrap();

    assert_eq!(
        fetcher.confirm("streamed", &meta.chunk_hashes).await,
        Err(TransferError::NotRequested("streamed".to_string()))
    );
    assert_eq!(
        fetcher.download("streamed", None).await,
        Err(TransferError::AwaitingConfirmation("streamed".to_string()))
    );
    let pending = fetcher.pending_downloads().await.unwrap();
    assert!(
        pending.iter().all(|p| !p.full_download),
        "the cache never became a full download: {pending:?}"
    );
    assert!(fetcher.completed_from(&pending).is_empty());

    // An explicit download request for the same id is what unlocks it.
    let meta = fetcher
        .request_download_meta(&[alice.provider_ref()], "streamed", entry.root)
        .await
        .unwrap();
    fetcher
        .confirm("streamed", &meta.chunk_hashes)
        .await
        .unwrap();
    let report = fetcher.download("streamed", None).await.unwrap();
    assert!(report.complete.is_some());
}
