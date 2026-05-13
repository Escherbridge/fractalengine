//! Inter-plugin event system for the extension SDK.
//!
//! Plugins can publish and subscribe to named events using [`PluginEvent`].
//! The engine routes events between plugins based on [`EventSubscription`]s.

use serde::{Deserialize, Serialize};

/// An event that can be sent between plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginEvent {
    /// A custom event with arbitrary JSON payload.
    Custom {
        /// The plugin that emitted this event.
        source: String,
        /// Event name (used for subscription matching).
        name: String,
        /// Arbitrary event data.
        data: serde_json::Value,
    },
}

/// A subscription binding a plugin to a named event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSubscription {
    /// The event name to subscribe to.
    pub event_name: String,
    /// The plugin that is subscribed.
    pub plugin_id: String,
}
