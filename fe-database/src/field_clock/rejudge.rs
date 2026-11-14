//! DB-local re-judge of clocked rows on role change; see field_clock/AGENTS.md §rejudge.

use super::judge::VerseView;
use super::store;
use crate::handlers::inbound_admissions::RowReport;
use crate::handlers::replicated_row::SceneChangeSender;
use crate::handlers::scene_publication::Rejudged;
use crate::repo::Db;
use fe_runtime::messages::ReplicatedRowOutcome;
use std::sync::Mutex;

/// Rows re-judged between yields to the runtime; a larger set is warned once and still finished in this call.
pub(crate) const REJUDGE_ROW_CAP: usize = 4096;
/// Local re-judge results held for the DB loop's next echo before the oldest is dropped.
const LOCAL_OUTBOX_CAP: usize = 4096;

/// What one re-judge call changed: rows to echo, and nodes it removed locally (node id, petal id).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Rejudge {
    pub(crate) reports: Vec<RowReport>,
    pub(crate) removed_nodes: Vec<(String, String)>,
}

impl Rejudge {
    fn extend(&mut self, other: Rejudge) {
        self.reports.extend(other.reports);
        self.removed_nodes.extend(other.removed_nodes);
    }
}

/// Recompute the winners of every row holding a version by `dids`; one report per changed row.
pub(crate) async fn rejudge_authors(
    db: &Db,
    verse_id: &str,
    dids: &[String],
    changes: Option<&SceneChangeSender>,
) -> anyhow::Result<Vec<RowReport>> {
    Ok(rejudge_authors_detailed(db, verse_id, dids, changes).await?.reports)
}

/// [`rejudge_authors`] keeping the removed nodes (the GUI drops them by `DbResult::NodeDeleted`).
pub(crate) async fn rejudge_authors_detailed(
    db: &Db,
    verse_id: &str,
    dids: &[String],
    changes: Option<&SceneChangeSender>,
) -> anyhow::Result<Rejudge> {
    let mut rejudged = Rejudge::default();
    let rows = store::load_rows_by_authors(db, verse_id, dids).await?;
    if rows.is_empty() {
        return Ok(rejudged);
    }
    // Rows are sorted, so a cap would skip the same tail on every call and leave its winners unjudged (fail-open).
    if rows.len() > REJUDGE_ROW_CAP {
        tracing::warn!(verse_id, rows = rows.len(), chunk = REJUDGE_ROW_CAP, "large re-judge; yielding between chunks");
    }
    let view = VerseView::load(db, verse_id).await?;
    if view.owner.is_empty() {
        // Nothing is admissible before the manifest names an owner; demoting now would erase every key.
        tracing::warn!(verse_id, "re-judge skipped: the verse owner is not known yet");
        return Ok(rejudged);
    }
    for (index, (table, rid)) in rows.iter().enumerate() {
        if index > 0 && index % REJUDGE_ROW_CAP == 0 {
            tokio::task::yield_now().await;
        }
        let result = match table.as_str() {
            // A present node is recomputed from its stored versions (judged against the admitted statements);
            // an absent one is left to the candidate re-judge, which holds its petal identity.
            "node" => crate::handlers::scene_publication::rejudge(db, verse_id, rid, &[], &view.owner, changes, false)
                .await
                .map(|node| match node {
                    Rejudged::Unchanged => None,
                    Rejudged::Restored(outcome) => Some(outcome),
                    Rejudged::Removed { petal_id } => {
                        rejudged.removed_nodes.push((rid.clone(), petal_id));
                        None
                    }
                }),
            "verse" | "fractal" | "petal" => {
                crate::handlers::clocked_row::rematerialize(db, verse_id, table, rid, &view)
                    .await
                    .map(|applied| matches!(applied, ReplicatedRowOutcome::Applied).then_some(applied))
            }
            _ => continue,
        };
        match result {
            Ok(Some(row_outcome)) => rejudged.reports.push(RowReport {
                verse_id: verse_id.to_owned(),
                table: table.clone(),
                record_id: rid.clone(),
                outcome: row_outcome,
            }),
            Ok(None) => {}
            // One row's failure never blocks the others; its next re-judge or inbound write retries it.
            Err(error) => tracing::warn!(verse_id, table = %table, rid = %rid, "clocked row re-judge failed: {error:#}"),
        }
    }
    if !rejudged.reports.is_empty() || !rejudged.removed_nodes.is_empty() {
        tracing::info!(
            verse_id,
            authors = dids.len(),
            changed = rejudged.reports.len(),
            removed = rejudged.removed_nodes.len(),
            "re-judged clocked rows after a role change"
        );
    }
    Ok(rejudged)
}

/// Re-judge results of LOCAL role statements, held until the DB loop's next author-role retry echoes them.
static LOCAL_OUTBOX: Mutex<Vec<(String, Rejudge)>> = Mutex::new(Vec::new());

/// Re-judge after a local role statement and hold the result for [`take_local`] (the caller has no echo channel).
pub(crate) async fn rejudge_local(db: &Db, verse_id: &str, dids: &[String]) -> anyhow::Result<()> {
    let rejudged = rejudge_authors_detailed(db, verse_id, dids, None).await?;
    if rejudged.reports.is_empty() && rejudged.removed_nodes.is_empty() {
        return Ok(());
    }
    let Ok(mut outbox) = LOCAL_OUTBOX.lock() else {
        tracing::warn!(verse_id, "local re-judge outbox poisoned; this re-judge is not echoed");
        return Ok(());
    };
    outbox.push((verse_id.to_owned(), rejudged));
    let size = |held: &Rejudge| held.reports.len() + held.removed_nodes.len();
    let mut held: usize = outbox.iter().map(|(_, held)| size(held)).sum();
    if held > LOCAL_OUTBOX_CAP {
        tracing::warn!(held, cap = LOCAL_OUTBOX_CAP, "local re-judge outbox full; the oldest echoes are dropped");
    }
    // Keep the newest entry even when it alone exceeds the cap (its rows are already written).
    while held > LOCAL_OUTBOX_CAP && outbox.len() > 1 {
        held -= size(&outbox.remove(0).1);
    }
    Ok(())
}

/// Take every held local re-judge result of `verse_id`.
pub(crate) fn take_local(verse_id: &str) -> Rejudge {
    let mut taken = Rejudge::default();
    let Ok(mut outbox) = LOCAL_OUTBOX.lock() else {
        return taken;
    };
    let mut kept = Vec::with_capacity(outbox.len());
    for (verse, held) in outbox.drain(..) {
        if verse == verse_id {
            taken.extend(held);
        } else {
            kept.push((verse, held));
        }
    }
    *outbox = kept;
    taken
}
