//! Plugin execution context — the "world view" available to plugin code.
//!
//! Every callback on [`FractalExtension`](crate::FractalExtension) receives a
//! `&mut PluginContext` that provides scoped access to the engine through the
//! capability token and the command channel.

use crossbeam::channel::Sender;

use crate::capability::CapabilityToken;
use crate::transaction::PluginTransaction;
use crate::PluginCommand;

/// Runtime context provided to plugin callbacks.
///
/// Wraps the crossbeam sender so plugins can issue commands back to the host
/// without holding a reference to the full engine.
#[derive(Debug, Clone)]
pub struct PluginContext {
    /// The plugin's unique identifier.
    pub(crate) plugin_id: String,
    /// The petal scope this plugin instance is bound to.
    pub(crate) petal_id: String,
    /// The minted capability token controlling access.
    pub(crate) capabilities: CapabilityToken,
    /// Sender end of the plugin command channel.
    pub(crate) tx: Sender<PluginCommand>,
}

impl PluginContext {
    /// Create a new plugin context.
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
        }
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
