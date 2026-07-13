//! Texture registry for hexon-installed blob textures (FR-4, C6).
//!
//! Copy-adapted from [`crate::ui::UiExtensionRegistry`]: append-only,
//! per-plugin `register`/`unregister_all`/`get`. Entries reference
//! content-addressed blob hashes already installed via a hexon package —
//! v1 never accepts raw texture bytes from a plugin (C6).

use serde::{Deserialize, Serialize};

/// A single texture entry available for a primitive's `texture_ref`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureEntry {
    /// Stable id primitives reference via `texture_ref`.
    pub id: String,
    /// The plugin/hexon that registered this entry.
    pub plugin_id: String,
    /// Content-addressed blob hash for the albedo texture (role-resolved
    /// via `MaterialHandle` at load time — see `fe-hexon::handlers::material`).
    pub blob_hash: String,
    /// Display label for UI pickers.
    pub label: String,
}

/// Registry of hexon-installed textures available to primitives.
#[derive(Debug, Clone, Default)]
pub struct TextureRegistry {
    entries: Vec<TextureEntry>,
}

impl TextureRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a texture entry. Re-registering the same `id` replaces the prior entry.
    pub fn register(&mut self, entry: TextureEntry) {
        self.entries.retain(|e| e.id != entry.id);
        self.entries.push(entry);
    }

    /// Remove all entries registered by a specific plugin.
    pub fn unregister_all(&mut self, plugin_id: &str) {
        self.entries.retain(|e| e.plugin_id != plugin_id);
    }

    /// Look up a texture entry by id.
    pub fn get(&self, id: &str) -> Option<&TextureEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// All registered entries.
    pub fn all(&self) -> impl Iterator<Item = &TextureEntry> {
        self.entries.iter()
    }

    /// Total number of registered entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, plugin_id: &str) -> TextureEntry {
        TextureEntry {
            id: id.to_string(),
            plugin_id: plugin_id.to_string(),
            blob_hash: format!("hash-{id}"),
            label: format!("Texture {id}"),
        }
    }

    #[test]
    fn register_and_get() {
        let mut reg = TextureRegistry::new();
        reg.register(entry("brick-01", "plugin-a"));
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get("brick-01").unwrap().blob_hash, "hash-brick-01");
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn register_same_id_replaces_entry() {
        let mut reg = TextureRegistry::new();
        reg.register(entry("brick-01", "plugin-a"));
        let mut updated = entry("brick-01", "plugin-a");
        updated.label = "Updated".to_string();
        reg.register(updated);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get("brick-01").unwrap().label, "Updated");
    }

    #[test]
    fn unregister_all_removes_only_target_plugin() {
        let mut reg = TextureRegistry::new();
        reg.register(entry("a", "plugin-a"));
        reg.register(entry("b", "plugin-a"));
        reg.register(entry("c", "plugin-b"));
        assert_eq!(reg.len(), 3);

        reg.unregister_all("plugin-a");

        assert_eq!(reg.len(), 1);
        assert!(reg.get("c").is_some());
        assert!(reg.get("a").is_none());
    }

    #[test]
    fn empty_registry_reports_empty() {
        let reg = TextureRegistry::new();
        assert!(reg.is_empty());
    }
}
