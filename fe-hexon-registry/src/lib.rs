//! Hosted hexon registry HTTP service — see `AGENTS.md` for design rationale.

pub mod config;
pub mod index;
pub mod routes;

pub use config::RegistryConfig;
pub use index::{scan_dir, HexonIndexEntry};
pub use routes::{build_router, ApiResponse, RegistryState};
