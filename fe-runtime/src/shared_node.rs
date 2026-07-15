//! Shared Node Structure - The data structure bridging Tauri ↔ Bevy.
//!
//! This module provides the unified representation of scene nodes that flows between
//! Bevy (Rust) and the Tauri webview (JS).
//!
//! ## Key Types
//!
//! - [`SharedNode`] - The core node data structure with transform, URL, asset path, and properties
//! - [`PropertyValue`] - Flexible property storage (String, Number, Boolean, Array)
//! - [`WebViewInteraction`] - Events that flow between webview and Bevy

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ============================================================================
// Shared Node Structure
// ============================================================================

/// A unified representation of a scene node that bridges Tauri ↔ Bevy.
///
/// This structure is passed alongside events to bridge Tauri↔Bevy interactions:
/// - **Bevy → WebView**: When a node is selected in egui, the shared node is passed to the webview
/// - **WebView → Bevy**: When the user interacts with content in the webview, the shared node is passed back
///
/// # Fields
/// - `node_id`: Unique node identifier
/// - `verse_id`: Parent verse ID
/// - `fractal_id`: Parent fractal ID
/// - `petal_id`: Parent petal ID
/// - `position`: Transform position [x, y, z]
/// - `rotation`: Transform rotation quaternion [x, y, z, w]
/// - `scale`: Transform scale [x, y, z]
/// - `webpage_url`: Optional PetalPortal URL
/// - `asset_path`: Optional GLTF/GLB asset path
/// - `properties`: Custom node properties
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SharedNode {
    pub node_id: String,
    pub verse_id: String,
    pub fractal_id: String,
    pub petal_id: String,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub webpage_url: Option<String>,
    pub asset_path: Option<String>,
    pub properties: HashMap<String, PropertyValue>,
}

impl SharedNode {
    /// Create a new SharedNode with default transform values (identity).
    pub fn new(node_id: String, verse_id: String, fractal_id: String, petal_id: String) -> Self {
        Self {
            node_id,
            verse_id,
            fractal_id,
            petal_id,
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0], // Identity quaternion
            scale: [1.0, 1.0, 1.0],
            webpage_url: None,
            asset_path: None,
            properties: HashMap::new(),
        }
    }

    /// Validate that the node has valid transform data.
    pub fn validate_transform(&self) -> Result<(), String> {
        // Check position is finite
        for (i, &coord) in self.position.iter().enumerate() {
            if !coord.is_finite() {
                return Err(format!("Position[{}] is not finite: {}", i, coord));
            }
        }

        // Check rotation quaternion is normalized
        let rot_len = (self.rotation[0].powi(2)
            + self.rotation[1].powi(2)
            + self.rotation[2].powi(2)
            + self.rotation[3].powi(2))
        .sqrt();
        if (rot_len - 1.0).abs() > 0.001 {
            return Err(format!(
                "Rotation quaternion not normalized: length = {}",
                rot_len
            ));
        }

        // Check scale is positive
        for (i, &s) in self.scale.iter().enumerate() {
            if s <= 0.0 {
                return Err(format!("Scale[{}] is not positive: {}", i, s));
            }
        }

        Ok(())
    }
}

// ============================================================================
// Property Value
// ============================================================================

/// Flexible property storage for node properties.
///
/// Uses `#[serde(untagged)]` to allow direct serialization/deserialization
/// of variant values without a tag field. Round-trip is lossless for JSON
/// objects; scalar/array `Json` payloads normalize to the corresponding
/// scalar/`Array` variant on deserialization (variant-order fallthrough of
/// untagged deser). Store geometry/primitive descriptors as JSON objects.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum PropertyValue {
    String(String),
    Number(f64),
    Boolean(bool),
    Array(Vec<PropertyValue>),
    /// Arbitrary JSON for complex/nested data (e.g. primitive descriptors, C5).
    Json(serde_json::Value),
}

impl PropertyValue {
    /// Get a string value if this is a String variant.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            PropertyValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get a number value if this is a Number variant.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            PropertyValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Get a boolean value if this is a Boolean variant.
    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            PropertyValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Get an array value if this is an Array variant.
    pub fn as_array(&self) -> Option<&Vec<PropertyValue>> {
        match self {
            PropertyValue::Array(arr) => Some(arr),
            _ => None,
        }
    }

