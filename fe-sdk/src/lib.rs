//! fe-sdk — Stable extension SDK for FractalEngine plugins.
//!
//! The public API surface extension authors program against: serde-light deps,
//! decoupled from engine internals (module map: src/AGENTS.md §modules).
//!
//! Boundary: guest/manifest modules (`manifest_v2`, `node`, `property`, `scene`,
//! `transaction`, `query`, `storage`, `ui`, `units`) live separately from
//! host-wiring modules (`plugin_host`, `legacy_identifier`), whose internals are
//! not re-exported at the crate root.

#![warn(missing_docs)]

pub mod api;
pub mod context;
pub mod device_format;
pub mod events;
/// Host-only canonical identifier mapping for legacy database records; use via `fe_sdk::legacy_identifier::*`.
pub mod legacy_identifier;
pub mod manifest_v2;
pub mod node;
pub mod path_asset;
pub mod path_geometry;
/// Host-only runtime traits (`NodeReader`, `TransactionSink`, `PluginLease`); use via `fe_sdk::plugin_host::*`.
pub mod plugin_host;
pub mod primitive;
pub mod property;
pub mod property_schema;
pub mod query;
pub mod render_environment;
pub mod render_style;
pub mod scene;
pub mod source_class;
pub mod storage;
pub mod texture;
pub mod transaction;
pub mod ui;
pub mod units;

// Guest-facing convenience re-exports (host-internal `plugin_host` and `legacy_identifier` excluded).
pub use api::{ApiExtensionHandle, ExtensionRoute, HttpMethod};
pub use context::PluginContext;
pub use device_format::DeviceFormatDescriptor;
pub use events::{EventSubscription, PluginEvent};
pub use node::NodeSnapshot;
pub use path_asset::{PathAssetDescriptor, SpacingMode, PATH_ASSET_PROPERTY_KEY};
pub use primitive::{PrimitiveDescriptor, PrimitiveKind, PRIMITIVE_PROPERTY_KEY};
pub use render_environment::{RenderEnvironmentDescriptor, RENDER_ENVIRONMENT_PROPERTY_KEY};
pub use render_style::{RenderAlphaMode, RenderStyleDescriptor, RENDER_STYLE_PROPERTY_KEY};
pub use property::{PropertyBag, PropertyValue};
pub use query::{is_select_only, ExtensionQueryApi, QueryError, CAP_QUERY_SELECT};
pub use scene::{SceneChange, SceneChangeBatch};
pub use storage::{ExtensionStorageApi, StorageError, CAP_STORAGE_READ, CAP_STORAGE_WRITE};
pub use texture::{TextureEntry, TextureRegistry};
pub use transaction::PluginTransaction;
pub use ui::{UiContribution, UiExtensionRegistry, UiSlot, UiWhen};

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
