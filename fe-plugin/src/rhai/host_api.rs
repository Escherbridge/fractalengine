//! Rhai host API — functions registered into the Rhai engine that bridge
//! back to the engine through [`PluginContext`].
//!
//! Each function captures a shared reference to the plugin context so Rhai
//! scripts can interact with the scene graph without direct Bevy access.

use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Engine, Map};

use crate::context::PluginContext;
use crate::PluginCommand;

/// Register all host API functions into a Rhai [`Engine`].
///
/// The `ctx` is wrapped in `Arc<Mutex<>>` so the closures can share it.
pub fn register_host_api(engine: &mut Engine, ctx: Arc<Mutex<PluginContext>>) {
    // ---- get_node(id: String) -> Map ----
    {
        let ctx = ctx.clone();
        engine.register_fn("get_node", move |id: &str| -> Dynamic {
            let ctx = ctx.lock().unwrap();
            let _ = ctx.send_command(PluginCommand::GetNode {
                plugin_id: ctx.plugin_id().to_string(),
                node_id: id.to_string(),
            });
            // In Phase 9A.1 we return a placeholder map. Phase 9A.2 will add
            // a request-reply pattern through the command channel.
            let mut map = Map::new();
            map.insert("node_id".into(), Dynamic::from(id.to_string()));
            map.insert("name".into(), Dynamic::from(String::new()));
            map.insert("properties".into(), Dynamic::from(Map::new()));
            Dynamic::from(map)
        });
    }

    // ---- set_property(node_id: String, key: String, value: Dynamic) ----
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_property",
            move |node_id: &str, key: &str, value: Dynamic| {
                let ctx = ctx.lock().unwrap();
                let json_value = dynamic_to_json(&value);
                let _ = ctx.send_command(PluginCommand::SetProperty {
                    plugin_id: ctx.plugin_id().to_string(),
                    node_id: node_id.to_string(),
                    key: key.to_string(),
                    value: json_value,
                });
            },
        );
    }

    // ---- query_nodes(petal_id: String, filter: String) -> Array ----
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "query_nodes",
            move |petal_id: &str, _filter: &str| -> rhai::Array {
                let ctx = ctx.lock().unwrap();
                let _ = ctx.send_command(PluginCommand::QueryNodes {
                    plugin_id: ctx.plugin_id().to_string(),
                    petal_id: petal_id.to_string(),
                });
                // Placeholder: return empty array. Phase 9A.2 adds request-reply.
                rhai::Array::new()
            },
        );
    }

    // ---- create_node(petal_id: String, name: String) -> String ----
    {
        let ctx = ctx.clone();
        engine.register_fn("create_node", move |petal_id: &str, name: &str| -> String {
            let ctx = ctx.lock().unwrap();
            let node_id = ulid::Ulid::new().to_string();
            let _ = ctx.send_command(PluginCommand::CreateNode {
                plugin_id: ctx.plugin_id().to_string(),
                petal_id: petal_id.to_string(),
                name: name.to_string(),
                provisional_id: node_id.clone(),
            });
            node_id
        });
    }

    // ---- Logging functions ----
    {
        let ctx_debug = ctx.clone();
        engine.register_fn("log_debug", move |msg: &str| {
            let ctx = ctx_debug.lock().unwrap();
            tracing::debug!(plugin = %ctx.plugin_id(), "{}", msg);
        });
    }
    {
        let ctx_info = ctx.clone();
        engine.register_fn("log_info", move |msg: &str| {
            let ctx = ctx_info.lock().unwrap();
            tracing::info!(plugin = %ctx.plugin_id(), "{}", msg);
        });
    }
    {
        let ctx_warn = ctx.clone();
        engine.register_fn("log_warn", move |msg: &str| {
            let ctx = ctx_warn.lock().unwrap();
            tracing::warn!(plugin = %ctx.plugin_id(), "{}", msg);
        });
    }
    {
        engine.register_fn("log_error", move |msg: &str| {
            let ctx = ctx.lock().unwrap();
            tracing::error!(plugin = %ctx.plugin_id(), "{}", msg);
        });
    }
}

/// Convert a Rhai [`Dynamic`] value to a [`serde_json::Value`].
pub(crate) fn dynamic_to_json(value: &Dynamic) -> serde_json::Value {
    if value.is_unit() {
        serde_json::Value::Null
    } else if let Ok(b) = value.as_bool() {
        serde_json::Value::Bool(b)
    } else if let Ok(i) = value.as_int() {
        serde_json::json!(i)
    } else if let Ok(f) = value.as_float() {
        serde_json::json!(f)
    } else if let Ok(s) = value.clone().into_string() {
        serde_json::Value::String(s)
    } else if value.is_array() {
        let arr = value
            .clone()
            .into_typed_array::<Dynamic>()
            .unwrap_or_default();
        serde_json::Value::Array(arr.iter().map(dynamic_to_json).collect())
    } else if value.is_map() {
        let map = value.clone().cast::<Map>();
        let mut obj = serde_json::Map::new();
        for (k, v) in map.iter() {
            obj.insert(k.to_string(), dynamic_to_json(v));
        }
        serde_json::Value::Object(obj)
    } else {
        serde_json::Value::Null
    }
}
