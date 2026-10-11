//! SurrealQL query submission action, plus the GUI query-id minting shared by
//! every GUI `RawQuery` sender (fe-ui/src/AGENTS.md §gis-query-ui).

use std::sync::atomic::{AtomicU64, Ordering};

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

use crate::plugin::InspectorFormState;

static NEXT_UI_QUERY_ID: AtomicU64 = AtomicU64::new(1);

/// A fresh GUI `RawQuery` correlation id, `ui-{kind}-{n}` — the hyphens mean
/// it can never collide with an API ULID.
pub(crate) fn next_ui_query_id(kind: &str) -> String {
    format!(
        "ui-{kind}-{}",
        NEXT_UI_QUERY_ID.fetch_add(1, Ordering::Relaxed)
    )
}

/// Send the Query tab's SQL; its reply is claimed by `query_request_id`.
pub(crate) fn submit(db_sender: &DbCommandSender, inspector: &mut InspectorFormState, sql: String) {
    let request_id = next_ui_query_id("query");
    inspector.query_request_id = Some(request_id.clone());
    if db_sender
        .0
        .send(DbCommand::RawQuery {
            correlation_id: Some(request_id),
            sql,
            vars: std::collections::HashMap::new(),
        })
        .is_err()
    {
        // No reply will ever claim this tab: end the spinner here.
        inspector.query_request_id = None;
        inspector.query_loading = false;
        inspector.query_result = Some("Error: DB channel closed".to_string());
        bevy::log::warn!("db_sender channel closed — RawQuery not dispatched");
    }
}

#[cfg(test)]
mod tests {
    /// GUI query ids are unique per send and never ULID-shaped (Crockford
    /// base32 has no '-'), so a GUI reply can never claim an API waiter.
    #[test]
    fn ui_query_ids_are_unique_and_never_ulid_shaped() {
        let first = super::next_ui_query_id("gis");
        let second = super::next_ui_query_id("gis");
        assert_ne!(first, second);
        assert!(first.starts_with("ui-gis-"), "{first}");
    }

    /// The Query tab's sent `RawQuery` carries the id its reply is claimed by;
    /// a dead DB channel ends the tab's spinner instead of leaving it stuck.
    #[test]
    fn submit_stores_its_claim_id_and_a_dead_channel_ends_loading() {
        use crate::plugin::InspectorFormState;
        use fe_runtime::app::DbCommandSender;
        use fe_runtime::messages::DbCommand;

        let (tx, rx) = crossbeam::channel::bounded(4);
        let sender = DbCommandSender(tx);
        let mut inspector = InspectorFormState {
            query_loading: true,
            ..Default::default()
        };
        super::submit(&sender, &mut inspector, "SELECT * FROM node".to_string());
        match rx.try_recv() {
            Ok(DbCommand::RawQuery { correlation_id, .. }) => {
                assert!(correlation_id.is_some());
                assert_eq!(correlation_id, inspector.query_request_id);
            }
            other => panic!("expected a correlated RawQuery, got {other:?}"),
        }
        assert!(inspector.query_loading, "still awaiting its reply");

        drop(rx);
        super::submit(&sender, &mut inspector, "SELECT * FROM node".to_string());
        assert!(!inspector.query_loading);
        assert!(inspector.query_request_id.is_none());
        assert_eq!(
            inspector.query_result.as_deref(),
            Some("Error: DB channel closed")
        );
    }
}
