---
type: Track Spec
title: P2P Asset Streaming — fine-grained hexon transfer + scene-driven residency
description: Ratify-and-build round for streaming parts of multiple hexons based on what is actually in the scene (render distance + entity caps) instead of forcing full downloads; fills the existing chunk-protocol stubs over iroh, adds registry partial-fetch parity, and hardens integrity + relay-EOL posture
tags: [feature, p2p, streaming, hexon, iroh, p2p_asset_streaming_20260718, pending]
timestamp: 2026-07-18T00:00:00Z
resource: ./metadata.json
---

# Specification: P2P Asset Streaming

**Track ID:** `p2p_asset_streaming_20260718`
**Priority:** P1 Platform — user-directed 2026-07-18 ("P2P is the main differentiator", D-71)
**Crates:** `fe-sync` (primary transport), `fe-terrain` (tile residency), `fe-ui`
(residency ledger), `fe-hexon` + `fe-hexon-registry` (partial fetch + integrity),
`fe-format` (integrity fields, via hexon_unification)

## Overview

Verbatim user ask (2026-07-18): *"create a decision round on how fine grained
p2p can be and how we can handle streaming asset data instead of forcing full
downloads as I'll predict these can be big. Ideally we can create an engine that
allows us to stream in parts of multiple hexons based on what's actually in the
scene and work based on render distance and entity caps."*

The decision round lives in
[`conductor/decisions/p2p-streaming-20260718.md`](../../decisions/p2p-streaming-20260718.md)
(**RATIFIED 2026-07-18 — D-73…D-77 locked to staged defaults A4/ledger/C1/D2/E2;
D-78 application-settings added**). Ratifying directive: *"no new primitive if we
can leverage pointers to multiple hexons instead"* — the active scene is an
**ephemeral pointer-set** `{(hexon_uri, bundle_hash), …}` the residency ledger
recomputes each frame, not a persisted artifact (see the decision record's
"## Ratification" section for the P2P / renderer / limit-enforcement mechanics).
The exploration memo with
full evidence is [`./memo.md`](./memo.md). Headline finding: a complete chunked
tileset protocol already exists in types and UI wiring
(`RequestTilesetMeta`/`RequestChunk`/`ChunkReceived`, `package_chunked`,
sequential UI driver) — the sync-thread handlers are stubs. This track is
"ratify granularity, then fill the stubs and wire residency", not green-field
invention.

## Functional Requirements

- **FR-1 — Relay loud-fail hardening (decision-independent, no gate).** Wire
  `RelayConfig` (Custom/Disabled/default-n0) into `SyncEndpoint::new`
  (`fe-sync/src/endpoint.rs:33-41`), surface relay health in `SyncStatus`
  (`status.rs:20-28`), and log a startup warning when running on the EOL-bound
  default n0 relays. Post-2026-12-31 must fail loudly, not queue silently.
- **FR-2 — Evidence pack for the decision round (decision-independent).**
  (a) Tile/asset size histogram from real hexons (gis-tile-etl output + user's
  installed tilesets) to ground D-73 bundle sizing; (b) digest of the two
  mandated archived prior-art specs (`render_distance_lod_20260407`,
  `relay_data_horizon_20260407` — the latter uncovered by the exploration
  sweep); (c) memory math for the double-residency fix (`Arc<HexonTileSource>`).
- **FR-3 — Bundle-granular P2P transfer** *(gated on D-73)*. Implement the
  staged default (A4): hexon published as an iroh HashSeq of size-tuned chunk
  bundles reusing `package_chunked`/`ChunkIndex`; real bodies for
  `handle_request_tileset_meta`/`handle_request_chunk`
  (`fe-sync/src/sync_thread.rs:727-759`); ALPN/protocol router registration so
  inbound transfer works at all; transport behind a trait per D-77 (no
  RangeSpec types in public seams). Cross-restart resume for the download
  tracker.
