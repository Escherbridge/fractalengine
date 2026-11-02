//! Trusted instance binding for the scene bridge; see src/AGENTS.md §scene-host.

use crate::{capability::CapabilityToken, context::PluginContext, PluginCommand};
use bevy::prelude::Resource;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// An opaque host-minted origin, never deserialized from plugin arguments.
#[derive(Clone)]
pub struct PluginSession {
    id: String,
    active: Arc<AtomicBool>,
}

impl PluginSession {
    pub(crate) fn mint() -> Self {
        Self {
            id: ulid::Ulid::new().to_string(),
            active: Arc::new(AtomicBool::new(true)),
        }
    }
    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }
    fn revoke(&self) {
        self.active.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for PluginSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PluginSession(<opaque>)")
    }
}

/// Host-only construction of per-instance scope-bound backends.
pub trait SceneBackendFactory: Send + Sync {
    /// Inject services bound to the actual instance identity, token and petal.
    fn bind(&self, context: PluginContext) -> Result<PluginContext, String>;
}

/// Admitted runtime contexts, addressed by opaque origin rather than plugin labels.
#[derive(Clone, Default, Resource)]
pub struct PluginContexts {
    contexts: Arc<Mutex<HashMap<String, PluginContext>>>,
    factory: Option<Arc<dyn SceneBackendFactory>>,
}

impl PluginContexts {
    /// Configure the production factory before adding PluginHostPlugin.
    pub fn with_factory(mut self, factory: Arc<dyn SceneBackendFactory>) -> Self {
        self.factory = Some(factory);
        self
    }

    /// Bind an instance after the trusted host has admitted its manifest.
    pub fn bind(
        &self,
        plugin_id: String,
        petal_id: String,
        capabilities: CapabilityToken,
        tx: crossbeam::channel::Sender<PluginCommand>,
    ) -> Result<PluginContext, String> {
        if capabilities.plugin_id != plugin_id {
            return Err("plugin token belongs to another instance".into());
        }
        let mut context = PluginContext::new(plugin_id, petal_id, capabilities, tx);
        if let Some(factory) = &self.factory {
            context = factory.bind(context)?;
        }
        let mut contexts = self
            .contexts
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?;
        if contexts
            .values()
            .any(|existing| existing.plugin_id() == context.plugin_id())
        {
            return Err("plugin instance is already bound".into());
        }
        contexts.insert(context.session.id.clone(), context.clone());
        Ok(context)
    }

    /// Revoke queued command origins when the trusted host stops an instance.
    pub fn unbind(&self, plugin_id: &str) -> Result<(), String> {
        self.contexts
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?
            .retain(|_, context| {
                if context.plugin_id() == plugin_id {
                    context.session.revoke();
                    false
                } else {
                    true
                }
            });
        Ok(())
    }

    pub(crate) fn resolve(&self, session: &PluginSession) -> Result<PluginContext, String> {
        self.contexts
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?
            .get(&session.id)
            .cloned()
            .ok_or_else(|| "plugin origin is not admitted".into())
    }
}
