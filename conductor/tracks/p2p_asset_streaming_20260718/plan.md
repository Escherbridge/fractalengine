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

- [x] Task: Tile/asset size histogram across real hexons (user's installed
      tilesets + gis-tile-etl outputs); record p50/p90/p99 and per-zoom
      distributions into ./evidence.md (grounds D-73 bundle sizing)
      — DONE 2026-10-11: `./evidence.md` §1-§3 (7 archives, per-(hexon,kind,zoom) p50/p90/p99/max; real tile pair ~110 KB p50 → bundle floor ~128 KiB, 1 MiB ≈ 6-9 pairs).
- [x] Task: Read + digest `_archive/render_distance_lod_20260407` and
      `_archive/relay_data_horizon_20260407`; fold deltas into the decision
      record before ratification (relay_data_horizon was NOT covered by the
      exploration sweep)
      — DONE 2026-10-11: `./evidence.md` §4; deltas folded into the decision record's "Evidence (2026-10-11)" section (data horizon orthogonal to D-74; archived pinning `RelayConfig` ≠ D-77 relay-server `RelayConfig`).
- [x] Task: Double-residency memory math + `Arc<HexonTileSource>` dedup
      sketch (consumer map of `registry.load_all` vs per-assignment loads)
      — DONE 2026-10-11: `./evidence.md` §5 (load_all consumed only by fe-api tile routes; renderer copy from `petal_binding.rs:153`; 14.6 MiB today, ~3.8 GB projected at z12; registry-owned `Arc` sketch).
- [x] Verification: decision record updated with evidence deltas [checkpoint]
      — DONE 2026-10-11: `conductor/decisions/p2p-streaming-20260718.md` "## Evidence (2026-10-11)" + risk #8 + D-74 prior-art line updated.

## Phase 1: Relay loud-fail hardening (FR-1) — no gate

- [x] Task: `RelayConfig` (Default/Custom/Disabled) plumbed into
      `SyncEndpoint::new`; startup warning on EOL-bound default n0 relays (TDD:
      config parsing + warning emission)
      — AUDIT 2026-10-10: DONE 7bdcfbe — `fe-sync/src/relay_config.rs:14` enum + `:100` EOL warn + `:133` `is_default_infra`; `fe-sync/src/endpoint.rs:72` `SyncEndpoint::new(secret_key, &RelayConfig)`; tests `relay_config.rs:206-276` (incl. `is_default_infra_only_for_default_variant`).
- [x] Task: Relay health surfaced in `SyncStatus` (TDD: status transitions)
      — AUDIT 2026-10-10: DONE 7bdcfbe — `fe-sync/src/status.rs:30` `health: RelayHealth`, `:104` `SyncEvent::RelayHealthChanged` applied; startup bind + gossip-spawn outcomes only (continuous `home_relay()` watcher still `TODO(ultrapilot)` at `fe-sync/src/sync_thread.rs:586`).
- [ ] Verification: headless run shows relay mode + health; sweep-deferred [checkpoint]
      — AUDIT 2026-10-10: OPEN — no recorded headless run showing relay mode/health.

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
      — AUDIT 2026-10-10: PARTIAL — soft distance-ranked allowance with hysteresis taper landed 7bdcfbe (`fe-ui/src/verse_manager/spawn.rs:34` `distance_ranked_allowance`, tests `spawn.rs:321-338`; called from `petal_respawn.rs:98`, `primitive_materialize.rs:149`, `path_asset_materialize.rs:424`), but it still sits behind the `mesh_budget.exceeded` boolean and no pointer-set `{(hexon_uri, bundle_hash)}` ledger resource exists (D-74 shape).
- [ ] Task: Lazy `HexonTileSource` (offset-index header phase, on-demand member
      reads); async `fetch_tile` dispatch; double-residency dedup via Arc (TDD)
- [ ] Task: Despawn-on-exit-ring eviction wired to the ledger (TDD)
- [ ] Verification: NFR-1/NFR-2 evidence — no spawn spike across ring
      transitions, frame-time delta within noise on tile faults [checkpoint]

## Phase 4b: Application settings surface (FR-7 / D-78) — decision-independent

