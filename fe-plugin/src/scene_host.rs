//! Trusted instance binding for the scene bridge; see src/AGENTS.md §scene-host.

use crate::{bus::InstanceKey, capability::CapabilityToken, context::PluginContext, PluginCommand};
use bevy::prelude::Resource;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// An opaque host-minted origin, never deserialized from plugin arguments.
#[derive(Clone)]
pub struct PluginSession {
    id: String,
    lease: fe_sdk::plugin_host::PluginLease,
}

impl PluginSession {
    pub(crate) fn mint() -> Self {
        Self {
            id: ulid::Ulid::new().to_string(),
            lease: fe_sdk::plugin_host::PluginLease::default(),
        }
    }
    pub(crate) fn is_active(&self) -> bool {
        self.lease.require_active().is_ok()
    }
    pub(crate) fn lease(&self) -> fe_sdk::plugin_host::PluginLease {
        self.lease.clone()
    }
    fn revoke(&self) {
        self.lease.revoke();
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
    /// Built but not admitted: an update's new build runs `on-activate` here beside the running one.
    staged: Arc<Mutex<HashMap<String, PluginContext>>>,
    /// Session id -> the `(petal, package)` a 2.0 instance was staged for; one admitted context per key.
    /// Lock order: `contexts` (or `staged`) first, then `keys`.
    keys: Arc<Mutex<HashMap<String, InstanceKey>>>,
    factory: Option<Arc<dyn SceneBackendFactory>>,
    closed: Arc<AtomicBool>,
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
        let context = self.build(plugin_id, petal_id, capabilities, tx)?;
        let mut contexts = self
            .contexts
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?;
        if self.is_closed() {
            return Err("plugin host is shutting down".into());
        }
        if contexts
            .values()
            .any(|existing| existing.plugin_id() == context.plugin_id())
        {
            return Err("plugin instance is already bound".into());
        }
        contexts.insert(context.session.id.clone(), context.clone());
        Ok(context)
    }

    /// A host-minted context with the factory's backends; nothing is registered yet.
    fn build(
        &self,
        plugin_id: String,
        petal_id: String,
        capabilities: CapabilityToken,
        tx: crossbeam::channel::Sender<PluginCommand>,
    ) -> Result<PluginContext, String> {
        if self.is_closed() {
            return Err("plugin host is shutting down".into());
        }
        if capabilities.plugin_id != plugin_id {
            return Err("plugin token belongs to another instance".into());
        }
        let context = PluginContext::new(plugin_id, petal_id, capabilities, tx);
        match &self.factory {
            Some(factory) => factory.bind(context),
            None => Ok(context),
        }
    }

    /// Build a context with a live lease but leave it unadmitted (src/AGENTS.md §update): `admit` or `discard` it.
    pub fn stage(
        &self,
        plugin_id: String,
        petal_id: String,
        capabilities: CapabilityToken,
        tx: crossbeam::channel::Sender<PluginCommand>,
    ) -> Result<PluginContext, String> {
        let context = self.build(plugin_id, petal_id, capabilities, tx)?;
        let mut staged = self
            .staged
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?;
        if self.is_closed() {
            context.session.revoke();
            return Err("plugin host is shutting down".into());
        }
        staged.insert(context.session.id.clone(), context.clone());
        Ok(context)
    }

    /// [`stage`](Self::stage) for a 2.0 instance keyed `(petal, package)` (bindings.md §5.1); refused while
    /// another instance id serves that key (an update re-stages under the same id), and `admit` re-checks.
    pub fn stage_instance(
        &self,
        key: &InstanceKey,
        plugin_id: String,
        capabilities: CapabilityToken,
        tx: crossbeam::channel::Sender<PluginCommand>,
    ) -> Result<PluginContext, String> {
        if let Some(holder) = self.instance_at(key).filter(|holder| *holder != plugin_id) {
            return Err(format!("{holder} already runs {} at petal {}", key.package_id, key.petal_id));
        }
        let context = self.stage(plugin_id, key.petal_id.clone(), capabilities, tx)?;
        let recorded = self
            .keys
            .lock()
            .map(|mut keys| keys.insert(context.session.id.clone(), key.clone()));
        if recorded.is_err() {
            self.discard(&context);
            return Err("plugin context registry lock poisoned".into());
        }
        Ok(context)
    }

