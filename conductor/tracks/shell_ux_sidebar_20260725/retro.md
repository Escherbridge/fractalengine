---
type: Track Retro
title: "Shell UX Sidebar (T6) Retro — sticky left sidebar, Settings + Maps as sections"
tags: [shell_ux_sidebar_20260725, retro, spatial-builder, ui-shell, wave-0]
timestamp: 2026-10-11T00:00:00Z
resource: ./spec.md
---

# Retro: Shell UX Sidebar (T6)

## What shipped

- **b3f4df4** (Wave 0): everything this track had to build.
  - FR-3: the left sidebar is user-sticky. `left_visibility(policy, user_intent)` has the
    single `Manual` policy (auto-collapse removed, D-A11 / Q-3). The topbar ☰ button and
    Ctrl/Cmd+B flip `user_intent`, and the per-frame `!right_open` stomp is gone.
  - FR-1: Settings is the `RightSidebarSection::Settings` section
    (`render_settings_section`). `dialogs/settings.rs` and `ActiveDialog::Settings` were
    deleted.
  - FR-2: Maps (the hexon manager) is the `RightSidebarSection::Maps` section.
  - The Wave-1 registration scaffold, so T2/T3/T4 never edited `right_sidebar.rs`.
- **623145f**: re-swept as part of the serial integration pass.
- **This batch (2026-10-11 spatial-builder-residue, commit pending)**: the directory docs
  now match the code.
  - `ui_shell/AGENTS.md` §left describes the 2-arg sticky policy and Ctrl+B with no
    stomp. §topbar and §right list the six `ALL_SECTIONS`.
  - `dialogs/AGENTS.md` §settings is now a pointer to the sidebar section, and the
    classifier sentence lists the real hit kinds.
  - The `SettingsToggle` seam sentence in `fe-ui/src/AGENTS.md` is gone.
  - `grep -rn "AutoCollapse\|SettingsToggle" fe-ui --include=*.md` returns 0.

## Lessons

- **Docs rot fastest at the seam you just removed.** The code removal was complete in
  b3f4df4, but three directory docs went on describing the deleted policy and window for
  2.5 months. The 2026-10-10 audit caught it only by reading prose against code. When a
  commit deletes a variant or file, grep the `AGENTS.md` tree for its name in the same
  commit.
- **Owning the shared seam worked.** Making T6 the single owner of `ui_shell/*` and
  `panels/mod.rs` let three Wave-1 leaves run in parallel without collisions. It also
  meant their wiring had to wait for the serial integration wave (623145f). That cost
  is expected, so budget for it.

## Open residuals

- **USER-GATED:** in-app verify. The left sidebar should stay put through selection,
  right-section changes and petal switches. Settings and Maps sections should match the
  old windows' controls.
- No separate control-parity checklist artifact was filed for Settings/Maps (noted by the
  2026-10-10 audit). The in-app pass covers parity.
