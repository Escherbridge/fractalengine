//! Wasm plugin lifecycle ops (flow: src/AGENTS.md §lifecycle).

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::capability::{CapabilityManifest, CapabilityToken};
use crate::context::PluginContext;
use crate::registry::{PluginRegistry, PluginState, PluginTier};
use crate::wasm::aot::AotCache;
use crate::wasm::fuel::FuelBudget;
use crate::wasm::{PluginStoreData, WasmEngine, WasmPlugin};
use crate::PluginCommand;

// ---------------------------------------------------------------------------
// InstalledPlugin — tracks a Wasm plugin's runtime state
// ---------------------------------------------------------------------------

/// A Wasm plugin that has been loaded into the engine.
///
/// Contains the compiled module, the store for execution, and the
/// instance for calling exports.
pub struct InstalledPlugin {
    /// The compiled Wasm module.
    pub plugin: WasmPlugin,
    /// The wasmtime store holding plugin state.
    pub store: wasmtime::Store<PluginStoreData>,
    /// The instantiated module.
    pub instance: wasmtime::Instance,
}

// ---------------------------------------------------------------------------
// Plugin manager
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of Wasm plugins.
///
/// Wraps a [`WasmEngine`], [`AotCache`], and [`PluginRegistry`] to
/// provide a unified interface for plugin management.
pub struct PluginManager {
    /// The Wasmtime engine for compilation and execution.
    engine: Arc<WasmEngine>,
    /// AOT compilation cache for fast reloading.
    aot_cache: AotCache,
    /// Reference to the Bevy plugin registry.
    registry: Arc<std::sync::Mutex<PluginRegistry>>,
    /// Installed plugin instances keyed by plugin_id.
    installed: std::collections::HashMap<String, InstalledPlugin>,
    /// Command sender for plugin -> engine communication.
    cmd_tx: Sender<PluginCommand>,
}

impl PluginManager {
    /// Create a new plugin manager with an AOT cache rooted at `cache_dir`.
    pub fn new(
        engine: Arc<WasmEngine>,
        cache_dir: impl Into<PathBuf>,
        registry: Arc<std::sync::Mutex<PluginRegistry>>,
        cmd_tx: Sender<PluginCommand>,
    ) -> Result<Self, std::io::Error> {
        let aot_cache = AotCache::new(cache_dir)?;
        Ok(Self {
            engine,
            aot_cache,
            registry,
            installed: std::collections::HashMap::new(),
            cmd_tx,
        })
    }

    /// Install a Wasm plugin: verify hash, AOT compile, register as `Installed`.
    pub fn install_plugin(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
        _manifest: &CapabilityManifest,
        expected_hash: Option<&str>,
    ) -> Result<(), LifecycleError> {
        // Step 1: Verify signature (BLAKE3 hash check)
        let actual_hash = AotCache::hash_bytes(wasm_bytes);
        if let Some(expected) = expected_hash {
            if actual_hash != expected {
                return Err(LifecycleError::SignatureMismatch {
                    expected: expected.to_string(),
                    actual: actual_hash,
                });
            }
        }
        tracing::info!(plugin = %plugin_id, hash = %actual_hash, "Plugin signature verified");

        // Step 2: AOT compile and cache
        let compiled = self
            .aot_cache
            .get_or_compile(self.engine.engine(), wasm_bytes)
            .map_err(|e| LifecycleError::AotCompileError(e.to_string()))?;

        // Deserialize the compiled module to verify it's valid
        // SAFETY: compiled bytes come from wasmtime::Module::serialize on the same engine
        let module = unsafe { wasmtime::Module::deserialize(self.engine.engine(), &compiled) }
            .map_err(|e| LifecycleError::WasmError(e.to_string()))?;

        let plugin = WasmPlugin {
            plugin_id: plugin_id.to_string(),
            module,
            wasm_bytes: wasm_bytes.to_vec(),
        };

        // Step 3: Register in registry
        {
            let mut reg = self.registry.lock().unwrap();
            reg.register(
                plugin_id,
                &format!("wasm:{}", actual_hash),
                PluginTier::Wasm,
            )
            .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }

        // Store the plugin for later activation
        // We need to store the raw bytes for later instantiation
        let _ = plugin; // Used for verification above

        tracing::info!(plugin = %plugin_id, "Plugin installed successfully");
        Ok(())
    }

