//! Host functions for Wasm plugins, matching the Rhai API surface.
//!
//! These functions are registered into a [`wasmtime::Linker`] and are
//! called by Wasm guest modules. They bridge back to the Bevy ECS
//! through the [`PluginContext`] stored in the [`Store`](wasmtime::Store)'s data.

use wasmtime::{AsContext, AsContextMut, Caller, Linker};

use crate::wasm::PluginStoreData;
use crate::LogLevel;

macro_rules! guest_input {
    ($caller:expr, $ptr:expr, $len:expr, $failure:expr) => {
        match read_string_from_memory($caller, $ptr, $len) {
            Ok(value) => value,
            Err(error) => { tracing::warn!(%error, "WASM input refused"); return $failure; }
        }
    };
}

macro_rules! guest_json {
    ($input:expr, $failure:expr) => {
        match serde_json::from_str($input) {
            Ok(value) => value,
            Err(error) => { tracing::warn!(%error, "WASM JSON refused"); return $failure; }
        }
    };
}

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
            let node_id = guest_input!(&mut caller, node_id_ptr, node_id_len, -1);
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
            let node_id = guest_input!(&mut caller, node_id_ptr, node_id_len, ());
            let key = guest_input!(&mut caller, key_ptr, key_len, ());
            let value_str = guest_input!(&mut caller, value_ptr, value_len, ());
            let value = guest_json!(&value_str, ());
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
            let petal_id = guest_input!(&mut caller, petal_id_ptr, petal_id_len, -1);
            let filter = guest_input!(&mut caller, filter_ptr, filter_len, -1);
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
            let petal_id = guest_input!(&mut caller, petal_id_ptr, petal_id_len, -1);
            let name = guest_input!(&mut caller, name_ptr, name_len, -1);
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
            let node_id = guest_input!(&mut caller, node_id_ptr, node_id_len, ());
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
            let node_id = guest_input!(&mut caller, node_id_ptr, node_id_len, -1);
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
            let node_id = guest_input!(&mut caller, node_id_ptr, node_id_len, ());
            let key = guest_input!(&mut caller, key_ptr, key_len, ());
            let value_str = guest_input!(&mut caller, value_ptr, value_len, ());
            let value = fe_sdk::plugin_host::json_property(guest_json!(&value_str, ()));
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
            let sql = guest_input!(&mut caller, sql_ptr, sql_len, -1);
            let params_str = guest_input!(&mut caller, params_ptr, params_len, -1);
            let params: std::collections::HashMap<String, serde_json::Value> =
                guest_json!(&params_str, -1);
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
            let key = guest_input!(&mut caller, key_ptr, key_len, -1);
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
            let key = guest_input!(&mut caller, key_ptr, key_len, ());
            let value_str = guest_input!(&mut caller, value_ptr, value_len, ());
            let value = fe_sdk::plugin_host::json_property(guest_json!(&value_str, ()));
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
            let msg = guest_input!(&mut caller, msg_ptr, msg_len, ());
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

/// Decode one bounded UTF-8 guest span without substituting invalid input.
fn read_string_from_memory(
    caller: &mut Caller<'_, PluginStoreData>,
    ptr: i32,
    len: i32,
) -> Result<String, String> {
    if ptr < 0 || len < 0 || len as usize > fe_sdk::plugin_host::MAX_HOST_BYTES {
        return Err("invalid guest memory span".into());
    }
    let memory = caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .ok_or("guest memory export is missing")?;
    decode_guest_span(memory.data(caller.as_context()), ptr as usize, len as usize)
}

fn decode_guest_span(data: &[u8], ptr: usize, len: usize) -> Result<String, String> {
    if len > fe_sdk::plugin_host::MAX_HOST_BYTES {
        return Err("guest input exceeds 65536 bytes".into());
    }
    let end = ptr.checked_add(len).ok_or("guest memory span overflow")?;
    let bytes = data
        .get(ptr..end)
        .ok_or("guest memory span is out of bounds")?;
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| "guest input is not UTF-8".into())
}

