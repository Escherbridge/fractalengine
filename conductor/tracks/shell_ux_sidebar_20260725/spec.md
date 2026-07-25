---
type: Track Spec
title: Shell UX — Settings & Maps into the Sidebar, User-Sticky Left Sidebar
description: Finish the "no modes, one shell" direction (D-A1/D-A10/D-A11) by migrating the Settings and Maps (hexon manager) modals into tool-contextual right-sidebar sections like every other tool surface, and making the left sidebar user-driven sticky (open until the user closes it) instead of auto-collapsing on selection. Owns the fe-ui shell seam exclusively; Wave 0, independent of the data spine.
tags: [chore, shell_ux_sidebar_20260725, pending]
timestamp: 2026-07-25T00:00:00Z
resource: ./metadata.json
---

# Specification: Shell UX — Modals→Sidebar + Sticky Sidebar

**Track ID:** `shell_ux_sidebar_20260725`
**Type:** chore (UX refactor on the landed ui_shell seam)
**Wave:** 0 · **depends_on:** none (consumes the landed `ui_shell_architecture_20260724` seam)
**Crates:** `fe-ui` only — the shell seam owner (no new dependencies)

Anchor: [`../../decisions/spatial-builder-program-20260725.md`](../../decisions/spatial-builder-program-20260725.md).
Foundation: [`../ui_shell_architecture_20260724/spec.md`](../ui_shell_architecture_20260724/spec.md)
(the manager split + one-section right-sidebar rail this track extends).
Styleguide: [`../../code_styleguides/ui_ux.md`](../../code_styleguides/ui_ux.md) (§5 SACRED).

## Overview

Two user asks from 2026-07-25 in-app QA, both continuations of the just-landed
shell:

1. > "Settings is still in a modal, maps are still in a modal — would be nice to
   > get them in the left sidebar panel as well on select similar to the rest of
   > the tools."
2. > "allow the left side bar to stay open until closed — it should be a user
   > driven toggle, it automatically closes too often."

Per D-A1/D-A10 there are no modes: Settings and Maps are just more
tool-contextual sections in the one right-sidebar rail (the ui_shell track
already dissolved the floating tool windows into that rail — Path tools /
Terrain tools / Proposal report / Tool / Inspector sections). Per D-A11 the
left sidebar becomes user-sticky.

### Ground truth (2026-07-25)

- The right-sidebar **one-section-at-a-time rail** and its section-fn seam exist
  (`fe-ui/src/ui_shell/right_sidebar.rs`, ui_shell FR-6, RATIFIED Q-2). Adding
  Settings + Maps is registering two more sections here.
- Settings is `fe-ui/src/dialogs/settings.rs`; the Maps modal is the hexon/tile
  source manager `fe-ui/src/dialogs/hexon_manager.rs` (Map Manager). Both are in
  the `ActiveDialog` floating-dialog set.
- The left-sidebar auto-collapse was a per-frame overwrite (`panels/mod.rs`);
  ui_shell FR-5 already moved it into an **owned, tested policy** in
  `ui_shell/left_sidebar.rs` ("so user intent can be respected later"). This
  track flips the default to user-sticky — the "later" is now.

## Functional Requirements

- **FR-1 — Settings → right-sidebar section.** Move the Settings dialog body
  into a right-sidebar section revealed by its topbar/rail toggle, one-at-a-time
  like the other sections. Logic preserved verbatim (settings read/write, any
  `UiAction` emissions); only the container changes. The floating Settings
  dialog is removed from `ActiveDialog`. *Acceptance:* every Settings control
  present and functional in the section; the floating dialog is gone; empty/
  loading states are calm hints (§7).

- **FR-2 — Maps (hexon manager) → right-sidebar section.** Same migration for
  the Map Manager (`hexon_manager.rs`) — the per-petal map/tile-source chooser
  becomes a right-sidebar section. Map selection continues to drive the existing
  per-petal terrain/map flow (no change to the choose-a-map-per-petal wiring).
  *Acceptance:* map browse + select + install work from the section; floating
  Map Manager removed; terminology follows §9 (map vs path).

