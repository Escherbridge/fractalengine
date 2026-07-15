//! Fuel metering for Wasm plugin execution.
//!
//! Each Wasm plugin instance gets a [`FuelBudget`] that controls how much
//! computation it can perform per tick. When fuel is exhausted, the
//! plugin is marked as [`Degraded`](crate::registry::PluginState::Degraded).
//!
//! Wasmtime's fuel feature is enabled at the engine level. Each store
//! starts with `initial` fuel and is refilled with `per_tick` before
//! each tick. If fuel drops below `warning_threshold`, a warning is logged.

/// Fuel budget configuration for a Wasm plugin instance.
#[derive(Debug, Clone, Copy)]
pub struct FuelBudget {
    /// Initial fuel granted on first instantiation.
    pub initial: u64,
    /// Fuel replenished before each tick call.
    pub per_tick: u64,
    /// Log a warning when remaining fuel drops below this threshold.
    pub warning_threshold: u64,
}

impl FuelBudget {
    /// Create a new fuel budget.
    ///
    /// # Arguments
    /// * `initial` — Fuel on first instantiation. Must be > 0.
    /// * `per_tick` — Fuel refilled each tick. Must be > 0.
    /// * `warning_threshold` — Log warning below this level.
    pub fn new(initial: u64, per_tick: u64, warning_threshold: u64) -> Self {
        Self {
            initial,
            per_tick,
            warning_threshold,
        }
    }

    /// Default budget suitable for most plugins.
    pub fn default_budget() -> Self {
        Self {
            initial: 10_000,
            per_tick: 5_000,
            warning_threshold: 500,
        }
    }

    /// Check if current fuel is below the warning threshold.
    pub fn is_below_threshold(&self, current_fuel: u64) -> bool {
        current_fuel < self.warning_threshold
    }
}

impl Default for FuelBudget {
    fn default() -> Self {
        Self::default_budget()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuel_budget_creation() {
        let budget = FuelBudget::new(1000, 500, 100);
        assert_eq!(budget.initial, 1000);
        assert_eq!(budget.per_tick, 500);
        assert_eq!(budget.warning_threshold, 100);
    }

    #[test]
    fn fuel_budget_default() {
        let budget = FuelBudget::default();
        assert_eq!(budget.initial, 10_000);
        assert_eq!(budget.per_tick, 5_000);
        assert_eq!(budget.warning_threshold, 500);
    }

    #[test]
    fn fuel_budget_threshold_check() {
        let budget = FuelBudget::new(1000, 500, 100);
        assert!(budget.is_below_threshold(50));
        assert!(budget.is_below_threshold(99));
        assert!(!budget.is_below_threshold(100));
        assert!(!budget.is_below_threshold(200));
    }

    #[test]
    fn fuel_budget_clone_copy() {
        let budget = FuelBudget::new(100, 50, 10);
        let cloned = budget;
        let copied = budget;
        assert_eq!(cloned.initial, copied.initial);
    }

    #[test]
    fn fuel_metering_in_store() {
        use crate::capability::{CapabilityManifest, CapabilityToken};
        use crate::context::PluginContext;
        use crate::wasm::WasmEngine;

        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let engine = WasmEngine::new().unwrap();

            // Simple module that does nothing
            let wat = r#"(module (func (export "noop")))"#;
            let bytes = wat::parse_str(wat).unwrap();
            let plugin = engine.load_module(&bytes, "fuel-test").unwrap();

            let manifest = CapabilityManifest::from_json(
                r#"{"property_scope": ["*"], "verse_scope": ["*"], "petal_scope": ["*"]}"#,
            )
            .unwrap();
            let token = CapabilityToken::mint("fuel-plugin", &manifest);
            let (tx, _rx) = crossbeam::channel::unbounded();
            let ctx = PluginContext::new("fuel-plugin".into(), "test-petal".into(), token, tx);

            let budget = FuelBudget::new(1000, 500, 100);
            let (mut store, _instance) = engine.instantiate(&plugin, ctx, budget).await.unwrap();

            // Check initial fuel
            let fuel = store.get_fuel().unwrap();
            assert_eq!(fuel, 1000);

            // Consume fuel
            let mut fuel = store.get_fuel().unwrap();
            fuel -= 300;
            store.set_fuel(fuel).unwrap();
            assert_eq!(fuel, 700);

            // Below threshold check
            let budget = store.data().fuel_budget;
            assert!(!budget.is_below_threshold(fuel));

            // Consume more to go below threshold
            fuel -= 650;
            store.set_fuel(fuel).unwrap();
            assert_eq!(fuel, 50);
            assert!(budget.is_below_threshold(fuel));
        });
    }

    #[test]
    fn fuel_exhaustion_traps() {
        use crate::capability::{CapabilityManifest, CapabilityToken};
        use crate::context::PluginContext;
        use crate::wasm::WasmEngine;

        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
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
            let plugin = engine.load_module(&bytes, "burn-test").unwrap();

            let manifest = CapabilityManifest::from_json(r#"{"property_scope": ["*"]}"#).unwrap();
            let token = CapabilityToken::mint("burn-plugin", &manifest);
            let (tx, _rx) = crossbeam::channel::unbounded();
            let ctx = PluginContext::new("burn-plugin".into(), "test-petal".into(), token, tx);

            // Very small fuel budget: loop will exhaust it
            let budget = FuelBudget::new(10, 5, 1);
            let (mut store, instance) = engine.instantiate(&plugin, ctx, budget).await.unwrap();

            // Call the burn function — it will exhaust the 10 fuel
            let result = engine
                .call_export(&mut store, &instance, "burn", &[], &mut [])
                .await;

            assert!(
                result.is_err(),
                "Expected fuel exhaustion error, got: {:?}",
                result
            );
            assert!(
                WasmEngine::is_degraded(&store),
                "Plugin should be marked degraded after fuel exhaustion"
            );
        });
    }
}
