//! Property value types for the extension SDK.
//!
//! [`PropertyValue`] is the SDK's type-safe representation of node properties.
//! [`PropertyBag`] wraps a `HashMap<String, PropertyValue>` with typed getters.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A typed property value that can be stored on a node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PropertyValue {
    /// A UTF-8 string value.
    String(String),
    /// A 64-bit floating-point number.
    Number(f64),
    /// A boolean value.
    Bool(bool),
    /// An arbitrary JSON value for complex/nested data.
    Json(serde_json::Value),
}

/// A bag of named properties with typed accessor methods.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PropertyBag {
    inner: HashMap<String, PropertyValue>,
}

impl PropertyBag {
    /// Create an empty property bag.
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Create a property bag from an existing map.
    pub fn from_map(map: HashMap<String, PropertyValue>) -> Self {
        Self { inner: map }
    }

    /// Insert a property value.
    pub fn insert(&mut self, key: impl Into<String>, value: PropertyValue) {
        self.inner.insert(key.into(), value);
    }

    /// Remove a property by key.
    pub fn remove(&mut self, key: &str) -> Option<PropertyValue> {
        self.inner.remove(key)
    }

    /// Get a raw property value by key.
    pub fn get(&self, key: &str) -> Option<&PropertyValue> {
        self.inner.get(key)
    }

    /// Get a string property, returning `None` if missing or wrong type.
    pub fn get_string(&self, key: &str) -> Option<&str> {
        match self.inner.get(key) {
            Some(PropertyValue::String(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Get a number property, returning `None` if missing or wrong type.
    pub fn get_number(&self, key: &str) -> Option<f64> {
        match self.inner.get(key) {
            Some(PropertyValue::Number(n)) => Some(*n),
            _ => None,
        }
    }

    /// Get a bool property, returning `None` if missing or wrong type.
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.inner.get(key) {
            Some(PropertyValue::Bool(b)) => Some(*b),
            _ => None,
        }
    }

    /// Number of properties in the bag.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the bag is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate over all properties.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PropertyValue)> {
        self.inner.iter()
    }

    /// Consume the bag into its inner map.
    pub fn into_inner(self) -> HashMap<String, PropertyValue> {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_value_serde_roundtrip() {
        let values = vec![
            PropertyValue::String("hello".into()),
            PropertyValue::Number(42.5),
            PropertyValue::Bool(true),
            PropertyValue::Json(serde_json::json!({"nested": [1, 2, 3]})),
        ];
        for val in &values {
            let json = serde_json::to_string(val).unwrap();
            let roundtripped: PropertyValue = serde_json::from_str(&json).unwrap();
            assert_eq!(val, &roundtripped);
        }
    }

    #[test]
    fn property_bag_typed_getters() {
        let mut bag = PropertyBag::new();
        bag.insert("name", PropertyValue::String("Alice".into()));
        bag.insert("score", PropertyValue::Number(99.5));
        bag.insert("active", PropertyValue::Bool(true));
        bag.insert("meta", PropertyValue::Json(serde_json::json!(null)));

        assert_eq!(bag.get_string("name"), Some("Alice"));
        assert_eq!(bag.get_number("score"), Some(99.5));
        assert_eq!(bag.get_bool("active"), Some(true));

        // Wrong type returns None
        assert_eq!(bag.get_string("score"), None);
        assert_eq!(bag.get_number("name"), None);
        assert_eq!(bag.get_bool("name"), None);

        // Missing key returns None
        assert_eq!(bag.get_string("missing"), None);

        assert_eq!(bag.len(), 4);
        assert!(!bag.is_empty());
    }
}
