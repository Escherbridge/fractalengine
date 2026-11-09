//! fe-plugin — plugin host: [`PluginHostPlugin`] spawns the plugin runtime thread (thread 7).
//! Thread/channel architecture and the module map: src/AGENTS.md §architecture.

pub mod bindings;
pub mod bus;
pub mod capability;
pub mod context;
pub mod host_env;
pub mod lifecycle;
pub mod net;
pub mod package;
pub mod registry;
pub mod registry_client;
pub mod scene_host;
pub mod rhai;
pub mod tick;
pub mod transaction;
pub mod trust_roots;
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
pub use fe_sdk::plugin_host::{NodeReader, NodeQuery, TransactionSink, TransactionReceipt};
pub use fe_sdk::property::{PropertyBag, PropertyValue};
pub use fe_sdk::query::{ExtensionQueryApi, QueryError};
pub use fe_sdk::scene::{SceneChange, SceneChangeBatch};
pub use fe_sdk::storage::{ExtensionStorageApi, StorageError};

// ---------------------------------------------------------------------------
// Channel message types
// ---------------------------------------------------------------------------

/// Commands sent from plugins to the engine host.
#[derive(Debug, Clone)]
pub enum PluginCommand {
    /// A context-owned command envelope; scripts cannot select or mint its origin.
    Bound {session:scene_host::PluginSession,command:Box<PluginCommand>},
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
    QueryNodes { plugin_id: String, petal_id: String, filter:String },
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
    TransactionCommitted { plugin_id: String, op_count: usize, receipt:TransactionReceipt },
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

#[derive(Resource)]
pub struct PluginHostService(pub std::sync::Mutex<Option<fe_runtime::service_lifecycle::OwnedService>>);

// ---------------------------------------------------------------------------
// Bevy plugin
// ---------------------------------------------------------------------------

/// Runtime opt-in env var for package admission (deny-by-default, mirrors `FE_API_SCREENSHOT_ALLOW`; src/AGENTS.md §admission-gate).
pub const PLUGIN_HOST_ENV: &str = "FE_PLUGIN_HOST_ALLOW";

/// Whether an env value opts admission in: exactly `"1"`; unset, empty or garbage stay off.
pub fn plugin_host_allowed(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Read [`PLUGIN_HOST_ENV`] once at startup; a loud WARN fires when admission is switched on.
pub fn plugin_host_allowed_from_env() -> bool {
    let allowed = plugin_host_allowed(std::env::var(PLUGIN_HOST_ENV).ok().as_deref());
    if allowed {
        tracing::warn!("{PLUGIN_HOST_ENV}=1: plugin package admission ENABLED (WASM sandbox is un-audited; see SECURITY.md)");
    }
    allowed
}

/// Bevy resource: whether third-party packages may be admitted (binary feature + env, decided once at startup).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PackageAdmission(pub bool);

/// Bevy resource: the host's integration bus broker (cloneable; shared with fe-api and the bridge).
#[derive(Resource, Clone)]
pub struct BusHandle(pub bus::Bus);

/// Bevy plugin that initializes the plugin host infrastructure.
///
/// Spawns a dedicated thread (thread 7) with its own Tokio runtime for
/// running plugin logic. Creates crossbeam channels for bidirectional
/// communication and registers the [`PluginRegistry`] as a Bevy resource.
pub struct PluginHostPlugin;

impl Plugin for PluginHostPlugin {
    fn build(&self, app: &mut App) {
        let contexts=app.world().get_resource::<scene_host::PluginContexts>().cloned().unwrap_or_default();
        app.insert_resource(contexts.clone());
        // The lab pre-inserts a TickHost on its SimClock; every other process gets the wall clock.
        let ticks = app
            .world()
            .get_resource::<tick::TickHost>()
            .cloned()
            .unwrap_or_default()
            .with_contexts(contexts.clone());
        app.insert_resource(ticks.clone());
        // The integration bus (src/bus/AGENTS.md): E7 pre-inserts a BusHandle with real sources; otherwise fail-closed.
        let bus = app
            .world()
            .get_resource::<BusHandle>()
            .cloned()
            .unwrap_or_else(|| BusHandle(bus::Bus::new(ticks.clock(), bus::BusSources::fail_closed(), String::new())));
        app.insert_resource(bus.clone());
        let worker_bus = bus.0;
        // Create crossbeam channels for plugin <-> engine comms
        let (cmd_tx, cmd_rx) = crossbeam::channel::bounded::<PluginCommand>(256);
        let (result_tx, result_rx) = crossbeam::channel::bounded::<PluginResult>(256);

        // Spawn the plugin runtime thread (thread 7)
        let stop = fe_runtime::service_lifecycle::StopSignal::default();
        let worker_stop = stop.clone();
        let thread = std::thread::Builder::new()
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
                    plugin_thread_loop(cmd_rx, result_tx,contexts,worker_stop,ticks,worker_bus).await;
                    tracing::info!("Plugin host thread stopped");
                });
            })
            .expect("Failed to spawn plugin host thread");
        app.insert_resource(PluginHostService(std::sync::Mutex::new(Some(
            fe_runtime::service_lifecycle::OwnedService::new("plugin", stop, thread),
        ))));

        // Register resources
        app.insert_resource(PluginRegistry::new());
        app.insert_resource(PluginCommandSender(cmd_tx));
        app.insert_resource(PluginResultReceiver(std::sync::Arc::new(
            std::sync::Mutex::new(result_rx),
        )));

        // Add drain system
        app.add_systems(Update, (drain_plugin_results, apply_tick_events));
        app.add_systems(Last,shutdown_plugin_contexts);
    }
}

