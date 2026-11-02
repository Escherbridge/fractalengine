//! Rhai host API — functions registered into the Rhai engine that bridge
//! back to the engine through [`PluginContext`].
//!
//! Each function captures a shared reference to the plugin context so Rhai
//! scripts can interact with the scene graph without direct Bevy access.

use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Engine, Map};

use crate::context::PluginContext;

/// Register all host API functions into a Rhai [`Engine`].
///
/// The `ctx` is wrapped in `Arc<Mutex<>>` so the closures can share it.
pub fn register_host_api(engine: &mut Engine, ctx: Arc<Mutex<PluginContext>>) {
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "get_node",
            move |id: &str| -> Result<Dynamic, Box<rhai::EvalAltResult>> {
                let ctx = ctx
                    .lock()
                    .map_err(|_| script_error("plugin context lock poisoned"))?;
                let node = ctx.get_node(id).map_err(script_error)?;
                Ok(node
                    .map(|node| {
                        super::storage_api::json_to_dynamic(&fe_sdk::plugin_host::snapshot_json(
                            &node,
                        ))
                    })
                    .unwrap_or(Dynamic::UNIT))
            },
        );
    }
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "query_nodes",
            move |petal_id: &str, filter: &str| -> Result<rhai::Array, Box<rhai::EvalAltResult>> {
                let query = fe_sdk::plugin_host::NodeQuery::parse(filter).map_err(script_error)?;
                let ctx = ctx
                    .lock()
                    .map_err(|_| script_error("plugin context lock poisoned"))?;
                let nodes = ctx.query_nodes(petal_id, &query).map_err(script_error)?;
                Ok(nodes
                    .iter()
                    .map(|node| {
                        super::storage_api::json_to_dynamic(&fe_sdk::plugin_host::snapshot_json(
                            node,
                        ))
                    })
                    .collect())
            },
        );
    }
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "set_property",
            move |node_id: &str,
                  key: &str,
                  value: Dynamic|
                  -> Result<(), Box<rhai::EvalAltResult>> {
                let ctx = ctx
                    .lock()
                    .map_err(|_| script_error("plugin context lock poisoned"))?;
                ctx.commit_ops(&[fe_sdk::plugin_host::PendingOp::SetProperty {
                    node_id: node_id.into(),
                    key: key.into(),
                    value: dynamic_to_json(&value),
                }])
                .map_err(script_error)?;
                Ok(())
            },
        );
    }
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "create_node",
            move |petal_id: &str, name: &str| -> Result<String, Box<rhai::EvalAltResult>> {
                let ctx = ctx
                    .lock()
                    .map_err(|_| script_error("plugin context lock poisoned"))?;
                let receipt = ctx
                    .commit_ops(&[fe_sdk::plugin_host::PendingOp::CreateNode {
                        petal_id: petal_id.into(),
                        name: name.into(),
                    }])
                    .map_err(script_error)?;
                receipt
                    .created_node_ids
                    .into_iter()
                    .next()
                    .ok_or_else(|| script_error("committed creation has no node ID"))
            },
        );
    }
    {
        let ctx = ctx.clone();
        engine.register_fn(
            "delete_node",
            move |node_id: &str| -> Result<(), Box<rhai::EvalAltResult>> {
                let ctx = ctx
                    .lock()
                    .map_err(|_| script_error("plugin context lock poisoned"))?;
                ctx.commit_ops(&[fe_sdk::plugin_host::PendingOp::DeleteNode {
                    node_id: node_id.into(),
                }])
                .map_err(script_error)?;
                Ok(())
            },
        );
    }

    {
        let ctx = ctx.clone();
        engine.register_fn(
            "commit_transaction",
            move |petal_id: &str, encoded: &str| -> Result<Dynamic, Box<rhai::EvalAltResult>> {
                if encoded.len() > fe_sdk::plugin_host::MAX_HOST_BYTES {
                    return Err(script_error("plugin transaction exceeds byte limit"));
                }
                let ops: Vec<fe_sdk::plugin_host::PendingOp> =
                    serde_json::from_str(encoded).map_err(script_error)?;
                let ctx = ctx
                    .lock()
                    .map_err(|_| script_error("plugin context lock poisoned"))?;
                if petal_id != ctx.petal_id() {
                    return Err(script_error("plugin transaction targets another petal"));
                }
                let receipt = ctx.commit_ops(&ops).map_err(script_error)?;
                Ok(super::storage_api::json_to_dynamic(
                    &serde_json::to_value(receipt).map_err(script_error)?,
                ))
            },
        );
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

fn script_error(error: impl std::fmt::Display) -> Box<rhai::EvalAltResult> {
    tracing::warn!(error=%error,"plugin scene host call failed");
    Box::new(rhai::EvalAltResult::ErrorRuntime(
        Dynamic::from(error.to_string()),
        rhai::Position::NONE,
    ))
}
