//! fe-sdk — Stable extension SDK for FractalEngine plugins.
//!
//! The public API surface extension authors program against: serde-only deps,
//! decoupled from engine internals (module map: src/AGENTS.md §modules).

#![warn(missing_docs)]

pub mod api;
pub mod context;
pub mod events;
pub mod node;
pub mod path_asset;
pub mod primitive;
pub mod property;
pub mod query;
pub mod scene;
pub mod storage;
pub mod texture;
pub mod transaction;
pub mod ui;
pub mod units;

// Re-exports for convenience
pub use api::{ApiExtensionHandle, ExtensionRoute, HttpMethod};
pub use context::PluginContext;
pub use events::{EventSubscription, PluginEvent};
pub use node::NodeSnapshot;
pub use path_asset::{PathAssetDescriptor, SpacingMode, PATH_ASSET_PROPERTY_KEY};
pub use primitive::{PrimitiveDescriptor, PrimitiveKind, PRIMITIVE_PROPERTY_KEY};
pub use property::{PropertyBag, PropertyValue};
pub use query::{is_select_only, ExtensionQueryApi, QueryError, CAP_QUERY_SELECT};
pub use scene::{SceneChange, SceneChangeBatch};
pub use storage::{ExtensionStorageApi, StorageError, CAP_STORAGE_READ, CAP_STORAGE_WRITE};
pub use texture::{TextureEntry, TextureRegistry};
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