    /// Get a JSON value if this is a Json variant.
    pub fn as_json(&self) -> Option<&serde_json::Value> {
        match self {
            PropertyValue::Json(v) => Some(v),
            _ => None,
        }
    }
}

// ============================================================================
// fe-sdk PropertyValue convertibility (C5)
// ============================================================================
//
// The `fe-runtime` PropertyValue (above) is the Tauri↔Bevy bridge's
// authoritative shape; `fe_sdk::property::PropertyValue` is the
// extension-facing mirror. See `fe-runtime/src/AGENTS.md` §property-bridge
// for the rationale — keep these two convertible, never fork geometry
// semantics across them.

impl From<fe_sdk::property::PropertyValue> for PropertyValue {
    fn from(v: fe_sdk::property::PropertyValue) -> Self {
        match v {
            fe_sdk::property::PropertyValue::String(s) => PropertyValue::String(s),
            fe_sdk::property::PropertyValue::Number(n) => PropertyValue::Number(n),
            fe_sdk::property::PropertyValue::Bool(b) => PropertyValue::Boolean(b),
            fe_sdk::property::PropertyValue::Json(j) => PropertyValue::Json(j),
        }
    }
}

impl From<PropertyValue> for fe_sdk::property::PropertyValue {
    fn from(v: PropertyValue) -> Self {
        match v {
            PropertyValue::String(s) => fe_sdk::property::PropertyValue::String(s),
            PropertyValue::Number(n) => fe_sdk::property::PropertyValue::Number(n),
            PropertyValue::Boolean(b) => fe_sdk::property::PropertyValue::Bool(b),
            // The SDK mirror has no Array variant; fold it into Json losslessly.
            PropertyValue::Array(arr) => {
                let json = serde_json::to_value(arr).unwrap_or(serde_json::Value::Null);
                fe_sdk::property::PropertyValue::Json(json)
            }
            PropertyValue::Json(j) => fe_sdk::property::PropertyValue::Json(j),
        }
    }
}

// ============================================================================
// Web View Interaction
// ============================================================================

/// Events that flow between webview and Bevy.
///
/// These interactions are bidirectional:
/// - **egui → WebView**: Selection triggers navigate command with node data
/// - **WebView → egui**: WebView interaction emits event caught by command handler
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WebViewInteraction {
    /// A node was selected in the webview.
    NodeSelected { node: SharedNode },
    /// A node was deselected in the webview.
    NodeDeselected { node_id: String },
    /// A node's transform changed (position, rotation, or scale).
    TransformChanged {
        node_id: String,
        position: [f32; 3],
        rotation: [f32; 4],
        scale: [f32; 3],
    },
    /// A node's property changed.
    PropertyChanged {
        node_id: String,
        key: String,
        value: PropertyValue,
    },
    /// A node's URL changed.
    UrlChanged { node_id: String, url: String },
}

// ============================================================================
// Asset Path Validation
// ============================================================================

