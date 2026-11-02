//! Checked development reset; see handlers/AGENTS.md.

use crate::admin::DevelopmentResetPermit;
use crate::repo::Db;
use crate::BlobStoreHandle;

pub(crate) struct ResetOutcome {
    pub cleared: bool,
    pub seed: anyhow::Result<(String, Vec<String>)>,
}

pub(crate) async fn reset_database_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    local_did: &str,
    permit: &DevelopmentResetPermit,
) -> ResetOutcome {
    if let Err(error) = crate::admin::clear_for_development_reset(db, local_did, permit).await {
        return ResetOutcome { cleared: false, seed: Err(error) };
    }
    ResetOutcome {
        cleared: true,
        seed: super::seed::seed_default_data(db, blob_store, local_did).await
            .map_err(|error| anyhow::anyhow!("Local database cleared, but default content could not be restored: {error}")),
    }
}
