---
type: Implementation Plan
title: "Implementation Plan: Shell UX — Modals→Sidebar + Sticky Sidebar"
tags: [shell_ux_sidebar_20260725]
resource: ./spec.md
---

# Implementation Plan: Shell UX — Modals→Sidebar + Sticky Sidebar

Three small, independently landable phases on the landed ui_shell seam. TDD on
the pure helpers; single sweep at the end (N-6). Cheapest, highest daily-
satisfaction track in the program — good momentum opener for Wave 0.

## Phase 1: User-sticky left sidebar (FR-3) [P0]

- [ ] Task: replace the auto-collapse default in `ui_shell/left_sidebar.rs` with
      a user-intent-driven policy; pure `left_visibility(policy, user_intent)`
      helper + unit tests (open survives selection + right-section open + petal
      switch; closed stays closed).
- [ ] Task: wire the explicit topbar toggle + shortcut to set user_intent; prove
      no per-frame frame-stomp remains (`panels/mod.rs`).

## Phase 2: Settings → right-sidebar section (FR-1) [P1]

- [ ] Task: register a Settings section in `ui_shell/right_sidebar.rs`; move the
      `dialogs/settings.rs` body into the section container, logic verbatim.
- [ ] Task: remove Settings from the `ActiveDialog` floating set; control-parity
      checklist (every widget present + functional); calm empty state.

## Phase 3: Maps (hexon manager) → right-sidebar section (FR-2) [P1]

- [ ] Task: register a Maps section; move the `dialogs/hexon_manager.rs` body in,
      preserving the choose-a-map-per-petal wiring and map/tile-source select.
- [ ] Task: remove the floating Map Manager; control-parity checklist; §9
      terminology check (map vs path).

## Phase 4: Docs + integrated sweep [P1]

- [ ] Task: update `fe-ui/src/ui_shell/AGENTS.md` + `panels/AGENTS.md` — sticky
      policy + the two new sections (N-7).
- [ ] Task: single sweep — `clippy -D warnings`, `fmt --check`, fe-ui tests
      (N-6); ui_ux §5/§7/§9 pre-merge checklist.
- [ ] Task: retro; in-app verify is user-gated (note it, do not self-sign).
