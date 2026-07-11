//! Extension storage API — host-provided read/write over node properties + KV.
//!
//! The engine binary implements [`ExtensionStorageApi`]; the plugin host
//! (`fe-plugin`) routes Rhai/WASM calls through the injected trait object.
//! fe-sdk stays serde-only — see `fe-sdk/src/AGENTS.md` §storage.

use std::collections::HashMap;

use crate::property::PropertyValue;

/// Capability granting read access to node properties + per-extension KV.
pub const CAP_STORAGE_READ: &str = "storage.read";
/// Capability granting write access to node properties + per-extension KV.
pub const CAP_STORAGE_WRITE: &str = "storage.write";

/// Max byte length accepted for a node id at the host boundary.
pub const MAX_NODE_ID_LEN: usize = 128;
/// Max byte length accepted for a property / KV key at the host boundary.
pub const MAX_KEY_LEN: usize = 256;

/// Host-provided storage access for extensions (node properties + KV).
///
/// Object-safe so `fe-plugin` can hold an `Arc<dyn ExtensionStorageApi>`.
/// `namespace` on the KV methods is filled by the host with the extension's
/// own id so extensions can never read each other's keys.
pub trait ExtensionStorageApi: Send + Sync {
    /// Read all custom properties of a node.
    fn node_get_properties(
        &self,
        node_id: &str,
    ) -> Result<HashMap<String, PropertyValue>, StorageError>;

    /// Set a single custom property on a node.
    fn node_set_property(
        &self,
        node_id: &str,
        key: &str,
        value: PropertyValue,
    ) -> Result<(), StorageError>;

    /// Read a per-extension namespaced KV value.
    fn storage_get(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<PropertyValue>, StorageError>;

    /// Write a per-extension namespaced KV value.
    fn storage_set(
        &self,
        namespace: &str,
        key: &str,
        value: PropertyValue,
    ) -> Result<(), StorageError>;
}

/// Validate a node id at the host boundary. Fails closed on empty/oversized ids.
pub fn validate_node_id(node_id: &str) -> Result<(), StorageError> {
    if node_id.is_empty() {
        return Err(StorageError::InvalidInput("node_id must not be empty".into()));
    }
    if node_id.len() > MAX_NODE_ID_LEN {
        return Err(StorageError::InvalidInput(format!(
            "node_id exceeds {MAX_NODE_ID_LEN} bytes"
        )));
    }
    Ok(())
}

/// Validate a property / KV key at the host boundary.
pub fn validate_key(key: &str) -> Result<(), StorageError> {
    if key.is_empty() {
        return Err(StorageError::InvalidInput("key must not be empty".into()));
    }
    if key.len() > MAX_KEY_LEN {
        return Err(StorageError::InvalidInput(format!(
            "key exceeds {MAX_KEY_LEN} bytes"
        )));
    }
    Ok(())
}

/// Errors surfaced by the storage API. Always propagated to the caller —
/// never swallowed — so the script sees a typed failure.
#[derive(Debug, Clone, PartialEq)]
pub enum StorageError {
    /// The extension lacks the required capability grant.
    CapabilityDenied(String),
    /// An argument failed host-boundary validation.
    InvalidInput(String),
    /// No implementation was injected by the host binary.
    NotAvailable,
    /// The underlying store failed.
    Backend(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::CapabilityDenied(c) => write!(f, "capability '{c}' not granted"),
            StorageError::InvalidInput(m) => write!(f, "invalid input: {m}"),
            StorageError::NotAvailable => write!(f, "storage API not available"),
            StorageError::Backend(m) => write!(f, "storage backend error: {m}"),
        }
    }
}

impl std::error::Error for StorageError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_node_id_rejects_empty_and_oversized() {
        assert!(validate_node_id("").is_err());
        assert!(validate_node_id("node_01").is_ok());
        assert!(validate_node_id(&"x".repeat(MAX_NODE_ID_LEN + 1)).is_err());
    }

    #[test]
    fn validate_key_rejects_empty_and_oversized() {
        assert!(validate_key("").is_err());
        assert!(validate_key("color").is_ok());
        assert!(validate_key(&"k".repeat(MAX_KEY_LEN + 1)).is_err());
    }

    #[test]
    fn storage_error_display() {
        assert!(StorageError::CapabilityDenied("storage.read".into())
            .to_string()
            .contains("storage.read"));
        assert_eq!(
            StorageError::NotAvailable.to_string(),
            "storage API not available"
        );
    }
}
