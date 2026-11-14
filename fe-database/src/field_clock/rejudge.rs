//! DB-local re-judge of clocked rows on role change (body: CLOCK-10); see field_clock/AGENTS.md §rejudge.

use crate::handlers::inbound_admissions::RowReport;
use crate::handlers::replicated_row::SceneChangeSender;
use crate::repo::Db;

/// Recompute the winners of every row holding a version by `dids`; one report per changed row.
pub(crate) async fn rejudge_authors(
    db: &Db,
    verse_id: &str,
    dids: &[String],
    changes: Option<&SceneChangeSender>,
) -> anyhow::Result<Vec<RowReport>> {
    let _ = (db, verse_id, dids, changes);
    unimplemented!("CLOCK-10")
}
