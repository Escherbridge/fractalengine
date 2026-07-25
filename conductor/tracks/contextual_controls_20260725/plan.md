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

- [ ] Task: pure `menu_for(hit: HitTarget) -> Vec<Verb>` table (built from the
      existing dispatch classification); unit test over every `HitTarget`.
- [ ] Task: render it via the modal manager (`ui_shell/modal.rs` +
      `dialogs/context_menu.rs`); empty-ground keeps create/place.

## Phase 2: Delete + cascade confirm (FR-2) [P0]

- [ ] Task: wire Delete → T1 tombstone delete; parent → T1 cascade with a
      confirm dialog showing descendant count.
- [ ] Task: prove clear-properties is a distinct verb that keeps the node (the
      husk-bug regression, paired with T1's).

## Phase 3: Comprehensive verbs (FR-3) [P1]

- [ ] Task: duplicate / rename / edit-properties / promote-to-node (T1 FR-5)
      wired per object type; verbs absent when target invalid; tooltips.

## Phase 4: API-seam coordination + docs + sweep (FR-4) [P1]

- [ ] Task: copy-API-string / report-query call the T5 seam; disabled-with-hint
      when the seam is absent (§6, no silent failure); light up when T5 lands.
- [ ] Task: `fe-ui/src/dialogs/AGENTS.md` — menu table + verb→action map (N-7).
- [ ] Task: single sweep — `clippy -D warnings`, `fmt --check`, fe-ui tests
      (N-6); ui_ux checklist. Retro; in-app verify user-gated.
