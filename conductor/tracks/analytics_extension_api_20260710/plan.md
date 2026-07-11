---
type: Implementation Plan
title: Analytics Extension API
tags: [feature, analytics_extension_api_20260710, in_progress]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Implementation Plan: Analytics Extension API

**Track ID:** `analytics_extension_api_20260710`
**Type:** Feature
**Crates:** `fe-database`, `fe-query`, `fe-plugin`, `fractalengine`

See [./spec.md](./spec.md) for full requirements and the "what already exists"
inventory — `fe-sdk::storage`/`query` traits, `fe-plugin::host_env::HostEnv`
capability-gated routing, and the WIT `query-api` interface are already built.
This plan only covers wiring a real backend behind them.

---

## Phase 1: Production ExtensionStorageApi (FR-1)

**Goal:** Real, op-log-backed implementation of `ExtensionStorageApi`.

**Files touched:** `fe-database/src/handlers/extension_storage.rs` (new), `fe-database/src/schema.rs`

### Tasks

- [ ] Task 1.1: Add `extension_kv` table to schema, keyed by `(petal_id, extension_id, key)` (TDD: write a schema-application test asserting the table exists; add `DEFINE TABLE extension_kv SCHEMAFULL` + fields)
- [ ] Task 1.2: Implement `node_get_properties`/`node_set_property` against existing node-properties tables (TDD: round-trip test — set then get returns the same `PropertyValue`)
- [ ] Task 1.3: Implement `storage_get`/`storage_set` against `extension_kv`, namespace-isolated per extension (TDD: two different `extension_id`s writing the same key must not collide — mirrors the existing `host_env.rs` `kv_roundtrip_is_namespaced` test but against the real backend)
- [ ] Task 1.4: Route all writes through `op_log` (TDD: assert an `op_log` entry is written for every `storage_set`/`node_set_property` call)
- [ ] Verification: `cargo test -p fe-database`, `cargo clippy -p fe-database -- -D warnings`. [checkpoint marker]

## Phase 2: Production ExtensionQueryApi (FR-2)

**Goal:** `fe-query`-builder-backed `ExtensionQueryApi`, scoped to the extension's capability grant.

**Files touched:** `fe-query/src/builder/extension.rs` (new)

### Tasks

- [ ] Task 2.1: Implement `ExtensionQueryApi` using `fe_query::builder::QueryBuilder` (TDD: a SELECT within the granted petal scope returns rows; a SELECT attempting to escape scope — e.g. via a crafted `WHERE`  — is rejected or scoped away, not just relying on `is_select_only()`)
- [ ] Task 2.2: Enforce `MAX_RESULT_ROWS` at the builder layer (TDD: request > cap, assert truncation)
- [ ] Verification: `cargo test -p fe-query`. [checkpoint marker]

## Phase 3: Wire HostEnv into the binary (FR-3)

**Goal:** Real plugin instances get real data access.

**Files touched:** `fractalengine/src/main.rs` (or wherever `PluginContext`/`PluginRegistry` is constructed)

### Tasks

- [ ] Task 3.1: Construct `HostEnv::new().with_storage(Arc::new(<Phase1 impl>)).with_query(Arc::new(<Phase2 impl>))` and attach via `PluginContext::with_host_env` (TDD: integration test using `fe-plugin-test`'s `MockHostEnv`-adjacent real-backend harness — a Rhai fixture calling `storage_set`/`query_select` gets real data, not `NotAvailable`)
- [ ] Verification: `cargo test -p fractalengine -p fe-plugin`. [checkpoint marker]

## Phase 4: Analytics query surface (FR-4)

**Goal:** At least one structured analytics query shape beyond raw SELECT.

**Files touched:** `fe-sdk/src/query.rs`, `fe-query/src/builder/extension.rs`

### Tasks

- [ ] Task 4.1: Add `count_by_property(petal_id, property_key) -> Vec<(PropertyValue, u64)>` (or similar) built via `fe-query::builder`, capability-gated the same as `query_select` (TDD: seed nodes with varying property values, assert correct grouped counts)
- [ ] Verification: `cargo test -p fe-sdk -p fe-query`. [checkpoint marker]

## Phase 5: WIT parity (FR-5)

**Goal:** WIT `query-api` interface stays in lockstep with the Rust surface.

**Files touched:** `fe-plugin/wit/hexon-plugin.wit`, `fe-plugin-test` WASM fixture

### Tasks

- [ ] Task 5.1: Add WIT function for the Phase 4 analytics shape; regenerate bindings (TDD: WASM fixture calls the new function and asserts a result)
- [ ] Verification: `cargo test -p fe-plugin -p fe-plugin-test`, full workspace quality gate per `conductor/workflow.md`. [checkpoint marker]
