//! Extracted DB dispatch-loop command handlers (domain map:
//! fe-database/src/AGENTS.md §handlers).

pub(crate) mod checked_mutation;
pub(crate) mod stamp_overrides;
pub mod admin;
pub mod api_token;
pub mod asset_backfill;
pub mod asset_gc;
pub mod blob_migration;
pub mod claim_orphaned_verse;
pub mod crate_registry;
pub mod crud;
pub mod device_payload;
pub mod elevated_query;
pub mod entity;
pub mod entity_property;
pub mod field_def;
pub mod inbound_admissions;
pub mod invite;
pub(crate) mod local_catalog;
pub mod iot_reading;
pub mod node_history;
pub mod node_log;
pub mod node_measurements;
pub mod node_view;
pub mod petal_render_distance;
pub mod petal_terrain;
pub(crate) mod petal_requirements;
pub mod plugin_host;
pub(crate) mod preconditions;
pub mod raw_query;
pub mod rbac;
pub mod replicated_row;
pub mod scene_materials;
pub mod scoped_query;
pub mod seed;
pub mod transform;
pub mod ts_partial;
pub mod verse_settings;
pub mod visibility_groups;

use crossbeam::channel::Sender;
use fe_runtime::messages::DbResult;

/// Send a DbResult to the response channel, logging if the receiver is gone.
pub fn send_result(tx: &Sender<DbResult>, result: DbResult) {
    if tx.send(result).is_err() {
        tracing::warn!("Result channel closed — UI may have shut down");
    }
}

/// A failed command's reply: `RequestFailed` echoing a correlated (API)
/// caller's id, the legacy wildcard `Error` for an uncorrelated (GUI) one.
/// See fe-runtime src/AGENTS.md §api-reply-correlation.
pub(crate) fn correlated_failure(correlation_id: &Option<String>, error: String) -> DbResult {
    match correlation_id {
        Some(correlation_id) => DbResult::RequestFailed {
            error,
            correlation_id: correlation_id.clone(),
        },
        None => DbResult::Error(error),
    }
}

pub(crate) mod entity_access;