- **FR-4 — Scene-driven residency ledger** *(gated on D-74)*. Distance-ranked
  spawn allowance replacing the boolean `mesh_budget.exceeded` gate (extend
  `MeshInstanceBudget`), written as the first system in the VerseManagerPlugin
  chain; radial rings from `PetalManifest.render_distance` + camera-forward
  weighting with hysteresis; lazy tile-byte residency in fe-terrain (offset-index
  `HexonTileSource`, async `fetch_tile` dispatch); despawn-on-exit-ring
  eviction; double-residency dedup. This **implements the specced-but-unbuilt
  FR-6 "render horizon"** of `runtime_instance_guardrails_20260717` — coordinate
  scope with that track rather than duplicating it.
- **FR-5 — Registry partial-fetch parity** *(gated on D-75)*. `PartialHexonFetch`
  trait with an HTTP impl (member-granular routes on fe-hexon-registry: chunk by
  seq, meta without bytes) and the iroh impl from FR-3; fe-hexon's
  `FetchStrategy` becomes the shared policy layer; `HexonArchive::import` split
  into header phase + on-demand member reads.
- **FR-6 — Integrity for partial fetch** *(gated on D-76; format fields via
  `hexon_unification_20260716`)*. Per-chunk blake3 in `ChunkIndex`/
  `TilesetMetaReceived`; signed manifest commits to entries digest + chunk-index
  digest; `asset_hash` enforced at install and registry publish; signatures
  verified at publish. bao remains transport-layer verification only.
- **FR-7 — Application settings surface** *(D-78, decision-independent)*. New
  `AppSettings` Bevy `Resource` in fe-ui + RON/TOML persistence under the platform
  config dir + an `ActiveDialog::Settings` egui window. Exposes render distance,
  entity/mesh budget ceiling (`MeshInstanceBudget.ceiling` first — already a
  runtime field), stamp caps, tile source mode, camera sensitivity/zoom/easing,
  and P2P relay/peer config (folds in FR-1's `RelayConfig`). Route the hardcoded
  `const` limits through the resource with current constants as defaults;
  `PetalManifest.render_distance` becomes the per-petal override,
  `AppSettings.render_distance` the global default. Resurrects the archived-but-
  unbuilt `render_distance_lod_20260407` `AppSettings`/`SettingsPanel` design.
  Consumed by the FR-4 ledger and by `terrain_editor_overhaul_20260718`.

## Non-Functional Requirements

- **NFR-1 — Prevention, not recovery.** Bevy render buffers never shrink after
  a spiked frame; the residency ledger must gate spawns *before* any spike.
  Hysteresis mandatory (stamp groups already churn-respawn every 30-60 s).
- **NFR-2 — No main-thread hitches.** Lazy tile loading must not stall
  `get_tile_sync` callers; if it does, the async task-pool path becomes
  mandatory, not optional.
- **NFR-3 — Sync thread stays responsive.** Current-thread tokio: all transfer
  file I/O via spawn_blocking.
- **NFR-4 — Offline-provenance separation preserved.** Offline mode must never
  read online-origin cache entries.

## Out of scope

- Camera ground-clamp / distance defaults (`ux_interaction_hardening_20260718`
  FR-5, same-day work).
- Hexon format unification + signature-scheme collapse (owned by
  `hexon_unification_20260716`; FR-6 feeds requirements into it).
- The iroh 0.35 → 1.0 jump itself (`iroh_1_0_upgrade` track; D-77 sets its
  deadline posture).
- Auth/membership DAG, op-log signing (hexon-p2p-commons D1-D6 scope).
- Retiring either P2P stack (D-71 ratified KEEP fe-network).

## Open questions

- ~~D-73…D-77 option choices~~ — **RESOLVED 2026-07-18** (A4 / ledger-pointer-set
  in fe-ui / C1 / D2 / E2; D-78 settings added).
- Whether FR-4's ledger subsumes `runtime_instance_guardrails` FR-6 wholesale
  or that track keeps the crash-guardrail half (plan task 2.6 — still a scope
  call, not a ratification gate).
- Self-hosted relay bridge vs hard 1.0-upgrade deadline if 0.90+ blobs
  production-readiness slips (risk #10 in the decision record).
- FR-7 settings persistence format (RON per the archived design vs TOML) and
  config-dir strategy on Windows (the dev/primary platform).
