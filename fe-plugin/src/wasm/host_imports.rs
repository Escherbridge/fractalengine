//! Host functions for Wasm plugins, matching the Rhai API surface.
//!
//! These functions are registered into a [`wasmtime::Linker`] and are
//! called by Wasm guest modules. They bridge back to the Bevy ECS
//! through the [`PluginContext`] stored in the [`Store`](wasmtime::Store)'s data.

use wasmtime::{AsContext, AsContextMut, Caller, Linker};

use crate::LogLevel;
use crate::PluginCommand;
use crate::wasm::PluginStoreData;

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
            let plugin_id = caller.data().ctx.plugin_id().to_string();
            let tx = caller.data().ctx.tx.clone();

            let _ = tx.send(PluginCommand::GetNode {
                plugin_id: plugin_id.clone(),
                node_id: node_id.clone(),
            });

            // For now, return a placeholder JSON object. In a full implementation
            // this would use a shared memory buffer with request-reply.
            let result = serde_json::json!({
                "node_id": node_id,
                "name": "",
                "properties": {}
            });
            write_string_to_memory(&mut caller, &result.to_string())
        },
    )?;

    // ---- fe::set_property(node_id_ptr, node_id_len, key_ptr, key_len, value_ptr, value_len) ----
    linker.func_wrap(
        namespace,
        "set_property",
        |mut caller: Caller<'_, PluginStoreData>,
         node_id_ptr: i32, node_id_len: i32,
         key_ptr: i32, key_len: i32,
         value_ptr: i32, value_len: i32| {
            let node_id = read_string_from_memory(&mut caller, node_id_ptr, node_id_len);
            let key = read_string_from_memory(&mut caller, key_ptr, key_len);
            let value_str = read_string_from_memory(&mut caller, value_ptr, value_len);
            let plugin_id = caller.data().ctx.plugin_id().to_string();
            let tx = caller.data().ctx.tx.clone();

            let value: serde_json::Value = serde_json::from_str(&value_str)
                .unwrap_or(serde_json::Value::String(value_str));

            let _ = tx.send(PluginCommand::SetProperty {
                plugin_id,
                node_id,
                key,
                value,
            });
        },
    )?;

    // ---- fe::query_nodes(petal_id_ptr, petal_id_len, filter_ptr, filter_len) -> i32 ----
    linker.func_wrap(
        namespace,
        "query_nodes",
        |mut caller: Caller<'_, PluginStoreData>,
         petal_id_ptr: i32, petal_id_len: i32,
         filter_ptr: i32, filter_len: i32| -> i32 {
            let petal_id = read_string_from_memory(&mut caller, petal_id_ptr, petal_id_len);
            let _filter = read_string_from_memory(&mut caller, filter_ptr, filter_len);
            let plugin_id = caller.data().ctx.plugin_id().to_string();
            let tx = caller.data().ctx.tx.clone();

            let _ = tx.send(PluginCommand::QueryNodes {
                plugin_id,
                petal_id,
            });

            // Return empty JSON array for now
            let result: serde_json::Value = serde_json::json!([]);
            write_string_to_memory(&mut caller, &result.to_string())
        },
    )?;

    // ---- fe::create_node(petal_id_ptr, petal_id_len, name_ptr, name_len) -> i32 ----
    linker.func_wrap(
        namespace,
        "create_node",
        |mut caller: Caller<'_, PluginStoreData>,
         petal_id_ptr: i32, petal_id_len: i32,
         name_ptr: i32, name_len: i32| -> i32 {
            let petal_id = read_string_from_memory(&mut caller, petal_id_ptr, petal_id_len);
            let name = read_string_from_memory(&mut caller, name_ptr, name_len);
            let plugin_id = caller.data().ctx.plugin_id().to_string();
            let tx = caller.data().ctx.tx.clone();

            let node_id = ulid::Ulid::new().to_string();

            let _ = tx.send(PluginCommand::CreateNode {
                plugin_id,
                petal_id,
                name,
                provisional_id: node_id.clone(),
            });

            write_string_to_memory(&mut caller, &node_id)
        },
    )?;

    // ---- fe::delete_node(node_id_ptr, node_id_len) ----
    linker.func_wrap(
        namespace,
        "delete_node",
        |mut caller: Caller<'_, PluginStoreData>, node_id_ptr: i32, node_id_len: i32| {
            let node_id = read_string_from_memory(&mut caller, node_id_ptr, node_id_len);
            let plugin_id = caller.data().ctx.plugin_id().to_string();
            let tx = caller.data().ctx.tx.clone();

            // Use a transaction to send the delete op
            let _ = tx.send(PluginCommand::CommitTransaction {
                plugin_id,
                ops: vec![crate::transaction::PendingOp::DeleteNode { node_id }],
            });
        },
    )?;

    // ---- Logging functions ----
    register_log_func(linker, namespace, "log_debug", LogLevel::Debug)?;
    register_log_func(linker, namespace, "log_info", LogLevel::Info)?;
    register_log_func(linker, namespace, "log_warn", LogLevel::Warn)?;
    register_log_func(linker, namespace, "log_error", LogLevel::Error)?;

    Ok(())
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
            let tx = caller.data().ctx.tx.clone();

            match level {
                LogLevel::Debug => tracing::debug!(plugin = %plugin_id, "{}", msg),
                LogLevel::Info => tracing::info!(plugin = %plugin_id, "{}", msg),
                LogLevel::Warn => tracing::warn!(plugin = %plugin_id, "{}", msg),
                LogLevel::Error => tracing::error!(plugin = %plugin_id, "{}", msg),
            }

            let _ = tx.send(PluginCommand::Log {
                plugin_id,
                level,
                message: msg,
            });
        },
    )?;
    Ok(())
}

/// Read a UTF-8 string from the guest's linear memory.
///
/// Uses the memory export named "memory" (conventional for WASI/wasm modules).
/// If the module doesn't export memory, returns an empty string.
fn read_string_from_memory(
    caller: &mut Caller<'_, PluginStoreData>,
    ptr: i32,
    len: i32,
) -> String {
    if ptr < 0 || len <= 0 {
        return String::new();
    }

    let memory = match caller.get_export("memory") {
        Some(mem) => mem.into_memory().unwrap(),
        None => return String::new(),
    };

    let data = memory.data(caller.as_context());
    let ptr = ptr as usize;
    let len = len as usize;

    if ptr + len > data.len() {
        tracing::warn!("Host import read out of bounds: ptr={ptr}, len={len}, mem_size={}", data.len());
        return String::new();
    }

    String::from_utf8_lossy(&data[ptr..ptr + len]).to_string()
}

/// Write a string into the guest's linear memory and return the pointer.
///
/// This is a simplified approach that writes to a pre-allocated buffer area.
/// In production, this would use a proper allocator or shared buffer protocol.
fn write_string_to_memory(
    caller: &mut Caller<'_, PluginStoreData>,
    value: &str,
) -> i32 {
    let bytes = value.as_bytes();

    let memory = match caller.get_export("memory") {
        Some(mem) => mem.into_memory().unwrap(),
        None => return -1,
    };

    let data = memory.data_mut(caller.as_context_mut());
    let mem_len = data.len();

    if bytes.len() > mem_len {
        tracing::warn!("Host import write exceeds memory: bytes={}, mem_size={}", bytes.len(), mem_len);
        return -1;
    }

    // Write to the beginning of memory (simplified — production would use an allocator)
    let write_len = bytes.len().min(mem_len);
    data[..write_len].copy_from_slice(&bytes[..write_len]);

    // Store the length at a known offset so the guest can read it back
    // For now, just return 0 as the pointer and the guest reads from there
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
