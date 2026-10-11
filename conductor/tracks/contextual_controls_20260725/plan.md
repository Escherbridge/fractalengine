---
type: Implementation Plan
title: "Implementation Plan: Contextual Right-Click Controls"
tags: [contextual_controls_20260725]
resource: ./spec.md
---

# Implementation Plan: Contextual Right-Click Controls

Four phases. The object-aware menu table + Delete land first (they fix the
reported bug); the richer verbs and the API-seam coordination follow. TDD on the
pure `menu_for(hit)` table; single sweep at the end (N-6).

## Phase 1: Object-aware menu skeleton (FR-1) [P0]

- [x] Task: pure `menu_for(hit: HitTarget) -> Vec<Verb>` table (built from the
      existing dispatch classification); unit test over every `HitTarget`. **[audit 2026-10-10: DONE 4f765fe — `menu_for` fe-ui/src/dialogs/context_menu.rs:73; tests node_menu_is_the_ratified_node_set, stamp_menu_has_promote_scale_slide_and_delete, gimbal_axis_has_no_object_menu, every_deletable_object_offers_a_delete_verb]**
- [x] Task: render it via the modal manager (`ui_shell/modal.rs` +
      `dialogs/context_menu.rs`); empty-ground keeps create/place. **[audit 2026-10-10: DONE 4f765fe+623145f — `render_context_menu` context_menu.rs:435 + ui_shell/modal.rs; real HitTarget from node_manager/context_pick.rs (623145f)]**

## Phase 2: Delete + cascade confirm (FR-2) [P0]

- [x] Task: wire Delete → T1 tombstone delete; parent → T1 cascade with a
      confirm dialog showing descendant count. **[audit 2026-10-10: DONE 4f765fe+623145f — actions/node.rs:31-43 TombstoneNode/CascadeTombstoneNode; two-step confirm with live `CountNodeDescendants` count context_menu.rs:525,719; spine test count_descendants_excludes_root_and_tombstoned_rows (fe-database/tests/node_lifecycle_test.rs:657)]**
- [x] Task: prove clear-properties is a distinct verb that keeps the node (the
      husk-bug regression, paired with T1's). **[audit 2026-10-10: DONE — test clear_properties_is_a_node_only_verb_distinct_from_delete context_menu.rs:1045]**

## Phase 3: Comprehensive verbs (FR-3) [P1]

- [x] Task: duplicate / rename / edit-properties / promote-to-node (T1 FR-5)
      wired per object type; verbs absent when target invalid; tooltips. **[audit 2026-10-10: DONE 623145f — DuplicateNode/RenameNode actions/node.rs:53-81 (spine tests rename_is_authorized_persists_and_emits_one_event, duplicate_copies_row_minus_path_identity_keys); verb_action_routes_node_verbs_and_selects_cascade context_menu.rs:1246]**

## Phase 4: API-seam coordination + docs + sweep (FR-4) [P1]

- [x] Task: copy-API-string / report-query call the T5 seam; disabled-with-hint
      when the seam is absent (§6, no silent failure); light up when T5 lands. **[audit 2026-10-10: DONE 4f765fe+cdd3291 — `crate::gis::egress_strings::{api_string_for,report_for}` actions/node.rs:141-152, context_menu.rs:673; test only_egress_verbs_are_seam_gated]**
- [x] Task: `fe-ui/src/dialogs/AGENTS.md` — menu table + verb→action map (N-7). **[audit 2026-10-10: DONE — fe-ui/src/dialogs/AGENTS.md §context-menu (:48)]**
- [x] Task: single sweep — `clippy -D warnings`, `fmt --check`, fe-ui tests
      (N-6); ui_ux checklist. **[audit 2026-10-10: DONE 623145f — fmt clean, clippy -D warnings clean, 2050 passed / 0 failed; later workspace sweeps green (RUNBOOK §2, 3,245 passed)]**
- [ ] Task: Retro (not written) + in-app verify (USER-GATED; 2026-07-26 in-app pass was PARTIAL FAIL on tools, repaired in 631f389 — re-verify outstanding). *(split out of the sweep task by the 2026-10-10 audit)*
