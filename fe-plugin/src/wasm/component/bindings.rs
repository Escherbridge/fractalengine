//! Generated host bindings for WIT `fractalengine:plugin@2.0.0`, world `extension`.
//!
//! Points at the FILE, never the `wit/` directory: the directory also holds the superseded 1.0
//! package (fe-plugin/wit/AGENTS.md §one-file). Sync imports and exports; every guest call is
//! driven from the plugin worker (see src/wasm/AGENTS.md §bindgen).

wasmtime::component::bindgen!({
    world: "extension",
    path: "wit/fractal-plugin-2.wit",
});
