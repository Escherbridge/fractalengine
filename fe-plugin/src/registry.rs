//! Plugin registry — tracks installed plugins and their lifecycle state.

use std::collections::HashMap;

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Plugin tier
// ---------------------------------------------------------------------------

/// Execution tier for a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginTier {
    /// Compiled directly into the host binary (full Bevy access).
    Native,
    /// Interpreted via Rhai (sandboxed, hot-reloadable).
    Rhai,
    /// Compiled to WebAssembly (Phase 9A.2 — not yet implemented).
    Wasm,
}

// ---------------------------------------------------------------------------
// Plugin state
// ---------------------------------------------------------------------------

/// Lifecycle state of an installed plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginState {
    /// Installed but not yet activated.
    Installed,
    /// Running normally.
    Active,
    /// Running with reduced capabilities; carries a reason string.
    Degraded(String),
    /// Explicitly deactivated by the user or host.
    Deactivated,
}

// ---------------------------------------------------------------------------
// Plugin entry
// ---------------------------------------------------------------------------

/// Metadata for a single registered plugin.
#[derive(Debug, Clone)]
pub struct PluginEntry {
    /// Manifest identifier (e.g. hexon URI or crate name).
    pub manifest_id: String,
    /// Current lifecycle state.
    pub state: PluginState,
    /// Execution tier.
    pub tier: PluginTier,
}

// ---------------------------------------------------------------------------
// Plugin registry (Bevy Resource)
// ---------------------------------------------------------------------------

/// Central registry of all installed plugins.
///
/// Stored as a Bevy [`Resource`] so systems can query plugin state.
#[derive(Debug, Resource)]
pub struct PluginRegistry {
    plugins: HashMap<String, PluginEntry>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Register a new plugin. Returns `Err` if a plugin with the same ID
    /// already exists.
    pub fn register(
        &mut self,
        plugin_id: &str,
        manifest_id: &str,
        tier: PluginTier,
    ) -> Result<(), RegistryError> {
        if self.plugins.contains_key(plugin_id) {
            return Err(RegistryError::AlreadyExists {
                plugin_id: plugin_id.to_string(),
            });
        }
        self.plugins.insert(
            plugin_id.to_string(),
            PluginEntry {
                manifest_id: manifest_id.to_string(),
                state: PluginState::Installed,
                tier,
            },
        );
        Ok(())
    }

    /// Activate a registered plugin. Only transitions from `Installed` or
    /// `Deactivated`.
    pub fn activate(&mut self, plugin_id: &str) -> Result<(), RegistryError> {
        let entry = self.plugins.get_mut(plugin_id).ok_or_else(|| {
            RegistryError::NotFound {
                plugin_id: plugin_id.to_string(),
            }
        })?;
        match &entry.state {
            PluginState::Installed | PluginState::Deactivated => {
                entry.state = PluginState::Active;
                Ok(())
            }
            PluginState::Active => Err(RegistryError::InvalidTransition {
                plugin_id: plugin_id.to_string(),
                from: "Active".to_string(),
                to: "Active".to_string(),
            }),
            PluginState::Degraded(_) => {
                // Allow re-activation from degraded
                entry.state = PluginState::Active;
                Ok(())
            }
        }
    }

    /// Deactivate a running plugin.
    pub fn deactivate(&mut self, plugin_id: &str) -> Result<(), RegistryError> {
        let entry = self.plugins.get_mut(plugin_id).ok_or_else(|| {
            RegistryError::NotFound {
                plugin_id: plugin_id.to_string(),
            }
        })?;
        match &entry.state {
            PluginState::Active | PluginState::Degraded(_) => {
                entry.state = PluginState::Deactivated;
                Ok(())
            }
            PluginState::Installed => Err(RegistryError::InvalidTransition {
                plugin_id: plugin_id.to_string(),
                from: "Installed".to_string(),
                to: "Deactivated".to_string(),
            }),
            PluginState::Deactivated => Err(RegistryError::InvalidTransition {
                plugin_id: plugin_id.to_string(),
                from: "Deactivated".to_string(),
                to: "Deactivated".to_string(),
            }),
        }
    }

