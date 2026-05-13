//! Plugin context trait for the extension SDK.
//!
//! Extension authors receive a `&dyn PluginContext` to access engine services.
//! The trait is object-safe so the engine can provide its own concrete
//! implementation without leaking internal types.

use crate::transaction::PluginTransaction;

/// The execution context available to a plugin during callbacks.
///
/// Object-safe: the engine provides its own implementation that bridges
/// to internal channel/ECS infrastructure.
pub trait PluginContext: Send + Sync {
    /// The petal scope this plugin instance is bound to.
    fn petal_id(&self) -> &str;

    /// The plugin's unique identifier.
    fn plugin_id(&self) -> &str;

    /// Create a new transaction for batching scene mutations.
    fn transaction(&self) -> Box<dyn PluginTransaction>;
}
