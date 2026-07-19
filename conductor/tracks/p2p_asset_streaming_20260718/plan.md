---
type: Implementation Plan
title: "Implementation Plan: P2P Asset Streaming"
tags: [p2p_asset_streaming_20260718]
resource: ./spec.md
---

# Implementation Plan: P2P Asset Streaming

## Overview

Phases 0–1 are decision-independent and agent-executable now. Phase 2 is the
ratification wall (D-73…D-77). Phases 3–6 execute the ratified choices; their
internal ordering is transfer-before-residency so the residency ledger has a
real byte source to stream from, but FR-4's ledger work can start against the
local-disk source the moment D-74 is ratified (it does not require FR-3).
Single workspace sweep at the end per standing policy.

## Phase 0: Evidence pack (FR-2) — no gate

- [ ] Task: Tile/asset size histogram across real hexons (user's installed
      tilesets + gis-tile-etl outputs); record p50/p90/p99 and per-zoom
      distributions into ./evidence.md (grounds D-73 bundle sizing)
- [ ] Task: Read + digest `_archive/render_distance_lod_20260407` and
      `_archive/relay_data_horizon_20260407`; fold deltas into the decision
      record before ratification (relay_data_horizon was NOT covered by the
      exploration sweep)
- [ ] Task: Double-residency memory math + `Arc<HexonTileSource>` dedup
      sketch (consumer map of `registry.load_all` vs per-assignment loads)
- [ ] Verification: decision record updated with evidence deltas [checkpoint]

## Phase 1: Relay loud-fail hardening (FR-1) — no gate

- [ ] Task: `RelayConfig` (Default/Custom/Disabled) plumbed into
      `SyncEndpoint::new`; startup warning on EOL-bound default n0 relays (TDD:
      config parsing + warning emission)
- [ ] Task: Relay health surfaced in `SyncStatus` (TDD: status transitions)
- [ ] Verification: headless run shows relay mode + health; sweep-deferred [checkpoint]

## Phase 2: Ratification wall — RESOLVED 2026-07-18

- [x] 2.1 D-73 granularity → **A4** bundles-as-HashSeq
- [x] 2.2 D-74 residency shape → **ledger driving a pointer-set** (no artifact),
      in VerseManagerPlugin; scene = `{(hexon_uri, bundle_hash), …}` recomputed
      per frame
- [x] 2.3 D-75 registry parity → **C1** member-granular + `PartialHexonFetch`
- [x] 2.4 D-76 integrity → **D2** Merkle-extended signature root
- [x] 2.5 D-77 iroh timing → **E2** build-on-0.35 behind transport trait
- [x] 2.7 D-78 → **application settings surface** ratified (see Phase 4b / FR-7)
- [ ] 2.6 Scope call (NOT a ratification gate): does FR-4 subsume
      runtime_instance_guardrails FR-6? Decide at Phase 4 start.

## Phase 3: Transport + protocol fill (FR-3, after D-73/D-77)

- [ ] Task: Transport trait (hash + ranges + HashSeq semantics only); iroh 0.35
      impl; ALPN/protocol router registration (TDD against in-memory transport)
- [ ] Task: Real `handle_request_tileset_meta` / `handle_request_chunk` bodies;
      per-chunk blake3 verified on receipt; spawn_blocking I/O (TDD)
- [ ] Task: Cross-restart resume for TilesetDownloadTracker (TDD)
- [ ] Verification: two-instance loopback transfer of a chunked hexon [checkpoint]

## Phase 4: Residency ledger + lazy tiles (FR-4, after D-74)

- [ ] Task: Distance-ranked ledger resource replacing `mesh_budget.exceeded`
      boolean; first system in VerseManagerPlugin chain; hysteresis (TDD: pure
      ledger math, ring membership, hysteresis windows)
- [ ] Task: Lazy `HexonTileSource` (offset-index header phase, on-demand member
      reads); async `fetch_tile` dispatch; double-residency dedup via Arc (TDD)
- [ ] Task: Despawn-on-exit-ring eviction wired to the ledger (TDD)
- [ ] Verification: NFR-1/NFR-2 evidence — no spawn spike across ring
      transitions, frame-time delta within noise on tile faults [checkpoint]

## Phase 4b: Application settings surface (FR-7 / D-78) — decision-independent

- [ ] Task: `AppSettings` Bevy `Resource` in fe-ui + RON/TOML persistence under
      the platform config dir (load-on-start, save-on-change; RON per the
      archived render_distance_lod design unless TOML is chosen — see open Q)
- [ ] Task: `ActiveDialog::Settings` egui window; first knob = live
      `MeshInstanceBudget.ceiling` (already a runtime field), then render
      distance, entity/stamp caps, tile mode, camera sensitivity/zoom/easing,
      relay/peer config (TDD: settings (de)serialize round-trip, clamp ranges)
- [ ] Task: Route the hardcoded `const` limits through `AppSettings` with current
      constants as defaults; `PetalManifest.render_distance` = per-petal override
- [ ] Verification: settings persist across restart; ledger + terrain editor read
      the same resource [checkpoint]

## Phase 5: Registry parity + integrity (FR-5 + FR-6, after D-75/D-76)

- [ ] Task: `PartialHexonFetch` trait; HTTP impl (chunk-by-seq + meta routes on
      fe-hexon-registry); FetchStrategy as shared policy layer (TDD)
- [ ] Task: `HexonArchive::import` split (header phase / member reads) (TDD)
- [ ] Task: Integrity enforcement — signed digests committed, asset_hash checked
      at install + publish (coordinate manifest fields with hexon_unification)
- [ ] Verification: hosted and P2P paths pass the same partial-fetch conformance
      suite [checkpoint]

## Phase 6: Close-out

- [ ] Task: Single end-of-track workspace sweep (test/clippy/fmt)
- [ ] Task: Retro + archive per track-per-feature workflow; decision record
      status flipped to closed with dated resolutions
