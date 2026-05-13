//! Rhai test runner — execute Rhai scripts against a [`MockHostEnv`].
//!
//! [`RhaiTestRunner`] creates a sandboxed Rhai engine with host functions that
//! read from and write to the mock host instead of a real engine. After
//! evaluation, tests can inspect the mock for mutations and spy recordings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rhai::{Dynamic, Engine, Map};

use fe_sdk::node::NodeSnapshot;
use fe_sdk::property::PropertyValue;
use fe_sdk::scene::SceneChange;

use crate::mock_host::MockHostEnv;

/// A test runner that evaluates Rhai scripts against a mock host environment.
pub struct RhaiTestRunner {
    host: Arc<Mutex<MockHostEnv>>,
    engine: Engine,
}

impl RhaiTestRunner {
    /// Create a new test runner with the given mock host environment.
    ///
    /// Registers all host API functions (`get_node`, `set_property`,
    /// `create_node`, `query_nodes`, `log_*`) that operate on the mock.
    pub fn new(host: MockHostEnv) -> Self {
        let host = Arc::new(Mutex::new(host));
        let mut engine = Engine::new();

        // Apply basic sandbox limits (no eval/import, size limits)
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

        // ---- get_node(id: String) -> Map ----
        {
            let h = host.clone();
            engine.register_fn("get_node", move |id: &str| -> Dynamic {
                let mut host = h.lock().unwrap();
                host.spy.get_node_calls.push(id.to_string());

                if let Some(snapshot) = host.nodes.get(id) {
                    snapshot_to_dynamic(snapshot)
                } else {
                    Dynamic::UNIT
                }
            });
        }

        // ---- set_property(node_id: String, key: String, value: String) ----
        {
            let h = host.clone();
            engine.register_fn(
                "set_property",
                move |node_id: &str, key: &str, value: &str| {
                    let mut host = h.lock().unwrap();
                    host.spy.set_property_calls.push((
                        node_id.to_string(),
                        key.to_string(),
                        value.to_string(),
                    ));
                    host.properties
                        .insert((node_id.to_string(), key.to_string()), value.to_string());
                    host.scene_changes.push(SceneChange::PropertyChanged {
                        node_id: node_id.to_string(),
                        key: key.to_string(),
                        value: PropertyValue::String(value.to_string()),
                    });
                },
            );
        }

        // ---- set_property overload for Dynamic values ----
        {
            let h = host.clone();
            engine.register_fn(
                "set_property",
                move |node_id: &str, key: &str, value: Dynamic| {
                    let value_str = if let Ok(s) = value.clone().into_string() {
                        s
                    } else {
                        format!("{value}")
                    };
                    let mut host = h.lock().unwrap();
                    host.spy.set_property_calls.push((
                        node_id.to_string(),
                        key.to_string(),
                        value_str.clone(),
                    ));
                    host.properties
                        .insert((node_id.to_string(), key.to_string()), value_str.clone());
                    host.scene_changes.push(SceneChange::PropertyChanged {
                        node_id: node_id.to_string(),
                        key: key.to_string(),
                        value: PropertyValue::String(value_str),
                    });
                },
            );
        }

        // ---- create_node(petal_id: String, name: String) -> String ----
        {
            let h = host.clone();
            engine.register_fn("create_node", move |petal_id: &str, name: &str| -> String {
                let node_id = ulid::Ulid::new().to_string();
                let mut host = h.lock().unwrap();
                host.spy
                    .create_node_calls
                    .push((petal_id.to_string(), name.to_string()));

                let snapshot = NodeSnapshot {
                    node_id: node_id.clone(),
                    petal_id: petal_id.to_string(),
                    name: name.to_string(),
                    position: [0.0, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                    properties: HashMap::new(),
                };
                host.nodes.insert(node_id.clone(), snapshot);

                host.scene_changes.push(SceneChange::NodeAdded {
                    node_id: node_id.clone(),
                    petal_id: petal_id.to_string(),
                    name: name.to_string(),
                });

                node_id
            });
        }

        // ---- query_nodes(petal_id: String, filter: String) -> Array ----
        {
            let h = host.clone();
            engine.register_fn(
                "query_nodes",
                move |petal_id: &str, filter: &str| -> rhai::Array {
                    let mut host = h.lock().unwrap();
                    host.spy
                        .query_calls
                        .push((petal_id.to_string(), filter.to_string()));

                    let matching: Vec<Dynamic> = host
                        .nodes
                        .values()
                        .filter(|n| n.petal_id == petal_id)
                        .map(|n| snapshot_to_dynamic(n))
                        .collect();
                    matching
                },
            );
        }

        // ---- Logging functions ----
        for level_name in &["log_debug", "log_info", "log_warn", "log_error"] {
            let h = host.clone();
            let level = level_name.to_string();
            engine.register_fn(*level_name, move |msg: &str| {
                let mut host = h.lock().unwrap();
                host.spy
                    .log_messages
                    .push((level.clone(), msg.to_string()));
            });
        }

        // ---- parse_float(s: String) -> f64 ----
        engine.register_fn("parse_float", |s: &str| -> f64 {
            s.parse::<f64>().unwrap_or(0.0)
        });

        // ---- parse_int(s: String) -> i64 ----
        engine.register_fn("parse_int", |s: &str| -> i64 {
            s.parse::<i64>().unwrap_or(0)
        });

        Self { host, engine }
    }

    /// Evaluate a Rhai script string against the mock host.
    ///
    /// Returns `Ok(())` on success or `Err(message)` on script error.
    pub fn eval_script(&self, script: &str) -> Result<(), String> {
        self.engine
            .eval::<Dynamic>(script)
            .map(|_| ())
            .map_err(|e| format!("{e}"))
    }

    /// Get a reference to the mock host (behind Arc<Mutex>).
    ///
    /// Useful for inspecting state after script evaluation.
    pub fn host(&self) -> MockHostEnv {
        self.host.lock().unwrap().clone()
    }

    /// Get direct access to the inner Arc<Mutex<MockHostEnv>>.
    pub fn host_ref(&self) -> &Arc<Mutex<MockHostEnv>> {
        &self.host
    }
}

/// Convert a [`NodeSnapshot`] into a Rhai [`Dynamic`] map.
fn snapshot_to_dynamic(snapshot: &NodeSnapshot) -> Dynamic {
    let mut map = Map::new();
    map.insert("node_id".into(), Dynamic::from(snapshot.node_id.clone()));
    map.insert("petal_id".into(), Dynamic::from(snapshot.petal_id.clone()));
    map.insert("name".into(), Dynamic::from(snapshot.name.clone()));

    // Properties as a nested map
    let mut props = Map::new();
    for (k, v) in &snapshot.properties {
        let dyn_val = match v {
            PropertyValue::String(s) => Dynamic::from(s.clone()),
            PropertyValue::Number(n) => Dynamic::from(*n),
            PropertyValue::Bool(b) => Dynamic::from(*b),
            PropertyValue::Json(j) => Dynamic::from(j.to_string()),
        };
        props.insert(k.clone().into(), dyn_val);
    }
    map.insert("properties".into(), Dynamic::from(props));

    Dynamic::from(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assertions::*;
    use crate::fixtures;

    #[test]
    fn eval_simple_script() {
        let host = MockHostEnv::new();
        let runner = RhaiTestRunner::new(host);

        let result = runner.eval_script("let x = 40 + 2;");
        assert!(result.is_ok(), "Simple script should succeed: {:?}", result);
    }

    #[test]
    fn eval_script_error() {
        let host = MockHostEnv::new();
        let runner = RhaiTestRunner::new(host);

        let result = runner.eval_script("let x = ;");
        assert!(result.is_err(), "Invalid script should fail");
    }

    #[test]
    fn get_node_records_in_spy() {
        let mut host = MockHostEnv::new();
        host.insert_node(
            "n1",
            NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "Test Node".into(),
                position: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                properties: HashMap::new(),
            },
        );

        let runner = RhaiTestRunner::new(host);
        runner
            .eval_script(r#"let node = get_node("n1");"#)
            .unwrap();

        let host = runner.host();
        assert!(host.spy().was_called("get_node"));
        assert_eq!(host.spy().node_reads(), &["n1".to_string()]);
    }

    #[test]
    fn set_property_records_and_stores() {
        let host = MockHostEnv::new();
        let runner = RhaiTestRunner::new(host);

        runner
            .eval_script(r#"set_property("n1", "color", "red");"#)
            .unwrap();

        let host = runner.host();
        assert_property_set(&host, "n1", "color", "red").unwrap();
        assert_eq!(
            host.properties
                .get(&("n1".into(), "color".into())),
            Some(&"red".to_string())
        );
    }

    #[test]
    fn create_node_records_and_adds_to_mock() {
        let host = MockHostEnv::new();
        let runner = RhaiTestRunner::new(host);

        runner
            .eval_script(r#"let id = create_node("petal_01", "New Node");"#)
            .unwrap();

        let host = runner.host();
        assert_nodes_created(&host, 1).unwrap();
        assert_eq!(host.spy().create_node_calls[0].0, "petal_01");
        assert_eq!(host.spy().create_node_calls[0].1, "New Node");
        // The node should exist in the mock
        assert_eq!(host.nodes.len(), 1);
    }

    #[test]
    fn log_functions_record_messages() {
        let host = MockHostEnv::new();
        let runner = RhaiTestRunner::new(host);

        runner
            .eval_script(
                r#"
                log_info("starting process");
                log_warn("something fishy");
                log_debug("debug detail");
                log_error("critical failure");
            "#,
            )
            .unwrap();

        let host = runner.host();
        assert_logged(&host, "log_info", "starting").unwrap();
        assert_logged(&host, "log_warn", "fishy").unwrap();
        assert_logged(&host, "log_debug", "debug").unwrap();
        assert_logged(&host, "log_error", "critical").unwrap();
        assert_eq!(host.spy().log_messages.len(), 4);
    }

    #[test]
    fn query_nodes_returns_matching_petal() {
        let fixture = fixtures::sensor_network();
        let host = fixture.to_mock_host();
        let runner = RhaiTestRunner::new(host);

        runner
            .eval_script(r#"let nodes = query_nodes("sensor_petal", "all");"#)
            .unwrap();

        let host = runner.host();
        assert!(host.spy().was_called("query_nodes"));
        assert_eq!(host.spy().query_calls[0].0, "sensor_petal");
    }

    #[test]
    fn property_transformer_pattern() {
        let mut host = MockHostEnv::new();
        host.insert_node(
            "sensor_01",
            NodeSnapshot {
                node_id: "sensor_01".into(),
                petal_id: "p1".into(),
                name: "Sensor".into(),
                position: [0.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0, 1.0, 1.0],
                properties: HashMap::new(),
            },
        );

        let runner = RhaiTestRunner::new(host);

        // Simulate the property transformer pattern
        runner
            .eval_script(
                r#"
                let celsius = 25.0;
                let fahrenheit = celsius * 9.0 / 5.0 + 32.0;
                set_property("sensor_01", "temperature_f", fahrenheit.to_string());
            "#,
            )
            .unwrap();

        let host = runner.host();
        assert_property_set(&host, "sensor_01", "temperature_f", "77.0").unwrap();
    }

    #[test]
    fn no_writes_assertion_with_read_only_script() {
        let fixture = fixtures::basic_terrain();
        let host = fixture.to_mock_host();
        let runner = RhaiTestRunner::new(host);

        runner
            .eval_script(r#"let node = get_node("terrain_000");"#)
            .unwrap();

        let host = runner.host();
        assert_no_writes(&host).unwrap();
    }
}
