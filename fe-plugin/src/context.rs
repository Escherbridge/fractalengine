//! Plugin execution context — the "world view" available to plugin code.
//!
//! Every callback on [`FractalExtension`](crate::FractalExtension) receives a
//! `&mut PluginContext` that provides scoped access to the engine through the
//! capability token and the command channel.

use crossbeam::channel::Sender;

use crate::capability::CapabilityToken;
use crate::host_env::HostEnv;
use crate::transaction::PluginTransaction;
use crate::PluginCommand;

/// Runtime context provided to plugin callbacks.
///
/// Wraps the crossbeam sender so plugins can issue commands back to the host
/// without holding a reference to the full engine. The optional [`HostEnv`]
/// carries the binary-injected storage/query trait objects (empty by default).
#[derive(Clone)]
pub struct PluginContext {
    /// The plugin's unique identifier.
    pub(crate) plugin_id: String,
    /// The petal scope this plugin instance is bound to.
    pub(crate) petal_id: String,
    /// The minted capability token controlling access.
    pub(crate) capabilities: CapabilityToken,
    /// Sender end of the plugin command channel.
    pub(crate) tx: Sender<PluginCommand>,
    /// Host-injected storage/query services (empty until the binary injects).
    pub(crate) host: HostEnv,
}

impl PluginContext {
    /// Create a new plugin context with no injected host services.
    pub fn new(
        plugin_id: String,
        petal_id: String,
        capabilities: CapabilityToken,
        tx: Sender<PluginCommand>,
    ) -> Self {
        Self {
            plugin_id,
            petal_id,
            capabilities,
            tx,
            host: HostEnv::new(),
        }
    }

    /// Attach a [`HostEnv`] carrying binary-injected storage/query services.
    pub fn with_host_env(mut self, host: HostEnv) -> Self {
        self.host = host;
        self
    }

    /// The host-injected storage/query services for this plugin.
    pub fn host(&self) -> &HostEnv {
        &self.host
    }

    /// Begin a new transaction for batching operations.
    pub fn transaction(&self) -> PluginTransaction {
        PluginTransaction::new(
            self.plugin_id.clone(),
            self.capabilities.clone(),
            self.tx.clone(),
        )
    }

    /// The petal this plugin instance is scoped to.
    pub fn petal_id(&self) -> &str {
        &self.petal_id
    }

    /// The plugin's unique identifier.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// The capability token controlling this plugin's access.
    pub fn capabilities(&self) -> &CapabilityToken {
        &self.capabilities
    }

    /// Send a raw command to the plugin host.
    pub fn send_command(&self, cmd: PluginCommand) -> Result<(), crossbeam::channel::SendError<PluginCommand>> {
        self.tx.send(cmd)
    }
}

impl std::fmt::Debug for PluginContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginContext")
            .field("plugin_id", &self.plugin_id)
            .field("petal_id", &self.petal_id)
            .field("capabilities", &self.capabilities)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}
