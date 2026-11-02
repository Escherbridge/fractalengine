//! Bounded property-schema validation; see src/AGENTS.md §property-schema.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum encoded schema or validated property size.
pub const MAX_PROPERTY_BYTES: usize = 65_536;
const MAX_DEPTH: usize = 16;
const MAX_SCHEMA_NODES: usize = 256;

/// The supported JSON value kinds in the property-schema vocabulary.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ValueKind {
    /// JSON object.
    Object,
    /// JSON array.
    Array,
    /// JSON string.
    String,
    /// Finite JSON number.
    Number,
    /// Integral JSON number.
    Integer,
    /// JSON boolean.
    Boolean,
    /// Explicit JSON null.
    Null,
}

/// Versioned, deliberately bounded property-schema descriptor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnforcedPropertySchema {
    /// Positive, consecutive definition revision, starting at one.
    pub version: u32,
    /// Validation rules for one outer property value.
    pub schema: PropertySchema,
}

/// A supported property rule; unknown keywords are errors, including `$ref`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertySchema {
    /// Expected value kind.
    #[serde(rename = "type")]
    pub kind: ValueKind,
    /// Permit explicit null as well as the declared kind.
    #[serde(default)]
    pub nullable: bool,
    /// Rules for named object members.
    #[serde(default)]
    pub properties: BTreeMap<String, PropertySchema>,
    /// Object members that must be present.
    #[serde(default)]
    pub required: Vec<String>,
    /// Permit object members not named in `properties`.
    #[serde(default = "yes", rename = "additionalProperties")]
    pub additional_properties: bool,
    /// A single rule for every array element.
    #[serde(default)]
    pub items: Option<Box<PropertySchema>>,
    /// Minimum array length.
    #[serde(default, rename = "minItems")]
    pub min_items: Option<usize>,
    /// Maximum array length.
    #[serde(default, rename = "maxItems")]
    pub max_items: Option<usize>,
    /// Minimum string length in Unicode scalar values.
    #[serde(default, rename = "minLength")]
    pub min_length: Option<usize>,
    /// Maximum string length in Unicode scalar values.
    #[serde(default, rename = "maxLength")]
    pub max_length: Option<usize>,
    /// Inclusive numeric minimum.
    #[serde(default)]
    pub minimum: Option<f64>,
    /// Inclusive numeric maximum.
    #[serde(default)]
    pub maximum: Option<f64>,
    /// Allowed values, compared with exact JSON types.
    #[serde(default, rename = "enum")]
    pub choices: Option<Vec<Value>>,
}

fn yes() -> bool {
    true
}

impl EnforcedPropertySchema {
    /// Parse and check a descriptor, rejecting unsupported or inconsistent rules.
    pub fn parse(value: &Value) -> Result<Self, String> {
        check_size(value)?;
        let descriptor: Self = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if descriptor.version == 0 {
            return Err("schema version must start at one".into());
        }
        let mut count = 0;
        descriptor.schema.check(0, &mut count)?;
        Ok(descriptor)
    }

    /// Validate without applying defaults, converting types, or changing the value.
    pub fn validate(&self, value: &Value) -> Result<(), String> {
        check_size(value)?;
        self.schema.validate_at(value, "$", 0)
    }
}

fn check_size(value: &Value) -> Result<(), String> {
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > MAX_PROPERTY_BYTES {
        return Err(format!(
            "property/schema exceeds {MAX_PROPERTY_BYTES} bytes"
        ));
    }
    Ok(())
}

