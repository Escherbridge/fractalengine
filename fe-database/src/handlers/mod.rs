//! Extracted command handlers for the DB dispatch loop.
//!
//! Each sub-module groups related handlers by domain:
//! - `crud` — Verse/Fractal/Petal/Node creation, GLTF import, hierarchy loading
//! - `entity` — Rename, delete, description updates
//! - `transform` — Node position/rotation/scale and URL persistence
//! - `rbac` — Role resolution, assignment, revocation
//! - `invite` — Verse invite generation and join-by-invite
//! - `api_token` — API token minting, revocation, and listing
//! - `seed` — Default data seeding
//! - `admin` — Database reset

pub mod admin;
pub mod api_token;
pub mod crud;
pub mod entity;
pub mod invite;
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
