//! JSON value types and parsing for Inspector property forms.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropertyValueKind {
    String,
    Number,
    Boolean,
    Object,
    Array,
    Null,
}

impl PropertyValueKind {
    pub(crate) fn from_selection(selected: &str) -> Option<Self> {
        match selected {
            "string" | "datetime" | "geometry_point" | "geometry_polygon" | "blob_ref"
            | "hexon_ref" | "address" => Some(Self::String),
            "number" => Some(Self::Number),
            "bool" => Some(Self::Boolean),
            "object" => Some(Self::Object),
            "array" => Some(Self::Array),
            _ => None,
        }
    }

    pub(crate) fn of(value: &Value) -> Self {
        match value {
            Value::String(_) => Self::String,
            Value::Number(_) => Self::Number,
            Value::Bool(_) => Self::Boolean,
            Value::Object(_) => Self::Object,
            Value::Array(_) => Self::Array,
            Value::Null => Self::Null,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "bool",
            Self::Object => "object",
            Self::Array => "array",
            Self::Null => "null",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::String => "Text is saved exactly, including whitespace.",
            Self::Number => {
                "A finite JSON number, for example 12 or -1.5e2. Integers use signed 64-bit storage."
            }
            Self::Boolean => "Use true or false, in lowercase.",
            Self::Object => "A JSON object, for example {\"name\": \"value\"}.",
            Self::Array => "A JSON array, for example [1, {\"name\": \"value\"}].",
            Self::Null => "Use the JSON literal null.",
        }
    }

    pub(crate) fn parse(self, raw: &str) -> Result<Value, String> {
        if self == Self::String {
            return Ok(Value::String(raw.to_owned()));
        }
        if self == Self::Boolean {
            return match raw.trim() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(self.hint().to_owned()),
            };
        }
        let value: Value = serde_json::from_str(raw.trim()).map_err(|error| {
            format!(
                "{} Invalid JSON at line {}, column {}.",
                self.hint(),
                error.line(),
                error.column()
            )
        })?;
        let valid = match self {
            Self::Number => value
                .as_number()
                .is_some_and(|n| n.as_f64().is_some_and(f64::is_finite)),
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
            Self::Null => value.is_null(),
            Self::String | Self::Boolean => unreachable!(),
        };
        if valid {
            fe_sdk::property_schema::validate_persistable_numbers(&value)?;
            Ok(value)
        } else {
            Err(self.hint().to_owned())
        }
    }

    pub(crate) fn format(value: &Value) -> String {
        match value {
            Value::String(value) => value.clone(),
            Value::Object(_) | Value::Array(_) => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            }
            other => other.to_string(),
        }
    }
}