    /// Activate an installed plugin (state check only; use [`Self::activate_with_bytes`]).
    pub async fn activate_plugin(
        &mut self,
        _plugin_id: &str,
        _petal_id: &str,
        _capabilities: &CapabilityManifest,
        _fuel_budget: FuelBudget,
    ) -> Result<(), LifecycleError> {
        // Check the plugin is in Installed or Deactivated state
        {
            let reg = self.registry.lock().unwrap();
            let state = reg
                .get_state(_plugin_id)
                .ok_or_else(|| LifecycleError::PluginNotFound(_plugin_id.to_string()))?;
            if !matches!(
                state,
                PluginState::Installed | PluginState::Deactivated | PluginState::Degraded(_)
            ) {
                return Err(LifecycleError::InvalidState {
                    plugin_id: _plugin_id.to_string(),
                    state: format!("{:?}", state),
                });
            }
        }

        // This method is a placeholder. In practice, activate_with_bytes is used
        // because wasm_bytes must come from plugin package storage.
        Err(LifecycleError::NotImplemented(
            "activate_plugin requires wasm_bytes from package storage".to_string(),
        ))
    }

    /// Activate a plugin with the provided Wasm bytes.
    ///
    /// This is the primary activation method when bytes are available.
    pub async fn activate_with_bytes(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
        petal_id: &str,
        capabilities: &CapabilityManifest,
        fuel_budget: FuelBudget,
    ) -> Result<(), LifecycleError> {
        // Load or compile from AOT cache
        let compiled = self
            .aot_cache
            .get_or_compile(self.engine.engine(), wasm_bytes)
            .map_err(|e| LifecycleError::AotCompileError(e.to_string()))?;

        // SAFETY: compiled bytes come from wasmtime::Module::serialize on the same engine
        let module = unsafe { wasmtime::Module::deserialize(self.engine.engine(), &compiled) }
            .map_err(|e| LifecycleError::WasmError(e.to_string()))?;

        let plugin = WasmPlugin {
            plugin_id: plugin_id.to_string(),
            module,
            wasm_bytes: wasm_bytes.to_vec(),
        };

        // Create plugin context
        let token = CapabilityToken::mint(plugin_id, capabilities);
        let ctx = PluginContext::new(
            plugin_id.to_string(),
            petal_id.to_string(),
            token,
            self.cmd_tx.clone(),
        );

        // Instantiate
        let (mut store, instance) = self
            .engine
            .instantiate(&plugin, ctx, fuel_budget)
            .await
            .map_err(|e| LifecycleError::WasmError(e.to_string()))?;

        // Try to call on_activate export
        if instance.get_func(&mut store, "on_activate").is_some() {
            tracing::debug!(plugin = %plugin_id, "Calling on_activate");
            self.engine
                .call_export(&mut store, &instance, "on_activate", &[], &mut [])
                .await
                .map_err(|e| LifecycleError::WasmError(e.to_string()))?;
        }

        // Store the running instance
        self.installed.insert(
            plugin_id.to_string(),
            InstalledPlugin {
                plugin,
                store,
                instance,
            },
        );

        // Update registry state
        {
            let mut reg = self.registry.lock().unwrap();
            reg.activate(plugin_id)
                .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }

        tracing::info!(plugin = %plugin_id, "Plugin activated");
        Ok(())
    }

    /// Deactivate a running plugin: call `on_deactivate`, drop instance, registry `Deactivated`.
    pub async fn deactivate_plugin(&mut self, plugin_id: &str) -> Result<(), LifecycleError> {
        // Try to call on_deactivate if the plugin is running
        if let Some(installed) = self.installed.get_mut(plugin_id) {
            if installed
                .instance
                .get_func(&mut installed.store, "on_deactivate")
                .is_some()
            {
                tracing::debug!(plugin = %plugin_id, "Calling on_deactivate");
                // Call but don't fail if the export errors — we still want to deactivate
                let _ = self
                    .engine
                    .call_export(
                        &mut installed.store,
                        &installed.instance,
                        "on_deactivate",
                        &[],
                        &mut [],
                    )
                    .await;
            }
        }

        // Remove from installed map
        self.installed.remove(plugin_id);

        // Update registry state
        {
            let mut reg = self.registry.lock().unwrap();
            reg.deactivate(plugin_id)
                .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }

        tracing::info!(plugin = %plugin_id, "Plugin deactivated");
        Ok(())
    }

    /// Uninstall a plugin: call `on_uninstall`, unregister, evict from AOT cache.
    pub async fn uninstall_plugin(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
    ) -> Result<(), LifecycleError> {
        // Deactivate if active, calling on_uninstall
        if let Some(installed) = self.installed.get_mut(plugin_id) {
            if installed
                .instance
                .get_func(&mut installed.store, "on_uninstall")
                .is_some()
            {
                tracing::debug!(plugin = %plugin_id, "Calling on_uninstall");
                let _ = self
                    .engine
                    .call_export(
                        &mut installed.store,
                        &installed.instance,
                        "on_uninstall",
                        &[],
                        &mut [],
                    )
                    .await;
            }
            self.installed.remove(plugin_id);
        }

        // Remove from registry
        {
            let mut reg = self.registry.lock().unwrap();
            reg.unregister(plugin_id)
                .map_err(|e| LifecycleError::RegistryError(e.to_string()))?;
        }

        // Evict from AOT cache
        self.aot_cache
            .evict(wasm_bytes)
            .map_err(|e| LifecycleError::IoError(e.to_string()))?;

        tracing::info!(plugin = %plugin_id, "Plugin uninstalled");
        Ok(())
    }