- [x] Task: `AppSettings` Bevy `Resource` in fe-ui + RON/TOML persistence under
      the platform config dir (load-on-start, save-on-change; RON per the
      archived render_distance_lod design unless TOML is chosen — see open Q)
      — DONE 2026-10-11 (PAS-P4B, commit pending): RON at `dirs::config_dir()/fractalengine/settings.ron` (`fe-ui/src/settings.rs::{settings_path, load_settings_from, save_settings_to, sanitize}`); missing → defaults, partial → `#[serde(default)]` fill, corrupt → defaults + bad file kept as `settings.ron.corrupt-<ts>`; atomic tmp+rename save; trailing-edge 1 s debounced save (`SettingsSaveDebounce` / `save_app_settings_debounced`, only when the GUI binary inserts `SettingsPersistence`); loaded in `fractalengine/src/main.rs` before any thread spawns. Tests `saved_settings_survive_a_restart`, `missing_file_loads_defaults`, `corrupt_file_loads_defaults_and_keeps_a_backup`, `partial_file_fills_missing_fields_from_defaults`, `loaded_values_are_sanitized`, `sanitize_clamps_every_knob`, `debounce_saves_once_after_the_quiet_period`, `debounced_save_writes_only_with_persistence_configured`.
- [x] Task: `ActiveDialog::Settings` egui window; first knob = live
      `MeshInstanceBudget.ceiling` (already a runtime field), then render
      distance, entity/stamp caps, tile mode, camera sensitivity/zoom/easing,
      relay/peer config (TDD: settings (de)serialize round-trip, clamp ranges)
      — DONE 2026-10-11 (PAS-P4B, commit pending): right-sidebar Settings section (`fe-ui/src/ui_shell/right_sidebar.rs::render_settings_section`) now exposes render distance, mesh ceiling, entity cap, stamp ceiling, Offline-tiles checkbox, camera orbit-sensitivity / zoom-speed multipliers (live via `settings::sync_app_settings_to_camera`, test `camera_multipliers_scale_the_live_controller` + `camera_base_consts_match_the_controller_defaults`) and a relay text field validated by `RelayConfig::parse` ("Applies after restart"). **Not exposed (ui_ux §6, no faked settings):** `camera_easing` — persisted, no consumer (controller `easing_rate` is a rate, not a 0–1 factor); peer config — no consumer. **Assumption (tile mode):** `AppSettings.tile_mode = Offline` is a global no-network override applied by `fractalengine/src/terrain_bridge.rs::apply_global_tile_mode`; any other value lets the petal's `TerrainConfig.tile_source_mode` govern (test `global_offline_setting_overrides_the_petal_tile_mode`). Reversal: drop that system + the checkbox. Relay: `fe-sync/src/relay_config.rs::resolve` (env `FE_SYNC_RELAY` > settings > Default; invalid → Default, test `resolve_precedence_table`); the GUI registers the spec via `set_settings_relay_spec` before spawning the sync thread.
- [x] Task: Route the hardcoded `const` limits through `AppSettings` with current
      constants as defaults; `PetalManifest.render_distance` = per-petal override
      — DONE 2026-10-11 (PAS-P4B, commit pending): `entity_cap` / `stamp_ceiling` / `mesh_budget_ceiling` default to and are clamped under the hard caps; `effective_render_distance` + session `PetalRenderOverride` (set by `actions/hexon.rs::manifest_save` for the active petal, cleared on petal change); every spawner reads `ResidencyBudget::render_distance_for` (`petal_respawn.rs`, `primitive_materialize.rs`, `path_asset_materialize.rs`). Tests `effective_render_distance_prefers_a_valid_override`, `active_petal_manifest_save_overrides_the_residency_render_distance`. **Open decision:** `PetalManifest` has no durable store (manifest_save was a log stub; `petal.hexon_manifest` has no handlers and is a signed publishing artifact), so the override is session-only — where should a durable per-petal render distance live?
- [x] Verification: settings persist across restart; ledger + terrain editor read
      the same resource [checkpoint]
      — 2026-10-11: restart persistence proven at the save/load seam (`saved_settings_survive_a_restart`); residency spawners and the terrain bridge read the one `AppSettings` resource. `cargo +1.99.0 clippy --all-targets -D warnings` clean on fe-sync/fe-terrain/fe-ui/fractalengine; `cargo test -p fe-ui` 965 passed, `-p fractalengine` 62 passed, `-p fe-sync` 225 passed (logs `.omc/research/pas-test-*.log`). **USER_GATED:** change a knob → restart → value persists; relay field takes effect after restart; camera multipliers feel right; Offline checkbox stops tile network fetches.

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
