//! Host functions for Wasm plugins, matching the Rhai API surface.
//!
//! These functions are registered into a [`wasmtime::Linker`] and are
//! called by Wasm guest modules. They bridge back to the Bevy ECS
//! through the [`PluginContext`] stored in the [`Store`](wasmtime::Store)'s data.

use wasmtime::{AsContext, AsContextMut, Caller, Linker};

use crate::wasm::PluginStoreData;
use crate::LogLevel;

/// Register all host import functions into a wasmtime [`Linker`].
///
/// The function signatures mirror the Rhai host API:
/// - `get_node(node_id) -> String` (JSON-serialized node data)
/// - `set_property(node_id, key, value)` (value as JSON string)
/// - `query_nodes(petal_id, filter) -> String` (JSON array)
/// - `create_node(petal_id, name) -> String` (returns new node_id)
/// - `delete_node(node_id)`
/// - `log_debug`, `log_info`, `log_warn`, `log_error` (message string)
pub fn register_host_imports(
    linker: &mut Linker<PluginStoreData>,
) -> Result<(), crate::wasm::WasmError> {
    let namespace = "fe";

    // ---- fe::get_node(node_id_ptr: i32, node_id_len: i32) -> i32 ----
    // Returns a pointer to a JSON-serialized node snapshot.
    linker.func_wrap(
        namespace,
        "get_node",
        |mut caller: Caller<'_, PluginStoreData>, node_id_ptr: i32, node_id_len: i32| -> i32 {
            let node_id = read_string_from_memory(&mut caller, node_id_ptr, node_id_len);
            let result = caller.data().ctx.get_node(&node_id).and_then(|node| {
                serde_json::to_string(&node.as_ref().map(fe_sdk::plugin_host::snapshot_json))
                    .map_err(|error| error.to_string())
            });
            match result {
                Ok(encoded) => write_string_to_memory(&mut caller, &encoded),
                Err(error) => {
                    tracing::warn!(%error,"WASM get_node failed");
                    -1
                }
            }
        },
    )?;

    // ---- fe::set_property(node_id_ptr, node_id_len, key_ptr, key_len, value_ptr, value_len) ----
    linker.func_wrap(
        namespace,
        "set_property",
        |mut caller: Caller<'_, PluginStoreData>,
         node_id_ptr: i32,
         node_id_len: i32,
         key_ptr: i32,
         key_len: i32,
         value_ptr: i32,
         value_len: i32| {
            let node_id = read_string_from_memory(&mut caller, node_id_ptr, node_id_len);
            let key = read_string_from_memory(&mut caller, key_ptr, key_len);
            let value_str = read_string_from_memory(&mut caller, value_ptr, value_len);
            let value =
                serde_json::from_str(&value_str).unwrap_or(serde_json::Value::String(value_str));
            if let Err(error) =
                caller
                    .data()
                    .ctx
                    .commit_ops(&[crate::transaction::PendingOp::SetProperty {
                        node_id,
                        key,
                        value,
                    }])
            {
                tracing::warn!(%error,"WASM set_property failed");
            }
        },
    )?;

    // ---- fe::query_nodes(petal_id_ptr, petal_id_len, filter_ptr, filter_len) -> i32 ----
    linker.func_wrap(
        namespace,
        "query_nodes",
        |mut caller: Caller<'_, PluginStoreData>,
         petal_id_ptr: i32,
         petal_id_len: i32,
         filter_ptr: i32,
         filter_len: i32|
         -> i32 {
            let petal_id = read_string_from_memory(&mut caller, petal_id_ptr, petal_id_len);
            let filter = read_string_from_memory(&mut caller, filter_ptr, filter_len);
            let result = fe_sdk::plugin_host::NodeQuery::parse(&filter)
                .and_then(|query| caller.data().ctx.query_nodes(&petal_id, &query))
                .and_then(|nodes| {
                    serde_json::to_string(
                        &nodes
                            .iter()
                            .map(fe_sdk::plugin_host::snapshot_json)
                            .collect::<Vec<_>>(),
                    )
                    .map_err(|error| error.to_string())
                });
            match result {
                Ok(encoded) => write_string_to_memory(&mut caller, &encoded),
                Err(error) => {
                    tracing::warn!(%error,"WASM query_nodes failed");
                    -1
                }
            }
        },
    )?;

    // ---- fe::create_node(petal_id_ptr, petal_id_len, name_ptr, name_len) -> i32 ----
    linker.func_wrap(
        namespace,
        "create_node",
        |mut caller: Caller<'_, PluginStoreData>,
         petal_id_ptr: i32,
         petal_id_len: i32,
         name_ptr: i32,
         name_len: i32|
         -> i32 {
            let petal_id = read_string_from_memory(&mut caller, petal_id_ptr, petal_id_len);
            let name = read_string_from_memory(&mut caller, name_ptr, name_len);
            let result = caller
                .data()
                .ctx
                .commit_ops(&[crate::transaction::PendingOp::CreateNode { petal_id, name }]);
            match result {
                Ok(receipt) => match receipt.created_node_ids.first() {
                    Some(id) => write_string_to_memory(&mut caller, id),
                    None => -1,
                },
                Err(error) => {
                    tracing::warn!(%error,"WASM create_node failed");
                    -1
                }
            }
        },
    )?;

    // ---- fe::delete_node(node_id_ptr, node_id_len) ----
    linker.func_wrap(
        namespace,
        "delete_node",
        |mut caller: Caller<'_, PluginStoreData>, node_id_ptr: i32, node_id_len: i32| {
            let node_id = read_string_from_memory(&mut caller, node_id_ptr, node_id_len);
            if let Err(error) = caller
                .data()
                .ctx
                .commit_ops(&[crate::transaction::PendingOp::DeleteNode { node_id }])
            {
                tracing::warn!(%error,"WASM delete_node failed");
            }
        },
    )?;

    // ---- Storage + query host imports (call-time capability gated) ----
    register_storage_query_imports(linker, namespace)?;

    // ---- Logging functions ----
    register_log_func(linker, namespace, "log_debug", LogLevel::Debug)?;
    register_log_func(linker, namespace, "log_info", LogLevel::Info)?;
    register_log_func(linker, namespace, "log_warn", LogLevel::Warn)?;
    register_log_func(linker, namespace, "log_error", LogLevel::Error)?;

    Ok(())
}

