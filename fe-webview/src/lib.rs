//! Embedded portal backends with an explicit external-browser fallback; see src/AGENTS.md.

pub mod backend;
pub mod backends;
pub mod ipc;
pub mod overlay;
pub mod petal_portal;
pub mod plugin;
pub mod security;
#[cfg(feature = "backend-tauri")]
pub mod tauri_commands;
