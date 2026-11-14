---
type: Review
track: hexon_scale_orchestration_20260712
critic: C-HEXON
round: 4
date: 2026-11-13
---

# C-HEXON completeness: hexon_scale_orchestration_20260712 (round 4)

Every spec requirement (FR-1..FR-10, NFR-1..NFR-5) and plan task checked against code at HEAD. The only item planned this round was HEXON-19 (the in-world dimension-callout label). No cargo was run, and no tests were run (user-gated).

## Requirements and tasks

| Item | Status | Evidence |
|---|---|---|
| FR-1 additive scale metadata on `TilesetMeta` | complete | `fe-format/src/manifest.rs:167` (`ground_sample_distance_m`, serde-defaulted alongside native_scale/scale_bounds) |
| FR-2 derivation from bounds + zoom | complete | `fe-format/src/manifest.rs:323` `derive_scale_from_bounds`, used `:497` |
| FR-3 backfill on load, idempotent | complete | `fe-terrain/src/tiles/store.rs:90,339`; tests `:861,873,892` |
| FR-4 composite reconciles per-source GSD | complete | `fe-terrain/src/tiles/composite.rs:22-35` (finest GSD, per-source zoom pick), `:191` `resolved_gsd_and_scale`, frame `:433-435` |
| FR-5 hexon-authoritative world_scale + bounds | complete | `fe-terrain/src/petal_binding.rs:56,89-90`, user nudge clamped inside hexon bounds `:229-258` |
| FR-6 bounds surfaced to UI, no fe-terrain edge | complete | `fe-ui/src/terrain_map/mod.rs:69`; clamp `fe-ui/src/actions/hexon.rs:170-173,208`; fe-ui/Cargo.toml has no fe-terrain dependency |
| FR-7 ruler pure math | complete | `fe-terrain/src/ruler.rs:7,29,39,50`; Bevy appears only in a `render`-gated test (`:453-457`, inside `#[cfg(test)]` `:202`) |
| FR-8 scale bar HUD | complete | `fe-terrain/src/ruler_plugin.rs:128,146` |
| FR-9 tape / area / bearing + GPX length | complete | `fe-ui/src/node_manager/measure.rs:84,145,204`; GPX length `fe-ui/src/node_manager/path_segment_interaction.rs:396` |
| FR-10 adaptive grid + callouts | complete | `fe-terrain/src/ruler.rs:164,189`; callout persistence + strict reader `fe-ui/src/actions/dimension_callout.rs:212` (test `:526`); HEXON-19 label pass `dimension_callout.rs:404`, registered after `gardener_ui_system` at `fe-ui/src/plugin.rs:1108` |
| NFR-1 backward compat | complete | FR-3 tests (legacy meta backfilled, populated fields untouched) |
| NFR-2 purity/testability | complete | `ruler.rs`, `scale.rs`, `measure.rs` reducers are pure; batch filter `actions::dimension_callout` / `node_manager::measure` in `conductor/test-batches.md:85` |
| NFR-3 quality gates | partial | the "clippy -D warnings" part is obsolete by the 2026-11-09 no-clippy decision; fmt and test runs are user-gated |
| NFR-4 layering | complete | see FR-6 |
| NFR-5 per-frame cost | complete with a kept note | R-HEXON-1 R1-16 KEPT: the label builds a small `MeasureState` and a String per visible callout per frame (bounded by visible callouts) |
| Plan Phases 1-5, all tasks + verifications | complete | ticked in plan.md, evidence above |
| Plan Phase 6 tasks (grid spacing, fade, callouts) | complete | as FR-10; the plan's "in-world text label is deferred" note is now closed by HEXON-19 |

## Excluded by the scoper (not gaps)

- blocked-user: Phase 6 Verification (grid snaps / re-subdivides / fades in-app; callout persists; full sweep).

## Gate hand-offs (did each reach its owner?)

- R-HEXON-1 reviewed HEXON-19 with no fix needed beyond R1-16 (KEPT). The plugin registration NEEDS reached I-UI (`fe-ui/src/plugin.rs:1108`).
- SEC-INBOX: no entry names a file of this track.

## HANDED-OFF (owner = next round)

- HANDED-OFF (round 5, T-HEXON): amend the Phase 6 callout task note in plan.md ("in-world text label is deferred") to say the label pass landed (`dimension_callout.rs:404`, `plugin.rs:1108`).

Verdict: COMPLETE in code. Every FR is satisfied at HEAD, and the only open item is the user-gated Phase 6 in-app check plus the sweep.
