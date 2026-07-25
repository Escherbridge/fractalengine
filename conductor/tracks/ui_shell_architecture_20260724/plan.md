---
type: Implementation Plan
title: "Implementation Plan: UI Shell Architecture — Manager Split, Right-Sidebar Tool Surfaces, P0 Fixes"
tags: [ui_shell_architecture_20260724]
resource: ./spec.md
---

# Implementation Plan: UI Shell Architecture + P0 Fixes

## Overview

Seven phases, strictly ordered so the P0 fixes land first and alone
(Phase 0 is committable before any refactor starts), then the manager seams
(pointer → area managers → modal), then the two user-visible migrations
(floating windows → right sidebar; description panel → tooltips), then docs +
one integrated sweep. Every phase is independently landable; behavior-preserving
phases (1-3) must show all pre-existing tests green as their own gate.

Priorities: **Phase 0 = P0** (ship immediately). **Phases 1-6 = P1.** P2 items
(carried-forward tool_inspector_ux models: select filters, `SnapSettings`,
`TransformConstraints`) are explicitly NOT in this plan — they are backlog
inside the right-sidebar Tool section, noted in the spec's supersession table.

TDD throughout: pure decision helpers first, thin egui shells after. Single
workspace sweep at the very end (standing policy; use
`RUST_MIN_STACK=134217728 cargo test --workspace -j2` if the OOM blocker is
live). In-app verification steps are listed per checkpoint and are user-gated.

## Phase 0: P0 bug fixes (land first, alone) [P0]

Goal: the terrain tools no longer crash the app, and path points/handles are
selectable + draggable from direct viewport interaction.

Tasks:
- [ ] Task: **Crash repro + root cause (FR-1).** NOT DONE this pass (2026-07-24
      worker: FR-2 + panic-hook diagnostic infra only). The fc9800f/190188f
      data-shape fix already landed on `main` (proposals-only `terrain_json`
      shape hardening), but the live panic payload from the user's reported
      crash is still uncaptured — no finding doc, no confirmed hypothesis.
      `fractalengine/src/panic_log.rs` (this pass) is the diagnostic
      prerequisite: the next terrain-tools repro attempt now appends
      timestamp/thread/payload/location/backtrace to `data/panic.log` instead
      of only surfacing bevy_egui's opaque "Encountered a panic in system" +
      exit 101. Run with `RUST_BACKTRACE=1`;
      repro matrix: Tools → Terrain Tools → palette clicks / "Add proposal" /
      select + delete proposal, each on (a) a petal WITH an installed map and
      (b) a petal WITHOUT terrain config — hypothesis H-C1 predicts (b) crashes
      via the proposals-only `terrain_json` from `embed_proposals`
      (`actions/terrain_proposal.rs:18-28,39-46`). Capture the true panic
      payload; write the finding (hypothesis held + backtrace) to
      `conductor/tracks/ui_shell_architecture_20260724/finding-crash.md`.
      Check H-C2 (stale `selected_point`/`selected_segment` indexing in fresh
      pen-curve panel code) and H-C3 (egui invariant) only if H-C1 clears.
- [ ] Task: **Crash fix + regression test (FR-1).** NOT DONE this pass — blocked
      on the repro/root-cause task above. (TDD: write the failing
      test first from the captured repro — e.g. if H-C1: a unit test feeding a
      proposals-only terrain doc through the consumer that panicked; then fix;
      then refactor.) No new `unwrap`/`expect`/unguarded index in the touched
      path.
- [x] Task: **Point-selection enabling fix (FR-2).** DONE (2026-07-24, local,
      not committed). Eager-load
      `path_state.tracks` on active-petal change: extended the petal-change
      branch of `open_track_on_select` (`node_manager/viewport_pick.rs:103-110`,
      factored into the new `advance_petal_tracking` helper) to push the
      track-list refresh (reuses the `request_tracks`/`query_tracks` idiom via
      `UiAction::PathQueryTracks`, the same action the Paths tab pushes —
      `actions/path.rs:17-30`), replacing sole reliance on the render-gated
      load at `panels/gis_panel.rs:108-116`. `request_track_list_refresh` skips
      the push while `tracks_pending` (dedup guard). Traced-warning-on-silent-
      no-op (`viewport_pick.rs:163-168`) DEFERRED: the eager-load fix removes
      the main starvation path, and reliably distinguishing "clicked node looks
      path-like" from an ordinary node click needs data not available at that
      call site (e.g. a `TrackPickShape` check) — adding a warning without it
      risks per-click log spam on every non-track selection while the list is
      still loading. Left for a follow-up pass.
      (TDD: exactly one refresh request per petal change
      [`advance_petal_tracking_fires_exactly_once_per_transition`]; `track_to_open`
      existing tests green; refresh not re-fired while `tracks_pending`
      [`request_track_list_refresh_noop_while_already_pending`].)
