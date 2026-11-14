//! Per-version write-time admission (body: CLOCK-05); see field_clock/AGENTS.md §admission.

use super::wire::FieldVersion;
use crate::repo::Db;

/// A verse's owner and admitted role statements, the only inputs to admission.
pub(crate) struct VerseView {
    pub(crate) owner: String,
    pub(crate) admitted: Vec<fe_policy::RoleStatement>,
}

impl VerseView {
    /// Load the verse owner and its admitted role statements.
    pub(crate) async fn load(db: &Db, verse_id: &str) -> anyhow::Result<Self> {
        let _ = (db, verse_id);
        unimplemented!("CLOCK-05")
    }
}

/// Whether `version`'s author held the key's required level at the version's hlc.
pub(crate) async fn admissible(
    db: &Db,
    verse_id: &str,
    table: &str,
    rid: &str,
    version: &FieldVersion,
    view: &VerseView,
) -> anyhow::Result<bool> {
    let _ = (db, verse_id, table, rid, version, view);
    unimplemented!("CLOCK-05")
}
