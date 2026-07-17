//! fe-plugin — plugin host: [`PluginHostPlugin`] spawns the plugin runtime thread (thread 7).
//! Thread/channel architecture: src/AGENTS.md §architecture.

pub mod capability;
pub mod context;
pub mod host_env;
pub mod lifecycle;
pub mod registry;
pub mod rhai;
pub mod transaction;
pub mod wasm;

use bevy::prelude::*;
use crossbeam::channel::{Receiver, Sender};
use serde::{Deserialize, Serialize};

use crate::context::PluginContext;
use crate::registry::PluginRegistry;
use crate::transaction::PendingOp;

// ---------------------------------------------------------------------------
// Unified SDK types — fe-sdk is the single source of truth for the stable,
// serde-only data types the extension API is expressed in. fe-plugin no longer
// maintains parallel definitions; see fe-plugin/src/AGENTS.md §type-unification.
// ---------------------------------------------------------------------------

pub use crate::host_env::{HostApiError, HostEnv};
pub use fe_sdk::node::NodeSnapshot;
pub use fe_sdk::property::{PropertyBag, PropertyValue};
pub use fe_sdk::query::{ExtensionQueryApi, QueryError};
pub use fe_sdk::scene::{SceneChange, SceneChangeBatch};
pub use fe_sdk::storage::{ExtensionStorageApi, StorageError};

// ---------------------------------------------------------------------------
// Channel message types
// ---------------------------------------------------------------------------

/// Commands sent from plugins to the engine host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginCommand {
    /// Commit a batch of operations from a plugin transaction.
    CommitTransaction {
        plugin_id: String,
        ops: Vec<PendingOp>,
    },
    /// Request a node snapshot (placeholder for request-reply in Phase 9A.2).
    GetNode { plugin_id: String, node_id: String },
    /// Set a property on a node.
    SetProperty {
        plugin_id: String,
        node_id: String,
        key: String,
        value: serde_json::Value,
    },
    /// Query nodes in a petal.
    QueryNodes { plugin_id: String, petal_id: String },
    /// Create a new node.
    CreateNode {
        plugin_id: String,
        petal_id: String,
        name: String,
        provisional_id: String,
    },
    /// Log a message from a plugin.
    Log {
        plugin_id: String,
        level: LogLevel,
        message: String,
    },
}

/// Log level for plugin log messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// Results sent from the engine host back to plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginResult {
    /// Transaction was committed successfully.
    TransactionCommitted { plugin_id: String, op_count: usize },
    /// A requested node snapshot.
    NodeSnapshot {
        plugin_id: String,
        node_id: String,
        data: serde_json::Value,
    },
    /// An error occurred processing a plugin command.
    Error { plugin_id: String, message: String },
}

// ---------------------------------------------------------------------------
// FractalExtension trait
// ---------------------------------------------------------------------------

/// The core extension trait that all plugins implement.
///
/// Native plugins implement this directly. Rhai plugins are wrapped by the
/// host with generated bridge code that maps each method to script callbacks.
///
/// All methods receive a `&mut PluginContext` that provides scoped access to
/// the engine (transactions, logging, etc.).
pub trait FractalExtension: Send + Sync {
    /// Unique identifier for this extension (typically a ULID or URI).
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn name(&self) -> &str;

    /// The execution tier of this extension.
    fn tier(&self) -> crate::registry::PluginTier;