- [x] Task: **Reachability confirmation tests (FR-2).** DONE at the unit level
      (2026-07-24): `track_to_open`'s existing tests plus the new
      `advance_petal_tracking`/`request_track_list_refresh` tests together pin
      the chain from track-list arrival through the `PathSelectTrack` dispatch;
      `path_handle_interaction.rs`'s pre-existing gate tests (untouched, assume
      `editing_track_id: Some`) are unaffected and stay green. The deeper
      in-app chain (`start_editing` → handle/vertex markers spawn → drag) is
      NOT re-verified here — it was already covered by that file's own tests
      and this pass didn't touch it; full end-to-end reachability remains
      in-app/user-gated (see Verification below).
- [ ] Verification: NOT DONE this pass — user-gated in-app step, and FR-1
      (terrain-tools crash) is still open so the full Phase-0 checkpoint isn't
      closeable yet. fresh session → click imported path in viewport → markers
      appear, vertex + bezier-handle drag work under Select/Move/Pen; terrain
      tools full repro matrix crash-free. Commit Phase 0 as its own batch
      (`fix(fe-ui): ...`) — deliberately NOT committed this pass per the
      orchestrator's no-commit instruction. In-app confirmation user-gated. [checkpoint]

## Phase 1: Pointer-operations manager (FR-3) [P1]

Goal: one documented seam owns click claiming + object-aware dispatch +
cross-authority coordination. Behavior-preserving.

Tasks:
- [x] Task: **Claim-priority table.** (landed 2026-07-24, swarm) Extract the effective claim order
      (handle > vertex > segment > gimbal-axis > node pick > empty) into a
      pure, exported table next to `ClickArbiter` (`node_manager/router.rs:34-124`)
      with a test asserting the order covers every `HitTarget` variant
      (`dispatch.rs:28-47`) exactly once. (TDD: table test first; then assert
      each consumer system's registration order in `NodeManagerPlugin` matches
      the table — a compile-time-adjacent constants test, not a runtime probe.)
- [x] Task: **Re-home the cross-authority bridge.** (landed 2026-07-24, swarm) Move `open_track_on_select`
      (+ `track_to_open`, `spawned_in_petal` and the Phase 0 eager-load) from
      `viewport_pick.rs:92-168` into the pointer module, keeping tests; the
      pointer manager is now the ONLY writer that coordinates the two
      authorities, exclusively via queued `UiAction`s (NFR-1). (TDD: moved
      tests green from the new path; grep-test/doc-assert that no panels module
      references `NodeManager.selected` write paths.)
- [x] Task: **No-bypass audit.** (landed 2026-07-24, swarm) Sweep the six consumer systems
      (`viewport_pick`, `path_point_interaction`, `path_handle_interaction`,
      `path_segment_interaction`, `path_gimbal_drag`, `gimbal_interaction`) —
      every left-click decision routes through `ClickArbiter` claim +
      `resolve_operation`; fix any stragglers found. (TDD: existing
      interaction tests stay green; add a table test for any newly-routed arm.)
- [ ] Verification: full `node_manager` test module green; in-app smoke —
      click-claim behavior unchanged (node pick, vertex drag, ribbon segment,
      gimbal). [checkpoint]

## Phase 2: Area tab managers — topbar, left, right (FR-4, FR-5, FR-6 shell) [P1]

Goal: `gardener_console` becomes a thin composition over three area managers;
the right sidebar gains its section/rail model (sections still hosting only the
existing inspector content this phase).

