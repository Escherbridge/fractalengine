//! Wasmtime embedding engine for FractalEngine plugins.
//!
//! Provides a [`WasmEngine`] that wraps wasmtime with pooling allocation,
//! fuel metering, and host imports matching the Rhai API surface.
//!
//! # Sub-modules
//!
//! - [`host_imports`] — Host functions registered into the Wasmtime [`Linker`](wasmtime::Linker).
//! - [`fuel`] — [`FuelBudget`] for per-call fuel accounting.
//! - [`aot`] — [`AotCache`] for ahead-of-time precompilation with BLAKE3 hashing.

pub mod aot;
pub mod fuel;
pub mod host_imports;

use crate::context::PluginContext;

// ---------------------------------------------------------------------------
// WasmPlugin — a loaded WebAssembly module ready for instantiation
// ---------------------------------------------------------------------------

/// A compiled WebAssembly module loaded into the [`WasmEngine`].
///
/// Holds the [`wasmtime::Module`] handle and a unique plugin identifier.
/// Can be instantiated into a [`wasmtime::Store`] with host imports
/// and a [`FuelBudget`](fuel::FuelBudget).
pub struct WasmPlugin {
    /// Unique plugin identifier.
    pub plugin_id: String,
    /// The compiled wasmtime module.
    pub module: wasmtime::Module,
    /// Raw WASM bytes (kept for re-compilation / AOT cache).
    pub wasm_bytes: Vec<u8>,
}

// ---------------------------------------------------------------------------
// WasmEngine — the wasmtime host environment
// ---------------------------------------------------------------------------

/// The Wasmtime embedding engine.
///
/// Wraps a [`wasmtime::Engine`] configured with:
/// - [`wasmtime::InstanceAllocationStrategy::Pooling`] for fast instantiation.
/// - Fuel metering enabled for per-call resource accounting.
/// - Async support for host function interop.
///
/// The engine also owns a [`wasmtime::Linker`] pre-populated with all
/// host import functions that mirror the Rhai API surface.
pub struct WasmEngine {
    /// The global wasmtime compilation + runtime environment.
    engine: wasmtime::Engine,
    /// Pre-configured linker with all host imports registered.
    linker: wasmtime::Linker<PluginStoreData>,
}

impl WasmEngine {
    /// Create a new Wasmtime engine with pooling allocation and fuel metering.
    pub fn new() -> Result<Self, WasmError> {
        let mut config = wasmtime::Config::new();
        config.async_support(true);
        config.consume_fuel(true);

        // Configure pooling allocation strategy
        let mut pooling_config = wasmtime::PoolingAllocationConfig::default();
        pooling_config
            .total_memories(64)
            .max_memory_size(16 * 1024 * 1024) // 16 MiB per instance
            .max_memories_per_module(1)
            .max_tables_per_module(4)
            .total_tables(256)
            .table_elements(65_536);
        config
            .allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(
                pooling_config,
            ));

        let engine = wasmtime::Engine::new(&config)?;

        // Build the linker with host imports
        let mut linker: wasmtime::Linker<PluginStoreData> =
            wasmtime::Linker::new(&engine);
        host_imports::register_host_imports(&mut linker)?;