/// The plugin thread's main loop. Receives commands from Bevy and dispatches
/// them, sending results back.
async fn plugin_thread_loop(cmd_rx: Receiver<PluginCommand>, result_tx: Sender<PluginResult>,contexts:scene_host::PluginContexts,stop:fe_runtime::service_lifecycle::StopSignal,ticks:tick::TickHost,bus:bus::Bus) {
    loop {
        if contexts.is_closed() || stop.is_requested() {break;}
        // Block no longer than the earliest tick deadline (src/AGENTS.md §tick-scheduler).
        match cmd_rx.recv_timeout(ticks.next_wait(std::time::Duration::from_millis(100))) {
            Ok(cmd) => {
                handle_plugin_command(cmd, &result_tx,&contexts);
            }
            Err(crossbeam::channel::RecvTimeoutError::Timeout)=>{}
            Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
                // Channel closed — Bevy is shutting down
                tracing::info!("Plugin command channel closed, shutting down");
                break;
            }
        }
        // Bus turn first, then each seat's bus and network dispatch inside `ticks.poll` (src/AGENTS.md §bus-seat).
        bus.poll();
        ticks.poll();
    }
    // Every seat leaves the bus and the network broker (dial-outs stop) before its final flush (OD-E1).
    ticks.stop_all(tick::DeactivateReason::HostStopping);
}

/// Bevy system: apply scheduler events (degraded, stopped) to the registry; the ticks themselves never run in Bevy.
fn apply_tick_events(ticks: Res<tick::TickHost>, mut registry: ResMut<PluginRegistry>) {
    use tick::{DeactivateReason as Reason, TickHostEventKind as Kind};
    for event in ticks.take_events() {
        let id = event.plugin_id.as_str();
        let applied = match event.kind {
            Kind::Degraded(reason) => registry.degrade(id, &reason),
            Kind::Stopped(Reason::Revoked) => registry.revoke(id, "revoked"),
            Kind::Stopped(Reason::Disabled) => registry.disable(id, "disabled"),
            Kind::Stopped(Reason::PolicyDenied) => registry.disable(id, "policy-denied"),
            Kind::Stopped(Reason::RoleLost) => registry.disable(id, "role-lost"),
            Kind::Stopped(Reason::Degraded) => Ok(()),
            Kind::Stopped(_) => match registry.get_state(id) {
                Some(crate::registry::PluginState::Active | crate::registry::PluginState::Degraded(_)) => registry.deactivate(id),
                _ => Ok(()),
            },
        };
        if let Err(error) = applied {
            tracing::debug!(plugin = %id, %error, "tick event for an unregistered plugin");
        }
    }
}

fn shutdown_plugin_contexts(mut exit:MessageReader<bevy::app::AppExit>,contexts:Res<scene_host::PluginContexts>) {
    if exit.read().next().is_some() {
        if let Err(error)=contexts.shutdown() {tracing::warn!(%error,"plugin host shutdown failed");}
    }
}

