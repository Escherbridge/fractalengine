---
type: Track Spec
title: Contextual Right-Click Controls — Object-Aware Menu, Real Delete, Comprehensive Verbs
description: Replace the thin "create empty node / place asset" right-click menu with an object-aware context menu whose verbs depend on what's under the cursor (empty ground, node, stamp, path, path point, earthwork region). Ships the missing Delete (wired to the node spine's tombstone+cascade+re-flow, fixing the "no way to remove a node / empty husk" bug) plus duplicate, promote-to-node, rename, edit-properties, and the copy-API-string / report / query verbs. Menu-first per D-A9; radial deferred. Wave 1.
tags: [feature, contextual_controls_20260725, pending]
timestamp: 2026-07-25T00:00:00Z
resource: ./metadata.json
---

# Specification: Contextual Right-Click Controls

**Track ID:** `contextual_controls_20260725`
**Type:** feature · **Wave:** 1 · **depends_on:** `node_lifecycle_addressing_20260725` · **coordinates:** `endpoint_api_surface_20260725`
**Crates:** `fe-ui` (`ui_shell/modal.rs`, `dialogs/context_menu.rs`, `dialogs/node_options.rs`)

Anchor: [`../../decisions/spatial-builder-program-20260725.md`](../../decisions/spatial-builder-program-20260725.md).
Foundation: [`../node_lifecycle_addressing_20260725/spec.md`](../node_lifecycle_addressing_20260725/spec.md)
(delete/cascade FR-1/FR-2), the `HitTarget`/`Operation` dispatch
(`node_manager/dispatch.rs`), and the modal manager (ui_shell FR-7).

## Overview

Verbatim user asks (2026-07-25 in-app QA):

1. > "The right click options on assets + paths need to be updated. It needs to
   > show at least an option to delete — currently it doesn't seem like there is
   > a way to remove a node once added; even removing properties just leaves an
   > empty node."
2. > "In general we need more comprehensive context-specific controls on right
   > click — not just create empty node or place asset."

Per D-A9 the first surface is a **classic context menu whose action set is
object-aware** — built from what the pointer hit. Delete is table-stakes and is
wired to T1's real delete (tombstone + cascade + re-flow), which is what
actually fixes the "empty husk" (clearing properties ≠ deleting). A game-style
radial menu is explicitly deferred to a later polish track.

### Ground truth (2026-07-25)

- A context menu file already exists: `fe-ui/src/dialogs/context_menu.rs`; node
  options today are the thin `dialogs/node_options.rs` (create empty node / place
  asset). The modal manager owns transient overlays incl. the context menu
  (ui_shell FR-7, `ui_shell/modal.rs`).
- Object-awareness is available: `HitTarget`/`Operation` enums +
  `resolve_operation` (`node_manager/dispatch.rs`) already classify node / vertex
  / handle / segment / stamp / proposal / gimbal hits — the menu is built from
  the same classification, not a new one.
- Delete has no sync-safe path today (T1 adds it). Property edits go through
  `actions/node_props.rs` — clearing them leaves the node (the reported bug).

## Functional Requirements

- **FR-1 — Object-aware menu.** Right-clicking builds the menu from the hit
  target (empty ground / node / stamp / path / path point / earthwork region),
  showing only verbs valid for that object. Empty ground keeps create-node /
  place-asset. *Acceptance:* each object type yields its correct verb set (pure
  `menu_for(hit) -> Vec<Verb>` table, unit-tested over every `HitTarget`); no
  verb appears for an object it can't act on.

- **FR-2 — Delete (fixes the husk bug).** Every deletable object shows **Delete**,
  wired to T1's tombstone delete; parent deletes route through T1's cascade with
  a confirm (child count shown). This is the real remove-a-node path.
  *Acceptance:* deleting a node removes it (not an empty husk); cascade confirm
  shows descendant count and, on confirm, removes the subtree; clearing
  properties is a distinct verb that keeps the node; delete is undoable-safe per
  the op-log (no raw drop).

