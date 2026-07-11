# Track: Plugin Host — Wasmtime + Rhai Tiered Plugin Runtime

**Created:** 2026-05-09
**Status:** Draft
**Priority:** P1
**Depends on:** Open Crate Format (Phase 6.5), Crate Registry (Phase 8), Entity Data Layer Phase 5 (node_log)
**Blocks:** Extension SDK, Extension UI, Plugin Testing, Community Marketplace

---

## Problem Statement

FractalEngine currently has no mechanism for third-party code execution. All functionality is compiled into the binary. For a platform that distributes `.hexon` packages via P2P, we need:

1. **Sandboxed execution** of untrusted WASM plugins from the marketplace
2. **Fast scripting** for per-frame behaviors (Rhai — 50-100ns/call vs WASM's 2-8us)
3. **Plugin lifecycle management** (install, activate, deactivate, uninstall)
4. **Fuel-based CPU budgeting** to prevent plugins from stalling the render loop
5. **Capability-scoped security** — plugins declare permissions, engine enforces them
6. **Data ownership tracking** — plugin-created nodes are tagged for uninstall cleanup

This track does NOT make FractalEngine a BIM tool — it keeps the core generic while domain-specific capabilities ship as optional extensions.

---

## Goals

1. **Three-tier runtime**: Native (Bevy Plugin, 0ns), Rhai (50-100ns/call), Wasmtime (80-800ns/call)
2. **`fe-plugin` crate** with PluginEngine, PluginRegistry, PluginInstance types
3. **7th thread pool** for WASM execution — never blocks Bevy main or DB threads
4. **Wasmtime instance pooling** (1-5us hot-swap — unique capability)
5. **Fuel metering** for all WASM execution (configurable per-plugin budget)
6. **Rhai ScriptEngine** for `.rhai` scripts embedded in .hexon archives
7. **`FractalExtension` trait** — common interface across all three tiers
8. **PluginTransaction** — all plugin writes go through node_log (audit trail)
9. **Mandatory ed25519 signing** for `kind: script` hexon entries
10. **Capability manifest** — declared permissions enforced at the token level

## Non-Goals (this track)

- WIT interface definition (Phase 9B — SDK)
- UI extension slots (Phase 9B — SDK)
- Plugin testing harness (Phase 9C — Testing)
- Multi-language PDK beyond Rust (Phase 10)
- Plugin-to-plugin event bus (Phase 10)
- GPU access from plugins

---

## Architecture Overview

### Runtime Decision (Research-Backed)

| Tier | Runtime | Overhead | Use Case |
|------|---------|----------|----------|
| T1: Native | Bevy Plugin + Cargo features | ~0 ns (LTO) | First-party crates |
| T2: Scripting | Rhai (`rhai` crate) | 50-100 ns | Per-frame behaviors, prototyping |
| T3: Sandboxed | Wasmtime (component model) | 80-800 ns | Marketplace, untrusted, multi-lang |

**Why Wasmtime**: Instance pooling (unique), fuel metering, WASI 0.2 stable, component model production-ready since April 2024. Wasmer rejected (4 breaking API rewrites, no pooling, WASIX dead-end). wasm3/wasmi rejected (5-50x compute overhead).

**Why Rhai over Lua**: Native Rust types without serialization, `#[forbid(unsafe_code)]`, configurable resource limits, zero toolchain for authors.

### New Crate: `fe-plugin`

```
fe-plugin/
├── Cargo.toml
└── src/
    ├── lib.rs              # PluginHostPlugin (Bevy), FractalExtension trait
    ├── engine.rs           # PluginEngine (wasmtime::Engine + pool config)
    ├── registry.rs         # PluginRegistry (HashMap<HexonUri, PluginEntry>)
    ├── instance.rs         # PluginInstance (Store + fuel + capabilities)
    ├── transaction.rs      # PluginTransaction (scoped node_log writes)
    ├── capability.rs       # CapabilityManifest, CapabilityToken (scoped JWT)
    ├── rhai/
    │   ├── mod.rs          # RhaiEngine wrapper
    │   ├── host_api.rs     # Registered host functions (get_node, set_property, etc.)
    │   └── sandbox.rs      # Resource limits, disabled symbols
    ├── wasm/
    │   ├── mod.rs          # WasmEngine wrapper (wasmtime embedding)
    │   ├── host_imports.rs # WASM host function imports
    │   ├── fuel.rs         # Fuel budget management
    │   └── aot.rs          # AOT compilation + .cwasm cache
    ├── lifecycle.rs        # Install, activate, deactivate, uninstall flows
    ├── signing.rs          # Mandatory ed25519 verification for kind:script
    └── blocklist.rs        # Fetched blocklist (publisher DID + hexon_id)
```

### FractalExtension Trait

```rust
pub trait FractalExtension: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    fn tier(&self) -> PluginTier; // Native, Rhai, Wasm

    fn on_install(&mut self, ctx: &mut PluginContext) -> Result<()>;
    fn on_activate(&mut self, ctx: &mut PluginContext) -> Result<()>;
    fn on_deactivate(&mut self, ctx: &mut PluginContext) -> Result<()>;
    fn on_uninstall(&mut self, ctx: &mut PluginContext) -> Result<()>;

    fn on_scene_change(&mut self, ctx: &mut PluginContext, batch: &SceneChangeBatch) -> Result<Vec<SceneChange>>;
    fn on_tick(&mut self, ctx: &mut PluginContext, delta_ms: u32) -> Result<()>;
}
```

### PluginTransaction (Write Audit Trail)

Every plugin write goes through the node_log — no direct DB mutations:

```rust
pub struct PluginTransaction {
    plugin_id: HexonUri,
    scope: CapabilityToken,
    pending_ops: Vec<NodeLogOp>,
}

impl PluginTransaction {
    pub fn set_property(&mut self, node_id: &str, key: &str, value: &str) -> Result<()>;
    pub fn create_node(&mut self, petal_id: &str, name: &str) -> Result<String>;
    pub fn commit(self) -> Result<()>; // Writes to node_log with plugin_id attribution
}
```

### Capability Manifest (in hexon manifest.json)

```json
{
  "plugin": {
    "sdk_api_version": "1.0",
    "runtime": "wasm",
    "entry_point": "plugin.wasm",
    "capabilities": {
      "verse_scope": ["read"],
      "petal_scope": ["verse#v1/fractal#f1/petal#p1"],
      "property_scope": ["read:sensor_*", "write:processed_*"],
      "network_scope": "none",
      "external_http": []
    },
    "cpu_budget_ms_per_tick": 10,
    "activation": {
      "triggers": ["onPetalType:terrain", "onNodeProperty:gpx_type"],
      "lazy": true
    }
  }
}
```

### Data Ownership on Uninstall

When a plugin is uninstalled:
1. Write `hexon_uninstalled` op_log entry marking all nodes created by this plugin
2. Snapshot plugin-contributed property schemas into node properties (self-contained)
3. Peers without the plugin see "created by [hexon_id], now uninstalled" — data is NOT deleted
4. User can explicitly delete flagged nodes if desired

### Thread Model

```
Existing 6 threads:
  1. Bevy main (render + ECS)
  2. DB (own tokio)
  3. Network (own tokio)
  4. Sync (own tokio)
  5. Replication bridge
  6. API gateway (multi-thread tokio)

New:
  7. Plugin runtime (tokio::runtime::Builder::new_multi_thread().worker_threads(2))
     - WASM execution happens here (never on Bevy main)
     - Rhai scripts can run on Bevy main (fast enough) or offloaded here
     - crossbeam::channel::bounded(256) for plugin <-> engine communication
```

---

## Phases

### Phase 1: Core Runtime + Rhai

- `fe-plugin` crate scaffolding
- `FractalExtension` trait definition
- `PluginEngine` with wasmtime::Engine + PoolingAllocationConfig
- `RhaiEngine` wrapper with host API (get_node, set_property, query_nodes)
- Rhai sandbox (max_operations, max_array_size, disabled symbols)
- `PluginRegistry` resource (Bevy Resource, Arc<RwLock<>>)
- 7th thread: plugin runtime tokio pool
- crossbeam channels for plugin <-> engine messages
- `PluginHostPlugin` for Bevy (registers systems, drains channels)

### Phase 2: Wasmtime Embedding + WASM Execution

- Wasmtime Store + Instance creation with fuel metering
- Host function imports (get_node, set_property, query_nodes — mirrors Rhai API)
- AOT compilation + .cwasm cache (Engine::precompile_component)
- Instance pooling (PoolingAllocationConfig)
- WASI Preview 2 capability grants (per-plugin WasiCtx)
- Load `.wasm` entries from .hexon archives
- Fuel budget enforcement (Store::add_fuel, OutOfFuel trap handling)

### Phase 3: Security + Lifecycle

- Mandatory ed25519 signature verification for kind:script
- CapabilityManifest parsing from hexon manifest.json plugin section
- CapabilityToken generation (scoped JWT per plugin)
- Blocklist infrastructure (fetch from well-known URL at startup)
- PluginTransaction (all writes through node_log with plugin attribution)
- Data ownership tracking (hexon_installed/hexon_uninstalled op_log ops)
- Install/activate/deactivate/uninstall lifecycle flows
- Lazy activation by declared triggers

---

## Key Dependencies (Rust Crates)

| Crate | Version | Purpose |
|-------|---------|---------|
| `wasmtime` | 21+ | WASM component model runtime |
| `wasmtime-wasi` | 21+ | WASI Preview 2 host implementation |
| `rhai` | 1.19+ | Scripting runtime |
| `ed25519-dalek` | 2 | Signature verification (already in fe-identity) |
| `jsonwebtoken` | 9 | Capability token generation |

---

## Success Criteria

- [ ] Load a `.rhai` script from a .hexon archive, execute on_tick, verify property writes in node_log
- [ ] Load a `.wasm` component, execute on_scene_change, verify output SceneChanges applied
- [ ] Fuel exhaustion triggers graceful OutOfFuel error (no crash, plugin marked degraded)
- [ ] Unsigned kind:script entries are rejected at install time
- [ ] Plugin with `property_scope: ["write:processed_*"]` cannot write `sensor_raw` properties
- [ ] Uninstall writes hexon_uninstalled op_log, nodes remain visible with "uninstalled" flag
- [ ] Plugin execution never blocks Bevy main thread (runs on 7th thread pool)
- [ ] Instance pool hot-swap < 10us (benchmark test)
