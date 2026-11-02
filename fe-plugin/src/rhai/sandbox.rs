//! Rhai sandbox configuration — safety limits and restricted symbols.
//!
//! Prevents plugin scripts from performing dangerous operations like `eval`
//! or `import`, and enforces instruction-counting via the `on_progress`
//! callback to terminate runaway scripts.

use rhai::{Engine, AST};

/// Default maximum operations before a script is terminated.
pub const DEFAULT_MAX_OPERATIONS: u64 = 1_000_000;

/// Configure safety limits on a Rhai [`Engine`].
///
/// - Disables `eval` and `import` symbols.
/// - Sets array, map, and string size limits.
/// - Installs an `on_progress` callback for instruction counting.
pub fn configure_sandbox(engine: &mut Engine, max_operations: u64) {
    // Disable dangerous symbols
    engine.disable_symbol("eval");
    engine.disable_symbol("import");

    // Size limits
    engine.set_max_array_size(10_000);
    engine.set_max_map_size(10_000);
    engine.set_max_string_size(65_536);

    // Instruction counting via on_progress callback.
    // The callback receives the number of operations executed so far.
    // Returning `Some(token)` terminates the script.
    engine.on_progress(move |ops| {
        if ops >= max_operations {
            Some(rhai::Dynamic::from(format!(
                "Script exceeded operation limit ({max_operations})"
            )))
        } else {
            None
        }
    });
}

