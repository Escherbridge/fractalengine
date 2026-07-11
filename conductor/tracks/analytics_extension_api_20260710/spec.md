---
type: Track Spec
title: Analytics Extension API — Storage + Query API for Extensions
tags: [feature, analytics_extension_api_20260710, in_progress]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Specification: Analytics Extension API

**Track ID:** `analytics_extension_api_20260710`
**Type:** Feature
**Status:** In progress
**Goal alignment:** 3D P2P analytics engine on the hexon format, with an extension storage/query API and Rhai/WASM scripting.

## Overview

Extensions (Rhai and WASM) need a capability-gated way to read/write node data and
run analytical queries against the entity store without depending on `fe-database`
directly. This track finishes wiring that surface end-to-end: from a real
`fe-database`-backed implementation, through `fe-plugin`'s host-boundary routing,
out to both scripting runtimes and the WIT component-model contract.

## Background — what already exists (verified 2026-07-10)

A prior pass (Phase 9A/9B, `plugin_host_20260509` / `extension_sdk_ui_20260509`)
already built most of the *contract* layer:

- `fe-sdk/src/storage.rs` (138 lines) — `ExtensionStorageApi` trait, `StorageError`.
- `fe-sdk/src/query.rs` (161 lines) — `ExtensionQueryApi` trait, `QueryError`,
  `MAX_RESULT_ROWS`, `is_select_only()` guard.
- `fe-plugin/src/host_env.rs` — `HostEnv` (capability-gated, fail-closed router):
  `node_get_properties`, `node_set_property`, `storage_get`/`storage_set`
  (namespaced per-extension KV), `query_select` (SELECT-only, row-capped).
  Every call requires a capability (`storage.read`, `storage.write`,
  `query.select`) via `CapabilityToken::has_capability`; absent host services
  fail closed with `HostApiError::NotAvailable`, not a panic or silent no-op.
- `fe-plugin/wit/hexon-plugin.wit` — `query-api` interface (`node-get-properties`,
  `node-set-property`, `query-select`, `ext-storage-get`, `ext-storage-set`),
  value-for-value aligned with the Rust `HostEnv` API and already imported by
  the `fractal-plugin` world.
- `fe-plugin/Cargo.toml` already depends on `fe-sdk` (the MEMORY.md "fe-plugin
  should depend on fe-sdk" known issue is **already resolved** — verify on next
  pass before re-opening it).
- Unit tests in `host_env.rs` cover: capability denial, `NotAvailable` when no
  backend injected, input validation, KV namespace isolation, non-SELECT
  rejection.

**What is missing:** every implementation of `ExtensionStorageApi` /
`ExtensionQueryApi` found in the workspace is a test mock (`host_env.rs`'s
`RecordingStore`, `fe-plugin-test/src/mock_storage.rs`). There is no
`fe-database`-backed production implementation, and nothing in the
`fractalengine` binary calls `HostEnv::with_storage`/`with_query` — so in the
running application, every extension storage/query call fails closed with
`HostApiError::NotAvailable`. The capability-gated contract is real; the data
path behind it is not connected.

## Functional Requirements

### FR-1: Production `ExtensionStorageApi` backed by fe-database

**Description:** Implement `ExtensionStorageApi` for a type in `fe-database`
(or a thin adapter crate) that routes `node_get_properties`/`node_set_property`
through the existing node-properties tables and `storage_get`/`storage_set`
through a new per-extension KV table (namespaced by extension id, scoped to
the petal the extension instance is bound to).

**Acceptance Criteria:**
- A non-test type implements `fe_sdk::storage::ExtensionStorageApi`.
- KV writes are scoped to `(petal_id, extension_id, key)` — one extension
  cannot read another extension's KV namespace even within the same petal.
- All writes go through the existing op-log convention (see
  `fe-database/src/op_log.rs`), consistent with every other mutation path.
- Errors map to `StorageError` variants; no `unwrap()`/`expect()` in the impl.

### FR-2: Production `ExtensionQueryApi` backed by fe-query

**Description:** Implement `ExtensionQueryApi` using `fe-query`'s existing
`QueryBuilder` (Phase 6.1) rather than raw SurrealQL string interpolation, so
the SELECT-only guarantee is enforced at both the `is_select_only()` string
guard (defense in depth) and the query-builder layer (structural guarantee).

**Acceptance Criteria:**
- A non-test type implements `fe_sdk::query::ExtensionQueryApi` using
  `fe-query::builder`.
- Query scope is restricted to the extension's bound petal (or explicitly
  granted verse-wide scope via `CapabilityManifest.verse_scope`) — an
  extension cannot query nodes outside its capability grant regardless of
  what SQL it constructs.
- `MAX_RESULT_ROWS` truncation is preserved at this layer, not just in
  `HostEnv`.

### FR-3: Wire HostEnv into the fractalengine binary's plugin host

**Description:** In the `fractalengine` binary's plugin-host setup (wherever
`PluginContext::new` / `PluginRegistry` is constructed today), call
`.with_host_env(HostEnv::new().with_storage(...).with_query(...))` using the
FR-1/FR-2 implementations, so real plugin instances get real data access
instead of failing closed.

**Acceptance Criteria:**
- `PluginContext` instances created by the running binary carry a `HostEnv`
  with both `has_storage()` and `has_query()` true.
- An integration test (using `fe-plugin-test`'s harness) exercises a Rhai or
  WASM extension calling `query_select`/`storage_get`/`storage_set` against
  the real backing store (in-memory SurrealKV is fine) and gets real data
  back, not `NotAvailable`.

### FR-4: Analytics-oriented query surface

**Description:** The current `query_select` is a single raw SELECT statement.
The "3D P2P analytics engine" goal needs at least basic aggregation
(count/group-by over node properties, time-range filters using the op-log's
HLC timestamps) exposed as a safe, structured surface — not by loosening the
SELECT-only guard, but by adding a small set of pre-defined analytical query
shapes extensions can request through `fe-query`.

**Acceptance Criteria:**
- At least one structured analytics query shape (e.g. "count nodes by
  property value within a petal") is exposed through `ExtensionQueryApi` or a
  sibling method, implemented via `fe-query::builder`, not string
  concatenation.
- Every analytics query still passes through the same capability gate
  (`query.select` or a new, more specific capability — see
  `auth_policy_pattern_20260710` for the general shape this should take).

### FR-5: WIT parity check

**Description:** Re-verify the WIT `query-api` interface still matches the
Rust `HostEnv` surface after FR-1–FR-4 land (it may need a new function for
FR-4's analytics shape).

**Acceptance Criteria:**
- Every `HostEnv` public method has a corresponding WIT function (or a
  documented reason it's Rhai/host-only).
- `fe-plugin-test` gets a fixture exercising the WASM path against the new
  analytics shape, mirroring the existing Rhai fixture.

## Out of Scope

- Full OLAP/columnar analytics (DataFusion) — that remains Phase 6.2,
  intentionally deferred per MEMORY.md.
- Cross-petal or cross-verse federated analytics.
- A generic authorization policy engine — see `auth_policy_pattern_20260710`
  (this track should adopt that engine once it exists, not build its own
  parallel capability check).

## Dependencies

- `fe-sdk` (storage.rs, query.rs) — exists.
- `fe-plugin` (host_env.rs, capability.rs) — exists.
- `fe-query` (Phase 6.1, builder/) — exists.
- `fe-database` (op_log, schema) — exists; needs the new per-extension KV
  table (FR-1) and a query-builder-backed adapter (FR-2).
