//! Bounded property-schema validation; see src/AGENTS.md §property-schema.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Maximum encoded schema or validated property size.
pub const MAX_PROPERTY_BYTES: usize = 65_536;
const MAX_DEPTH: usize = 16;
const MAX_SCHEMA_NODES: usize = 256;

/// Check the signed-integer storage boundary without changing JSON values.
pub fn validate_persistable_numbers(value: &Value) -> Result<(), String> {
    match value {
        Value::Number(number) if number.as_u64().is_some_and(|v| v > i64::MAX as u64) => {
            return Err("integer exceeds supported signed 64-bit storage range".into());
        }
        Value::Object(object) => {
            for child in object.values() {
                validate_persistable_numbers(child)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                validate_persistable_numbers(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

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
    pub minimum: Option<serde_json::Number>,
    /// Inclusive numeric maximum.
    #[serde(default)]
    pub maximum: Option<serde_json::Number>,
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
        reject_null_constraints(&value["schema"])?;
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
        check_value_depth(value, 0)?;
        self.schema.validate_at(value, "$", 0)
    }
}

fn reject_null_constraints(rule: &Value) -> Result<(), String> {
    for key in [
        "items",
        "minItems",
        "maxItems",
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "enum",
    ] {
        if rule.get(key).is_some_and(Value::is_null) {
            return Err(format!("schema constraint {key} cannot be null"));
        }
    }
    if let Some(properties) = rule.get("properties").and_then(Value::as_object) {
        for child in properties.values() {
            reject_null_constraints(child)?;
        }
    }
    if let Some(items) = rule.get("items") {
        reject_null_constraints(items)?;
    }
    Ok(())
}

fn compare_numbers(
    a: &serde_json::Number,
    b: &serde_json::Number,
) -> Result<std::cmp::Ordering, String> {
    use std::cmp::Ordering;
    if let (Some(a), Some(b)) = (a.as_i64(), b.as_i64()) {
        return Ok(a.cmp(&b));
    }
    if let (Some(a), Some(b)) = (a.as_u64(), b.as_u64()) {
        return Ok(a.cmp(&b));
    }
    if let (Some(a), Some(b)) = (a.as_i64(), b.as_u64()) {
        return Ok(if a < 0 {
            Ordering::Less
        } else {
            (a as u64).cmp(&b)
        });
    }
    if let (Some(a), Some(b)) = (a.as_u64(), b.as_i64()) {
        return Ok(if b < 0 {
            Ordering::Greater
        } else {
            a.cmp(&(b as u64))
        });
    }
    // Mixed integer/float comparisons refuse values outside exact f64 integer precision.
    for number in [a, b] {
        if (number.is_i64() || number.is_u64())
            && number
                .as_f64()
                .is_some_and(|n| n.abs() >= 9_007_199_254_740_992.0)
        {
            return Err(
                "mixed integer/float bound comparison exceeds exact numeric precision".into(),
            );
        }
    }
    a.as_f64()
        .and_then(|a| b.as_f64().and_then(|b| a.partial_cmp(&b)))
        .ok_or_else(|| "non-finite numeric bound".into())
}

fn check_value_depth(value: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("value exceeds depth limit 16".into());
    }
    match value {
        Value::Object(object) => {
            for child in object.values() {
                check_value_depth(child, depth + 1)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                check_value_depth(child, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
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
            || self
                .minimum
                .as_ref()
                .zip(self.maximum.as_ref())
                .map(|(a, b)| compare_numbers(a, b))
                .transpose()?
                .is_some_and(|order| order.is_gt())
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
                check_value_depth(choice, 0)?;
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
        if let Some(number) = value.as_number() {
            if self
                .minimum
                .as_ref()
                .map(|min| compare_numbers(number, min))
                .transpose()?
                .is_some_and(|order| order.is_lt())
                || self
                    .maximum
                    .as_ref()
                    .map(|max| compare_numbers(number, max))
                    .transpose()?
                    .is_some_and(|order| order.is_gt())
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
