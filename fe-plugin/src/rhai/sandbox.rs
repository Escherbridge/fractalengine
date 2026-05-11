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
            err_msg.contains("exceeded") || err_msg.contains("progress")
                || err_msg.contains("terminated") || err_msg.contains("limit"),
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
