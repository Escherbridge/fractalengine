//! Verified native publication and canonical shared-store persistence; see AGENTS.md.

use super::admission::{admit, admit_job};
use crate::{blob_transport::IrohBlobTransport, scene_asset_gate::SceneAssetGate};
use bytes::Bytes;
use fe_runtime::{
    bevy_blob_reader::{BlobFallbackSlot, BlobSourceRequest},
    blob_store::{hash_from_hex, hash_to_hex, BlobHash, BlobStoreHandle},
    blob_transport::{hash_seq_root, BlobTransport, ProviderRef},
    messages::DbCommand,
    scene_asset::{verify_scene_asset_bytes, SceneAssetTransferPermit, SceneAssetTransferPurpose},
    service_lifecycle::StopSignal,
};
use std::{sync::Arc, time::{Duration, Instant}};

/// How long the bound `blob.source` hook may take after every provider missed.
pub(super) const SOURCE_TIMEOUT: Duration = Duration::from_secs(35);

pub(super) struct JobLease {
    pub(super) stop: StopSignal,
    pub(super) io_owner: Arc<tokio::sync::OwnedSemaphorePermit>,
}

async fn read_shared(
    blobs: &BlobStoreHandle,
    hash: BlobHash,
    cap: u64,
    io_owner: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
) -> anyhow::Result<Bytes> {
    let blobs = blobs.clone();
    tokio::task::spawn_blocking(move || {
        let _io_owner = io_owner;
        use std::io::Read;
        let path = blobs
            .get_blob_path(&hash)
            .ok_or_else(|| anyhow::anyhow!("Canonical local bytes are absent"))?;
        let file = std::fs::File::open(path)?;
        anyhow::ensure!(
            file.metadata()?.is_file(),
            "Scene asset is not a regular file"
        );
        let mut bytes = Vec::new();
        file.take(cap + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= cap && *blake3::hash(&bytes).as_bytes() == hash,
            "Canonical scene bytes exceed the bound or digest"
        );
        Ok::<_, anyhow::Error>(Bytes::from(bytes))
    })
    .await?
}

pub(super) async fn publish(
    blobs: &BlobStoreHandle,
    transport: &Arc<IrohBlobTransport>,
    gate: &SceneAssetGate,
    db: &Option<crossbeam::channel::Sender<DbCommand>>,
    verse: &str,
    row_hash: BlobHash,
    lease: &JobLease,
) -> anyhow::Result<String> {
    let metadata = read_shared(blobs, row_hash, 32 * 1024, Some(lease.io_owner.clone())).await?;
    let row: serde_json::Value = serde_json::from_slice(&metadata)?;
    anyhow::ensure!(
        row[fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER] == 1,
        "unsupported or missing scene-asset routing version"
    );
    let hash = hash_from_hex(
        row["content_hash"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Scene asset row lacks digest"))?,
    )?;
    let purpose = SceneAssetTransferPurpose::Publish {
        asset_id: row["asset_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Scene metadata lacks asset identity"))?
            .into(),
        petal_id: row["petal_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Scene metadata lacks Petal identity"))?
            .into(),
    };
    let permit = admit_job(db, verse, hash, purpose.clone()).await?;
    anyhow::ensure!(
        row["asset_id"].as_str() == Some(permit.asset_id.as_str())
            && row["petal_id"].as_str() == Some(permit.petal_id.as_str())
            && row["content_type"].as_str() == Some(permit.content_type.as_str())
            && row["size_bytes"].as_u64() == Some(permit.size_bytes),
        "Scene asset publication metadata changed"
    );
    let bytes = read_shared(blobs, hash, permit.size_bytes, Some(lease.io_owner.clone())).await?;
    verify_scene_asset_bytes(&bytes, &hash, &permit.content_type, permit.size_bytes)?;
    let current = admit_job(db, verse, hash, purpose.clone()).await?;
    anyhow::ensure!(
        current == permit && !lease.stop.is_requested(),
        "Scene asset publication authority changed"
    );
    let root = hash_seq_root(&[hash]);
    gate.classify(verse, hash, lease.io_owner.clone()).await?;
    let _temporary = TemporarySceneBlobs {
        transport: transport.clone(),
        hashes: [hash, root],
    };
    let result = async {
        anyhow::ensure!(
            transport.add(bytes).await? == hash,
            "Native scene blob hash mismatch"
        );
        anyhow::ensure!(
            transport.add_hash_seq(&[hash]).await? == root,
            "Native scene retention root mismatch"
        );
        anyhow::ensure!(
            !lease.stop.is_requested(),
            "Scene asset publication owner expired"
        );
        transport
            .set_tag(
                &format!(
                    "fe/scene-asset/{}/{}",
                    hex::encode(blake3::hash(verse.as_bytes()).as_bytes()),
                    hash_to_hex(&hash)
                ),
                root,
            )
            .await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    result?;
    let current = admit_job(db, verse, hash, purpose.clone()).await?;
    anyhow::ensure!(
        current == permit && !lease.stop.is_requested(),
        "Scene asset publication changed before metadata emission"
    );
    Ok(permit.asset_id)
}

