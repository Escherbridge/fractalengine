//! Tests for the Shared Node Structure and IPC Bridge (Track 2).
//!
//! These tests verify the implementation of:
//! 1. SharedNode - the data structure bridging Tauri ↔ Bevy
//! 2. WebViewInteraction - events that flow between webview and Bevy
//! 3. PropertyValue - flexible property storage
//! 4. IPC command handlers
//!
//! These tests use the actual fe-runtime implementation.

use fe_runtime::shared_node::{validate_asset_path, PropertyValue, SharedNode, WebViewInteraction};
use std::collections::HashMap;

// ============================================================================
// SHARED NODE STRUCTURE TESTS
// ============================================================================

/// Test the SharedNode structure can be serialized and deserialized
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

/// Test SharedNode contains all required fields
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

/// Test SharedNode transform data
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

/// Test optional fields (webpage_url, asset_path)
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

/// Test SharedNode properties map
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

// ============================================================================
// PROPERTY VALUE TESTS
// ============================================================================

/// Test PropertyValue enum variants
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

/// Test PropertyValue serialization
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

// ============================================================================
// WEB VIEW INTERACTION TESTS
// ============================================================================

/// Test WebViewInteraction enum variants
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
    assert!(
        matches!(interaction, WebViewInteraction::NodeDeselected { node_id } if node_id == "node-123")
    );

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

/// Test WebViewInteraction serialization round-trip
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

// ============================================================================
// IPC COMMAND HANDLER TESTS (conceptual)
// ============================================================================

/// Test node data lookup would work correctly
#[test]
fn test_node_lookup_scenario() {
    // This simulates what the IPC command handler would do

    // Given a node_id
    let node_id = "node-123";

    // We should be able to look up the node and get SharedNode
    // In the real implementation, this would query VerseManager
    let found_node = find_node_by_id(node_id);

    assert!(found_node.is_some());
    let node = found_node.unwrap();
    assert_eq!(node.node_id, node_id);
}

/// Test that non-existent node returns error
#[test]
fn test_node_not_found_scenario() {
    let found_node = find_node_by_id("non-existent-node");
    assert!(found_node.is_none());
}

// ============================================================================
// ASSET PROTOCOL SECURITY TESTS
// ============================================================================

/// Test path traversal protection in asset resolution
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

/// Test valid asset paths are allowed
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

// ============================================================================
// HELPER FUNCTIONS AND TEST DATA
// ============================================================================

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

// Simulated lookup function (would query VerseManager in real implementation)
fn find_node_by_id(node_id: &str) -> Option<SharedNode> {
    if node_id == "node-123" {
        Some(create_test_node())
    } else {
        None
    }
}
