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

/// Parses a form value for any selectable type, validating semantic types (see panels/AGENTS.md property types).
pub(crate) fn parse_selected(selected: &str, raw: &str) -> Result<Value, String> {
    match selected {
        "datetime" => chrono::DateTime::parse_from_rfc3339(raw.trim())
            .map(|_| Value::String(raw.trim().to_owned()))
            .map_err(|_| {
                "datetime expects an RFC 3339 timestamp, for example 2026-11-13T09:30:00Z."
                    .to_owned()
            }),
        "address" => bounded_text("address", raw, 256, true),
        "blob_ref" => bounded_text("blob_ref", raw, 512, false),
        "hexon_ref" => bounded_text("hexon_ref", raw, 512, false),
        "geometry_point" => parse_geometry(selected, raw, GEOMETRY_POINT_HINT, is_point),
        "geometry_polygon" => parse_geometry(selected, raw, GEOMETRY_POLYGON_HINT, is_polygon),
        other => PropertyValueKind::from_selection(other)
            .ok_or_else(|| "Choose a supported property type.".to_owned())
            .and_then(|kind| kind.parse(raw)),
    }
}

const GEOMETRY_POINT_HINT: &str =
    "geometry_point expects GeoJSON like {\"type\":\"Point\",\"coordinates\":[x,y]} with finite numbers.";
const GEOMETRY_POLYGON_HINT: &str = "geometry_polygon expects GeoJSON Polygon coordinates: rings of at least 4 finite [x,y] positions, each ring closed (first equals last).";

fn bounded_text(
    type_name: &str,
    raw: &str,
    max_chars: usize,
    allow_inner_whitespace: bool,
) -> Result<Value, String> {
    let text = raw.trim();
    let valid = !text.is_empty()
        && text.chars().count() <= max_chars
        && (allow_inner_whitespace || !text.chars().any(char::is_whitespace));
    if valid {
        Ok(Value::String(text.to_owned()))
    } else if allow_inner_whitespace {
        Err(format!(
            "{type_name} expects non-empty text of at most {max_chars} characters."
        ))
    } else {
        Err(format!(
            "{type_name} expects a non-empty identifier without spaces, at most {max_chars} characters."
        ))
    }
}

fn parse_geometry(
    type_name: &str,
    raw: &str,
    hint: &str,
    shape_check: fn(&Value) -> bool,
) -> Result<Value, String> {
    let value = PropertyValueKind::Object
        .parse(raw)
        .map_err(|error| format!("{type_name}: {error} {hint}"))?;
    if shape_check(&value) {
        Ok(value)
    } else {
        Err(hint.to_owned())
    }
}

fn is_position(value: &Value) -> bool {
    value.as_array().is_some_and(|coords| {
        (coords.len() == 2 || coords.len() == 3)
            && coords
                .iter()
                .all(|c| c.as_f64().is_some_and(f64::is_finite))
    })
}

fn is_point(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("Point")
        && value.get("coordinates").is_some_and(is_position)
}

fn is_polygon(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("Polygon") {
        return false;
    }
    let Some(rings) = value.get("coordinates").and_then(Value::as_array) else {
        return false;
    };
    !rings.is_empty()
        && rings.iter().all(|ring| {
            ring.as_array().is_some_and(|positions| {
                positions.len() >= 4
                    && positions.iter().all(is_position)
                    && positions.first() == positions.last()
            })
        })
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
