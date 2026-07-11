//! Rhai scripting engine integration.
//!
//! Provides a sandboxed Rhai [`Engine`](rhai::Engine) with host API functions
//! that bridge back to the FractalEngine through [`PluginContext`].

pub mod host_api;
pub mod sandbox;
pub mod storage_api;

use std::sync::{Arc, Mutex};

use rhai::{Engine, AST};

use crate::context::PluginContext;

/// A configured Rhai engine with sandbox limits and host API bindings.
pub struct RhaiEngine {
    engine: Engine,
}

impl RhaiEngine {
    /// Create a new sandboxed Rhai engine bound to a plugin context.
    ///
    /// Registers all host API functions and configures safety limits.
    pub fn new(ctx: Arc<Mutex<PluginContext>>) -> Self {
        let mut engine = Engine::new();
        sandbox::configure_sandbox(&mut engine, sandbox::DEFAULT_MAX_OPERATIONS);
        host_api::register_host_api(&mut engine, ctx.clone());
        storage_api::register_storage_query_api(&mut engine, ctx);
        Self { engine }
    }

    /// Create a Rhai engine with custom operation limit.
    pub fn with_max_operations(ctx: Arc<Mutex<PluginContext>>, max_ops: u64) -> Self {
        let mut engine = Engine::new();
        sandbox::configure_sandbox(&mut engine, max_ops);
        host_api::register_host_api(&mut engine, ctx.clone());
        storage_api::register_storage_query_api(&mut engine, ctx);
        Self { engine }
    }

    /// Compile a script into an AST for repeated evaluation.
    pub fn compile(&self, source: &str) -> Result<AST, rhai::ParseError> {
        sandbox::compile_script(&self.engine, source)
    }

    /// Evaluate a script string and return the result.
    pub fn eval<T: Clone + Send + Sync + 'static>(
        &self,
        script: &str,
    ) -> Result<T, Box<rhai::EvalAltResult>> {
        self.engine.eval(script)
    }

    /// Evaluate a pre-compiled AST.
    pub fn eval_ast<T: Clone + Send + Sync + 'static>(
        &self,
        ast: &AST,
    ) -> Result<T, Box<rhai::EvalAltResult>> {
        self.engine.eval_ast(ast)
    }

    /// Get a reference to the underlying Rhai engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl std::fmt::Debug for RhaiEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RhaiEngine").finish_non_exhaustive()
    }
}
