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

- [x] Task: replace the auto-collapse default in `ui_shell/left_sidebar.rs` with
      a user-intent-driven policy; pure `left_visibility(policy, user_intent)`
      helper + unit tests (open survives selection + right-section open + petal
      switch; closed stays closed). **[audit 2026-10-10: DONE b3f4df4 — `left_visibility(policy, user_intent)` fe-ui/src/ui_shell/left_sidebar.rs:51 (Manual-only policy); tests default_state_is_manual_and_open, open_intent_survives_selection_right_open_and_petal_switch, closed_intent_stays_closed (:94-110)]**
- [x] Task: wire the explicit topbar toggle + shortcut to set user_intent; prove
      no per-frame frame-stomp remains (`panels/mod.rs`). **[audit 2026-10-10: DONE b3f4df4 — topbar ☰ button + Ctrl/Cmd+B flip `left.user_intent` fe-ui/src/ui_shell/topbar.rs:41-62; render_left_sidebar applies intent only (left_sidebar.rs:58-72), no `!right_open` stomp]**

## Phase 2: Settings → right-sidebar section (FR-1) [P1]

- [x] Task: register a Settings section in `ui_shell/right_sidebar.rs`; move the
      `dialogs/settings.rs` body into the section container, logic verbatim. **[audit 2026-10-10: DONE b3f4df4 — `RightSidebarSection::Settings` right_sidebar.rs:58, `render_settings_section` :469]**
- [x] Task: remove Settings from the `ActiveDialog` floating set; control-parity
      checklist (every widget present + functional); calm empty state. **[audit 2026-10-10: DONE b3f4df4 — dialogs/settings.rs deleted, ActiveDialog::Settings removed (dialogs/mod.rs); parity checklist not filed as a separate artifact]**

## Phase 3: Maps (hexon manager) → right-sidebar section (FR-2) [P1]

- [x] Task: register a Maps section; move the `dialogs/hexon_manager.rs` body in,
      preserving the choose-a-map-per-petal wiring and map/tile-source select. **[audit 2026-10-10: DONE b3f4df4 — `RightSidebarSection::Maps` right_sidebar.rs:60, `render_maps_section` :716 (carrier ActiveDialog::HexonManager lives only while Maps is active, :220-222)]**
- [x] Task: remove the floating Map Manager; control-parity checklist; §9
      terminology check (map vs path). **[audit 2026-10-10: DONE b3f4df4 — floating Map Manager window removed (hexon_manager.rs now renders inside the section); §9 terminology per fe-ui/src/dialogs/AGENTS.md §map-terminology]**

## Phase 4: Docs + integrated sweep [P1]

- [x] Task: update `fe-ui/src/ui_shell/AGENTS.md` + `panels/AGENTS.md` — sticky
      policy + the two new sections (N-7). **[audit 2026-10-10: OPEN — sections documented (fe-ui/src/ui_shell/AGENTS.md:67,104) but §left (:49-58) still describes the removed `AutoCollapse` policy and a 3-arg `left_visibility(policy, right_open, user_intent)`; §topbar (:35-40) still says Settings/Maps have no section variant; fe-ui/src/dialogs/AGENTS.md §settings (:119-142) still describes the deleted floating Settings window + `SettingsToggle` seam]** **[DONE 2026-10-11 spatial-builder-residue batch (commit pending): ui_shell/AGENTS.md §left = `left_visibility(policy, user_intent)` Manual-only + Ctrl/Cmd+B + no stomp, §topbar/§right list the six ALL_SECTIONS; dialogs/AGENTS.md §settings is a pointer to `render_settings_section` + real classifier hit kinds; fe-ui/src/AGENTS.md SettingsToggle sentence removed; `grep -rn "AutoCollapse\|SettingsToggle" fe-ui --include=*.md` = 0]**
- [x] Task: single sweep — `clippy -D warnings`, `fmt --check`, fe-ui tests
      (N-6); ui_ux §5/§7/§9 pre-merge checklist. **[audit 2026-10-10: DONE b3f4df4 — commit records compile + clippy -D warnings + fmt + full tests green; re-swept in 623145f]**
- [x] Task: retro; in-app verify is user-gated (note it, do not self-sign). **[retro.md written 2026-10-11 spatial-builder-residue batch (commit pending); in-app verify remains USER-GATED (not self-signed; track status stays implemented_pending_inapp_verify)]**
