# Track: Plugin Testing & Developer Experience

**Created:** 2026-05-09
**Status:** Draft
**Priority:** P1
**Depends on:** Plugin Host (Phase 9A), Extension SDK (Phase 9B)
**Blocks:** Community Marketplace (Phase 10)

---

## Problem Statement

Every successful plugin platform ships testing infrastructure on day one. Figma's #1 developer complaint is "can't test plugins without a real Figma session." Blender's community built `fake-bpy-module` because Blender didn't ship a mock. We need:

1. **Functional testing** — load a plugin against mock data, run it, validate output (no GPU, no Bevy window)
2. **Code-driven unit tests** — MockHostEnv with spy pattern, usable in `#[test]`
3. **Integration testing** — plugins in P2P sync scenarios (plugin v1 ↔ v2, uninstall propagation)
4. **WASM debug logging** — PluginLog host import routes to tracing
5. **Example plugins** — two working reference implementations for extension authors
6. **Replay-based debugging** — Shopify pattern: pure function plugins debuggable by replaying inputs

---

## Goals

1. **`fe-plugin-test` crate** — MockHostEnv trait, spy pattern, fixture loading
2. **`hexon-test` binary** — CLI tool to run plugin against fixtures (CI-friendly)
3. **Rhai script test runner** — test `.rhai` scripts without wasmtime
4. **Integration tests** in fe-test-harness (P2P plugin sync scenarios)
5. **Example plugins**: "property-transformer" (Rhai) + "scene-observer" (WASM)
6. **PluginLog host import** — routes WASM/Rhai print/log to tracing::debug!
7. **Source map support** for WASM debug builds (DWARF preservation)
8. **Plugin development documentation** with step-by-step tutorial

## Non-Goals (this track)

- Browser-based plugin IDE
- Visual plugin builder / node graph
- Automated plugin review / code scanning
- Performance profiling tools for plugins

---

## Architecture Overview

### fe-plugin-test Crate

```
fe-plugin-test/
├── Cargo.toml
└── src/
    ├── lib.rs              # Public API re-exports
    ├── mock_host.rs        # MockHostEnv (implements all host API functions)
    ├── spy.rs              # SpyRecorder (tracks all calls for assertions)
    ├── fixtures.rs         # Load scene fixtures from JSON files
    ├── assertions.rs       # Custom assertion helpers (assert_property_set!, etc.)
    ├── rhai_runner.rs      # Test runner for .rhai scripts (no wasmtime)
    └── wasm_runner.rs      # Test runner for .wasm components (wasmtime, headless)
```

### MockHostEnv

```rust
pub struct MockHostEnv {
    nodes: HashMap<String, NodeSnapshot>,
    properties: HashMap<(String, String), String>,  // (node_id, key) -> value
    spy: SpyRecorder,
    scene_changes: Vec<SceneChange>,
}

impl MockHostEnv {
    pub fn new() -> Self;
    pub fn from_fixture(path: &str) -> Result<Self>;

    // Populate test data
    pub fn insert_node(&mut self, id: &str, snapshot: NodeSnapshot);
    pub fn insert_property(&mut self, node_id: &str, key: &str, value: &str);

    // Load and run plugins
    pub fn load_rhai(&self, script_path: &str) -> Result<RhaiTestPlugin>;
    pub fn load_wasm(&self, wasm_path: &str) -> Result<WasmTestPlugin>;

    // Access spy for assertions
    pub fn spy(&self) -> &SpyRecorder;
    pub fn scene_changes(&self) -> &[SceneChange];
}
```

### SpyRecorder

```rust
pub struct SpyRecorder {
    get_node_calls: Vec<String>,           // node_ids requested
    set_property_calls: Vec<PropertyWrite>, // (node_id, key, value)
    create_node_calls: Vec<NodeCreate>,     // (petal_id, name)
    query_calls: Vec<QueryExecution>,       // (petal_id, filter)
    log_messages: Vec<(LogLevel, String)>,  // (level, message)
}

impl SpyRecorder {
    pub fn property_writes(&self) -> impl Iterator<Item = &PropertyWrite>;
    pub fn node_reads(&self) -> impl Iterator<Item = &str>;
    pub fn was_called(&self, method: &str) -> bool;
    pub fn call_count(&self, method: &str) -> usize;
}
```