impl PropertySchema {
    fn check(&self, depth: usize, count: &mut usize) -> Result<(), String> {
        *count += 1;
        if depth > MAX_DEPTH || *count > MAX_SCHEMA_NODES {
            return Err("schema exceeds depth/node limits (16/256)".into());
        }
        if self.kind != ValueKind::Object
            && (!self.properties.is_empty()
                || !self.required.is_empty()
                || !self.additional_properties)
        {
            return Err("object keywords require type object".into());
        }
        if self.kind != ValueKind::Array
            && (self.items.is_some() || self.min_items.is_some() || self.max_items.is_some())
        {
            return Err("array keywords require type array".into());
        }
        if self.kind != ValueKind::String
            && (self.min_length.is_some() || self.max_length.is_some())
        {
            return Err("string keywords require type string".into());
        }
        if !matches!(self.kind, ValueKind::Number | ValueKind::Integer)
            && (self.minimum.is_some() || self.maximum.is_some())
        {
            return Err("numeric keywords require type number or integer".into());
        }
        if self
            .min_items
            .zip(self.max_items)
            .is_some_and(|(a, b)| a > b)
            || self
                .min_length
                .zip(self.max_length)
                .is_some_and(|(a, b)| a > b)
            || self.minimum.zip(self.maximum).is_some_and(|(a, b)| a > b)
        {
            return Err("minimum constraint exceeds maximum".into());
        }
        let mut required = BTreeSet::new();
        for key in &self.required {
            if !required.insert(key) || !self.properties.contains_key(key) {
                return Err("required members must be unique and declared in properties".into());
            }
        }
        for child in self.properties.values() {
            child.check(depth + 1, count)?;
        }
        if let Some(child) = &self.items {
            child.check(depth + 1, count)?;
        }
        if let Some(choices) = &self.choices {
            if choices.is_empty() {
                return Err("enum must contain at least one value".into());
            }
            for choice in choices {
                self.validate_at(choice, "$enum", depth)?;
            }
        }
        Ok(())
    }

    fn validate_at(&self, value: &Value, path: &str, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(format!("{path}: value exceeds depth limit 16"));
        }
        let matches_kind = match self.kind {
            ValueKind::Object => value.is_object(),
            ValueKind::Array => value.is_array(),
            ValueKind::String => value.is_string(),
            ValueKind::Number => value.is_number(),
            ValueKind::Integer => value.as_f64().is_some_and(|n| n.fract() == 0.0),
            ValueKind::Boolean => value.is_boolean(),
            ValueKind::Null => value.is_null(),
        };
        if !matches_kind && !(value.is_null() && self.nullable) {
            return Err(format!("{path}: expected {:?}", self.kind));
        }
        if self
            .choices
            .as_ref()
            .is_some_and(|choices| !choices.contains(value))
        {
            return Err(format!("{path}: value is outside enum"));
        }
        if let Some(object) = value.as_object() {
            for key in &self.required {
                if !object.contains_key(key) {
                    return Err(format!("{path}: required member {key:?} is absent"));
                }
            }
            for (key, member) in object {
                if let Some(schema) = self.properties.get(key) {
                    schema.validate_at(member, &format!("{path}[{key:?}]"), depth + 1)?;
                } else if !self.additional_properties {
                    return Err(format!("{path}: unexpected member {key:?}"));
                }
            }
        }
        if let Some(array) = value.as_array() {
            check_range(array.len(), self.min_items, self.max_items, path)?;
            if let Some(schema) = &self.items {
                for (index, item) in array.iter().enumerate() {
                    schema.validate_at(item, &format!("{path}[{index}]"), depth + 1)?;
                }
            }
        }
        if let Some(string) = value.as_str() {
            check_range(
                string.chars().count(),
                self.min_length,
                self.max_length,
                path,
            )?;
        }
        if let Some(number) = value.as_f64() {
            if self.minimum.is_some_and(|min| number < min)
                || self.maximum.is_some_and(|max| number > max)
            {
                return Err(format!("{path}: number is outside bounds"));
            }
        }
        Ok(())
    }
}

fn check_range(
    value: usize,
    min: Option<usize>,
    max: Option<usize>,
    path: &str,
) -> Result<(), String> {
    if min.is_some_and(|min| value < min) || max.is_some_and(|max| value > max) {
        return Err(format!("{path}: length is outside bounds"));
    }
    Ok(())
}