/// Load and compile a `.rhai` script from source text.
///
/// The AST is parsed once and can be reused for multiple evaluations.
pub fn compile_script(engine: &Engine, source: &str) -> Result<AST, rhai::ParseError> {
    engine.compile(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infinite_loop_terminated() {
        let mut engine = Engine::new();
        configure_sandbox(&mut engine, 100);

        let result = engine.eval::<()>("loop { }");
        assert!(result.is_err(), "Infinite loop should be terminated");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("exceeded")
                || err_msg.contains("progress")
                || err_msg.contains("terminated")
                || err_msg.contains("limit"),
            "Error should mention operation limit, got: {err_msg}"
        );
    }

    #[test]
    fn eval_disabled() {
        let mut engine = Engine::new();
        configure_sandbox(&mut engine, DEFAULT_MAX_OPERATIONS);

        let result = engine.eval::<()>(r#"eval("let x = 1;")"#);
        assert!(result.is_err(), "eval should be disabled");
    }

    #[test]
    fn normal_script_runs_fine() {
        let mut engine = Engine::new();
        configure_sandbox(&mut engine, DEFAULT_MAX_OPERATIONS);

        let result: i64 = engine.eval("let x = 40; x + 2").unwrap();
        assert_eq!(result, 42);
        assert!(
            crate::rhai::host_api::checked_dynamic_to_json(&rhai::Dynamic::from(f64::NAN)).is_err()
        );
        assert!(
            crate::rhai::host_api::checked_dynamic_to_json(&rhai::Dynamic::from(f64::INFINITY))
                .is_err()
        );

        use crate::capability::{CapabilityManifest, CapabilityToken};
        use crate::scene_host::{PluginContexts, SceneBackendFactory};
        use crate::{PluginCommand, PluginResult};
        use fe_plugin_test::mock_host::MockHostEnv;
        use std::sync::{Arc, Mutex};
        struct MockFactory(Arc<Mutex<MockHostEnv>>);
        impl SceneBackendFactory for MockFactory {
            fn bind(
                &self,
                context: crate::context::PluginContext,
            ) -> Result<crate::context::PluginContext, String> {
                Ok(context
                    .with_node_reader(self.0.clone())
                    .with_transaction_sink(self.0.clone()))
            }
        }
        let mut host = MockHostEnv::new();
        host.insert_node(
            "n1",
            fe_sdk::NodeSnapshot {
                node_id: "n1".into(),
                petal_id: "p1".into(),
                name: "actual reader snapshot".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                properties: std::collections::HashMap::from([
                    (
                        "calibration".into(),
                        fe_sdk::PropertyValue::Json(serde_json::json!({"gain":2})),
                    ),
                    (
                        "secret".into(),
                        fe_sdk::PropertyValue::String("hidden".into()),
                    ),
                ]),
            },
        );
        let host = Arc::new(Mutex::new(host));
        let contexts = PluginContexts::default().with_factory(Arc::new(MockFactory(host.clone())));
        let (commands, receiver) = crossbeam::channel::bounded(8);
        let manifest=CapabilityManifest::from_json(r#"{"capabilities":["storage.read","storage.write","query.select"],"property_scope":["calibration"]}"#).unwrap();
        let context = contexts
            .bind(
                "plugin-a".into(),
                "p1".into(),
                CapabilityToken::mint("plugin-a", &manifest),
                commands.clone(),
            )
            .unwrap();
        let engine = crate::rhai::RhaiEngine::new(Arc::new(Mutex::new(context.clone())));
        engine.eval::<()>(r#"let node=get_node("n1"); assert(node.name=="actual reader snapshot"); assert(node.position[2]==3.0); assert(node.properties.len()==1); assert(node.properties.calibration.gain==2);"#).unwrap();
        engine.eval::<()>(r#"let nodes=query_nodes("p1",`{"name":"actual reader snapshot","property":{"key":"calibration","value":{"gain":2}},"limit":1}`); assert(nodes.len()==1);"#).unwrap();
        assert!(engine
            .eval::<()>(r#"query_nodes("p1",`{"property":{"key":"secret","value":"hidden"}}`);"#)
            .is_err());
        assert!(engine
            .eval::<()>(r#"query_nodes("p1",`{"sql":"DELETE node"}`);"#)
            .is_err());
        assert!(engine.eval::<()>(r#"query_nodes("other","all");"#).is_err());
        engine.eval::<()>(r#"let receipt=commit_transaction("p1",`[{"SetProperty":{"node_id":"n1","key":"calibration","value":{"gain":3}}},{"CreateNode":{"petal_id":"p1","name":"created"}}]`); assert(receipt.op_count==2); assert(get_node("n1").properties.calibration.gain==3);"#).unwrap();
        let count = host.lock().unwrap().transactions.len();
        assert!(engine.eval::<()>(r#"commit_transaction("p1",`[{"SetProperty":{"node_id":"n1","key":"calibration","value":{"gain":999}}},{"DeleteNode":{"node_id":"missing"}}]`);"#).is_err());
        assert_eq!(host.lock().unwrap().transactions.len(), count);
        let mut transaction = context.transaction();
        transaction.set_property("n1", "calibration", serde_json::json!({"gain":4}));
        assert_eq!(transaction.commit().unwrap(), 1);
        assert_eq!(
            host.lock().unwrap().nodes["n1"].properties["calibration"],
            fe_sdk::PropertyValue::Json(serde_json::json!({"gain":4}))
        );
        let (results, result_rx) = crossbeam::channel::bounded(8);
        context
            .send_command(PluginCommand::GetNode {
                plugin_id: "plugin-a".into(),
                node_id: "n1".into(),
            })
            .unwrap();
        crate::handle_plugin_command(receiver.recv().unwrap(), &results, &contexts);
        match result_rx.recv().unwrap() {
            PluginResult::NodeSnapshot { data, .. } => {
                assert_eq!(data["name"], "actual reader snapshot");
                assert_eq!(data["properties"]["calibration"]["gain"], 4);
                assert!(data["properties"].get("secret").is_none());
            }
            other => panic!("expected real reader snapshot, got {other:?}"),
        }
        context
            .send_command(PluginCommand::CommitTransaction {
                plugin_id: "plugin-a".into(),
                ops: vec![crate::transaction::PendingOp::SetProperty {
                    node_id: "n1".into(),
                    key: "calibration".into(),
                    value: serde_json::json!({"gain":5}),
                }],
            })
            .unwrap();
        crate::handle_plugin_command(receiver.recv().unwrap(), &results, &contexts);
        assert!(
            matches!(result_rx.recv().unwrap(),PluginResult::TransactionCommitted {receipt,..} if receipt.op_count==1)
        );
        assert_eq!(
            host.lock().unwrap().nodes["n1"].properties["calibration"],
            fe_sdk::PropertyValue::Json(serde_json::json!({"gain":5}))
        );
        let broad = CapabilityManifest::from_json(
            r#"{"capabilities":["storage.read","storage.write"],"property_scope":["*"]}"#,
        )
        .unwrap();
        let _other = contexts
            .bind(
                "plugin-b".into(),
                "p1".into(),
                CapabilityToken::mint("plugin-b", &broad),
                commands.clone(),
            )
            .unwrap();
        context
            .send_command(PluginCommand::SetProperty {
                plugin_id: "plugin-b".into(),
                node_id: "n1".into(),
                key: "secret".into(),
                value: serde_json::json!("stolen"),
            })
            .unwrap();
        crate::handle_plugin_command(receiver.recv().unwrap(), &results, &contexts);
        assert!(
            matches!(result_rx.recv().unwrap(),PluginResult::Error {plugin_id,..} if plugin_id=="plugin-a")
        );
        crate::handle_plugin_command(
            PluginCommand::GetNode {
                plugin_id: "plugin-b".into(),
                node_id: "n1".into(),
            },
            &results,
            &contexts,
        );
        assert!(
            matches!(result_rx.recv().unwrap(),PluginResult::Error {plugin_id,..} if plugin_id=="unbound")
        );
        assert_eq!(
            host.lock().unwrap().nodes["n1"].properties["secret"],
            fe_sdk::PropertyValue::String("hidden".into())
        );
        contexts.unbind("plugin-a").unwrap();
        assert!(
            context.get_node("n1").is_err(),
            "revoked contexts cannot keep using direct services"
        );
        context
            .send_command(PluginCommand::GetNode {
                plugin_id: "plugin-a".into(),
                node_id: "n1".into(),
            })
            .unwrap();
        crate::handle_plugin_command(receiver.recv().unwrap(), &results, &contexts);
        assert!(matches!(
            result_rx.recv().unwrap(),
            PluginResult::Error { .. }
        ));
        contexts.shutdown().unwrap();
        assert!(
            _other.get_node("n1").is_err(),
            "host shutdown revokes the remaining direct contexts"
        );
        assert!(contexts
            .bind(
                "after-shutdown".into(),
                "p1".into(),
                CapabilityToken::mint("after-shutdown", &manifest),
                commands
            )
            .is_err());
    }

    #[test]
    fn string_size_limit() {
        let mut engine = Engine::new();
        configure_sandbox(&mut engine, DEFAULT_MAX_OPERATIONS);

        // Rhai string size limit should prevent extremely long strings.
        // Build a string beyond the 65536 limit by repeated concatenation.
        let script = r#"
            let s = "a";
            for i in 0..20 {
                s += s;
            }
            s
        "#;
        let result = engine.eval::<String>(script);
        // This should fail because the string exceeds the limit
        assert!(result.is_err(), "Should reject oversized string");
    }

    #[test]
    fn compile_and_run_ast() {
        let mut engine = Engine::new();
        configure_sandbox(&mut engine, DEFAULT_MAX_OPERATIONS);

        let ast = compile_script(&engine, "let x = 10; x * 3").unwrap();
        let result: i64 = engine.eval_ast(&ast).unwrap();
        assert_eq!(result, 30);
    }
}
