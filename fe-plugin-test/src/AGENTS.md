# fe-plugin-test/src — module rationale

`fe-plugin-test` (Phase 9C.1) is the testing kit for FractalEngine plugins: a
mock host environment, spy recorder, scene fixtures, assertion helpers, and a
Rhai test runner so plugin authors can test their scripts without spinning up a
real engine. Like `fe-sdk`, it stays engine-free — it depends on `fe-sdk` (and
`rhai` for the runner) but never on `fe-plugin`, Bevy, or SurrealDB.

## §mock-host architecture

Everything hangs off `mock_host::MockHostEnv` — an in-memory substitute for the
real engine:

- `MockHostEnv` holds node snapshots, properties, scene changes, a
  `SpyRecorder`, and a `MockStorage`. Tests inject state *before* running a
  plugin script and inspect mutations *after*.
- `spy::SpyRecorder` is embedded inside the mock host and records every
  host-API call (name, args, order) so tests can assert on call counts,
  argument values, and ordering.
- `rhai_runner::RhaiTestRunner` builds a sandboxed Rhai engine whose host
  functions read/write the mock host instead of a real engine.
- `assertions` are plain functions (not macros) returning
  `Result<(), String>` so callers can use `?` or `.unwrap()`; error messages
  carry enough context to diagnose failures without a debugger.
- `prelude` re-exports the typical test surface.

## §mock-storage

`mock_storage::MockStorage` implements the `fe-sdk` `ExtensionStorageApi` +
`ExtensionQueryApi` traits in memory: `node_get_properties`,
`node_set_property`, `query_select`, and the per-extension KV. It applies the
same host-boundary validation and SELECT-only guard as the engine (both live
in `fe-sdk`, so there is exactly one policy) so tests observe realistic
fail-closed behavior. It is cheap to clone — all clones share one
`Arc<Mutex<..>>` state — so the same store can be seeded, injected into a
runner, and inspected afterwards.

## §fixtures

`fixtures::SceneFixture` provides reproducible starting states so plugin
authors don't hand-build dozens of nodes per test. Built-ins: `empty_scene()`,
`basic_terrain()`, `sensor_network()`. `SceneFixture::load_from_json` accepts:

```json
{
  "nodes": [ { "node_id": "...", "petal_id": "...", ... } ],
  "properties": { "node_id:key": "value", ... }
}
```

`properties` keys are `node_id:key` compound strings (split on the first `:`);
non-string values are stringified.

## Quick start

The canonical compile-checked walkthrough lives as the doctest in `lib.rs`
(kept there deliberately — `cargo test --doc` keeps it honest). Shape:
fixture → `to_mock_host()` → `RhaiTestRunner::new(host)` → `eval_script(..)` →
assert via `assertions::*` against `runner.host()`.
