//! SPEC-8 migration scaffolding: local-only staged migration surface wrapping —
//! never modifying — the existing legacy log-first seam. See `AGENTS.md` for
//! the runtime-env-var rationale, the permanence of the D-CL20 deferral, and
//! the rule that no shadow data is ever exposed through fe-api, fe-ui, or any
//! WebSocket surface.

pub mod boundary;
pub mod candidate;
pub mod comparator;
pub mod mapping_inventory;
pub mod mode;
pub mod rebuild;
pub mod report;
pub mod shadow_store;