Tasks:
- [x] Task: **`ui_shell` module scaffold.** (landed 2026-07-24, swarm) New `fe-ui/src/ui_shell/` with
      `topbar.rs`, `left_sidebar.rs`, `right_sidebar.rs` manager modules; each
      = small state resource + pure decision helpers + thin render fn.
      `panels/mod.rs::gardener_console` (`panels/mod.rs:51-191`) shrinks to
      ordered manager calls; `gardener_ui_system` registration
      (`plugin.rs:472-479`) unchanged — still one `EguiPrimaryContextPass`
      entry (NFR-4).
- [x] Task: **Topbar manager (FR-4).** (landed 2026-07-24, swarm) Owns the `TOOL_DEFS` mode switcher +
      the window/section toggle buttons; toggles now write right-sidebar
      section state instead of window-open flags (compat shim: the flags the
      floating windows still read this phase are mirrored until Phase 4
      removes them). (TDD: `TOOL_DEFS` single-source tests stay green; toggle
      round-trip test topbar→right-manager state.)
- [x] Task: **Left manager (FR-5).** (landed 2026-07-24, swarm) Replace the per-frame stomp
      `sidebar.open = !right_panel_open` (`panels/mod.rs:97-99`) with pure
      `left_visibility(policy, right_open, user_intent) -> bool` owned by the
      manager; default policy preserves today's behavior exactly. (TDD:
      truth-table test incl. the preserved default.)
- [x] Task: **Right manager shell (FR-6).** (landed 2026-07-24, swarm) `RightSidebarSection` enum
      (Inspector | Tool | PathTools | TerrainTools | ProposalReport) + rail
      rendering + section-reveal state; this phase it hosts Inspector (moved
      call, `inspector.rs:37` panel becomes manager-owned) and an empty calm
      placeholder for the rest; portal-open still swaps the whole region to
      the portal toolbar. (TDD: pure `active_section(state, selection,
      portal_open)` precedence test — portal > explicit toggle > selection
      default; never-blank guarantee.)
- [ ] Verification: pixel-parity smoke in-app (same surfaces, same places);
      all pre-existing panel tests green; `gardener_console` param list
      unchanged or reduced. [checkpoint]

## Phase 3: Modal manager + panel panic guard (FR-7) [P1]

Goal: tooltips/toasts/context-menu/dialogs under one transient-layer manager;
one broken panel can never again abort the app.

Tasks:
- [x] Task: **Modal manager.** (landed 2026-07-24, swarm) `ui_shell/modal.rs` owning render order for the
      transient layer: dialogs (`ActiveDialog` mutual-exclusion set,
      `dialogs/*.rs`), context menu, toast overlay (`panels/mod.rs:193-217`),
      and the tooltip helpers FR-8 will consume. Rendering stays last in the
      pass (layering preserved). (TDD: pure ordering/exclusivity test — one
      dialog at a time; toast unaffected by dialog state.)
- [x] Task: **Panic guard.** (landed 2026-07-24, swarm) `guarded(name, ui_fn)` wrapper using
      `catch_unwind(AssertUnwindSafe(...))` applied at every manager's
      panel/section/window boundary; on catch: `tracing::error!`, mark the
      panel disabled-for-session in modal-manager state, render a persistent
      status-bar error segment (`ui_ux.md` §6 Error tier); debug builds
      re-propagate under `cfg(debug_assertions)` (spec Open Q-5 recommendation
      — adjust if ratified otherwise). (TDD: deliberately-panicking test panel
      is caught once, disabled, frame completes; zero-allocation happy path
      [no Box/String when no panic]; debug-propagation behind cfg tested via
      the release-shaped path.)
- [ ] Verification: inject a panic into a scratch panel in-app — app survives,
      error chip visible, other panels functional. [checkpoint]

## Phase 4: Floating tool windows → right-sidebar sections (FR-9) [P1]

Goal: "Tools", "Terrain Tools", and "Proposal Report" dissolve into FR-6
sections with toggle reveal; zero control loss.

Tasks:
- [x] Task: **Control inventory checklist.** (landed 2026-07-24, swarm) Enumerate every widget in
      `tool_panel.rs:220-` (path-asset stamp, pen curve/shape, terrain toggle),
      `terrain_tools_panel.rs:42-232`, `proposal_report_panel.rs:45-90` into
      the track folder as the zero-loss diff basis.
