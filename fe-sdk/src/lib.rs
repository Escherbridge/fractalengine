//! fe-sdk — Stable extension SDK for FractalEngine plugins.
//!
//! This crate defines the **public API surface** that extension authors program
//! against. It has minimal dependencies (serde + serde_json only) and is
//! intentionally decoupled from engine internals (Bevy, SurrealDB, etc.).
//!
//! # Modules
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`node`] | [`NodeSnapshot`] — read-only view of a node |
//! | [`property`] | [`PropertyValue`], [`PropertyBag`] — typed properties |
//! | [`scene`] | [`SceneChange`], [`SceneChangeBatch`] — scene mutations |
//! | [`transaction`] | [`PluginTransaction`] trait — batched writes |
//! | [`context`] | [`PluginContext`] trait — engine services |
//! | [`storage`] | [`ExtensionStorageApi`] — node-property + KV access |
//! | [`query`] | [`ExtensionQueryApi`], [`is_select_only`] — SELECT-only reads |
//! | [`ui`] | [`UiSlot`], [`UiContribution`], [`UiExtensionRegistry`] |
//! | [`events`] | [`PluginEvent`], [`EventSubscription`] — inter-plugin events |

pub mod api;
pub mod context;
pub mod events;
pub mod node;
pub mod property;
pub mod query;
pub mod scene;
pub mod storage;
pub mod transaction;
pub mod ui;

// Re-exports for convenience
pub use api::{ApiExtensionHandle, ExtensionRoute, HttpMethod};
pub use context::PluginContext;
pub use events::{EventSubscription, PluginEvent};
pub use node::NodeSnapshot;
pub use property::{PropertyBag, PropertyValue};
pub use query::{is_select_only, ExtensionQueryApi, QueryError, CAP_QUERY_SELECT};
pub use scene::{SceneChange, SceneChangeBatch};
pub use storage::{
    ExtensionStorageApi, StorageError, CAP_STORAGE_READ, CAP_STORAGE_WRITE,
};
pub use transaction::PluginTransaction;
pub use ui::{UiContribution, UiExtensionRegistry, UiSlot};

/// The SDK API version. Extension manifests declare a minimum compatible
/// version; the engine checks this at load time for forward-compatibility.
pub const SDK_API_VERSION: &str = "1.0.0";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_api_version_is_1_0_0() {
        assert_eq!(SDK_API_VERSION, "1.0.0");
    }
}
