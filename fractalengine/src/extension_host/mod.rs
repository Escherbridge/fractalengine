//! Binary-side extension host modules; `plugin_bridge` wires them at startup (src/AGENTS.md §extension-host).

/// R19 rows 6-8, the Owner kill-list override, consent and override hydration, the durable audit sink.
pub(crate) mod governance;
/// The network broker over the host's roles, audit sink and local credential file.
pub(crate) mod net_sources;
/// The Extensions panel's op drain against the plugin manager.
pub(crate) mod ops_drain;
/// `petal.map` hook resolution for the terrain bridge.
pub(crate) mod petal_map;
/// The OD-8 signed revocation-list pull.
pub(crate) mod revocation_pull;
/// Configured registry sources (local, signed static index, protocol v1).
pub(crate) mod sources;
/// The terrain package's slot commands as editor actions.
pub(crate) mod terrain_commands;