        Ok(Self { engine, linker })
    }

    /// Get a reference to the underlying wasmtime engine.
    pub fn engine(&self) -> &wasmtime::Engine {
        &self.engine
    }

    /// Load WebAssembly bytes into a compiled [`WasmPlugin`].
    ///
    /// The bytes are compiled into a [`wasmtime::Module`] and wrapped
    /// with a generated plugin ID (from the provided ULID string).
    pub fn load_module(
        &self,
        wasm_bytes: &[u8],
        plugin_id: &str,
    ) -> Result<WasmPlugin, WasmError> {
        let module = wasmtime::Module::new(&self.engine, wasm_bytes)?;
        Ok(WasmPlugin {
            plugin_id: plugin_id.to_string(),
            module,
            wasm_bytes: wasm_bytes.to_vec(),
        })
    }

    /// Pre-compile WebAssembly bytes to a portable `.cwasm` binary.
    ///
    /// The output can be deserialized later with [`WasmEngine::load_module`]
    /// on a compatible engine (same CPU architecture and wasmtime version).
    pub fn precompile(&self, wasm_bytes: &[u8]) -> Result<Vec<u8>, WasmError> {
        let module = wasmtime::Module::new(&self.engine, wasm_bytes)?;
        module
            .serialize()
            .map_err(|e| WasmError::PrecompileError(e.to_string()))
    }

    /// Instantiate a plugin module in a new async store with the given
    /// context and fuel budget.
    ///
    /// Returns the instantiated [`wasmtime::Instance`] which can be used
    /// to call exported functions.
    pub async fn instantiate(
        &self,
        plugin: &WasmPlugin,
        ctx: PluginContext,
        fuel_budget: fuel::FuelBudget,
    ) -> Result<(wasmtime::Store<PluginStoreData>, wasmtime::Instance), WasmError>
    {
        let store_data = PluginStoreData {
            ctx,
            fuel_budget,
            degraded: false,
        };

        let mut store = wasmtime::Store::new(&self.engine, store_data);

        // Set initial fuel
        let initial_fuel = store.data().fuel_budget.initial;
        store.set_fuel(initial_fuel)?;

        let instance = self
            .linker
            .instantiate_async(&mut store, &plugin.module)
            .await?;

        Ok((store, instance))
    }

    /// Call an exported function on an instance by name.
    ///
    /// Automatically checks fuel and marks the plugin as degraded if
    /// fuel is exhausted during the call.
    pub async fn call_export(
        &self,
        store: &mut wasmtime::Store<PluginStoreData>,
        instance: &wasmtime::Instance,
        func_name: &str,
        args: &[wasmtime::Val],
        results: &mut [wasmtime::Val],
    ) -> Result<(), WasmError> {
        let func = instance
            .get_func(&mut *store, func_name)
            .ok_or_else(|| WasmError::MissingExport(func_name.to_string()))?;

        let call_result = func.call_async(&mut *store, args, results).await;
        match call_result {
            Ok(()) => Ok(()),
            Err(e) => {
                // Check if it's an out-of-fuel trap by checking remaining fuel
                let remaining_fuel = store.get_fuel().unwrap_or(0);
                let is_fuel_error = remaining_fuel == 0
                    || e.to_string().contains("all fuel consumed")
                    || e.to_string().contains("out of fuel")
                    || e.to_string().to_lowercase().contains("fuel");
                if is_fuel_error {
                    store.data_mut().degraded = true;
                    Err(WasmError::FuelExhausted)
                } else {
                    Err(WasmError::RuntimeError(e.to_string()))
                }
            }
        }
    }

    /// Refuel a store for the next tick based on its budget.
    pub fn refuel_for_tick(
        store: &mut wasmtime::Store<PluginStoreData>,
    ) -> Result<(), WasmError> {
        let budget = store.data().fuel_budget.per_tick;
        store.set_fuel(budget)?;
        Ok(())
    }

    /// Check if a plugin's store is in degraded state (fuel exhausted).
    pub fn is_degraded(store: &wasmtime::Store<PluginStoreData>) -> bool {
        store.data().degraded
    }
}

impl Default for WasmEngine {
    fn default() -> Self {
        Self::new().expect("Failed to create default WasmEngine")
    }
}

// ---------------------------------------------------------------------------
// PluginStoreData — per-instance host state stored in wasmtime::Store
// ---------------------------------------------------------------------------

/// Host data stored inside each [`wasmtime::Store`].
///
/// Carries the plugin context for host imports and the fuel budget
/// for metering.
pub struct PluginStoreData {
    /// Plugin execution context (capabilities, command channel).
    pub ctx: PluginContext,
    /// Fuel budget for this instance.
    pub fuel_budget: fuel::FuelBudget,
    /// Whether the plugin has been marked degraded.
    pub degraded: bool,
}

impl std::fmt::Debug for PluginStoreData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginStoreData")
            .field("fuel_budget", &self.fuel_budget)
            .field("degraded", &self.degraded)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by the Wasm engine.