/// Register the storage/query host imports mirroring the Rhai + WIT surface.
///
/// Unlike Rhai (where a missing grant means the function is never registered),
/// the WASM linker is shared across plugins, so gating happens per-call against
/// the store's capability token. A denied or failed call logs via `tracing` and
/// returns a sentinel (`-1` / no-op) instead of trapping.
fn register_storage_query_imports(
    linker: &mut Linker<PluginStoreData>,
    namespace: &str,
) -> Result<(), crate::wasm::WasmError> {
    // ---- fe::node_get_properties(node_id_ptr, node_id_len) -> i32 ----
    linker.func_wrap(
        namespace,
        "node_get_properties",
        |mut caller: Caller<'_, PluginStoreData>, node_id_ptr: i32, node_id_len: i32| -> i32 {
            let node_id = read_string_from_memory(&mut caller, node_id_ptr, node_id_len);
            // Scope the shared borrow so it ends before the mutable memory write.
            let result = {
                let data = caller.data();
                data.ctx.host().node_get_properties(&data.ctx.capabilities, &node_id)
            };
            match result {
                Ok(props) => {
                    let obj: serde_json::Map<String, serde_json::Value> = props
                        .iter()
                        .map(|(k, v)| (k.clone(), property_to_json(v)))
                        .collect();
                    write_string_to_memory(&mut caller, &serde_json::Value::Object(obj).to_string())
                }
                Err(e) => {
                    tracing::warn!(func = "node_get_properties", error = %e, "wasm host API call failed");
                    -1
                }
            }
        },
    )?;

    // ---- fe::node_set_property(node_id, key, value_json) ----
    linker.func_wrap(
        namespace,
        "node_set_property",
        |mut caller: Caller<'_, PluginStoreData>,
         node_id_ptr: i32,
         node_id_len: i32,
         key_ptr: i32,
         key_len: i32,
         value_ptr: i32,
         value_len: i32| {
            let node_id = read_string_from_memory(&mut caller, node_id_ptr, node_id_len);
            let key = read_string_from_memory(&mut caller, key_ptr, key_len);
            let value_str = read_string_from_memory(&mut caller, value_ptr, value_len);
            let value = json_str_to_property(&value_str);
            let ctx = caller.data();
            if let Err(e) =
                ctx.ctx
                    .host()
                    .node_set_property(&ctx.ctx.capabilities, &node_id, &key, value)
            {
                tracing::warn!(func = "node_set_property", error = %e, "wasm host API call failed");
            }
        },
    )?;

    // ---- fe::query_select(sql, params_json) -> i32 ----
    linker.func_wrap(
        namespace,
        "query_select",
        |mut caller: Caller<'_, PluginStoreData>,
         sql_ptr: i32,
         sql_len: i32,
         params_ptr: i32,
         params_len: i32|
         -> i32 {
            let sql = read_string_from_memory(&mut caller, sql_ptr, sql_len);
            let params_str = read_string_from_memory(&mut caller, params_ptr, params_len);
            let params: std::collections::HashMap<String, serde_json::Value> =
                serde_json::from_str(&params_str).unwrap_or_default();
            let result = {
                let data = caller.data();
                data.ctx
                    .host()
                    .query_select(&data.ctx.capabilities, &sql, params)
            };
            match result {
                Ok(rows) => {
                    write_string_to_memory(&mut caller, &serde_json::Value::Array(rows).to_string())
                }
                Err(e) => {
                    tracing::warn!(func = "query_select", error = %e, "wasm host API call failed");
                    -1
                }
            }
        },
    )?;

    // ---- fe::ext_storage_get(key) -> i32 (JSON value, "null" if absent) ----
    linker.func_wrap(
        namespace,
        "ext_storage_get",
        |mut caller: Caller<'_, PluginStoreData>, key_ptr: i32, key_len: i32| -> i32 {
            let key = read_string_from_memory(&mut caller, key_ptr, key_len);
            let result = {
                let data = caller.data();
                let ns = data.ctx.plugin_id().to_string();
                data.ctx.host().storage_get(&data.ctx.capabilities, &ns, &key)
            };
            match result {
                Ok(value) => {
                    let json = value
                        .as_ref()
                        .map(property_to_json)
                        .unwrap_or(serde_json::Value::Null);
                    write_string_to_memory(&mut caller, &json.to_string())
                }
                Err(e) => {
                    tracing::warn!(func = "ext_storage_get", error = %e, "wasm host API call failed");
                    -1
                }
            }
        },
    )?;

    // ---- fe::ext_storage_set(key, value_json) ----
    linker.func_wrap(
        namespace,
        "ext_storage_set",
        |mut caller: Caller<'_, PluginStoreData>,
         key_ptr: i32,
         key_len: i32,
         value_ptr: i32,
         value_len: i32| {
            let key = read_string_from_memory(&mut caller, key_ptr, key_len);
            let value_str = read_string_from_memory(&mut caller, value_ptr, value_len);
            let value = json_str_to_property(&value_str);
            let ctx = caller.data();
            let ns = ctx.ctx.plugin_id().to_string();
            if let Err(e) = ctx
                .ctx
                .host()
                .storage_set(&ctx.ctx.capabilities, &ns, &key, value)
            {
                tracing::warn!(func = "ext_storage_set", error = %e, "wasm host API call failed");
            }
        },
    )?;

    Ok(())
}

