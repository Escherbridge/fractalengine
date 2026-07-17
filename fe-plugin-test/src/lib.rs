//! fe-plugin-test — mock host + fixtures + assertions for engine-free plugin tests
//! (architecture and walkthrough: src/AGENTS.md).
//!
//! ```rust
//! use fe_plugin_test::prelude::*;
//!
//! let fixture = fixtures::basic_terrain();
//! let host = fixture.to_mock_host();
//! let runner = RhaiTestRunner::new(host);
//!
//! runner.eval_script(r#"
//!     let node = get_node("terrain_000");
//!     log_info("got node");
//! "#).unwrap();
//!
//! let host = runner.host();
//! assertions::assert_logged(&host, "log_info", "got node").unwrap();
//! assertions::assert_no_writes(&host).unwrap();
//! ```

pub mod assertions;
pub mod fixtures;
pub mod mock_host;
pub mod mock_storage;
pub mod rhai_runner;
pub mod spy;

/// Convenience re-exports for typical test usage.
pub mod prelude {
    pub use crate::assertions;
    pub use crate::fixtures;
    pub use crate::mock_host::MockHostEnv;
    pub use crate::mock_storage::MockStorage;
    pub use crate::rhai_runner::RhaiTestRunner;
    pub use crate::spy::SpyRecorder;
}

// Top-level re-exports
pub use assertions::{
    assert_kv_set, assert_logged, assert_no_writes, assert_node_property_set, assert_nodes_created,
    assert_property_set, assert_query_ran,
};
pub use fixtures::SceneFixture;
pub use mock_host::MockHostEnv;
pub use mock_storage::MockStorage;
pub use rhai_runner::RhaiTestRunner;
pub use spy::SpyRecorder;