- **FR-3 — Comprehensive verb set.** Beyond delete: **duplicate**, **rename**,
  **edit properties**, **promote to node** (for an un-promoted stamp, via T1
  FR-5), **copy API string**, **report / query** (open the object's egress/
  report). *Acceptance:* each verb performs its op against the correct object;
  verbs with no target are absent (FR-1); each verb has a tooltip (§ui_ux).

- **FR-4 — Graceful coordination with the API track.** **Copy API string** and
  **report / query** call a small seam that `endpoint_api_surface` provides
  (address → string / report). Until T5 lands that seam, these verbs are shown
  **disabled with an explanatory hint** (never silently absent — `ui_ux.md` §6),
  then light up when T5 lands. *Acceptance:* with the seam present the verbs
  produce the string/report; without it they are visibly disabled with a hint,
  not missing and not panicking.

## Non-Functional Requirements

Inherits the shared pool. Load-bearing: **N-3** (menu verbs respect the two-
authority split — path verbs act via `editing_track_id`, node verbs via
`NodeManager.selected`; no cross-writes), **N-5** (no authz in the menu — it
emits `UiAction`s; policy is enforced downstream), **N-8** (ui_ux checklist —
tooltips, no silent failure, calm chrome). No new crate dependencies.

## Dependencies & concurrency

- **depends_on:** `node_lifecycle_addressing_20260725` (delete FR-1, cascade
  FR-2, promote FR-5). **coordinates:** `endpoint_api_surface_20260725` (FR-4
  seam). **blocks:** none.
- **Owns (file partition):** `fe-ui/src/ui_shell/modal.rs`,
  `fe-ui/src/dialogs/context_menu.rs`, `fe-ui/src/dialogs/node_options.rs`.
  Disjoint from T6 (shell seam), T2 (path section), T3 (terrain section) within
  fe-ui — Wave 1 parallel. Does **not** edit `right_sidebar.rs`.

## Open questions (ratify before build)

- **Q-1 — Verb set per object type.** Ratify the per-type menus (recommended
  defaults): *empty ground* → create node / place asset; *node* → edit props /
  rename / duplicate / copy API / report / delete; *stamp* → promote to node /
  scale-rotate / copy API / report / delete (+ slide-along-path if T2 Q-1
  yes); *path* → edit / add-stamps / copy API / report / delete; *path point* →
  set corner/smooth/symmetric / delete point; *earthwork region* → edit params /
  report volume / copy API / delete.
- **Q-2 — Cascade confirm.** Always show the descendant count + confirm on any
  cascade (recommended, matches T1 Q-3), or a threshold?
- **Q-3 — T5-dependent verbs pre-T5.** Show copy-API / report **disabled with a
  hint** until T5 lands (recommended, §6), or hide them until then?
- **Q-4 — Radial.** Confirm menu-first, radial deferred to a later polish track
  (D-A9 — recommended).

## Ratified decisions (2026-07-25)

User ratified 2026-07-25 (all recommended defaults adopted; none vetoed).

- **Q-1 → RATIFIED: the recommended per-type verb tables.** *empty ground* →
  create node / place asset; *node* → edit props / rename / duplicate / copy API /
  report / delete; *stamp* → promote to node / scale-rotate / **slide-along-path**
  (T2 Q-1 ratified yes) / copy API / report / delete; *path* → edit / add-stamps /
  copy API / report / delete; *path point* → set corner/smooth/symmetric / delete
  point; *earthwork region* → edit params / report volume / copy API / delete.
  Gates FR-1/FR-3.
- **Q-2 → RATIFIED: always show descendant count + confirm on any cascade**
  (matches T1 Q-3). Gates FR-2.
- **Q-3 → RATIFIED: copy-API / report shown disabled-with-hint until T5 lands**
  (never silently absent, ui_ux §6), then light up. Gates FR-4.
- **Q-4 → RATIFIED: menu-first; radial deferred to a later polish track** (D-A9).

## Out of scope

- The delete/cascade/promote **primitives** (T1 owns them; this track wires the
  verbs).
- The API string / report **generation** (T5 owns the seam; this track calls it).
- The game-style radial menu (deferred per D-A9).
- Any right-sidebar section work (T6/T2/T3).