/// Convert a [`fe_sdk::property::PropertyValue`] to a plain JSON value.
fn property_to_json(value: &fe_sdk::property::PropertyValue) -> serde_json::Value {
    use fe_sdk::property::PropertyValue;
    match value {
        PropertyValue::String(s) => serde_json::Value::String(s.clone()),
        PropertyValue::Number(n) => serde_json::json!(n),
        PropertyValue::Bool(b) => serde_json::Value::Bool(*b),
        PropertyValue::Json(j) => j.clone(),
    }
}

/// Parse a JSON string into a [`fe_sdk::property::PropertyValue`].
fn json_str_to_property(s: &str) -> fe_sdk::property::PropertyValue {
    use fe_sdk::property::PropertyValue;
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(serde_json::Value::String(v)) => PropertyValue::String(v),
        Ok(serde_json::Value::Bool(b)) => PropertyValue::Bool(b),
        Ok(serde_json::Value::Number(n)) => PropertyValue::Number(n.as_f64().unwrap_or(0.0)),
        Ok(other) => PropertyValue::Json(other),
        Err(_) => PropertyValue::String(s.to_string()),
    }
}

/// Register a single logging host function.
fn register_log_func(
    linker: &mut Linker<PluginStoreData>,
    namespace: &str,
    name: &str,
    level: LogLevel,
) -> Result<(), crate::wasm::WasmError> {
    linker.func_wrap(
        namespace,
        name,
        move |mut caller: Caller<'_, PluginStoreData>, msg_ptr: i32, msg_len: i32| {
            let msg = read_string_from_memory(&mut caller, msg_ptr, msg_len);
            let plugin_id = caller.data().ctx.plugin_id().to_string();

            match level {
                LogLevel::Debug => tracing::debug!(plugin = %plugin_id, "{}", msg),
                LogLevel::Info => tracing::info!(plugin = %plugin_id, "{}", msg),
                LogLevel::Warn => tracing::warn!(plugin = %plugin_id, "{}", msg),
                LogLevel::Error => tracing::error!(plugin = %plugin_id, "{}", msg),
            }
        },
    )?;
    Ok(())
}