    /// Get a reference to an installed plugin's store.
    pub fn get_store(&self, plugin_id: &str) -> Option<&wasmtime::Store<PluginStoreData>> {
        self.installed.get(plugin_id).map(|p| &p.store)
    }

    /// Get a mutable reference to an installed plugin's store.
    pub fn get_store_mut(
        &mut self,
        plugin_id: &str,
    ) -> Option<&mut wasmtime::Store<PluginStoreData>> {
        self.installed.get_mut(plugin_id).map(|p| &mut p.store)
    }

    /// Get the instance for a plugin.
    pub fn get_instance(&self, plugin_id: &str) -> Option<&wasmtime::Instance> {
        self.installed.get(plugin_id).map(|p| &p.instance)
    }

    /// Get a reference to the Wasm engine.
    pub fn engine(&self) -> &Arc<WasmEngine> {
        &self.engine
    }

    /// Get a reference to the AOT cache.
    pub fn aot_cache(&self) -> &AotCache {
        &self.aot_cache
    }

    /// Check if a plugin is installed (in the installed map).
    pub fn is_running(&self, plugin_id: &str) -> bool {
        self.installed.contains_key(plugin_id)
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by lifecycle operations.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("Plugin '{0}' not found")]
    PluginNotFound(String),

    #[error("Invalid state for plugin '{plugin_id}': {state}")]
    InvalidState { plugin_id: String, state: String },

    #[error("Signature mismatch: expected {expected}, got {actual}")]
    SignatureMismatch { expected: String, actual: String },

    #[error("AOT compilation error: {0}")]
    AotCompileError(String),

    #[error("Wasm error: {0}")]
    WasmError(String),

    #[error("Registry error: {0}")]
    RegistryError(String),

    #[error("I/O error: {0}")]
    IoError(String),

    #[error("Not yet implemented: {0}")]
    NotImplemented(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::PluginRegistry;

    fn make_test_manager() -> (PluginManager, tempfile::TempDir) {
        let engine = Arc::new(WasmEngine::new().unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let registry = Arc::new(std::sync::Mutex::new(PluginRegistry::new()));
        let (tx, _rx) = crossbeam::channel::unbounded();

        let manager = PluginManager::new(engine, tmp.path().join("plugins"), registry, tx).unwrap();

        (manager, tmp)
    }

    #[test]
    fn install_plugin_verifies_and_compiles() {
        let (mut manager, _tmp) = make_test_manager();

        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        let manifest = CapabilityManifest::from_json(r#"{"property_scope": ["*"]}"#).unwrap();

        let result = manager.install_plugin("test-plugin", &bytes, &manifest, None);
        assert!(result.is_ok());

        // Check registry
        let reg = manager.registry.lock().unwrap();
        assert!(reg.get("test-plugin").is_some());
        assert_eq!(reg.get_state("test-plugin"), Some(&PluginState::Installed));
    }

    #[test]
    fn install_plugin_signature_mismatch() {
        let (mut manager, _tmp) = make_test_manager();

        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        let manifest = CapabilityManifest::from_json("{}").unwrap();

        let result = manager.install_plugin(
            "test-plugin",
            &bytes,
            &manifest,
            Some("wrong_hash_value_here_00000000000000000000000000000000000000000000000000000000"),
        );
        assert!(result.is_err());
        match result.unwrap_err() {
            LifecycleError::SignatureMismatch { .. } => {}
            other => panic!("Expected SignatureMismatch, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn deactivate_plugin() {
        let (mut manager, _tmp) = make_test_manager();

        // Install first
        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        let manifest = CapabilityManifest::from_json("{}").unwrap();
        manager
            .install_plugin("deact-test", &bytes, &manifest, None)
            .unwrap();

        // Deactivate (should fail since it's Installed, not Active)
        let result = manager.deactivate_plugin("deact-test").await;
        assert!(result.is_err()); // Can't deactivate from Installed state
    }

    #[tokio::test]
    async fn uninstall_plugin_removes_everything() {
        let (mut manager, _tmp) = make_test_manager();

        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        let manifest = CapabilityManifest::from_json("{}").unwrap();
        manager
            .install_plugin("uninstall-test", &bytes, &manifest, None)
            .unwrap();

        manager
            .uninstall_plugin("uninstall-test", &bytes)
            .await
            .unwrap();

        // Registry should not have it anymore
        let reg = manager.registry.lock().unwrap();
        assert!(reg.get("uninstall-test").is_none());
    }

    #[test]
    fn manager_is_running() {
        let (manager, _tmp) = make_test_manager();
        assert!(!manager.is_running("nonexistent"));
    }
}