/// Process a single plugin command and send the result.
fn handle_plugin_command(cmd: PluginCommand,result_tx:&Sender<PluginResult>,contexts:&scene_host::PluginContexts) {
    let resolved=match cmd {
        PluginCommand::Bound {session,command}=>contexts.resolve(&session).map(|context|(context,*command)),
        _=>Err("unbound plugin command refused".into()),
    };
    let result=match resolved {
        Ok((context,command))=>{
            let plugin_id=context.plugin_id().to_string();
            execute_plugin_command(&context,command).unwrap_or_else(|message|Some(PluginResult::Error {plugin_id,message}))
        }
        Err(message)=>Some(PluginResult::Error {plugin_id:"unbound".into(),message}),
    };
    if let Some(result)=result {
        let mut pending = result;
        while !contexts.is_closed() {
            match result_tx.send_timeout(pending, std::time::Duration::from_millis(100)) {
                Ok(()) => break,
                Err(crossbeam::channel::SendTimeoutError::Timeout(result)) => pending = result,
                Err(crossbeam::channel::SendTimeoutError::Disconnected(_)) => {
                    tracing::warn!("plugin result receiver closed");
                    break;
                }
            }
        }
    }
}

fn execute_plugin_command(context:&PluginContext,command:PluginCommand)->Result<Option<PluginResult>,String> {
    let claimed=match &command {
        PluginCommand::CommitTransaction {plugin_id,..}|PluginCommand::GetNode {plugin_id,..}
        |PluginCommand::SetProperty {plugin_id,..}|PluginCommand::QueryNodes {plugin_id,..}
        |PluginCommand::CreateNode {plugin_id,..}|PluginCommand::Log {plugin_id,..}=>plugin_id,
        PluginCommand::Bound {..}=>return Err("nested plugin origin envelope refused".into()),
    };
    if claimed!=context.plugin_id() {return Err("plugin command claims another instance".into());}
    let plugin_id=context.plugin_id().to_string();
    let receipt=match command {
        PluginCommand::CommitTransaction {ops,..}=>context.commit_ops(&ops)?,
        PluginCommand::SetProperty {node_id,key,value,..}=>context.commit_ops(&[PendingOp::SetProperty {node_id,key,value}])?,
        PluginCommand::CreateNode {petal_id,name,..}=>context.commit_ops(&[PendingOp::CreateNode {petal_id,name}])?,
        PluginCommand::GetNode {node_id,..}=>{
            let node=context.get_node(&node_id)?;
            return Ok(Some(PluginResult::NodeSnapshot {plugin_id,node_id,data:node.as_ref().map(fe_sdk::plugin_host::snapshot_json).unwrap_or(serde_json::Value::Null)}));
        }
        PluginCommand::QueryNodes {petal_id,filter,..}=>{
            let nodes=context.query_nodes(&petal_id,&NodeQuery::parse(&filter)?)?;
            return Ok(Some(PluginResult::NodeSnapshot {plugin_id,node_id:String::new(),data:serde_json::Value::Array(nodes.iter().map(fe_sdk::plugin_host::snapshot_json).collect())}));
        }
        PluginCommand::Log {level,message,..}=>{
            match level {
                LogLevel::Debug=>tracing::debug!(plugin=%plugin_id,"{}",message),
                LogLevel::Info=>tracing::info!(plugin=%plugin_id,"{}",message),
                LogLevel::Warn=>tracing::warn!(plugin=%plugin_id,"{}",message),
                LogLevel::Error=>tracing::error!(plugin=%plugin_id,"{}",message),
            }
            return Ok(None);
        }
        PluginCommand::Bound {..}=>unreachable!(),
    };
    Ok(Some(PluginResult::TransactionCommitted {plugin_id,op_count:receipt.op_count,receipt}))
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
                op_count, ..
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

#[cfg(test)]
mod host_gate_tests {
    use super::plugin_host_allowed;

    #[test]
    fn only_exact_one_opts_the_host_in() {
        for (value, expected) in [
            (None, false),
            (Some(""), false),
            (Some("0"), false),
            (Some("true"), false),
            (Some("yes"), false),
            (Some("1"), true),
        ] {
            assert_eq!(plugin_host_allowed(value), expected, "{value:?}");
        }
    }
}
