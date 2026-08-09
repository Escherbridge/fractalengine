//! Extracted DB dispatch-loop command handlers (domain map:
//! fe-database/src/AGENTS.md §handlers).

pub mod admin;
pub mod api_token;
pub mod crate_registry;
pub mod crud;
pub mod entity;
pub mod entity_property;
pub mod field_def;
pub mod invite;
pub mod iot_reading;
pub mod node_log;
pub mod petal_terrain;
pub(crate) mod preconditions;
pub mod rbac;
pub mod seed;
pub mod transform;

use crossbeam::channel::Sender;
use fe_runtime::messages::DbResult;

/// Send a DbResult to the response channel, logging if the receiver is gone.
pub fn send_result(tx: &Sender<DbResult>, result: DbResult) {
    if tx.send(result).is_err() {
        tracing::warn!("Result channel closed — UI may have shut down");
    }
}
