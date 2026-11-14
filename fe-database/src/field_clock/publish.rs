//! Outbound clocked hierarchy rows (body: CLOCK-08); see field_clock/AGENTS.md §wire.

use crate::repo::Db;

/// Stamp the local author's own versions of a row (`restamp` = re-clock unchanged keys) and publish it.
pub(crate) async fn publish_clocked_row(
    db: &Db,
    blobs: &crate::BlobStoreHandle,
    sender: Option<&crate::ReplicationSender>,
    local_did: &str,
    verse_id: &str,
    table: &str,
    rid: &str,
    restamp: bool,
) -> anyhow::Result<()> {
    let _ = (db, blobs, sender, local_did, verse_id, table, rid, restamp);
    unimplemented!("CLOCK-08")
}