/// Write NUL-terminated UTF-8 to the fixed guest response buffer at offset zero.
fn write_string_to_memory(caller: &mut Caller<'_, PluginStoreData>, value: &str) -> i32 {
    let bytes = value.as_bytes();

    let memory = match caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
    {
        Some(memory) => memory,
        None => return -1,
    };

    let data = memory.data_mut(caller.as_context_mut());
    let mem_len = data.len();

    if bytes.len() >= mem_len || bytes.len() > fe_sdk::plugin_host::MAX_HOST_BYTES {
        tracing::warn!(
            "Host import write exceeds memory: bytes={}, mem_size={}",
            bytes.len(),
            mem_len
        );
        return -1;
    }

    data[..bytes.len()].copy_from_slice(bytes);
    data[bytes.len()] = 0;
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
        assert!(decode_guest_span(b"ok", 1, 2).is_err());
        assert!(decode_guest_span(b"ok", usize::MAX, 2).is_err());
        assert!(decode_guest_span(&[255], 0, 1).is_err());
        assert!(decode_guest_span(&vec![0; 65_537], 0, 65_537).is_err());
        assert_eq!(decode_guest_span(b"", 0, 0).unwrap(), "");
        use crate::capability::{CapabilityManifest, CapabilityToken};
        use std::sync::{Arc, Mutex};
        let host = Arc::new(Mutex::new(fe_plugin_test::mock_host::MockHostEnv::new()));
        let storage = Arc::new(fe_plugin_test::mock_storage::MockStorage::new());
        let manifest = CapabilityManifest::from_json(
            r#"{"capabilities":["storage.read","storage.write","query.select"]}"#,
        )
        .unwrap();
        let (commands, _receiver) = crossbeam::channel::bounded(1);
        let ctx = crate::context::PluginContext::new(
            "wasm-input".into(),
            "p1".into(),
            CapabilityToken::mint("wasm-input", &manifest),
            commands,
        )
        .with_node_reader(host.clone())
        .with_transaction_sink(host.clone())
        .with_host_env(
            crate::host_env::HostEnv::new()
                .with_storage(storage.clone())
                .with_query(storage.clone()),
        );
        let imports = [
            ("get_node", 2, true),
            ("query_nodes", 4, true),
            ("create_node", 4, true),
            ("set_property", 6, false),
            ("delete_node", 2, false),
            ("node_get_properties", 2, true),
            ("node_set_property", 6, false),
            ("query_select", 4, true),
            ("ext_storage_get", 2, true),
            ("ext_storage_set", 4, false),
            ("log_debug", 2, false),
            ("log_info", 2, false),
            ("log_warn", 2, false),
            ("log_error", 2, false),
        ];
        let mut wat = String::from("(module ");
        for (name, arity, returns) in imports {
            wat.push_str(&format!("(import \"fe\" \"{name}\" (func ${name} (param {}) {})) (export \"{name}\" (func ${name})) ", "i32 ".repeat(arity), if returns {"(result i32)"} else {""}));
        }
        wat.push_str("(memory (export \"memory\") 1))");
        let plugin = engine
            .load_module(&wat::parse_str(&wat).unwrap(), "wasm-input")
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (mut store, instance) = runtime
            .block_on(engine.instantiate(
                &plugin,
                ctx,
                crate::wasm::fuel::FuelBudget::new(100_000, 100_000, 100),
            ))
            .unwrap();
        let memory = instance.get_memory(&mut store, "memory").unwrap();
        memory.write(&mut store, 64, b"p1").unwrap();
        memory.write(&mut store, 80, b"key").unwrap();
        memory.write(&mut store, 96, b"1").unwrap();
        memory.write(&mut store, 128, &[255]).unwrap();
        for (name, arity, returns) in imports {
            for slot in 0..arity / 2 {
                for (ptr, len) in [(-1, 1), (65_535, 2), (0, 65_537), (128, 1), (64, -1)] {
                    let mut args: Vec<_> = (0..arity / 2)
                        .flat_map(|index| {
                            [
                                wasmtime::Val::I32([64, 80, 96][index]),
                                wasmtime::Val::I32([2, 3, 1][index]),
                            ]
                        })
                        .collect();
                    args[slot * 2] = wasmtime::Val::I32(ptr);
                    args[slot * 2 + 1] = wasmtime::Val::I32(len);
                    let mut results = if returns {
                        vec![wasmtime::Val::I32(0)]
                    } else {
                        vec![]
                    };
                    runtime
                        .block_on(engine.call_export(
                            &mut store,
                            &instance,
                            name,
                            &args,
                            &mut results,
                        ))
                        .unwrap();
                    if returns {
                        assert_eq!(
                            results[0].i32(),
                            Some(-1),
                            "{name} must refuse invalid span {slot}"
                        );
                    }
                }
            }
        }
        memory.write(&mut store, 96, b"{").unwrap();
        for (name, args, returns) in [
            ("set_property", vec![64, 2, 80, 3, 96, 1], false),
            ("node_set_property", vec![64, 2, 80, 3, 96, 1], false),
            ("ext_storage_set", vec![80, 3, 96, 1], false),
            ("query_select", vec![64, 2, 96, 1], true),
            ("query_nodes", vec![64, 2, 96, 1], true),
        ] {
            let args: Vec<_> = args.into_iter().map(wasmtime::Val::I32).collect();
            let mut results = if returns {
                vec![wasmtime::Val::I32(0)]
            } else {
                vec![]
            };
            runtime
                .block_on(engine.call_export(&mut store, &instance, name, &args, &mut results))
                .unwrap();
            if returns {
                assert_eq!(results[0].i32(), Some(-1));
            }
        }
        assert!(host.lock().unwrap().transactions.is_empty());
        assert!(storage.property_sets().is_empty());
        assert!(storage.kv_sets().is_empty());
        assert!(storage.queries().is_empty());
    }
}
