//! Host-fn adapter: implements the HOST-FN CONTRACT (`node_get_properties`,
//! `node_set_property`, `query_select`, `ext_storage_get`/`ext_storage_set`) against a
//! small in-memory mock, with fail-closed capability enforcement from `manifest.json`.
//!
//! INTEGRATION STATUS: fe-plugin-test now hosts the same HOST-FN CONTRACT on
//! `RhaiTestRunner` (backed by `MockStorage`), and the extension's integration test
//! (`tests/bridge_loop.rs`) drives the real `iot_bridge.rhai` script through it — see
//! AGENTS.md "Integration". `IotHostAdapter` is deliberately retained as the *runtime*
//! host because it adds registration-time capability gating (`require`) that
//! `RhaiTestRunner` is capability-agnostic about; that gate is what makes `BridgeLoop`'s
//! fail-closed guarantee (`TickError::CapabilityDenied`) real and testable here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use fe_sdk::property::PropertyValue;
use rhai::{Dynamic, Engine, EvalAltResult, Map, Position, Scope, AST};

use crate::capability::{Capabilities, QUERY_SELECT, STORAGE_READ, STORAGE_WRITE};

/// In-memory state backing the mock host: per-node properties + a flat key/value store.
#[derive(Debug, Default, Clone)]
pub struct HostState {
    /// Properties keyed by node ID, then property key (e.g. `"iot.temperature"`).
    pub node_properties: HashMap<String, HashMap<String, PropertyValue>>,
    /// Extension-scoped key/value storage (`ext_storage_get`/`ext_storage_set`).
    pub ext_storage: HashMap<String, PropertyValue>,
}

impl HostState {
    /// All properties for a node, or an empty map if the node is unknown.
    pub fn properties_for(&self, node_id: &str) -> HashMap<String, PropertyValue> {
        self.node_properties.get(node_id).cloned().unwrap_or_default()
    }
}

/// Errors from a `tick()` invocation, classified so the caller knows whether to fail
/// closed (propagate) or treat it as a soft, skippable data-validation fault.
#[derive(Debug, Clone)]
pub enum TickError {
    /// A capability check failed inside a host function — a hard, fail-closed fault.
    CapabilityDenied { capability: String, operation: String },
    /// The script itself rejected the input (e.g. missing reading fields).
    ScriptFault(String),
}

impl std::fmt::Display for TickError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CapabilityDenied { capability, operation } => {
                write!(f, "capability '{capability}' denied for operation '{operation}'")
            }
            Self::ScriptFault(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for TickError {}

/// Rhai engine + shared state implementing the HOST-FN CONTRACT.
pub struct IotHostAdapter {
    state: Arc<Mutex<HostState>>,
    engine: Engine,
}

impl IotHostAdapter {
    /// Build an adapter granted exactly `caps`.
    pub fn new(caps: Capabilities) -> Self {
        let state: Arc<Mutex<HostState>> = Arc::new(Mutex::new(HostState::default()));
        let mut engine = Engine::new();
        configure_sandbox(&mut engine);

        // ---- node_get_properties(node_id: &str) -> Map ----
        {
            let state = state.clone();
            let caps = caps.clone();
            engine.register_fn(
                "node_get_properties",
                move |node_id: &str| -> Result<Map, Box<EvalAltResult>> {
                    require(&caps, STORAGE_READ, "node_get_properties")?;
                    let st = state.lock().unwrap();
                    let mut map = Map::new();
                    for (k, v) in st.properties_for(node_id) {
                        map.insert(k.into(), property_to_dynamic(&v));
                    }
                    Ok(map)
                },
            );
        }

        // ---- node_set_property(node_id: &str, key: &str, value: Dynamic) ----
        {
            let state = state.clone();
            let caps = caps.clone();
            engine.register_fn(
                "node_set_property",
                move |node_id: &str, key: &str, value: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    require(&caps, STORAGE_WRITE, "node_set_property")?;
                    let mut st = state.lock().unwrap();
                    st.node_properties
                        .entry(node_id.to_string())
                        .or_default()
                        .insert(key.to_string(), dynamic_to_property(&value));
                    Ok(())
                },
            );
        }

        // ---- query_select(sql: &str, params: Map) -> Array (SELECT-only) ----
        {
            let caps = caps.clone();
            engine.register_fn(
                "query_select",
                move |sql: &str, _params: Map| -> Result<rhai::Array, Box<EvalAltResult>> {
                    require(&caps, QUERY_SELECT, "query_select")?;
                    if !sql.trim_start().to_ascii_uppercase().starts_with("SELECT") {
                        return Err("query_select: only SELECT statements are allowed".into());
                    }
                    // No real query backend in this mock — always an empty result set.
                    Ok(rhai::Array::new())
                },
            );
        }

        // ---- ext_storage_get(key: &str) -> Dynamic ----
        {
            let state = state.clone();
            let caps = caps.clone();
            engine.register_fn(
                "ext_storage_get",
                move |key: &str| -> Result<Dynamic, Box<EvalAltResult>> {
                    require(&caps, STORAGE_READ, "ext_storage_get")?;
                    let st = state.lock().unwrap();
                    Ok(st.ext_storage.get(key).map(property_to_dynamic).unwrap_or(Dynamic::UNIT))
                },
            );
        }

        // ---- ext_storage_set(key: &str, value: Dynamic) ----
        {
            let state = state.clone();
            let caps = caps.clone();
            engine.register_fn(
                "ext_storage_set",
                move |key: &str, value: Dynamic| -> Result<(), Box<EvalAltResult>> {
                    require(&caps, STORAGE_WRITE, "ext_storage_set")?;
                    let mut st = state.lock().unwrap();
                    st.ext_storage.insert(key.to_string(), dynamic_to_property(&value));
                    Ok(())
                },
            );
        }

        Self { state, engine }
    }

    /// Compile Rhai source into a reusable [`AST`].
    pub fn compile(&self, source: &str) -> Result<AST, String> {
        self.engine.compile(source).map_err(|e| e.to_string())
    }

    /// Call the script's `tick(node_id, reading)` entry point.
    pub fn call_tick(&self, ast: &AST, node_id: &str, reading: Map) -> Result<Map, TickError> {
        let mut scope = Scope::new();
        self.engine
            .call_fn::<Map>(&mut scope, ast, "tick", (node_id.to_string(), reading))
            .map_err(|err| TickError::classify(*err))
    }

    /// Seed a node property directly, bypassing the script/capability boundary — for
    /// test setup that simulates an external actor (e.g. an operator UI) writing state.
    pub fn seed_property(&self, node_id: &str, key: &str, value: PropertyValue) {
        self.state
            .lock()
            .unwrap()
            .node_properties
            .entry(node_id.to_string())
            .or_default()
            .insert(key.to_string(), value);
    }

    /// Snapshot of the current mock state (for test assertions).
    pub fn state(&self) -> HostState {
        self.state.lock().unwrap().clone()
    }
}