### Custom Assertion Macros

```rust
// Assert a specific property was set
assert_property_set!(host, "node_001", "elevation_gain_m", "1234.5");

// Assert N nodes were created
assert_nodes_created!(host, 42);

// Assert no writes occurred (read-only plugin)
assert_no_writes!(host);

// Assert plugin logged a specific message
assert_logged!(host, LogLevel::Info, "Processing complete");
```

### hexon-test CLI Binary

```bash
# Run a .hexon plugin against a fixture scene
hexon-test run my-plugin.hexon \
  --scene fixtures/basic-terrain.json \
  --trigger on_scene_change \
  --assert-output expected-changes.json

# Run all .rhai scripts in a directory with fixtures
hexon-test test-rhai ./scripts/ --fixtures ./test-fixtures/

# Validate a hexon manifest (capabilities, signing, structure)
hexon-test validate my-plugin.hexon

# Benchmark a plugin's per-call overhead
hexon-test bench my-plugin.hexon --iterations 10000
```

The binary:
- Launches minimal host (no Bevy, no GPU, no window)
- Loads plugin (WASM or Rhai), calls lifecycle hooks with fixture data
- Validates output against expected scene changes
- Returns exit code 0/1 for CI integration
- GitHub Actions compatible (no display server needed)

### Integration Tests (extend fe-test-harness)

New test scenarios in `fe-test-harness/tests/`:

```rust
#[tokio::test]
async fn plugin_created_nodes_sync_to_peer_without_plugin() {
    let (peer_a, peer_b) = create_connected_peers().await;

    // Peer A has terrain plugin installed
    peer_a.install_plugin("terrain-processor.hexon").await;
    peer_a.trigger_plugin("on_activate", "petal_001").await;

    // Wait for sync
    sync_peers(&peer_a, &peer_b).await;

    // Peer B should have the nodes but see "created by terrain-processor, not installed"
    let nodes = peer_b.query_nodes("petal_001").await;
    assert!(nodes.iter().any(|n| n.properties.contains_key("hexon_ref")));
    assert!(nodes.iter().any(|n| n.properties.get("_plugin_status") == Some(&"uninstalled")));
}

#[tokio::test]
async fn plugin_v2_properties_handled_by_v1_peer() {
    let (peer_a, peer_b) = create_connected_peers().await;

    // Peer A has v2, Peer B has v1
    peer_a.install_plugin("sensor-proc@2.0.0.hexon").await;
    peer_b.install_plugin("sensor-proc@1.0.0.hexon").await;

    // v2 adds calibration_matrix property that v1 doesn't understand
    peer_a.trigger_plugin("on_tick", 16).await;
    sync_peers(&peer_a, &peer_b).await;

    // v1 should preserve unknown properties without corruption
    let node = peer_b.get_node("sensor_001").await;
    assert!(node.properties.contains_key("calibration_matrix")); // preserved, not dropped
}

#[tokio::test]
async fn plugin_uninstall_flags_nodes_correctly() {
    let peer = create_peer().await;
    peer.install_plugin("terrain.hexon").await;
    peer.trigger_plugin("on_activate", "petal_001").await;

    let pre_count = peer.query_nodes("petal_001").await.len();
    assert!(pre_count > 0);

    peer.uninstall_plugin("terrain.hexon").await;

    // Nodes still exist, but flagged
    let post_count = peer.query_nodes("petal_001").await.len();
    assert_eq!(pre_count, post_count); // NOT deleted

    // Op log has hexon_uninstalled entry
    let ops = peer.query_op_log("petal_001").await;
    assert!(ops.iter().any(|op| op.op == "hexon_uninstalled"));
}
```

### Example Plugins

**1. Property Transformer (Rhai)**

