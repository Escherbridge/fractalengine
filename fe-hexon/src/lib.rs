// dead_code kept per module (see fe-hexon/src/AGENTS.md §lint-scope); the compile sweep prunes.
#[allow(dead_code)]
pub mod authz;
#[allow(dead_code)]
pub mod entitlement_client;
#[allow(dead_code)]
pub mod entitlement_gate;
#[allow(dead_code)]
pub mod handlers;
#[allow(dead_code)]
pub mod manifest;
#[allow(dead_code)]
pub mod p2p;
#[allow(dead_code)]
pub mod package;
#[allow(dead_code)]
pub mod partial;
#[allow(dead_code)]
pub mod publisher;
#[allow(dead_code)]
pub mod registry;
#[allow(dead_code)]
pub mod registry_source;
#[cfg(feature = "remote")]
#[allow(dead_code)]
pub mod remote;
#[allow(dead_code)]
pub mod signature;
#[allow(dead_code)]
pub mod snapshot;