- **FR-3 — User-sticky left sidebar (D-A11).** The left sidebar stays in the
  state the user last set it to; the auto-collapse-on-right-panel-open default
  is removed. A single explicit toggle (topbar button + shortcut) opens/closes
  it, and that intent persists across selections and petal switches within a
  session. *Acceptance:* opening the left sidebar and then selecting a
  node/opening a right section leaves it open; closing it keeps it closed until
  the user reopens it; a pure `left_visibility(policy, user_intent)` helper is
  unit-tested; no per-frame frame-stomp remains.

## Non-Functional Requirements

Inherits the shared pool (N-1..N-10). Load-bearing: **N-2** (no modes — sections
not modes), **N-3** (SACRED selection split — Settings/Maps sections read
neither selection authority incorrectly), **N-8** (ui_ux checklist). No new crate
dependencies; single egui pass discipline (one `begin_pass`/`end_pass`).

## Dependencies & concurrency

- **depends_on:** none (the ui_shell seam is landed). **blocks:** none.
- **Owns exclusively (file partition):** `fe-ui/src/ui_shell/{mod,left_sidebar,
  right_sidebar,topbar}.rs`, `fe-ui/src/panels/mod.rs`, `fe-ui/src/dialogs/
  {settings,hexon_manager}.rs`. **T6 is the section-registry owner** — Wave-1
  tracks that need a new section route the one-line registration through this
  track; they do not edit `right_sidebar.rs` (see the anchor partition).
- **Wave-0 fe-ui registration scaffold (added by the slice re-grill).** Beyond
  its own FRs, T6 also owns and lays down in Wave 0 the fe-ui *registration
  spine* so Wave-1 tracks fan out collision-free: `fe-ui/src/actions/mod.rs`
  (all new `UiAction` variants + dispatch arms), `fe-ui/src/plugin.rs` (resource/
  system registration + `gardener_console` param threading), and empty handler
  stubs in `actions/{asset,path,node,node_props,terrain_proposal}.rs` that the
  Wave-1 tracks fill. Wave-1 fe-ui tracks never touch these four central files.
  See the anchor "Slice-time partition corrections".
- Runs Wave 0 fully parallel with the data spine (different crates entirely).

## Open questions (ratify before build)

- **Q-1 — Rail placement of Settings/Maps.** Settings + Maps as ordinary
  one-at-a-time sections in the existing right rail (recommended — consistent
  with D-A10 "like the rest of the tools"), or grouped under a separate
  "workspace" affordance? *Recommended:* ordinary sections.
- **Q-2 — Sticky scope.** User intent persists **within a session** (recommended)
  or also across app restarts (needs a settings-persistence hop)? *Recommended:*
  session-scoped now; persistence is a small follow-up.
- **Q-3 — Auto-collapse fate.** Remove auto-collapse entirely (recommended, per
  D-A11), or keep it available as an opt-in preference? *Recommended:* remove;
  revisit only if narrow-viewport users ask.

## Ratified decisions (2026-07-25)

User ratified 2026-07-25 (recommended defaults adopted; none vetoed).

- **Q-1 → RATIFIED: Settings + Maps become ordinary one-at-a-time sections** in
  the existing right rail, "like the rest of the tools" (D-A10). No separate
  "workspace" affordance. Gates FR-1/FR-2.
- **Q-2 → RATIFIED: sticky scope is session-only.** User intent persists across
  selections + petal switches within a session; cross-restart persistence is a
  small follow-up (needs a settings-persistence hop). Gates FR-3.
- **Q-3 → RATIFIED: remove auto-collapse entirely** (D-A11). Not kept as an
  opt-in preference; revisit only if narrow-viewport users ask. Gates FR-3.

## Out of scope

- The GIS "Data" window migration (ui_shell Q-4 RATIFIED: stays floating for now).
- Any new Settings *content* (this is a container move, not new options).
- Cross-restart persistence of sidebar/settings state (Q-2 follow-up).
- The radial menu / contextual verbs (T4).