- [x] Task: **Path tools section.** (landed 2026-07-24, swarm) Move the stamp + pen sections into
      `right_sidebar` PathTools; keep `installed_assets`/`filter_assets`/
      `build_descriptor` + tests intact; `UiAction::PathAssetApply` still
      targets `PathEditorState.editing_track_id` ONLY (NFR-1). (TDD: migrated
      helper tests green from new home; emitted `UiAction` variants unchanged.)
- [x] Task: **Terrain tools + report sections.** (landed 2026-07-24, swarm) Move palette/controls/proposal
      list and the report body; keep `TerrainToolMode` mirror +
      `to_proposal_op` (NFR-2) and `compute_report` + tests verbatim;
      `TerrainProposalAdd/Delete` emissions unchanged. The Phase 0 crash
      regression test must still pass against the migrated surface. (TDD: same
      emissions; report fixtures green.)
- [x] Task: **Remove the floating windows + compat flags.** (landed 2026-07-24, swarm) Delete the three
      `egui::Window` call sites from the shell (`panels/mod.rs:179-185`
      equivalents post-Phase-2), retire the mirrored open-flags from Phase 2's
      shim; topbar/rail toggles are now the only reveal path. (TDD: checklist
      diff shows zero lost controls; no dangling `terrain_tools_open`-class
      flags left unread.)
- [ ] Verification: every workflow (stamp along path, add/delete terrain
      proposal, read proposal report) executes from the right sidebar; old
      windows gone; crash repro matrix re-run clean. [checkpoint]

## Phase 5: Tool descriptions → tooltips; retire the left tool panel (FR-8) [P1]

Goal: directive 2 — reclaim the real estate; keep the legibility wins.

Tasks:
- [x] Task: **Rich tooltips from the single-source table.** (landed 2026-07-24, swarm) Extend `TOOL_DEFS`
      (or join it with `panel_descriptor`) so each mode button's hover tooltip
      renders title + shortcut + one-line description from ONE table. (TDD:
      tooltip-content test covers every `Tool`; extends the existing
      `tool_defs_cover_all_tools…` pattern so drift is impossible.)
- [x] Task: **Re-home live readouts.** (landed 2026-07-24, swarm) Move `selection_summary`,
      `gimbal_affordance_label`, `anchor_readout` (+ tests) from
      `panels/tool_inspector.rs` into the right-sidebar Tool section body.
      (TDD: helper tests green from new home; Tool section shows gimbal-active
      affordance exactly when a gimbal is drawn.)
- [x] Task: **Remove the left panel.** (landed 2026-07-24, swarm) Delete the `SidePanel::left("tool_inspector")`
      (`tool_inspector.rs:185`) + its call site (`panels/mod.rs:107-109`
      equivalent); honor Open Q-1's ratified answer (default: remove entirely).
      Update the supersession note in `tool_inspector_ux_20260719` handling —
      orchestrator flips that track's metadata to `superseded`.
- [ ] Verification: no always-open tool panel; hover each of S/G/R/X/P buttons
      shows its tooltip; Tool section carries the readouts; viewport visibly
      wider. [checkpoint]

## Phase 6: Docs + integrated sweep + close-out [P1]

Goal: directory docs reflect the manager topology; one green sweep; retro.

Tasks:
- [x] Task: **Directory AGENTS.md updates.** (landed 2026-07-24, swarm) New `fe-ui/src/ui_shell/AGENTS.md`
      (manager topology, claim-priority table, panic-guard contract, section
      model); update `panels/AGENTS.md` (removed windows/panels, tooltip
      source) and `node_manager/AGENTS.md` (§pointer-manager, bridge re-home);
      terse one-line pointers in code (NFR-7).
- [x] Task: **`ui_ux.md` conformance pass.** (landed 2026-07-24, swarm) Pre-merge checklist over every
      touched surface (§1 luminance, §2 units, §6 failure tiers — the new
      error segment, §7 calm empty states, §9 terminology).
- [x] Task: **Single end-of-track sweep** (1.97.1 clippy + push pending) — `cargo test --workspace`,
      `cargo clippy -- -D warnings` (latest stable; consult the 2026-07-23
      toolchain-drift playbook if CI-only warnings appear), `cargo fmt --check`.
      List the user-gated in-app verification steps in the track folder; write
      the retro; orchestrator archives + updates the board and flips
      `tool_inspector_ux_20260719` to superseded. [checkpoint]
