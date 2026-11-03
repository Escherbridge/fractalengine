//! Bounded private DB admission reads; see AGENTS.md.

use fe_runtime::{
    blob_store::BlobHash,
    messages::DbCommand,
    scene_asset::{SceneAssetTransferPermit, SceneAssetTransferPurpose, SceneAssetTransferRequest},
};
use std::time::{Duration, Instant};
const ADMISSION_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) async fn admit(
    db: &Option<crossbeam::channel::Sender<DbCommand>>,
    verse: &str,
    hash: BlobHash,
    purpose: SceneAssetTransferPurpose,
) -> anyhow::Result<SceneAssetTransferPermit> {
    let db = db
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("asset admission database unavailable"))?;
    let (reply, rx) = crossbeam::channel::bounded(1);
    let deadline = Instant::now() + ADMISSION_TIMEOUT;
    db.try_send(DbCommand::InspectSceneAssetTransfer {
        request: SceneAssetTransferRequest {
            verse_id: verse.into(),
            hash,
            node_id: None,
            purpose,
            deadline,
        },
        reply,
    })
    .map_err(|error| anyhow::anyhow!("asset admission queue unavailable: {error}"))?;
    loop {
        match rx.try_recv() {
            Ok(result) => return result.map_err(anyhow::Error::msg),
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                anyhow::bail!("asset admission owner disappeared")
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {}
        }
        anyhow::ensure!(Instant::now() < deadline, "asset admission timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(super) async fn admit_job(
    db: &Option<crossbeam::channel::Sender<DbCommand>>,
    verse: &str,
    hash: BlobHash,
    purpose: SceneAssetTransferPurpose,
) -> anyhow::Result<SceneAssetTransferPermit> {
    let mut last = None;
    for attempt in 0..6 {
        match admit(db, verse, hash, purpose.clone()).await {
            Ok(permit) => return Ok(permit),
            Err(error) => last = Some(error),
        }
        if attempt < 5 {
            tokio::time::sleep(Duration::from_millis(200 * (attempt + 1))).await;
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("Scene asset admission unavailable")))
}
