use serde::{Deserialize, Serialize};

/// A typed query value that can be bound as a parameter in SurrealQL.
///
/// All user-supplied values flow through this enum before being rendered
/// into parameterized queries, preventing injection attacks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum QueryValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Json(serde_json::Value),
    Point { x: f64, z: f64 },
    Polygon(Vec<[f64; 2]>),
    Null,
    Array(Vec<QueryValue>),
}

impl QueryValue {
    /// Convert to a `serde_json::Value` suitable for SurrealDB parameter binding.
    pub fn to_bind_value(&self) -> serde_json::Value {
        match self {
            QueryValue::String(s) => serde_json::Value::String(s.clone()),
            QueryValue::Int(n) => serde_json::json!(n),
            QueryValue::Float(f) => serde_json::json!(f),
            QueryValue::Bool(b) => serde_json::json!(b),
            QueryValue::Json(v) => v.clone(),
            QueryValue::Point { x, z } => {
                // SurrealDB geometry point: (lon, lat) or (x, z) for local coords
                serde_json::json!({
                    "type": "Point",
                    "coordinates": [x, z]
                })
            }
            QueryValue::Polygon(rings) => {
                serde_json::json!({
                    "type": "Polygon",
                    "coordinates": [rings]
                })
            }
            QueryValue::Null => serde_json::Value::Null,
            QueryValue::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(|v| v.to_bind_value()).collect())
            }
        }
    }
}

impl From<&str> for QueryValue {
    fn from(s: &str) -> Self {
        QueryValue::String(s.to_string())
    }
}

impl From<String> for QueryValue {
    fn from(s: String) -> Self {
        QueryValue::String(s)
    }
}

impl From<i64> for QueryValue {
    fn from(n: i64) -> Self {
        QueryValue::Int(n)
    }
}

impl From<f64> for QueryValue {
    fn from(f: f64) -> Self {
        QueryValue::Float(f)
    }
}

impl From<bool> for QueryValue {
    fn from(b: bool) -> Self {
        QueryValue::Bool(b)
    }
}

impl From<serde_json::Value> for QueryValue {
    fn from(v: serde_json::Value) -> Self {
        QueryValue::Json(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_conversion() {
        let v: QueryValue = "hello".into();
        assert_eq!(v.to_bind_value(), serde_json::json!("hello"));
    }

    #[test]
    fn int_conversion() {
        let v: QueryValue = 42i64.into();
        assert_eq!(v.to_bind_value(), serde_json::json!(42));
    }

    #[test]
    fn float_conversion() {
        let v: QueryValue = 2.5f64.into();
        assert_eq!(v.to_bind_value(), serde_json::json!(2.5));
    }

    #[test]
    fn bool_conversion() {
        let v: QueryValue = true.into();
        assert_eq!(v.to_bind_value(), serde_json::json!(true));
    }

    #[test]
    fn null_conversion() {
        assert_eq!(QueryValue::Null.to_bind_value(), serde_json::Value::Null);
    }

    #[test]
    fn point_bind_value() {
        let v = QueryValue::Point { x: 1.5, z: 2.5 };
        let bound = v.to_bind_value();
        assert_eq!(bound["type"], "Point");
        assert_eq!(bound["coordinates"][0], 1.5);
        assert_eq!(bound["coordinates"][1], 2.5);
    }

    #[test]
    fn polygon_bind_value() {
        let v = QueryValue::Polygon(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 0.0]]);
        let bound = v.to_bind_value();
        assert_eq!(bound["type"], "Polygon");
    }

    #[test]
    fn array_bind_value() {
        let v = QueryValue::Array(vec![
            QueryValue::Int(1),
            QueryValue::Int(2),
            QueryValue::Int(3),
        ]);
        assert_eq!(v.to_bind_value(), serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn json_passthrough() {
        let original = serde_json::json!({"nested": [1, 2, 3]});
        let v: QueryValue = original.clone().into();
        assert_eq!(v.to_bind_value(), original);
    }
}