    /// The instance id whose admitted context serves `key`, if any.
    pub fn instance_at(&self, key: &InstanceKey) -> Option<String> {
        let contexts = self.contexts.lock().ok()?;
        let keys = self.keys.lock().ok()?;
        contexts
            .iter()
            .find(|(session, _)| keys.get(*session) == Some(key))
            .map(|(_, context)| context.plugin_id().to_string())
    }

    /// Admit a staged context; refused (and its lease revoked) while another context of the plugin, or of its
    /// `(petal, package)` key, is admitted.
    pub fn admit(&self, context: &PluginContext) -> Result<(), String> {
        let staged = self
            .staged
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?
            .remove(&context.session.id);
        let Some(staged) = staged else {
            return Err("plugin context was not staged on this host".into());
        };
        let Ok(mut contexts) = self.contexts.lock() else {
            staged.session.revoke();
            self.forget_keys(std::slice::from_ref(&staged.session.id));
            return Err("plugin context registry lock poisoned".into());
        };
        let refused = if self.is_closed() {
            Err("plugin host is shutting down".to_string())
        } else if contexts
            .values()
            .any(|existing| existing.plugin_id() == staged.plugin_id())
        {
            Err("plugin instance is already bound".to_string())
        } else {
            self.claim_key(&contexts, &staged)
        };
        if let Err(reason) = refused {
            staged.session.revoke();
            self.forget_keys(std::slice::from_ref(&staged.session.id));
            return Err(reason);
        }
        contexts.insert(staged.session.id.clone(), staged);
        Ok(())
    }

    /// Refuse `staged` while an admitted context serves its instance key (unkeyed contexts always pass).
    fn claim_key(&self, contexts: &HashMap<String, PluginContext>, staged: &PluginContext) -> Result<(), String> {
        let keys = self.keys.lock().map_err(|_| "plugin context registry lock poisoned".to_string())?;
        let Some(key) = keys.get(&staged.session.id) else { return Ok(()) };
        if contexts.keys().any(|session| keys.get(session) == Some(key)) {
            return Err(format!("an instance of {} already runs at petal {}", key.package_id, key.petal_id));
        }
        Ok(())
    }

    /// Drop the instance keys of sessions that left the registry.
    fn forget_keys(&self, sessions: &[String]) {
        if sessions.is_empty() {
            return;
        }
        if let Ok(mut keys) = self.keys.lock() {
            for session in sessions {
                keys.remove(session);
            }
        }
    }

    /// Revoke a staged context that will not run; an admitted context is never touched here.
    pub fn discard(&self, context: &PluginContext) {
        let removed = self
            .staged
            .lock()
            .ok()
            .and_then(|mut staged| staged.remove(&context.session.id));
        if let Some(staged) = removed {
            staged.session.revoke();
            self.forget_keys(std::slice::from_ref(&staged.session.id));
        }
    }

    /// Revoke queued command origins when the trusted host stops an instance.
    pub fn unbind(&self, plugin_id: &str) -> Result<(), String> {
        let mut removed = Vec::new();
        self.contexts
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?
            .retain(|session, context| {
                if context.plugin_id() == plugin_id {
                    context.session.revoke();
                    removed.push(session.clone());
                    false
                } else {
                    true
                }
            });
        self.forget_keys(&removed);
        Ok(())
    }

    pub(crate) fn resolve(&self, session: &PluginSession) -> Result<PluginContext, String> {
        if self.is_closed() {
            return Err("plugin host is shutting down".into());
        }
        self.contexts
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?
            .get(&session.id)
            .cloned()
            .ok_or_else(|| "plugin origin is not admitted".into())
    }

    /// Revoke all instances (admitted and staged) and stop accepting work when the application exits.
    pub fn shutdown(&self) -> Result<(), String> {
        self.closed.store(true, Ordering::Release);
        for registry in [&self.contexts, &self.staged] {
            let mut contexts = registry
                .lock()
                .map_err(|_| "plugin context registry lock poisoned")?;
            for context in contexts.values() {
                context.session.revoke();
            }
            contexts.clear();
        }
        self.keys
            .lock()
            .map_err(|_| "plugin context registry lock poisoned")?
            .clear();
        Ok(())
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}
