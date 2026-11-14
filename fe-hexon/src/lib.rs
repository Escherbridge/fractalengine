// dead_code allowed per module (UI-09) until a compile sweep lists the true leftovers; see AGENTS.md §lint-scope.
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