impl TickError {
    fn classify(err: EvalAltResult) -> Self {
        if let EvalAltResult::ErrorRuntime(ref d, ..) = err {
            if d.is_map() {
                let map = d.clone().cast::<Map>();
                let kind = map.get("kind").and_then(|v| v.clone().into_string().ok());
                if kind.as_deref() == Some("capability_denied") {
                    let capability =
                        map.get("capability").and_then(|v| v.clone().into_string().ok()).unwrap_or_default();
                    let operation =
                        map.get("operation").and_then(|v| v.clone().into_string().ok()).unwrap_or_default();
                    return Self::CapabilityDenied { capability, operation };
                }
            }
        }
        Self::ScriptFault(err.to_string())
    }
}

/// Check a capability grant, returning a structured (map-tagged) fail-closed error if absent.
fn require(caps: &Capabilities, capability: &str, operation: &str) -> Result<(), Box<EvalAltResult>> {
    if caps.has(capability) {
        return Ok(());
    }
    let mut denial = Map::new();
    denial.insert("kind".into(), Dynamic::from("capability_denied".to_string()));
    denial.insert("capability".into(), Dynamic::from(capability.to_string()));
    denial.insert("operation".into(), Dynamic::from(operation.to_string()));
    Err(Box::new(EvalAltResult::ErrorRuntime(Dynamic::from(denial), Position::NONE)))
}

/// Sandbox limits mirroring fe-plugin's `rhai::sandbox` configuration.
fn configure_sandbox(engine: &mut Engine) {
    engine.disable_symbol("eval");
    engine.disable_symbol("import");
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(10_000);
    engine.set_max_string_size(65_536);
    engine.on_progress(|ops| {
        if ops >= 1_000_000 {
            Some(Dynamic::from("Script exceeded operation limit".to_string()))
        } else {
            None
        }
    });
}

/// Convert a Rhai [`Dynamic`] into an [`fe_sdk::property::PropertyValue`].
fn dynamic_to_property(value: &Dynamic) -> PropertyValue {
    if let Ok(b) = value.as_bool() {
        PropertyValue::Bool(b)
    } else if let Ok(i) = value.as_int() {
        PropertyValue::Number(i as f64)
    } else if let Ok(f) = value.as_float() {
        PropertyValue::Number(f)
    } else if let Ok(s) = value.clone().into_string() {
        PropertyValue::String(s)
    } else {
        PropertyValue::String(value.to_string())
    }
}

/// Convert an [`fe_sdk::property::PropertyValue`] into a Rhai [`Dynamic`].
fn property_to_dynamic(value: &PropertyValue) -> Dynamic {
    match value {
        PropertyValue::String(s) => Dynamic::from(s.clone()),
        PropertyValue::Number(n) => Dynamic::from(*n),
        PropertyValue::Bool(b) => Dynamic::from(*b),
        PropertyValue::Json(j) => Dynamic::from(j.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_allows_granted_capability() {
        let caps = Capabilities::full();
        assert!(require(&caps, STORAGE_READ, "node_get_properties").is_ok());
    }

    #[test]
    fn require_denies_missing_capability() {
        let caps = Capabilities::empty();
        let err = require(&caps, STORAGE_WRITE, "node_set_property").unwrap_err();
        let classified = TickError::classify(*err);
        assert!(matches!(classified, TickError::CapabilityDenied { .. }));
    }

    #[test]
    fn node_set_property_then_get_roundtrips() {
        let adapter = IotHostAdapter::new(Capabilities::full());
        let ast = adapter
            .compile(r#"fn tick(node_id, reading) { node_set_property(node_id, "iot.temperature", 21.5); node_get_properties(node_id) }"#)
            .unwrap();
        let result = adapter.call_tick(&ast, "n1", Map::new()).unwrap();
        assert_eq!(result.get("iot.temperature").and_then(|d| d.as_float().ok()), Some(21.5));
    }

    #[test]
    fn missing_capability_surfaces_as_capability_denied() {
        let adapter = IotHostAdapter::new(Capabilities::empty());
        let ast = adapter
            .compile(r#"fn tick(node_id, reading) { node_set_property(node_id, "iot.temperature", 21.5) }"#)
            .unwrap();
        let err = adapter.call_tick(&ast, "n1", Map::new()).unwrap_err();
        assert!(matches!(err, TickError::CapabilityDenied { .. }));
    }
}
