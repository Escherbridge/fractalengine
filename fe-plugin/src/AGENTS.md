# fe-plugin/src — module rationale

`fe-plugin` is the plugin **host** (thread 7): Rhai engine, Wasmtime embedding,
lifecycle, capability tokens, and the channel bridge to the Bevy ECS. It now
depends on `fe-sdk` and treats `fe-sdk` as the single source of truth for the
stable data types.

## §type-unification

Previously `fe-plugin` carried type definitions parallel to `fe-sdk`. It no
longer does. `lib.rs` re-exports the canonical `fe-sdk` data types
(`NodeSnapshot`, `PropertyValue`, `PropertyBag`, `SceneChange`,
`SceneChangeBatch`) so downstream code has one place to import them.

Key change: `FractalExtension::on_scene_change` now takes/returns
`fe_sdk::SceneChange` (the stable SDK mirror) instead of the engine-internal
`fe_runtime::messages::SceneChange`. The extension-author-facing trait should
reference the stable type; no external crate implements `FractalExtension`
(verified: only `fe-plugin` + `fe-plugin-test` touch these types, and
`fe-terrain` uses only `fe_sdk::ui` / `fe_sdk::api`), so this is a
non-breaking, behavior-preserving swap.

What is **not** unified: the concrete host structs `context::PluginContext` and
`transaction::PluginTransaction`. These are the runtime layer (crossbeam
senders, capability tokens, pending-op batching) and intentionally differ from
`fe-sdk`'s object-safe `PluginContext`/`PluginTransaction` *traits*, which are
the extension-author contract. Different layers, kept separate on purpose.

## §host-env

`host_env.rs` defines `HostEnv` — the bundle of binary-injected service trait
objects (`Arc<dyn ExtensionStorageApi>`, `Arc<dyn ExtensionQueryApi>`). It is the
one place capability gating + host-boundary validation happen, so the Rhai and
WASM paths share exactly one fail-closed policy:

1. capability check (`token.has_capability`) — fail closed with `CapabilityDenied`,
2. input validation (`fe_sdk::storage::validate_*`, `fe_sdk::query::validate_query_len`),
3. SELECT-only guard for queries (`fe_sdk::query::is_select_only`),
4. delegate to the injected trait object (or `NotAvailable` if none injected),
5. cap the query result at `MAX_RESULT_ROWS`.

The engine never depends on `fe-database`; the binary constructs a `HostEnv`
(mirroring the existing `ApiExtensionHandle` injection pattern) and attaches it
to `PluginContext` via `with_host_env`. `PluginContext::new` keeps its original
4-arg signature (empty `HostEnv`) so existing call sites and tests are unchanged.

## §storage-query

Two capability-gated host surfaces, registered in `rhai/storage_api.rs` and
`wasm/host_imports.rs`:

- `node_get_properties(node_id) -> Map`
- `node_set_property(node_id, key, value)`
- `query_select(sql, params) -> Array` (SELECT-only)
- `ext_storage_get(key) -> value` / `ext_storage_set(key, value)` (per-extension KV,
  namespaced by `plugin_id`)

Gating differs by runtime by necessity:

- **Rhai**: the engine is built per-plugin, so gating is *registration-time* —
  a missing grant means the function is **never registered**. Calling it then
  raises a clean Rhai "function not found" error, never a panic. (Defense in
  depth: `HostEnv` re-checks the grant anyway.)
- **WASM**: the `Linker` is shared across plugins, so gating is *call-time*
  against the store's capability token. A denied/failed call logs via `tracing`
  and returns a sentinel (`-1` / no-op), never a trap.

All host-fn failures surface to the script as typed errors *and* to `tracing`
(`rhai/storage_api.rs::to_script_error`, and the `tracing::warn!` calls in the
WASM imports). Fuel/op limits are unchanged: the new Rhai fns run under the same
`on_progress` op cap; the new WASM imports run under the same per-tick fuel.

## Capability manifest

`capability.rs` gains a `capabilities: Vec<String>` grant list on both
`CapabilityManifest` and the minted `CapabilityToken`, plus
`has_capability(&str)`. Absent = denied (fail closed). Canonical grant strings
live in `fe-sdk` (`CAP_STORAGE_READ`, `CAP_STORAGE_WRITE`, `CAP_QUERY_SELECT`).

## WIT

`wit/hexon-plugin.wit` gains a `query-api` interface **alongside** `node-api`
(additive to `@1.0.0`). Values cross the boundary as JSON strings so no new
record types are forced on existing `node-api` consumers. The world imports it
(`import query-api;`); guests that don't use it are unaffected.

The WIT is the **aspirational contract**, not the wired ABI: the running WASM
host registers core-ABI `func_wrap` imports with `-1` denial sentinels
(`wasm/host_imports.rs`), and no component-model bindgen exists yet. Keep the
two in sync by name/shape; wiring bindgen is future work.

## §capability-policy (auth_policy_pattern_20260710)

`host_env.rs::require()` now delegates to the shared policy engine:
`CapabilityToken::to_auth_context()` (capability.rs) bridges the token into
`fe_policy::AuthContext::Capability`, and a static `PolicyEngine` holding
`fe_policy::CapabilityPolicy` makes the decision (capability name travels as
`Action::Custom`). Behavior is identical to the old inline `has_capability`
check — fail closed, same `HostApiError::CapabilityDenied` — but the decision
and its log now come from the one engine every entry point shares. The
`CapabilityManifest`/`CapabilityToken` types themselves are unchanged by
design (spec: they were already the right shape).