#[derive(Debug, thiserror::Error)]
pub enum WasmError {
    #[error("Wasmtime error: {0}")]
    Wasmtime(#[from] anyhow::Error),

    #[error("Missing export: '{0}'")]
    MissingExport(String),

    #[error("Fuel exhausted — plugin marked degraded")]
    FuelExhausted,

    #[error("Runtime error: {0}")]
    RuntimeError(String),

    #[error("Precompile error: {0}")]
    PrecompileError(String),

    #[error("AOT cache error: {0}")]
    AotCacheError(String),

    #[error("Linker registration error: {0}")]
    LinkerError(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityManifest, CapabilityToken};

    fn make_test_context() -> PluginContext {
        let manifest = CapabilityManifest::from_json(r#"{"property_scope": ["*"], "verse_scope": ["*"], "petal_scope": ["*"]}"#).unwrap();
        let token = CapabilityToken::mint("test-plugin", &manifest);
        let (tx, _rx) = crossbeam::channel::unbounded();
        PluginContext::new("test-plugin".into(), "test-petal".into(), token, tx)
    }

    #[test]
    fn engine_creation_default() {
        let engine = WasmEngine::new();
        assert!(engine.is_ok(), "WasmEngine creation should succeed");
    }

    #[test]
    fn load_module_from_valid_wat() {
        let engine = WasmEngine::new().unwrap();

        // Minimal valid WASM (empty module)
        let wat = r#"(module)"#;
        let bytes = wat::parse_str(wat).unwrap();

        let plugin = engine.load_module(&bytes, "test-mod");
        assert!(plugin.is_ok());
        assert_eq!(plugin.unwrap().plugin_id, "test-mod");
    }

    #[test]
    fn load_module_from_invalid_bytes_fails() {
        let engine = WasmEngine::new().unwrap();
        let result = engine.load_module(b"not valid wasm", "bad");
        assert!(result.is_err());
    }

    #[test]
    fn precompile_produces_bytes() {
        let engine = WasmEngine::new().unwrap();

        let wat = r#"(module (func (export "dummy")))"#;
        let bytes = wat::parse_str(wat).unwrap();

        let compiled = engine.precompile(&bytes);
        assert!(compiled.is_ok());
        assert!(!compiled.unwrap().is_empty());
    }

    #[test]
    fn fuel_budget_refill() {
        use crate::wasm::fuel::FuelBudget;

        let engine = WasmEngine::new().unwrap();
        let wat = r#"(module (func (export "dummy")))"#;
        let bytes = wat::parse_str(wat).unwrap();
        let plugin = engine.load_module(&bytes, "fuel-test").unwrap();

        let ctx = make_test_context();
        let budget = FuelBudget::new(1000, 500, 100);

        // We need a tokio runtime for async
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (mut store, _instance) = rt
            .block_on(engine.instantiate(&plugin, ctx, budget))
            .unwrap();

        // Consume some fuel
        let mut fuel = store.get_fuel().unwrap();
        fuel -= 200;
        store.set_fuel(fuel).unwrap();
        assert_eq!(fuel, 800);

        // Refuel for next tick
        WasmEngine::refuel_for_tick(&mut store).unwrap();

        // Fuel should be back to per_tick amount
        let current = store.get_fuel().unwrap();
        assert_eq!(current, 500);
    }

    #[test]
    fn fuel_exhaustion_marks_degraded() {
        use crate::wasm::fuel::FuelBudget;

        let engine = WasmEngine::new().unwrap();

        // Module with a loop that will consume fuel
        let wat = r#"
            (module
                (func (export "burn")
                    (local $i i32)
                    (loop $loop
                        local.get $i
                        i32.const 1
                        i32.add
                        local.tee $i
                        i32.const 50
                        i32.lt_s
                        br_if $loop
                    )
                )
            )
        "#;
        let bytes = wat::parse_str(wat).unwrap();
        let plugin = engine.load_module(&bytes, "fuel-exhaust").unwrap();

        let ctx = make_test_context();
        // Very small budget: 10 fuel, loop will exhaust it
        let budget = FuelBudget::new(10, 10, 5);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let (mut store, instance) = rt
            .block_on(engine.instantiate(&plugin, ctx, budget))
            .unwrap();

        // Call the burn function — it will exhaust the 10 fuel
        let result = rt.block_on(engine.call_export(
            &mut store,
            &instance,
            "burn",
            &[],
            &mut [],
        ));

        // Should fail with fuel exhausted
        assert!(
            result.is_err(),
            "Fuel exhaustion should produce an error, got: {:?}",
            result
        );
        assert!(WasmEngine::is_degraded(&store), "Plugin should be marked degraded");
    }

    #[test]
    fn store_data_debug() {
        use crate::wasm::fuel::FuelBudget;

        let ctx = make_test_context();
        let budget = FuelBudget::new(1000, 500, 100);
        let data = PluginStoreData {
            ctx,
            fuel_budget: budget,
            degraded: false,
        };
        let debug_str = format!("{:?}", data);
        assert!(debug_str.contains("PluginStoreData"));
    }
}