    /// Called when the extension is first installed.
    fn on_install(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Called when the extension is activated (transitions to Active state).
    fn on_activate(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Called when the extension is deactivated.
    fn on_deactivate(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Called when the extension is uninstalled (permanent removal).
    fn on_uninstall(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let _ = ctx;
        Ok(())
    }

    /// Called when scene changes are batched and distributed to plugins.
    ///
    /// Returns a list of reactive scene changes the plugin wants to apply.
    fn on_scene_change(
        &mut self,
        ctx: &mut PluginContext,
        batch: &[SceneChange],
    ) -> Result<Vec<SceneChange>, PluginError> {
        let _ = (ctx, batch);
        Ok(vec![])
    }

    /// Called every tick with the elapsed time in milliseconds.
    fn on_tick(&mut self, ctx: &mut PluginContext, delta_ms: u64) -> Result<(), PluginError> {
        let _ = (ctx, delta_ms);
        Ok(())
    }
}

/// Errors returned by plugin extension methods.
#[derive(Debug, Clone, thiserror::Error)]
pub enum PluginError {
    #[error("Plugin execution error: {0}")]
    Execution(String),
    #[error("Plugin capability denied: {0}")]
    CapabilityDenied(String),
    #[error("Plugin script error: {0}")]
    Script(String),
    #[error("Plugin channel error: {0}")]
    Channel(String),
}

// ---------------------------------------------------------------------------
// Bevy resources for channel handles
// ---------------------------------------------------------------------------

/// Bevy resource: sender end of the plugin command channel.
#[derive(Resource, Clone)]
pub struct PluginCommandSender(pub Sender<PluginCommand>);

/// Bevy resource: receiver end of the plugin result channel (wrapped for Bevy Send+Sync).
#[derive(Resource)]
pub struct PluginResultReceiver(pub std::sync::Arc<std::sync::Mutex<Receiver<PluginResult>>>);

// ---------------------------------------------------------------------------
// Bevy plugin
// ---------------------------------------------------------------------------

/// Bevy plugin that initializes the plugin host infrastructure.
///
/// Spawns a dedicated thread (thread 7) with its own Tokio runtime for
/// running plugin logic. Creates crossbeam channels for bidirectional
/// communication and registers the [`PluginRegistry`] as a Bevy resource.
pub struct PluginHostPlugin;

impl Plugin for PluginHostPlugin {
    fn build(&self, app: &mut App) {
        // Create crossbeam channels for plugin <-> engine comms
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<PluginCommand>(256);
        let (result_tx, result_rx) = crossbeam::channel::bounded::<PluginResult>(256);

        // Spawn the plugin runtime thread (thread 7)
        std::thread::Builder::new()
            .name("fe-plugin".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("fe-plugin-tokio")
                    .enable_all()
                    .build()
                    .expect("Failed to build plugin Tokio runtime");

                rt.block_on(async move {
                    tracing::info!("Plugin host thread started");
                    plugin_thread_loop(cmd_rx, result_tx).await;
                    tracing::info!("Plugin host thread stopped");
                });
            })
            .expect("Failed to spawn plugin host thread");

        // Register resources
        app.insert_resource(PluginRegistry::new());
        app.insert_resource(PluginCommandSender(cmd_tx));
        app.insert_resource(PluginResultReceiver(std::sync::Arc::new(
            std::sync::Mutex::new(result_rx),
        )));

        // Add drain system
        app.add_systems(Update, drain_plugin_results);
    }
}

/// The plugin thread's main loop. Receives commands from Bevy and dispatches
/// them, sending results back.
async fn plugin_thread_loop(cmd_rx: Receiver<PluginCommand>, result_tx: Sender<PluginResult>) {
    loop {
        match cmd_rx.recv() {
            Ok(cmd) => {
                handle_plugin_command(cmd, &result_tx);
            }
            Err(_) => {
                // Channel closed — Bevy is shutting down
                tracing::info!("Plugin command channel closed, shutting down");
                break;
            }
        }
    }
}

/// Process a single plugin command and send the result.
fn handle_plugin_command(cmd: PluginCommand, result_tx: &Sender<PluginResult>) {
    match cmd {
        PluginCommand::CommitTransaction { plugin_id, ops } => {
            let op_count = ops.len();
            tracing::debug!(
                plugin = %plugin_id,
                ops = op_count,
                "Processing plugin transaction"
            );
            // In Phase 9A.1 we log the operations. Phase 9A.2 will forward
            // them to the DB thread via DbCommand variants.
            for op in &ops {
                tracing::debug!(plugin = %plugin_id, op = ?op, "Pending op");
            }
            result_tx
                .send(PluginResult::TransactionCommitted {
                    plugin_id,
                    op_count,
                })
                .ok();
        }
        PluginCommand::GetNode { plugin_id, node_id } => {
            tracing::debug!(plugin = %plugin_id, node = %node_id, "GetNode request");
            // Placeholder: return empty data. Phase 9A.2 wires to EntityStore.
            result_tx
                .send(PluginResult::NodeSnapshot {
                    plugin_id,
                    node_id,
                    data: serde_json::json!({}),
                })
                .ok();
        }
        PluginCommand::SetProperty {
            plugin_id,
            node_id,
            key,
            value,
        } => {
            tracing::debug!(
                plugin = %plugin_id,
                node = %node_id,
                key = %key,
                "SetProperty from plugin"
            );
            let _ = value;
            result_tx
                .send(PluginResult::TransactionCommitted {
                    plugin_id,
                    op_count: 1,
                })
                .ok();
        }
        PluginCommand::QueryNodes {
            plugin_id,
            petal_id,
        } => {
            tracing::debug!(plugin = %plugin_id, petal = %petal_id, "QueryNodes");
            result_tx
                .send(PluginResult::NodeSnapshot {
                    plugin_id,
                    node_id: String::new(),
                    data: serde_json::json!([]),
                })
                .ok();
        }
        PluginCommand::CreateNode {
            plugin_id,
            petal_id,
            name,
            provisional_id,
        } => {
            tracing::debug!(
                plugin = %plugin_id,
                petal = %petal_id,
                name = %name,
                id = %provisional_id,
                "CreateNode from plugin"
            );
            result_tx
                .send(PluginResult::TransactionCommitted {
                    plugin_id,
                    op_count: 1,
                })
                .ok();
        }
        PluginCommand::Log {
            plugin_id,
            level,
            message,
        } => match level {
            LogLevel::Debug => tracing::debug!(plugin = %plugin_id, "{}", message),
            LogLevel::Info => tracing::info!(plugin = %plugin_id, "{}", message),
            LogLevel::Warn => tracing::warn!(plugin = %plugin_id, "{}", message),
            LogLevel::Error => tracing::error!(plugin = %plugin_id, "{}", message),
        },
    }
}

/// Bevy system: drain plugin results from the crossbeam channel each frame.
///
/// Processes up to 64 results per frame to avoid stalling the main loop.
fn drain_plugin_results(receiver: Res<PluginResultReceiver>, mut registry: ResMut<PluginRegistry>) {
    let rx = receiver.0.lock().unwrap();
    let mut count = 0;
    while let Ok(result) = rx.try_recv() {
        match &result {
            PluginResult::TransactionCommitted {
                plugin_id,
                op_count,
            } => {
                tracing::trace!(
                    plugin = %plugin_id,
                    ops = op_count,
                    "Plugin transaction committed"
                );
            }
            PluginResult::NodeSnapshot {
                plugin_id, node_id, ..
            } => {
                tracing::trace!(
                    plugin = %plugin_id,
                    node = %node_id,
                    "Node snapshot delivered"
                );
            }
            PluginResult::Error { plugin_id, message } => {
                tracing::warn!(
                    plugin = %plugin_id,
                    error = %message,
                    "Plugin error"
                );
                // Mark plugin as degraded on error
                if let Some(state) = registry.get_state(plugin_id) {
                    if matches!(state, crate::registry::PluginState::Active) {
                        // Transition to Degraded with the error message
                        if let Some(entry) = registry.plugins_mut().get_mut(plugin_id) {
                            entry.state = crate::registry::PluginState::Degraded(message.clone());
                        }
                    }
                }
            }
        }
        count += 1;
        if count >= 64 {
            break;
        }
    }
}
