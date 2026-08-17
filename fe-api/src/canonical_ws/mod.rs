//! SPEC-7 commit/preview wire protocol surface (`/ws/canonical`) — opt-in, unmounted by design.
//! See `AGENTS.md` for what this module is, why it exists beside `crate::ws` rather than
//! inside it, and its current (deliberately unmounted) wiring status.

pub mod handler;
pub mod router;
pub mod state;

pub use router::build_router_with_canonical_log;
pub use state::CanonicalLogState;
