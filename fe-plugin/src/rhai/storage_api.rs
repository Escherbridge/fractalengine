//! Rhai bindings for the storage + query host API.
//!
//! Registers `node_get_properties`, `node_set_property`, `query_select`,
//! `ext_storage_get`, and `ext_storage_set` — but only the ones the plugin's
//! capability token grants. A missing grant means the function is never
//! registered, so calling it raises a clean Rhai "function not found" error
//! rather than silently succeeding. All routed errors surface to the script as
//! typed runtime errors and to `tracing`. See `fe-plugin/src/AGENTS.md`
//! §storage-query.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Position};

use fe_sdk::property::PropertyValue;
use fe_sdk::{CAP_QUERY_SELECT, CAP_STORAGE_READ, CAP_STORAGE_WRITE};

use crate::context::PluginContext;
use crate::host_env::HostApiError;
use crate::rhai::host_api::dynamic_to_json;

/// Register the storage/query host functions granted by the plugin's token.
pub fn register_storage_query_api(engine: &mut Engine, ctx: Arc<Mutex<PluginContext>>) {
    // Snapshot the grants once; registration itself is the capability gate.
    let (read, write, query) = {
        let c = ctx.lock().unwrap();
        (
            c.capabilities.has_capability(CAP_STORAGE_READ),
            c.capabilities.has_capability(CAP_STORAGE_WRITE),
            c.capabilities.has_capability(CAP_QUERY_SELECT),
        )
    };

    if read {
        // ---- node_get_properties(node_id: String) -> Map ----
        {
            let ctx = ctx.clone();
            engine.register_fn(
                "node_get_properties",
                move |node_id: &str| -> Result<Map, Box<EvalAltResult>> {
                    let ctx = ctx.lock().unwrap();
                    let props = ctx
                        .host()
                        .node_get_properties(&ctx.capabilities, node_id)
                        .map_err(|e| to_script_error(&ctx, "node_get_properties", e))?;
                    let mut map = Map::new();
                    for (k, v) in props {
                        map.insert(k.into(), property_to_dynamic(&v));
                    }
                    Ok(map)
                },
            );
        }

        // ---- ext_storage_get(key: String) -> Dynamic ----
        {
            let ctx = ctx.clone();
            engine.register_fn(
                "ext_storage_get",
                move |key: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                    let ctx = ctx.lock().unwrap();
                    let namespace = ctx.plugin_id().to_string();
                    let value = ctx
                        .host()
                        .storage_get(&ctx.capabilities, &namespace, key)
                        .map_err(|e| to_script_error(&ctx, "ext_storage_get", e))?;
                    Ok(value.map(|v| property_to_dynamic(&v)).unwrap_or(Dynamic::UNIT))
                },
            );
        }
    }

    if write {
        // ---- node_set_property(node_id, key, value: Dynamic) ----
        {
            let ctx = ctx.clone();
            engine.register_fn(
                "node_set_property",
                move |node_id: &str, key: &str, value: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    let prop = dynamic_to_property(&value);
                    let ctx = ctx.lock().unwrap();
                    ctx.host()
                        .node_set_property(&ctx.capabilities, node_id, key, prop)
                        .map_err(|e| to_script_error(&ctx, "node_set_property", e))
                },
            );
        }

        // ---- ext_storage_set(key, value: Dynamic) ----
        {
            let ctx = ctx.clone();
            engine.register_fn(
                "ext_storage_set",
                move |key: &str, value: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    let prop = dynamic_to_property(&value);
                    let ctx = ctx.lock().unwrap();
                    let namespace = ctx.plugin_id().to_string();
                    ctx.host()
                        .storage_set(&ctx.capabilities, &namespace, key, prop)
                        .map_err(|e| to_script_error(&ctx, "ext_storage_set", e))
                },
            );
        }
    }

    if query {
        // ---- query_select(sql: String, params: Map) -> Array ----
        {
            let ctx = ctx.clone();
            engine.register_fn(
                "query_select",
                move |sql: &str, params: Map| -> Result<Array, Box<EvalAltResult>> {
                    let params = map_to_params(&params);
                    let ctx = ctx.lock().unwrap();
                    let rows = ctx
                        .host()
                        .query_select(&ctx.capabilities, sql, params)
                        .map_err(|e| to_script_error(&ctx, "query_select", e))?;
                    Ok(rows.iter().map(json_to_dynamic).collect())
                },
            );
        }
    }
}

/// Convert a [`HostApiError`] into a Rhai runtime error and log it.
fn to_script_error(ctx: &PluginContext, func: &str, err: HostApiError) -> Box<EvalAltResult> {
    tracing::warn!(plugin = %ctx.plugin_id(), func = %func, error = %err, "host API call failed");
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(format!("{func}: {err}")),
        Position::NONE,
    ))
}

/// Convert a [`PropertyValue`] into a Rhai [`Dynamic`].
pub(crate) fn property_to_dynamic(value: &PropertyValue) -> Dynamic {
    match value {
        PropertyValue::String(s) => Dynamic::from(s.clone()),
        PropertyValue::Number(n) => Dynamic::from(*n),
        PropertyValue::Bool(b) => Dynamic::from(*b),
        PropertyValue::Json(j) => json_to_dynamic(j),
    }
}

/// Convert a Rhai [`Dynamic`] into a [`PropertyValue`].
pub(crate) fn dynamic_to_property(value: &Dynamic) -> PropertyValue {
    if let Ok(b) = value.as_bool() {
        PropertyValue::Bool(b)
    } else if let Ok(i) = value.as_int() {
        PropertyValue::Number(i as f64)
    } else if let Ok(f) = value.as_float() {
        PropertyValue::Number(f)
    } else if let Ok(s) = value.clone().into_string() {
        PropertyValue::String(s)
    } else {
        PropertyValue::Json(dynamic_to_json(value))
    }
}

/// Convert a Rhai param [`Map`] into JSON bind params.
fn map_to_params(map: &Map) -> HashMap<String, serde_json::Value> {
    map.iter()
        .map(|(k, v)| (k.to_string(), dynamic_to_json(v)))
        .collect()
}

/// Convert a [`serde_json::Value`] into a Rhai [`Dynamic`].
pub(crate) fn json_to_dynamic(value: &serde_json::Value) -> Dynamic {
    match value {
        serde_json::Value::Null => Dynamic::UNIT,
        serde_json::Value::Bool(b) => Dynamic::from(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Dynamic::from(i)
            } else {
                Dynamic::from(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Dynamic::from(s.clone()),
        serde_json::Value::Array(arr) => {
            Dynamic::from(arr.iter().map(json_to_dynamic).collect::<Array>())
        }
        serde_json::Value::Object(obj) => {
            let mut map = Map::new();
            for (k, v) in obj {
                map.insert(k.clone().into(), json_to_dynamic(v));
            }
            Dynamic::from(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_property_roundtrip() {
        assert_eq!(
            dynamic_to_property(&Dynamic::from("hi")),
            PropertyValue::String("hi".into())
        );
        assert_eq!(
            dynamic_to_property(&Dynamic::from(true)),
            PropertyValue::Bool(true)
        );
        assert_eq!(
            dynamic_to_property(&Dynamic::from(3.5_f64)),
            PropertyValue::Number(3.5)
        );
    }

    #[test]
    fn json_to_dynamic_nested() {
        let v = serde_json::json!({"a": [1, 2], "b": "x"});
        let d = json_to_dynamic(&v);
        assert!(d.is_map());
    }
}
