---
type: guide
title: Migrating a 1.0 plugin to fractalengine:plugin@2.0.0
track: extension_platform_20261107
status: draft (P3.4, slice E12D, reviewed by G3; the 1.0 contract is retired by P1.1 / slice E1)
---

# Migration: 1.0 to 2.0

The 1.0 contract (`fe-plugin/wit/hexon-plugin.wit`, world `fractal-plugin`) was never a third-party
target. A sealed plugin that declares it is refused as `unsupported-contract`, and a 1.0 core
module packaged as a `plugin` is refused for the same reason. This page maps each 1.0 surface to
its 2.0 replacement so a port is a rewrite against the new shape, not a search for equivalents.

There is no automatic converter. The 2.0 model differs in three ways that change code, not just names:

1. **Writes are staged, not immediate.** A 1.0 `set-property` took effect at once; in 2.0 it is flushed at the next tick.
2. **Every effect is declared.** 1.0 capability strings (`storage.read`, `query.select`) become manifest sections.
3. **Ticks are real.** In 1.0 `on-tick` was declared but never driven in production
   (`fe-plugin/src/lib.rs:157-161`, per the track's evidence section). 2.0 is the first contract whose tick the host schedules.

## Interfaces

| 1.0 | 2.0 | What changed |
|---|---|---|
| `node-api.get-node(id) -> option<node-snapshot>` | `scene.get-node(id) -> result<option<node-snapshot>, host-error>` | Refusals are now typed. |
| `node-snapshot { pos-x, pos-y, pos-z }` | `node-snapshot { position: vec3, rotation: quat, scale: vec3, properties: json }` | Full transform, and only the in-scope properties. |
| `node-api.query-nodes(petal-id, filter: string)` | `scene.query-nodes(query: json)` | The petal is implicit (the binding). The filter is the fe-sdk `NodeQuery` shape. |
| `node-api.set-property(...)` | `txn.stage-set-property(node, key, value)` | Staged; flushed at the next tick with everything else. |
| `node-api.create-node(petal, name) -> string` | `txn.stage-create-node(name) -> result<u32, host-error>` | Returns an ordinal. The id arrives in `tick-info.last-commit`. |
| `node-api.delete-node(id)` | `txn.stage-delete-node(node)` | Staged; plain nodes only. |
| `query-api.query-select(sql, params)` | removed | No raw SQL in 2.0 (OD-E10). Use `bus.subscribe("host/query", ...)` with a named query in the manifest. |
| `query-api.node-get-properties(id)` | `scene.get-node(id).properties` | |
| `query-api.node-set-property(...)` | `txn.stage-set-property` | As above. |
| `query-api.ext-storage-get/set` | `kv.get` / `kv.set` | Declared size (`kv.max_bytes`), host-local. |
| `plugin-log.log-*` | `log.write(level, message)` | One function, with a `level` enum. |
| (none) | `readings.stage`, `routes.*`, `ingress`, `egress`, `bus` | New surfaces, each behind a manifest grant. |

## Lifecycle

| 1.0 `scene-hooks` | 2.0 `lifecycle` | Note |
|---|---|---|
| `on-install(petal-id)` | `on-install(binding)` | Gets the full binding, including the identity DID. |
| `on-activate(petal-id)` | `on-activate(binding) -> result<_, string>` | An error refuses activation. |
| `on-deactivate()` | `on-deactivate(reason: deactivate-reason)` | The reason decides whether one final flush happens. |
| `on-uninstall()` | `on-uninstall(binding)` | One final flush follows, for cleanup. |
| `on-tick(delta-ms: u32)` | `on-tick(info: tick-info)` | `tick-info` carries `tick`, `now-ms`, `skipped` and `last-commit`. Read `skipped` to see overruns. |
| `on-scene-change(changes) -> list<scene-change>` | removed | Reactive changes are returned by staging writes in `on-tick`. Observe changes with a `host/node-changes` subscription. |

The 1.0 `on-scene-change` returned writes straight to the host. In 2.0 the same reaction is a
`stage-*` call in `on-tick`, and the change feed comes through `bus-hooks.on-events` (see the
[cookbook](cookbook.md#subscribe-to-a-host-stream)).

## Capabilities

| 1.0 capability string | 2.0 manifest |
|---|---|
| `storage.read` | `scene.read: true` |
| `storage.write` | `scene.write: [...]` and `scene.property_scope: [...]` |
| `query.select` | `bus.consume: [{"source": "host/query", "id": ..., "sql": ...}]` |
| (no key) | `tick: { "cadence_ms": ... }` (required for any write) |

Each 2.0 grant is per binding: one petal, and one installation. The 1.0 `verse_scope` and
`petal_scope` patterns are gone. The operator binds the instance (OD-E3). See the full table in
[manifest-v2.md](manifest-v2.md#from-the-10-capabilitymanifest).

## Errors

1.0 `host-error` had five variants. 2.0 drops `not-select-only` (there is no SQL to refuse) and adds five:

- New: `policy-denied`, `budget-exceeded`, `rate-limited`, `revoked`, `unknown-outcome`.
- `unknown-outcome` is never retried automatically. A transport timeout may or may not have landed.
- Any `match` on the error needs a catch-all arm, because the variant set is not closed across minor versions.

## Package

| 1.0 | 2.0 |
|---|---|
| Component targets `fractalengine:plugin@1.0.0`, world `fractal-plugin` | Component targets `fractalengine:plugin@2.0.0`, world `extension` |
| A core module is refused as a `plugin` package | A sealed `.hexon` with `HexonType::Plugin`, the `plugin/extension.json` header, and a signed component ([package-format.md](package-format.md)) |
| Trust by file location is not a concept in the contract | Trust comes from a publisher signature checked against a host trust root ([governance.md](governance.md#trust-roots)) |

## Port checklist

1. Start with `fe-ext new general <name>`, then write its `extension.json` (packed as `plugin/extension.json`)
   from the old capability list, using the table above.
2. Replace each `node-api` / `query-api` call with its `txn`, `scene` or `kv` equivalent. Expect every write to be deferred to the next tick.
3. Replace `query-select` SQL with a named `host/query` consume entry. If the SQL was not a single SELECT, it cannot be ported as-is.
4. Implement `on-tick(info)` and set `tick.cadence_ms` to the rate you actually need. Nothing runs without a `tick` block.
5. Replace `on-scene-change` with a `host/node-changes` subscription, and handle `resync-required` by re-reading the petal.
6. Run `fe-ext validate --project <dir>`, then `fe-ext run --sandbox --fixture <file>` with your old scene
   recreated as a sandbox fixture (`Fixture` in [`tools/fe-ext/src/sandbox.rs`](../../tools/fe-ext/src/sandbox.rs)),
   and compare outcomes (final nodes and readings in the trace) rather than call counts.

## Sharpen

- **Pattern:** *contract versioning by major.* The contract name carries the major version, so the host can refuse a mismatch at install rather than trap at run time.
- **Architecture:** removing raw SQL is the largest behavioural change. Guests lose a way to read arbitrary data, and gain a consent prompt that shows the exact query. That trade is the point of OD-E10.
- **Deferred:** an automatic 1.0 to 2.0 source converter. Not planned; the port is small, and a converter would hide the staging change.
- **Efficiency:** batch writes into the per-tick window and let one flush commit them. The window's op and reading caps are shared, so check `txn.window()` before staging a large batch.

*Question to check your grasp:* in 1.0, `create-node` returned an id you could use in the same hook. Why can't the 2.0 equivalent do that, and where does the id appear instead?