/// Read a UTF-8 string from the guest's linear memory.
///
/// Uses the memory export named "memory" (conventional for WASI/wasm modules).
/// If the module doesn't export memory, returns an empty string.
fn read_string_from_memory(caller: &mut Caller<'_, PluginStoreData>, ptr: i32, len: i32) -> String {
    if ptr < 0 || len <= 0 || len as usize > fe_sdk::plugin_host::MAX_HOST_BYTES {
        return String::new();
    }

    let memory = match caller.get_export("memory").and_then(|export|export.into_memory()) {
        Some(memory) => memory,
        None => return String::new(),
    };

    let data = memory.data(caller.as_context());
    let ptr = ptr as usize;
    let len = len as usize;

    if ptr + len > data.len() {
        tracing::warn!(
            "Host import read out of bounds: ptr={ptr}, len={len}, mem_size={}",
            data.len()
        );
        return String::new();
    }

    String::from_utf8(data[ptr..ptr + len].to_vec()).unwrap_or_default()
}

/// Write NUL-terminated UTF-8 to the fixed guest response buffer at offset zero.
fn write_string_to_memory(caller: &mut Caller<'_, PluginStoreData>, value: &str) -> i32 {
    let bytes = value.as_bytes();

    let memory = match caller.get_export("memory").and_then(|export|export.into_memory()) {
        Some(memory) => memory,
        None => return -1,
    };

    let data = memory.data_mut(caller.as_context_mut());
    let mem_len = data.len();

    if bytes.len() >= mem_len || bytes.len()>fe_sdk::plugin_host::MAX_HOST_BYTES {
        tracing::warn!(
            "Host import write exceeds memory: bytes={}, mem_size={}",
            bytes.len(),
            mem_len
        );
        return -1;
    }

    data[..bytes.len()].copy_from_slice(bytes);
    data[bytes.len()]=0;
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_host_imports_succeeds() {
        use crate::wasm::WasmEngine;

        let engine = WasmEngine::new().unwrap();
        let mut linker: wasmtime::Linker<crate::wasm::PluginStoreData> =
            wasmtime::Linker::new(engine.engine());

        let result = register_host_imports(&mut linker);
        assert!(result.is_ok(), "Host imports should register successfully");
    }
}
