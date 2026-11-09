//! Wasm plugin lifecycle ops (flow: src/AGENTS.md §lifecycle).

use std::path::PathBuf;
use std::sync::Arc;

use crossbeam::channel::Sender;

use crate::capability::{CapabilityManifest, CapabilityToken};
use crate::package::{payload_hash, verify_package, PackageError, SignatureEntry, UnsignedPolicy};
use crate::trust_roots::{RevocationSet, TrustRoots};
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
    contexts: crate::scene_host::PluginContexts,
    trust_roots: TrustRoots,
    unsigned_policy: UnsignedPolicy,
    revocations: RevocationSet,
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
            contexts: crate::scene_host::PluginContexts::default(),
            trust_roots: TrustRoots::default(),
            unsigned_policy: UnsignedPolicy::default(),
            revocations: RevocationSet::default(),
        })
    }

    /// Configure publisher trust roots and the unsigned-package policy (default: no roots, refuse unsigned).
    pub fn with_trust(mut self, trust_roots: TrustRoots, unsigned_policy: UnsignedPolicy) -> Self {
        self.trust_roots = trust_roots;
        self.unsigned_policy = unsigned_policy;
        self
    }

    /// Replace the verified revocation set consulted at install and activation.
    pub fn set_revocations(&mut self, revocations: RevocationSet) {
        self.revocations = revocations;
    }

    /// Install a Wasm plugin: verify publisher signature, check revocation, AOT compile, register as `Installed`.
    pub fn install_plugin(
        &mut self,
        plugin_id: &str,
        wasm_bytes: &[u8],
        manifest_bytes: &[u8],
        signatures: &[SignatureEntry],
        _manifest: &CapabilityManifest,
    ) -> Result<(), LifecycleError> {
        // Step 1: ed25519 over manifest_bytes || blake3(payload) against the trust roots
        let actual_hash = payload_hash(wasm_bytes);
        let verified = verify_package(
            plugin_id,
            manifest_bytes,
            wasm_bytes,
            signatures,
            &self.trust_roots,
            self.unsigned_policy,
        )?;
        if self.revocations.is_revoked(plugin_id, &actual_hash) {
            return Err(PackageError::Revoked {
                package_id: plugin_id.to_string(),
                content_hash: actual_hash,
            }
            .into());
        }
        tracing::info!(plugin = %plugin_id, hash = %actual_hash, ?verified, "Plugin signature verified");

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

    /// Attach the same admitted instance registry installed by PluginHostPlugin.
    pub fn with_contexts(mut self, contexts: crate::scene_host::PluginContexts) -> Self {
        self.contexts = contexts;
        self
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
        {
            let registry = self
                .registry
                .lock()
                .map_err(|_| LifecycleError::RegistryError("registry lock poisoned".into()))?;
            let state = registry
                .get_state(plugin_id)
                .ok_or_else(|| LifecycleError::PluginNotFound(plugin_id.into()))?;
            if !matches!(
                state,
                PluginState::Installed | PluginState::Deactivated | PluginState::Degraded(_)
            ) {
                return Err(LifecycleError::InvalidState {
                    plugin_id: plugin_id.into(),
                    state: format!("{state:?}"),
                });
            }
        }
        let content_hash = payload_hash(wasm_bytes);
        if self.revocations.is_revoked(plugin_id, &content_hash) {
            return Err(PackageError::Revoked {
                package_id: plugin_id.to_string(),
                content_hash,
            }
            .into());
        }
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
        let ctx = self
            .contexts
            .bind(
                plugin_id.to_string(),
                petal_id.to_string(),
                token,
                self.cmd_tx.clone(),
            )
            .map_err(LifecycleError::HostError)?;

        // Instantiate
        let instantiated = self.engine.instantiate(&plugin, ctx, fuel_budget).await;
        let (mut store, instance) = match instantiated {
            Ok(instance) => instance,
            Err(error) => {
                self.contexts
                    .unbind(plugin_id)
                    .map_err(LifecycleError::HostError)?;
                return Err(LifecycleError::WasmError(error.to_string()));
            }
        };

        // Try to call on_activate export
        if instance.get_func(&mut store, "on_activate").is_some() {
            tracing::debug!(plugin = %plugin_id, "Calling on_activate");
            if let Err(error) = self
                .engine
                .call_export(&mut store, &instance, "on_activate", &[], &mut [])
                .await
            {
                self.contexts
                    .unbind(plugin_id)
                    .map_err(LifecycleError::HostError)?;
                return Err(LifecycleError::WasmError(error.to_string()));
            }
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
        self.contexts
            .unbind(plugin_id)
            .map_err(LifecycleError::HostError)?;

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

        self.contexts
            .unbind(plugin_id)
            .map_err(LifecycleError::HostError)?;
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

    #[error("Package verification failed: {0}")]
    Package(#[from] PackageError),

    #[error("AOT compilation error: {0}")]
    AotCompileError(String),

    #[error("Wasm error: {0}")]
    WasmError(String),

    #[error("Registry error: {0}")]
    RegistryError(String),

    #[error("I/O error: {0}")]
    IoError(String),

    #[error("Plugin host binding failed: {0}")]
    HostError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::PluginRegistry;

    const MANIFEST_BYTES: &[u8] = br#"{"id":"test-plugin"}"#;

    fn publisher() -> ed25519_dalek::SigningKey {
        ed25519_dalek::SigningKey::from_bytes(&[9u8; 32])
    }

    fn trust_roots_for(key: &ed25519_dalek::SigningKey) -> TrustRoots {
        TrustRoots::from_toml_str(&format!(
            "[[publisher]]\nkey_id = \"pub\"\npublic_key = \"{}\"\n",
            hex::encode(key.verifying_key().to_bytes())
        ))
        .unwrap()
    }

    fn sign(key: &ed25519_dalek::SigningKey, key_id: &str, bytes: &[u8]) -> SignatureEntry {
        use ed25519_dalek::Signer;
        let message = crate::package::signed_message(MANIFEST_BYTES, bytes);
        SignatureEntry {
            key_id: key_id.into(),
            signature: hex::encode(key.sign(&message).to_bytes()),
        }
    }

    fn make_test_manager() -> (PluginManager, tempfile::TempDir) {
        let engine = Arc::new(WasmEngine::new().unwrap());
        let tmp = tempfile::tempdir().unwrap();
        let registry = Arc::new(std::sync::Mutex::new(PluginRegistry::new()));
        let (tx, _rx) = crossbeam::channel::unbounded();

        let manager = PluginManager::new(engine, tmp.path().join("plugins"), registry, tx)
            .unwrap()
            .with_trust(trust_roots_for(&publisher()), UnsignedPolicy::Refuse);

        (manager, tmp)
    }

    /// Installs an unsigned module through the dev flag so lifecycle tests need no signing.
    fn install_dev(manager: &mut PluginManager, id: &str, bytes: &[u8]) {
        manager.unsigned_policy = UnsignedPolicy::AllowDev;
        let manifest = CapabilityManifest::from_json("{}").unwrap();
        manager
            .install_plugin(id, bytes, MANIFEST_BYTES, &[], &manifest)
            .unwrap();
    }

    #[test]
    fn install_plugin_signature_journey() {
        let (mut manager, _tmp) = make_test_manager();
        let bytes = wat::parse_str(r#"(module)"#).unwrap();
        let manifest = CapabilityManifest::from_json(r#"{"property_scope": ["*"]}"#).unwrap();

        // Unsigned is refused by default.
        let err = manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, &[], &manifest)
            .unwrap_err();
        assert!(matches!(err, LifecycleError::Package(PackageError::Unsigned(_))));

        // Unlisted key is refused with a typed error.
        let stranger = ed25519_dalek::SigningKey::from_bytes(&[1u8; 32]);
        let err = manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, &[sign(&stranger, "stranger", &bytes)], &manifest)
            .unwrap_err();
        assert!(matches!(err, LifecycleError::Package(PackageError::UntrustedKey { .. })));

        // A trusted key signing different payload bytes fails verification.
        let err = manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, &[sign(&publisher(), "pub", b"other")], &manifest)
            .unwrap_err();
        assert!(matches!(err, LifecycleError::Package(PackageError::InvalidSignature { .. })));

        // Counter-signature list: an untrusted entry beside a valid trusted one is accepted.
        let signatures = [sign(&stranger, "stranger", &bytes), sign(&publisher(), "pub", &bytes)];
        manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, &signatures, &manifest)
            .unwrap();
        let reg = manager.registry.lock().unwrap();
        assert_eq!(reg.get_state("test-plugin"), Some(&PluginState::Installed));
    }

    #[test]
    fn install_plugin_refuses_revoked_pair() {
        let (mut manager, _tmp) = make_test_manager();
        let bytes = wat::parse_str(r#"(module)"#).unwrap();
        let manifest = CapabilityManifest::from_json("{}").unwrap();
        manager.set_revocations(RevocationSet::from_entries(&[crate::trust_roots::RevocationEntry {
            package_id: "test-plugin".into(),
            content_hash: payload_hash(&bytes),
            reason: "withdrawn".into(),
            revoked_at: "2026-11-08T00:00:00Z".into(),
        }]));
        let err = manager
            .install_plugin("test-plugin", &bytes, MANIFEST_BYTES, &[sign(&publisher(), "pub", &bytes)], &manifest)
            .unwrap_err();
        assert!(matches!(err, LifecycleError::Package(PackageError::Revoked { .. })));
    }

    #[tokio::test]
    async fn deactivate_plugin() {
        let (mut manager, _tmp) = make_test_manager();

        // Install first
        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        install_dev(&mut manager, "deact-test", &bytes);

        // Deactivate (should fail since it's Installed, not Active)
        let result = manager.deactivate_plugin("deact-test").await;
        assert!(result.is_err()); // Can't deactivate from Installed state
    }

    #[tokio::test]
    async fn uninstall_plugin_removes_everything() {
        let (mut manager, _tmp) = make_test_manager();

        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();
        install_dev(&mut manager, "uninstall-test", &bytes);

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
