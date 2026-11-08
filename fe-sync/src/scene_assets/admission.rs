//! Bounded private DB admission reads; see AGENTS.md.

use fe_runtime::{
    blob_store::BlobHash,
    messages::DbCommand,
    scene_asset::{SceneAssetTransferPermit, SceneAssetTransferPurpose, SceneAssetTransferRequest},
};
use std::time::{Duration, Instant};
const ADMISSION_TIMEOUT: Duration = Duration::from_secs(2);

/// The DB did not answer in time or its queue was full: retry soon, not a denial (QA-0143).
#[derive(Debug)]
pub(crate) struct AdmissionBusy(&'static str);

impl std::fmt::Display for AdmissionBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for AdmissionBusy {}

/// Whether `error` is a busy admission rather than a refusal.
pub(crate) fn is_busy(error: &anyhow::Error) -> bool {
    error.downcast_ref::<AdmissionBusy>().is_some()
}

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
    match db.try_send(DbCommand::InspectSceneAssetTransfer {
        request: SceneAssetTransferRequest {
            verse_id: verse.into(),
            hash,
            node_id: None,
            purpose,
            deadline,
        },
        reply,
    }) {
        Ok(()) => {}
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            return Err(AdmissionBusy("asset admission queue full").into())
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            anyhow::bail!("asset admission queue closed")
        }
    }
    loop {
        match rx.try_recv() {
            Ok(result) => return result.map_err(anyhow::Error::msg),
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                anyhow::bail!("asset admission owner disappeared")
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err(AdmissionBusy("asset admission timed out").into());
        }
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