    /// Get the current state of a plugin.
    pub fn get_state(&self, plugin_id: &str) -> Option<&PluginState> {
        self.plugins.get(plugin_id).map(|e| &e.state)
    }

    /// Get a full plugin entry.
    pub fn get(&self, plugin_id: &str) -> Option<&PluginEntry> {
        self.plugins.get(plugin_id)
    }

    /// Mutable access to the underlying map (for internal state transitions).
    pub fn plugins_mut(&mut self) -> &mut HashMap<String, PluginEntry> {
        &mut self.plugins
    }

    /// List all currently active plugin IDs.
    pub fn list_active(&self) -> Vec<String> {
        self.plugins
            .iter()
            .filter(|(_, e)| matches!(e.state, PluginState::Active))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Remove a plugin from the registry entirely.
    pub fn unregister(&mut self, plugin_id: &str) -> Result<PluginEntry, RegistryError> {
        self.plugins.remove(plugin_id).ok_or_else(|| {
            RegistryError::NotFound {
                plugin_id: plugin_id.to_string(),
            }
        })
    }

    /// Total number of registered plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, thiserror::Error)]
pub enum RegistryError {
    #[error("Plugin '{plugin_id}' already registered")]
    AlreadyExists { plugin_id: String },
    #[error("Plugin '{plugin_id}' not found")]
    NotFound { plugin_id: String },
    #[error("Invalid state transition for plugin '{plugin_id}': {from} -> {to}")]
    InvalidTransition {
        plugin_id: String,
        from: String,
        to: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get_state() {
        let mut reg = PluginRegistry::new();
        reg.register("p1", "manifest://test", PluginTier::Rhai).unwrap();
        assert_eq!(reg.get_state("p1"), Some(&PluginState::Installed));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn register_duplicate_fails() {
        let mut reg = PluginRegistry::new();
        reg.register("p1", "manifest://test", PluginTier::Rhai).unwrap();
        assert!(reg.register("p1", "manifest://test2", PluginTier::Native).is_err());
    }

    #[test]
    fn activate_deactivate_lifecycle() {
        let mut reg = PluginRegistry::new();
        reg.register("p1", "m", PluginTier::Rhai).unwrap();

        // Installed -> Active
        reg.activate("p1").unwrap();
        assert_eq!(reg.get_state("p1"), Some(&PluginState::Active));
        assert_eq!(reg.list_active(), vec!["p1".to_string()]);

        // Active -> Deactivated
        reg.deactivate("p1").unwrap();
        assert_eq!(reg.get_state("p1"), Some(&PluginState::Deactivated));
        assert!(reg.list_active().is_empty());

        // Deactivated -> Active (re-activation)
        reg.activate("p1").unwrap();
        assert_eq!(reg.get_state("p1"), Some(&PluginState::Active));
    }

    #[test]
    fn activate_nonexistent_fails() {
        let mut reg = PluginRegistry::new();
        assert!(reg.activate("ghost").is_err());
    }

    #[test]
    fn deactivate_installed_fails() {
        let mut reg = PluginRegistry::new();
        reg.register("p1", "m", PluginTier::Rhai).unwrap();
        // Cannot deactivate from Installed (must activate first)
        assert!(reg.deactivate("p1").is_err());
    }

    #[test]
    fn activate_already_active_fails() {
        let mut reg = PluginRegistry::new();
        reg.register("p1", "m", PluginTier::Native).unwrap();
        reg.activate("p1").unwrap();
        assert!(reg.activate("p1").is_err());
    }

    #[test]
    fn unregister_removes_plugin() {
        let mut reg = PluginRegistry::new();
        reg.register("p1", "m", PluginTier::Rhai).unwrap();
        let entry = reg.unregister("p1").unwrap();
        assert_eq!(entry.manifest_id, "m");
        assert!(reg.is_empty());
    }
}
