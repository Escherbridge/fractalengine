//! Extracted command handlers for the DB dispatch loop.
//!
//! Each sub-module groups related handlers by domain:
//! - `crud` — Verse/Fractal/Petal/Node creation, GLTF import, hierarchy loading
//! - `entity` — Rename, delete, description updates
//! - `entity_property` — Custom property CRUD for nodes
//! - `field_def` — Field definition schema CRUD
//! - `transform` — Node position/rotation/scale and URL persistence
//! - `rbac` — Role resolution, assignment, revocation
//! - `invite` — Verse invite generation and join-by-invite
//! - `api_token` — API token minting, revocation, and listing
//! - `seed` — Default data seeding
//! - `admin` — Database reset
//! - `crate_registry` — Hexon crate registry install/uninstall

pub mod admin;
pub mod api_token;
pub mod crate_registry;
pub mod crud;
pub mod entity;
pub mod entity_property;
pub mod field_def;
pub mod invite;
pub mod node_log;
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
