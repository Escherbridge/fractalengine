---
type: Track Spec
title: IoT Extension Slice — Device ↔ Node Push/Pull Proof
tags: [feature, iot_extension_slice_20260710, in_progress]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Specification: IoT Extension Slice

**Track ID:** `iot_extension_slice_20260710`
**Type:** Feature
**Status:** In progress
**Goal alignment:** 3D P2P analytics engine on the hexon format, with an extension storage/query API and Rhai/WASM scripting.

## Overview

Build one real IoT bridge extension end-to-end — the first extension to
actually exercise `analytics_extension_api_20260710`'s storage/query surface
in a production scenario, proving the full device-to-node-to-device loop
through the plugin host rather than a one-off integration.

## Background — what already exists (verified 2026-07-10)

- `fe-terrain/src/iot/path_tracker.rs` (304 lines) — `PathTracker::snap_to_route`
  snaps a raw device position onto a GPX route (`SnapResult`). One-directional
  (device → visualization), no persistence beyond the current frame, no
  extension/plugin involvement.
- `fe-terrain/src/iot/animation.rs` (168 lines) — animates a node along a
  tracked path.
- `fe-plugin-test/src/fixtures.rs::sensor_network()` — a 50-node fixture with
  `sensor_id` and temperature properties, built for exactly this kind of
  scenario, currently unused by any real extension.
- `analytics_extension_api_20260710` (sibling track, dependency) provides the
  capability-gated storage/query surface this extension will call through.

**What is missing:** an actual `FractalExtension` implementation that (a)
receives inbound device telemetry and writes it as node properties via the
storage API, and (b) reads node/query state and emits an outbound command
back toward a device — i.e. both directions of the loop, not just ingestion.

## Functional Requirements

### FR-1: Inbound push — device telemetry → node property

**Description:** An extension (Rhai first; WASM if time allows) receives a
telemetry payload (device id, timestamp, key/value readings) and writes each
reading as a node property via `ExtensionStorageApi::node_set_property`,
keyed by a device→node mapping (reuse `sensor_network()`'s `sensor_id`
convention: `node_id == "sensor_{device_id}"`).

**Acceptance Criteria:**
- Extension has a `on_telemetry` (or equivalent scene-hook-adjacent) entry
  point invoked with a telemetry payload.
- Writes go through `storage_api` (capability `storage.write`), not a
  side-channel — this is the proof that the analytics extension API is a
  real path, not just a spec.
- Unknown device ids fail closed (no property write, logged) rather than
  silently creating arbitrary nodes.
- Test: feed N telemetry events for existing `sensor_network()` nodes, assert
  node properties reflect the latest reading per key.

### FR-2: Outbound pull — node/query state → device command

**Description:** The same extension exposes a way to read aggregated node
state (e.g. "which sensors are above threshold X") via
`ExtensionQueryApi::query_select` (or the Phase 4 analytics shape from
`analytics_extension_api_20260710` if it lands first) and produce an outbound
command list — proving the pull/control direction, even if the actual device
transport is a stub (log the command, don't require real hardware).

**Acceptance Criteria:**
- A query-driven function returns the set of devices needing a command
  (e.g. temperature over threshold → "cool_down" command).
- Capability-gated via `query.select`; fails closed if not granted.
- Test: seed nodes above/below threshold, assert only the correct subset
  produces commands.

### FR-3: Round-trip proof test

**Description:** One `fe-plugin-test`-driven integration test exercises the
full loop: telemetry in (FR-1) → property persisted → query reads it back
(FR-2) → command list produced — using `RhaiTestRunner` and the existing
`sensor_network()` fixture, no real hardware or network involved.

**Acceptance Criteria:**
- Single test function demonstrates push then pull without manual setup
  beyond the fixture.
- Runs in `cargo test -p fe-plugin-test` with no external dependencies.

## Out of Scope

- Real device transport (MQTT, CoAP, physical hardware) — this track proves
  the plugin-host loop, not a production IoT ingestion pipeline.
- P2P distribution of telemetry (would layer on `hexon_delta_format_20260710`
  once that exists) — out of scope this round.
- Authorization policy design — this track is a *consumer* of whatever
  capability model `analytics_extension_api_20260710` /
  `auth_policy_pattern_20260710` establish, not a place to invent new rules.

## Dependencies

- `analytics_extension_api_20260710` — must have at least FR-1/FR-2/FR-3
  (production storage/query backend) landed before this track's FR-1/FR-2 can
  use real data instead of the mock host.
- `fe-plugin-test` (`sensor_network()` fixture, `RhaiTestRunner`) — exists.
- `fe-terrain/src/iot/` — existing one-directional telemetry code this track
  complements, does not replace.
