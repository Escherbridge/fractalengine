//! Shared engine configuration for both GUI and headless binaries.
//!
//! The actual thread-wiring logic lives in each binary's `main.rs` (or a
//! shared helper crate) because it depends on `fe-database`, `fe-sync`, and
//! `fe-network` — crates that `fe-runtime` intentionally does not depend on.

use std::sync::Arc;

/// Configuration for engine wiring.
///
/// Both the GUI binary and the headless relay binary construct an
/// `EngineConfig` with their platform-appropriate secret store, then pass it
/// to whatever wiring function spawns the background threads.
pub struct EngineConfig {
    /// Secret store backend (`OsKeystoreBackend` for desktop,
    /// `EnvBackend` for relay, `InMemoryBackend` for tests).
    pub secret_store: Arc<dyn fe_identity::SecretStore>,
    /// SurrealDB data path (default: `"data/fractalengine.db"`).
    pub db_path: String,
    /// API gateway bind address (default: `"127.0.0.1:8765"`).
    pub bind_addr: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            secret_store: Arc::new(fe_identity::InMemoryBackend::new()),
            db_path: "data/fractalengine.db".to_string(),
            bind_addr: "127.0.0.1:8765".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_config_default() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.db_path, "data/fractalengine.db");
        assert_eq!(cfg.bind_addr, "127.0.0.1:8765");
    }
}