pub(super) async fn fetch(
    transport: &IrohBlobTransport,
    gate: &SceneAssetGate,
    verse: &str,
    hash: BlobHash,
    permit: &SceneAssetTransferPermit,
    providers: &[ProviderRef],
    source: &BlobFallbackSlot,
    lease: &JobLease,
) -> anyhow::Result<Bytes> {
    // No provider is not final: a bound blob.source may still serve (asked below).
    gate.classify(verse, hash, lease.io_owner.clone()).await?;
    let mut last = if providers.is_empty() {
        "No native provider is known for this opened Verse".to_owned()
    } else {
        "No provider returned the scene blob".to_owned()
    };
    for provider in providers {
        let result = async {
            let size = transport
                .verify_provider_size(provider, hash, permit.size_bytes)
                .await?;
            anyhow::ensure!(
                size == permit.size_bytes,
                "Provider scene size disagrees with admitted metadata"
            );
            let bytes = transport.fetch(provider, hash, permit.size_bytes).await?;
            verify_scene_asset_bytes(&bytes, &hash, &permit.content_type, permit.size_bytes)?;
            Ok::<_, anyhow::Error>(bytes)
        }
        .await;
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last = error.to_string(),
        }
    }
    match source_fallback(source, hash, permit, lease).await {
        Ok(Some(bytes)) => Ok(bytes),
        Ok(None) => anyhow::bail!("{last}"),
        Err(error) => anyhow::bail!("{last}; blob.source bytes refused: {error:#}"),
    }
}

/// Ask the extension `blob.source` after every provider missed; the slot already hash- and size-checked the bytes.
async fn source_fallback(
    source: &BlobFallbackSlot,
    hash: BlobHash,
    permit: &SceneAssetTransferPermit,
    lease: &JobLease,
) -> anyhow::Result<Option<Bytes>> {
    if !source.is_installed() || lease.stop.is_requested() {
        return Ok(None);
    }
    let request = BlobSourceRequest {
        hash,
        expected_size: Some(permit.size_bytes),
        asset_id: Some(permit.asset_id.clone()),
        petal_id: Some(permit.petal_id.clone()),
    };
    // A verse closing mid-ask is handled by the job's outer `select!` on the stop signal, which drops this future.
    let Ok(answer) = tokio::time::timeout(SOURCE_TIMEOUT, source.source_blob(request)).await else {
        tracing::info!(hash = %hash_to_hex(&hash), "scene blob.source timed out");
        return Ok(None);
    };
    let Some(bytes) = answer else { return Ok(None) };
    verify_scene_asset_bytes(&bytes, &hash, &permit.content_type, permit.size_bytes)?;
    Ok(Some(bytes))
}

pub(super) async fn finish_fetch(
    blobs: &BlobStoreHandle,
    db: &Option<crossbeam::channel::Sender<DbCommand>>,
    lease: &JobLease,
    deadline: Instant,
    permit: SceneAssetTransferPermit,
    bytes: Bytes,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        !lease.stop.is_requested() && Instant::now() < deadline,
        "Scene asset transfer owner expired"
    );
    let current = admit(
        db,
        &permit.verse_id,
        permit.content_hash,
        SceneAssetTransferPurpose::Fetch,
    )
    .await?;
    anyhow::ensure!(
        current == permit && !lease.stop.is_requested() && Instant::now() < deadline,
        "Scene asset metadata or access changed during download"
    );
    let store = blobs.clone();
    let expected = permit.clone();
    let io_owner = lease.io_owner.clone();
    // Await this finite local write even after cancellation, so reset cannot outlive a late store write.
    tokio::task::spawn_blocking(move || {
        let _io_owner = io_owner;
        verify_scene_asset_bytes(
            &bytes,
            &expected.content_hash,
            &expected.content_type,
            expected.size_bytes,
        )?;
        anyhow::ensure!(
            store.add_blob(&bytes)? == expected.content_hash
                && store.has_blob(&expected.content_hash),
            "Scene asset persistence did not establish the expected blob"
        );
        let path = store
            .get_blob_path(&expected.content_hash)
            .ok_or_else(|| anyhow::anyhow!("Persisted scene bytes have no canonical path"))?;
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        anyhow::ensure!(
            file.metadata()?.is_file(),
            "Persisted scene asset is not a regular file"
        );
        let mut file = file.take(expected.size_bytes + 1);
        let mut hasher = blake3::Hasher::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            total += read as u64;
        }
        anyhow::ensure!(
            total == expected.size_bytes && *hasher.finalize().as_bytes() == expected.content_hash,
            "Persisted scene bytes failed readback verification"
        );
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    anyhow::ensure!(
        !lease.stop.is_requested() && Instant::now() < deadline,
        "Scene asset transfer cancelled after persistence; no readiness announced"
    );
    let current = admit(
        db,
        &permit.verse_id,
        permit.content_hash,
        SceneAssetTransferPurpose::Fetch,
    )
    .await?;
    anyhow::ensure!(
        current == permit,
        "Scene asset access changed after persistence; no readiness announced"
    );
    Ok(())
}

