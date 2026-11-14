//! Binary-side extension host modules; `plugin_bridge` wires them at startup (src/AGENTS.md §extension-host).

/// The local admin-channel listener (`fe-ext admin`) relaying OS-user requests to the ops worker.
pub(crate) mod admin_channel;
/// R19 rows 6-8, the Owner kill-list override, consent and override hydration, the durable audit sink.
pub(crate) mod governance;
/// `asset.import` over the binding worker (fe-api `ImportHooks`).
pub(crate) mod import_hooks;
/// Licence token requests through a source's checked transport, kept where governance row 6 reads them.
#[cfg(feature = "entitlement")]
pub(crate) mod licence_client;
/// The network broker over the host's roles, audit sink and local credential file.
pub(crate) mod net_sources;
/// The Extensions panel's op drain against the plugin manager.
pub(crate) mod ops_drain;
/// `petal.map` hook resolution for the terrain bridge.
pub(crate) mod petal_map;
/// The OD-8 signed revocation-list pull.
pub(crate) mod revocation_pull;
/// Other packages' slot commands onto the integration bus.
pub(crate) mod slot_drain;
/// Configured registry sources (local, signed static index, protocol v1).
pub(crate) mod sources;
/// The terrain package's slot commands as editor actions.
pub(crate) mod terrain_commands;
/// A build's manifest `ui` entries into the slot registry and the panel content set.
pub(crate) mod ui_registry;
