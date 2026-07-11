---
type: Implementation Plan
title: IoT Extension Slice
tags: [feature, iot_extension_slice_20260710, in_progress]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Implementation Plan: IoT Extension Slice

**Track ID:** `iot_extension_slice_20260710`
**Type:** Feature
**Crates:** `fe-plugin`, `fe-plugin-test`

See [./spec.md](./spec.md). Depends on `analytics_extension_api_20260710`
Phase 1-3 (production storage/query backend) landing first; this plan can
start against the existing mock host and swap to the real backend once
available.

---

## Phase 1: Inbound telemetry extension (FR-1)

**Goal:** A Rhai extension that writes device telemetry as node properties.

**Files touched:** `fe-plugin/src/rhai/` (extension script + host bindings if needed), `fe-plugin-test/src/fixtures.rs` (reuse `sensor_network()`)

### Tasks

- [ ] Task 1.1: Define telemetry payload shape (device_id, timestamp, readings map) and a device→node id mapping helper (TDD: mapping test for known/unknown device ids)
- [ ] Task 1.2: Implement `on_telemetry` handling that calls `storage_api.node_set_property` per reading (TDD: feed a payload, assert node properties updated via `MockHostEnv`/`SpyRecorder`)
- [ ] Task 1.3: Fail-closed handling for unknown device ids (TDD: unmapped device id produces no write + a logged rejection, not an error propagated to the caller)
- [ ] Verification: `cargo test -p fe-plugin -p fe-plugin-test`. [checkpoint marker]

## Phase 2: Outbound query-driven command (FR-2)

**Goal:** Read aggregated state, produce a command list.

**Files touched:** `fe-plugin/src/rhai/` (same extension, new function)

### Tasks

- [ ] Task 2.1: Implement threshold query using `query_select` (or Phase-4 analytics shape from the API track) (TDD: seed above/below-threshold nodes, assert correct subset selected)
- [ ] Task 2.2: Map query results to a stubbed outbound command list (log-only transport) (TDD: assert command list matches expected devices)
- [ ] Verification: `cargo test -p fe-plugin -p fe-plugin-test`. [checkpoint marker]

## Phase 3: Round-trip proof (FR-3)

**Goal:** One integration test proves push → persist → pull → command.

**Files touched:** `fe-plugin-test/tests/` (new integration test file)

### Tasks

- [ ] Task 3.1: Write the round-trip test using `RhaiTestRunner` + `sensor_network()` fixture (TDD: this test *is* the acceptance criterion — push N readings, assert query/command output reflects them)
- [ ] Verification: `cargo test -p fe-plugin-test`, then full workspace sweep per `conductor/workflow.md`. [checkpoint marker]