struct TemporarySceneBlobs {
    transport: Arc<IrohBlobTransport>,
    hashes: [BlobHash; 2],
}
impl Drop for TemporarySceneBlobs {
    fn drop(&mut self) {
        self.transport.discard_temporary(&self.hashes);
    }
}

/// Identify scene metadata without changing the existing non-scene asset publisher.
pub(crate) async fn is_scene_row(
    blobs: &BlobStoreHandle,
    row_hash: BlobHash,
) -> anyhow::Result<bool> {
    let bytes = read_shared(blobs, row_hash, 32 * 1024, None).await?;
    let row: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(row
        .get(fe_runtime::scene_asset::SCENE_ASSET_ROW_MARKER)
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::bevy_blob_reader::{BlobFallback, FallbackFuture, TilesetChildRequest};

    /// A bound source answering every blob ask with fixed bytes.
    struct FixedSource(Option<&'static [u8]>);
    impl BlobFallback for FixedSource {
        fn source_blob(&self, _request: BlobSourceRequest) -> FallbackFuture<'_> {
            let answer = self.0.map(Bytes::from_static);
            Box::pin(async move { answer })
        }
        fn source_tileset_child(&self, _request: TilesetChildRequest) -> FallbackFuture<'_> {
            Box::pin(async { None })
        }
    }

    /// Source-fallback leg of the scene-asset fetch: a bound source serves only exact, hash-true bytes that also pass
    /// the scene-asset intake (MIME container check) after the peers missed.
    #[tokio::test]
    async fn blob_source_serves_scene_bytes_only_when_exact() {
        let body: &'static [u8] = br#"{"asset":{"version":"2.0"}}"#;
        let hash = *blake3::hash(body).as_bytes();
        let permit = SceneAssetTransferPermit {
            verse_id: "verse".into(),
            petal_id: "petal".into(),
            node_id: None,
            asset_id: "asset".into(),
            content_hash: hash,
            content_type: "model/gltf+json".into(),
            size_bytes: body.len() as u64,
            asset_fingerprint: String::new(),
        };
        let lease = JobLease {
            stop: StopSignal::default(),
            io_owner: Arc::new(Arc::new(tokio::sync::Semaphore::new(1)).acquire_owned().await.unwrap()),
        };
        let slot = BlobFallbackSlot::default();
        assert!(source_fallback(&slot, hash, &permit, &lease).await.unwrap().is_none(), "no source installed");
        slot.install(Arc::new(FixedSource(Some(b"forged asset bytes!"))));
        assert!(source_fallback(&slot, hash, &permit, &lease).await.unwrap().is_none(), "forged bytes never pass");
        slot.install(Arc::new(FixedSource(Some(body))));
        let served = source_fallback(&slot, hash, &permit, &lease).await.unwrap();
        assert_eq!(served.as_deref(), Some(body), "exact bytes are served");
        // Hash-true bytes still meet the scene intake: a JSON document under a GLB MIME is refused, never stored.
        let glb_permit = SceneAssetTransferPermit { content_type: "model/gltf-binary".into(), ..permit.clone() };
        assert!(source_fallback(&slot, hash, &glb_permit, &lease).await.is_err(), "the intake check runs on source bytes");
        lease.stop.request();
        assert!(source_fallback(&slot, hash, &permit, &lease).await.unwrap().is_none(), "a closed verse asks nothing");
    }
}