`examples/property-transformer/plugin.rhai`:
```rhai
// Watches for nodes with "raw_temperature_c" and adds "temperature_f"
fn on_scene_change(batch) {
    for change in batch.property_changes {
        if change.key == "raw_temperature_c" {
            let celsius = parse_float(change.value);
            let fahrenheit = celsius * 9.0 / 5.0 + 32.0;
            set_property(change.node_id, "temperature_f", fahrenheit.to_string());
            log_info(`Converted ${celsius}°C → ${fahrenheit}°F for ${change.node_id}`);
        }
    }
}
```

**2. Scene Observer (WASM/Rust)**

`examples/scene-observer/src/lib.rs`:
```rust
use fe_sdk::prelude::*;

#[fractal_plugin]
struct SceneObserver {
    node_count: u32,
}

impl FractalExtension for SceneObserver {
    fn on_scene_change(&mut self, ctx: &mut PluginContext, batch: &SceneChangeBatch) -> Result<Vec<SceneChange>> {
        self.node_count += batch.added.len() as u32;
        log_info!("Total nodes observed: {}", self.node_count);

        // Set a dashboard metric property on the petal
        ctx.transaction()
            .set_property(&ctx.petal_id(), "observer_node_count", &self.node_count.to_string())?
            .commit()?;

        Ok(vec![])
    }
}
```

### PluginLog (Debug Output)

WASM and Rhai plugins need a way to see their own output:

```rust
// Host import for WASM
fn plugin_log(level: LogLevel, msg: &str) {
    tracing::event!(
        target: "fractalengine::plugin",
        level,
        plugin_id = %current_plugin_id,
        "{msg}"
    );
}
```

Output appears in:
- Terminal (via tracing subscriber)
- fe-ui "Plugin Console" panel (new dashboard widget)
- API: `GET /api/v1/plugins/:id/logs` (last 1000 entries, ring buffer)

---

## Phases

### Phase 1: MockHostEnv + Rhai Test Runner

- `fe-plugin-test` crate scaffolding
- MockHostEnv with node/property storage + SpyRecorder
- Fixture loading from JSON scene files
- Custom assertion macros (assert_property_set!, assert_nodes_created!, etc.)
- Rhai test runner (loads .rhai, calls hooks, validates via spy)
- Property Transformer example plugin (Rhai) + its tests

### Phase 2: WASM Test Runner + hexon-test CLI

- WasmTestRunner (headless wasmtime, no Bevy)
- `hexon-test` binary (run/validate/bench subcommands)
- Scene Observer example plugin (WASM/Rust) + its tests
- PluginLog host import → tracing integration
- Source map support for WASM debug builds (document toolchain flags)
- Plugin Console dashboard widget in fe-ui

### Phase 3: Integration Tests + Documentation

- P2P plugin sync tests in fe-test-harness
  - Plugin nodes sync to peer without plugin
  - Plugin v2 properties preserved by v1 peer
  - Uninstall flags nodes correctly
  - Capability violation rejected across peers
- "Writing Your First Extension" tutorial document
- "Plugin Architecture Deep Dive" reference document
- Example plugins published as template repositories

---

## Key Dependencies (Rust Crates)

| Crate | Version | Purpose |
|-------|---------|---------|
| `wasmtime` | 21+ | Headless WASM execution in tests |
| `rhai` | 1.19+ | Script test execution |
| `serde_json` | 1 | Fixture loading |
| `assert_matches` | 1 | Enhanced assertion support |

---

## Success Criteria

- [ ] MockHostEnv loads a JSON fixture and Rhai plugin passes all assertions
- [ ] `hexon-test run` exits 0 for valid plugin, exits 1 for failing assertions
- [ ] `hexon-test validate` catches unsigned plugins, invalid manifests, missing capabilities
- [ ] WASM plugin's log_info() appears in tracing output and Plugin Console
- [ ] P2P integration test: nodes from uninstalled plugin visible with correct flags
- [ ] P2P integration test: v2 properties preserved without corruption by v1 peer
- [ ] Both example plugins compile, test, and demonstrate the full extension pattern
- [ ] `hexon-test bench` reports <500ns for Rhai calls, <5us for WASM calls
- [ ] All tests run in CI without GPU or display server
