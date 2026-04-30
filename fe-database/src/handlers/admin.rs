use crate::BlobStoreHandle;
use crate::repo::Db;

/// Wipe all tables, re-apply schema, and re-seed default data.
pub(crate) async fn reset_database_handler(
    db: &Db,
    blob_store: &BlobStoreHandle,
    local_did: &str,
) -> anyhow::Result<(String, Vec<String>)> {
    crate::admin::clear_all_tables(db).await?;
    super::seed::seed_default_data(db, blob_store, local_did).await
}
