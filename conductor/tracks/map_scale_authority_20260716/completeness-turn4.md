---
type: Review
track: map_scale_authority_20260716
critic: C-CITY
round: 4
date: 2026-11-13
---

# C-CITY completeness check: map_scale_authority (round 4)

Checked against the working tree at `733a3256`. No cargo was run. Statuses: complete, partial, missing,
pending-run (code done, the run waits for the user to name the batch), obsolete and blocked-user (classes the
scoper excluded; they are not gaps).

Note: the plan's own annotations for Phase 1 docs and Phase 2 task 1 ("NOT DONE ... I-DB-1 / I-DOC-CORE
pending") were written before I-DB-1 and I-DOC-CORE landed. Both are done at HEAD (rows below); the
ticks in `plan.md` are stale.

## Spec requirements

| Requirement | Status | Evidence |
|---|---|---|
| FR-1 one canonical accessor | complete | `fe-format/src/scale.rs:12` sanitize, `:26` clamp, `:42`/`:47` convert, `:52` JSON accessor (ignores `enabled`, user default §9 #27); `fe-format/src/lib.rs:17` `pub mod scale`; `fe-terrain/src/scale.rs:3` re-export |
| FR-1 accessor table (null, missing, non-finite, <=0, clamped) | complete | `fe-format/src/scale.rs` tests (`sanitize_rejects_non_positive_and_non_finite`, `accessor_table`) |
| FR-2 placement default `[ws;3]` | complete | `fe-database/src/handlers/crud.rs:170` `petal_world_scale`, `:195-210` `create_node_as`, `:335` `import_gltf_handler` |
| FR-2 echoes carry the applied scale | partial | `SceneChange::NodeAdded` carries it (`fe-database/src/lib.rs:900-916`, `:936-939` via the committed row); `GltfImported` carries the row (`lib.rs:949`). `DbResult::NodeCreated` has no scale field (`fe-runtime/src/messages.rs:1629`), and `handle_node_created` builds the UI tree entry with a default transform (`fe-ui/src/verse_manager/db_results/nodes.rs:36`), so an empty node's UI entry shows scale 1 until a reload |
| FR-2 no migration, explicit-scale paths untouched | complete | only the two initial-create paths call `placement_scale`; `fe-database/src/AGENTS.md:707` |
| FR-2 helper reachable for place_asset | complete | `crud.rs:170` is `pub(crate)` inside handlers |
| FR-3 UI audit | complete | `fe-ui/Cargo.toml:22`; `db_results/terrain.rs:67`; `terrain_map/mod.rs:247`; `actions/hexon.rs:263`; `geometry.rs:9-37`; `path_segment_interaction.rs:256`; `path_asset_reconcile.rs:214`; `dialogs/gltf_import.rs:185` (meters, OQ-1); audit table `fe-ui/src/AGENTS.md:1178` |
| FR-3 OQ-3 single write path | complete | documented `fe-ui/src/AGENTS.md:1195`; `panels/inspector.rs:556` is the only writer of `InspectorFormState.world_scale` |
| FR-4 API unit contract | complete | `fe-api/AGENTS.md:2467` §units; `fe-api/src/types.rs:28,41,82,199` pointers |
| FR-5 terrain snap on place | complete (code) | `fe-ui/src/node_manager/ground_snap.rs:27`; `plugin.rs:1556`; `viewport.rs:146` -> `dialogs/context_menu.rs:726,735`; in-app check blocked-user |
| FR-6 round trip | obsolete | replaced by the DB-level round trip `crud.rs:1827` `placement_scale_follows_the_petal_map` (R1 batch) |
| NFR-1 one formula | complete | fe-ui copies are delegating shims (rows above) |
| NFR-2 layering | complete | no `fe-terrain` in `fe-ui/Cargo.toml`; new edges fe-database->fe-format (`fe-database/Cargo.toml:20`) and fe-ui->fe-format only |
| NFR-3 back-compat | complete | terrain-less petal -> 1.0 (`scale.rs:52-56`) |

## Plan tasks

| Task | Status | Evidence |
|---|---|---|
| P1 create `fe-format/src/scale.rs` | complete | `scale.rs:1-63` |
| P1 fe-terrain delegates | complete | `fe-terrain/src/scale.rs:3` |
| P1 docs | partial | `fe-format/src/AGENTS.md:391` §scale-accessor and the `fe-terrain/src/AGENTS.md` §scale delegation note exist; R-CITY-1 F17 asked for why the global range (1e-6..1e4) is wider than the stamp range (0.01..1e4), and that sentence is absent |
| P1 verification | pending-run | `cargo test -p fe-format` and the fe-terrain parity tests (`conductor/test-batches.md:397`, `:157`) |
| P2 fe-format dep + `petal_world_scale` | complete | `fe-database/Cargo.toml:20`, `crud.rs:170` |
| P2 handlers write scale + echoes | partial | see FR-2 echo row |
| P2 update harness consumers | obsolete | excluded by the scoper |
| P2 verification | pending-run / blocked-user | DB test in R1 (`test-batches.md:58`); in-app 1:1 vs 1:100 look is user-gated |
| P3 fe-ui dep + read routing + clamp test | complete | `db_results/terrain.rs:67`, test `:169-179` |
| P3 conversion shims | complete | see FR-3 |
| P3 audit remaining surfaces | complete | `fe-ui/src/AGENTS.md:1178` table |
| P3 API contract doc | complete | see FR-4 |
| P3 verification | pending-run | grep part done (shims only; no fe-terrain edge) |
| P4 sampler (OQ-2 = height field) | complete | `ground_snap.rs:27`, plateau row added by R-CITY-1 F9 |
| P4 ground-height seam | complete | `plugin.rs:1556`; bridge system obsolete per the scoper |
| P4 wire placement | complete | `viewport.rs:146`; the context-menu tests (`actions/mod.rs:2347`) still use y=0, the snapped height is proven in `ground_snap.rs:88` |
| P4 verification | blocked-user | excluded |
| P5 scripted round trip | obsolete | excluded; `crud.rs:1827` instead |
| P5 manual plan / sweep / verification | blocked-user | excluded |

## Security steer 21:55 (world-scale cap) and SEC-INBOX

| Rule | Status | Evidence |
|---|---|---|
| 1 global range in `sanitize_world_scale`, bounds only narrow | complete | `fe-format/src/scale.rs:5-41`; range 1e-6..1e4 instead of 0.01 (justified in R-CITY-1 F1: 1:10000 slider, zoom-0 native scale) |
| 2 f32-cast check in crud.rs | complete | `crud.rs:179-191` |
| 2 caller-supplied scale (`object_creation::prepare`) | missing | `fe-database/src/handlers/object_creation.rs:172-178` checks `is_finite()` on f64 only; `1e300` passes and becomes inf after the cast |
| 3 write-side refusal | missing | no caller of `is_world_scale_in_range` in `fe-database/src/handlers/petal_terrain.rs:59` `prepare_terrain_write` or `fe-api/src/terrain.rs:~248` |
| 4 test rows 1e300 / 1e-300 / f64::MAX | complete | 11 rows in `scale.rs` tests, 6 in `crud.rs` tests |

SEC-INBOX 2: CONFIRMED (partly fixed). Read side fixed (rules 1, 2 and 4); rules 2 (object_creation) and 3
are open and handed off below.

## Hand-offs from R-CITY-1: did they reach their owner?

| Line | Reached? | Evidence |
|---|---|---|
| F3 (CLOCK-09 / R-CLOCK-2) terrain write refusal | no | `petal_terrain.rs:59-80` unchanged; R-CLOCK-2 review does not mention it |
| F4 (R-HEXON-1) fe-api terrain validation | no | `fe-api/src/terrain.rs:244-247` validates pins only |
| F5 (orchestrator) object_creation f32 check | no | `object_creation.rs:172-178` |
| F6 (R-CITY-2) `trailing_points` in the pen literal | no | `fe-ui/src/node_manager/path_point_interaction.rs:564-572` still lacks the field: E0063, fe-ui does not compile |
| F7 (R-EXT-3 / EXT-12) RequestFailed clears pending create | no | `fe-ui/src/verse_manager/db_results/mod.rs:1000-1020` never calls `fail_pending_pen_create` |
| F8 (R-EXT-3 / EXT-12) pen flush journey with trailing points | no | `db_results/mod.rs:1959-2160` has no `PathAppendShape` assertion |
| F12 (I-DB-1) echo + Cargo dep | yes | `lib.rs:901`, `fe-database/Cargo.toml:20` |
| F13 (I-LIBS-1, I-UI-1) module and dep declarations | yes | `fe-format/src/lib.rs:17`, `fe-ui/Cargo.toml:22`, `node_manager/mod.rs:34`, `plugin.rs:1556` |
| F17 (I-DOC-CORE) range rationale | partial | see the P1 docs row |
| F18 (I-BATCH-1) filters | yes | `conductor/test-batches.md:58` (crud), `:85` (ground_snap), `:112` (gis::), `:397` (fe-format) |

## HANDED-OFF (owner = next round)

- HANDED-OFF (next round, owner of `fe-ui/src/node_manager/path_point_interaction.rs`): add `trailing_points: Vec::new(),` to the literal at :564 (E0063 compile break).
- HANDED-OFF (next round, owner of `fe-database/src/handlers/petal_terrain.rs`): refuse a terrain whose `world_scale` fails `fe_format::scale::is_world_scale_in_range` (steer rule 3).
- HANDED-OFF (next round, owner of `fe-api/src/terrain.rs`): `InvalidConfig` when `!is_world_scale_in_range(config.world_scale)` after `validate_publisher_pins`.
- HANDED-OFF (next round, owner of `fe-database/src/handlers/object_creation.rs`): add the `(v as f32)` finite and positive check on `scale` in the `prepare` ensure.
- HANDED-OFF (next round, owner of `fe-ui/src/verse_manager/db_results/mod.rs`): RequestFailed arm calls `path_state.fail_pending_pen_create(correlation_id)`; extend the pen flush case with trailing points.
- HANDED-OFF (next round, fe-runtime + fe-ui owners): carry the applied scale on `DbResult::NodeCreated` and set it on the new `NodeEntry.transform` (FR-2 echo).
- HANDED-OFF (next round, I-DOC-CORE successor): one sentence in `fe-format/src/AGENTS.md` §scale-accessor on why the global range is wider than the stamp range.

Verdict: PARTIAL. The canonical accessor, the placement default, the units audit, the API contract and the ground snap are in code; steer rule 3 (write-side refusal) and the object_creation half of rule 2 and the NodeCreated scale echo are open, and fe-ui does not compile until the F6 literal fix lands.