/// Validate an asset path for security (path traversal protection).
///
/// # Arguments
/// * `petal_id` - The petal ID (used as base directory)
/// * `path` - The requested asset path
///
/// # Returns
/// * `Ok(validated_path)` - If the path is valid and safe
/// * `Err(error)` - If the path contains traversal attempts
pub fn validate_asset_path(petal_id: &str, path: &str) -> Result<String, String> {
    // Check for path traversal attempts
    if path.contains("..") {
        return Err("Path traversal blocked: '..' not allowed".to_string());
    }

    // Check for absolute paths
    if path.starts_with('/') {
        return Err("Path traversal blocked: absolute paths not allowed".to_string());
    }

    // Reject Windows drive-letter absolute paths (C:\... or C:/...)
    let bytes = path.as_bytes();
    if bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/') {
        return Err("Path traversal blocked: absolute Windows paths not allowed".to_string());
    }

    // Return the validated path with petal_id prefix
    Ok(format!("{}/{}", petal_id, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test SharedNode structure serialization
    #[test]
    fn test_shared_node_serialization() {
        let node = create_test_node();

        // Serialize to JSON
        let json = serde_json::to_string(&node).expect("Failed to serialize SharedNode");
        assert!(json.contains("node-123"));
        assert!(json.contains("verse-456"));

        // Deserialize back
        let deserialized: SharedNode = serde_json::from_str(&json).expect("Failed to deserialize");
        assert_eq!(deserialized.node_id, "node-123");
        assert_eq!(deserialized.verse_id, "verse-456");
    }

    // Test SharedNode contains all required fields
    #[test]
    fn test_shared_node_fields() {
        let node = create_test_node();

        // Required string fields
        assert!(!node.node_id.is_empty());
        assert!(!node.verse_id.is_empty());
        assert!(!node.petal_id.is_empty());

        // Transform fields - arrays of correct size
        assert_eq!(node.position.len(), 3); // [x, y, z]
        assert_eq!(node.rotation.len(), 4); // quaternion [x, y, z, w]
        assert_eq!(node.scale.len(), 3); // [x, y, z]
    }

    // Test SharedNode transform data
    #[test]
    fn test_shared_node_transform() {
        let node = create_test_node();

        // Position should be reasonable values
        assert!(node.position[0].is_finite());
        assert!(node.position[1].is_finite());
        assert!(node.position[2].is_finite());

        // Rotation quaternion should be normalized (length ≈ 1)
        let rot_len = (node.rotation[0].powi(2)
            + node.rotation[1].powi(2)
            + node.rotation[2].powi(2)
            + node.rotation[3].powi(2))
        .sqrt();
        assert!(
            (rot_len - 1.0).abs() < 0.001,
            "Quaternion should be normalized"
        );

        // Scale should be positive
        assert!(node.scale[0] > 0.0);
        assert!(node.scale[1] > 0.0);
        assert!(node.scale[2] > 0.0);
    }

    // Test optional fields (webpage_url, asset_path)
    #[test]
    fn test_shared_node_optional_fields() {
        // Node with URL
        let mut node_with_url = create_test_node();
        node_with_url.webpage_url = Some("https://example.com/3d-model".to_string());
        assert!(node_with_url.webpage_url.is_some());

        // Node without URL
        let mut node_without_url = create_test_node();
        node_without_url.webpage_url = None;
        assert!(node_without_url.webpage_url.is_none());
    }

    // Test SharedNode properties map
    #[test]
    fn test_shared_node_properties() {
        let mut node = create_test_node();

        // Add some properties
        let mut props = HashMap::new();
        props.insert(
            "color".to_string(),
            PropertyValue::String("#FF0000".to_string()),
        );
        props.insert("visible".to_string(), PropertyValue::Boolean(true));
        props.insert("intensity".to_string(), PropertyValue::Number(0.8));

        node.properties = props;

        // Serialize and deserialize
        let json = serde_json::to_string(&node).expect("Failed to serialize");
        let deserialized: SharedNode = serde_json::from_str(&json).expect("Failed to deserialize");

        assert_eq!(
            deserialized.properties.get("color"),
            Some(&PropertyValue::String("#FF0000".to_string()))
        );
        assert_eq!(
            deserialized.properties.get("visible"),
            Some(&PropertyValue::Boolean(true))
        );
        assert_eq!(
            deserialized.properties.get("intensity"),
            Some(&PropertyValue::Number(0.8))
        );
    }

    // Test PropertyValue enum variants
    #[test]
    fn test_property_value_variants() {
        // String variant
        let pv_string = PropertyValue::String("hello".to_string());
        assert!(matches!(pv_string, PropertyValue::String(s) if s == "hello"));

        // Number variant
        let pv_number = PropertyValue::Number(42.5);
        assert!(matches!(pv_number, PropertyValue::Number(n) if n == 42.5));

        // Boolean variant
        let pv_bool = PropertyValue::Boolean(true);
        assert!(matches!(pv_bool, PropertyValue::Boolean(b) if b));

        // Array variant
        let pv_array =
            PropertyValue::Array(vec![PropertyValue::Number(1.0), PropertyValue::Number(2.0)]);
        assert!(matches!(pv_array, PropertyValue::Array(arr) if arr.len() == 2));
    }

    // Test PropertyValue serialization
    #[test]
    fn test_property_value_serialization() {
        let values = vec![
            PropertyValue::String("test".to_string()),
            PropertyValue::Number(2.5),
            PropertyValue::Boolean(false),
            PropertyValue::Array(vec![PropertyValue::Number(1.0)]),
        ];

        for value in values {
            let json = serde_json::to_string(&value).expect("Failed to serialize PropertyValue");
            let deserialized: PropertyValue =
                serde_json::from_str(&json).expect("Failed to deserialize");
            assert_eq!(value, deserialized);
        }
    }

    // Test WebViewInteraction enum variants
    #[test]
    fn test_webview_interaction_variants() {
        // NodeSelected variant
        let interaction = WebViewInteraction::NodeSelected {
            node: create_test_node(),
        };
        assert!(matches!(
            interaction,
            WebViewInteraction::NodeSelected { .. }
        ));

        // NodeDeselected variant
        let interaction = WebViewInteraction::NodeDeselected {
            node_id: "node-123".to_string(),
        };
        assert!(matches!(
            interaction,
            WebViewInteraction::NodeDeselected { node_id } if node_id == "node-123"
        ));

        // TransformChanged variant
        let interaction = WebViewInteraction::TransformChanged {
            node_id: "node-123".to_string(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        };
        assert!(matches!(
            interaction,
            WebViewInteraction::TransformChanged { .. }
        ));

        // PropertyChanged variant
        let interaction = WebViewInteraction::PropertyChanged {
            node_id: "node-123".to_string(),
            key: "color".to_string(),
            value: PropertyValue::String("#FF0000".to_string()),
        };
        assert!(matches!(
            interaction,
            WebViewInteraction::PropertyChanged { .. }
        ));

        // UrlChanged variant
        let interaction = WebViewInteraction::UrlChanged {
            node_id: "node-123".to_string(),
            url: "https://example.com".to_string(),
        };
        assert!(matches!(interaction, WebViewInteraction::UrlChanged { .. }));
    }

    // Test WebViewInteraction serialization round-trip
    #[test]
    fn test_webview_interaction_serialization() {
        let interactions = vec![
            WebViewInteraction::NodeSelected {
                node: create_test_node(),
            },
            WebViewInteraction::NodeDeselected {
                node_id: "node-123".to_string(),
            },
            WebViewInteraction::TransformChanged {
                node_id: "node-123".to_string(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
            },
            WebViewInteraction::PropertyChanged {
                node_id: "node-123".to_string(),
                key: "color".to_string(),
                value: PropertyValue::String("#FF0000".to_string()),
            },
            WebViewInteraction::UrlChanged {
                node_id: "node-123".to_string(),
                url: "https://example.com".to_string(),
            },
        ];

        for interaction in interactions {
            let json = serde_json::to_string(&interaction).expect("Failed to serialize");
            let deserialized: WebViewInteraction =
                serde_json::from_str(&json).expect("Failed to deserialize");
            // Note: Due to enum tagging, we just verify it round-trips without error
            assert!(serde_json::to_string(&deserialized).is_ok());
        }
    }

    // Test path traversal protection in asset resolution
    #[test]
    fn test_asset_path_traversal_protection() {
        // These paths should be blocked (path traversal attempts)
        let malicious_paths = vec![
            ("petal-123", "../../../etc/passwd"),
            ("petal-123", "..\\..\\windows\\system32\\config"),
            ("petal-123", "/absolute/path/attempt"),
            ("petal-123", "assets/../../../secret.txt"),
        ];

        for (petal_id, path) in malicious_paths {
            let result = validate_asset_path(petal_id, path);
            assert!(
                result.is_err(),
                "Path traversal should be blocked: {}",
                path
            );
        }
    }

    // Test valid asset paths are allowed
    #[test]
    fn test_asset_path_validation_valid() {
        // These paths should be allowed
        let valid_paths = vec![
            ("petal-123", "assets/model.glb"),
            ("petal-123", "textures/image.png"),
            ("petal-123", "scripts/script.js"),
            ("petal-456", "nested/folder/file.txt"),
        ];

        for (petal_id, path) in valid_paths {
            let result = validate_asset_path(petal_id, path);
            assert!(result.is_ok(), "Valid path should be allowed: {}", path);
        }
    }

    /// Create a test SharedNode with reasonable default values
    fn create_test_node() -> SharedNode {
        SharedNode {
            node_id: "node-123".to_string(),
            verse_id: "verse-456".to_string(),
            fractal_id: "fractal-789".to_string(),
            petal_id: "petal-abc".to_string(),
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0], // Identity quaternion
            scale: [1.0, 1.0, 1.0],
            webpage_url: Some("https://example.com".to_string()),
            asset_path: Some("assets/model.glb".to_string()),
            properties: HashMap::new(),
        }
    }
}
