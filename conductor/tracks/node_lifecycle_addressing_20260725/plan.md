---
type: Implementation Plan
title: "Implementation Plan: Node Lifecycle & Addressing (Program Spine)"
tags: [node_lifecycle_addressing_20260725]
resource: ./spec.md
---

# Implementation Plan: Node Lifecycle & Addressing

Six phases, TDD (pure decision helpers + op-log contracts first, wiring after).
Single workspace sweep at the very end (N-6). Phases 1-2 are the minimum the
Wave-1 tracks need; ship them first so consumers unblock early.

## Phase 1: Stable addressing scheme (FR-4) [P0]

- [x] Task: define the stable node address type + deterministic derivation from *(AUDIT 2026-10-10: fe-entity-store/src/address.rs `NodeAddress` (fe://verse/fractal/petal/node, Q-1 override) — tests `derive_from_petal_scope`, `address_is_stable_across_rename_and_move`; c6fdf0d)*
      scope + id; prove stability across move/rename with a pure test.
- [x] Task: resolver (address → node handle) + serialization round-trip test. *(AUDIT 2026-10-10: address.rs tests `uri_round_trip`, `serde_round_trip`, `from_uri_rejects_malformed`; REST resolver `GET /api/v1/address?uri=` (T5 cdd3291))*
- [x] Task: reconcile with `fe-renderer/src/addressing.rs` semantics (document *(AUDIT 2026-10-10: fe-renderer/src/addressing.rs `RenderNodeAddress` lock-step mirror, test `render_uri_matches_t1_format` (cdd3291); documented in fe-entity-store/src/AGENTS.md §addressing + fe-api/AGENTS.md §endpoint-surface)*
      the data-layer ↔ render-side mapping in `fe-entity-store/AGENTS.md`).

## Phase 2: Sync-safe delete op + authz (FR-1, FR-3) [P0]

- [x] Task: add the `delete(node)` op as a tombstone op-log entry (no raw drop); *(AUDIT 2026-10-10: fe-entity-store lib.rs tests `stale_replica_upsert_cannot_resurrect_tombstone`, `scene_node_removed_is_a_tombstone_not_hard_drop`; durable merge fe-database/src/merge.rs `apply_replicated_node` wired into live replication (handlers/replicated_row.rs:19); harness scenario `two_peer_tombstone_sync`; c6fdf0d)*
      failing merge test first (stale replica must not resurrect).
- [x] Task: authorize delete through `fe-policy` (Editor+ on scope); reject test. *(AUDIT 2026-10-10: fe-policy/src/node_lifecycle.rs tests `viewer_and_below_may_not_delete`, `editor_and_above_may_delete`; fe-database/tests/node_lifecycle_test.rs `tombstone_is_authorized_survives_reload_and_emits_one_event`, `local_caller_is_authorized_by_db_resolved_role` (aeba832))*
- [x] Task: **empty-husk regression** — prove clear-properties keeps an *(AUDIT 2026-10-10: fe-entity-store lib.rs test `clear_properties_keeps_node_delete_removes_it` (distinct `PropertiesCleared` vs `Deleted` ops))*
      addressable node and delete removes it; assert they are distinct ops.

## Phase 3: Cascade + re-flow lifecycle (FR-2) [P1]

- [x] Task: cascade tombstones across a subtree as one logical op; atomicity *(AUDIT 2026-10-10: tests `cascade_tombstones_three_level_subtree_atomically` (entity store) + `node_lifecycle_test.rs::cascade_tombstones_whole_subtree`; residual: op append and row UPDATE are not one transaction (fe-database/src/AGENTS.md §lifecycle) — ledgered)*
      test on a 3-level tree; partial-failure leaves no half-deleted subtree.
- [x] Task: emit `PathReflow`/stamp-delete lifecycle event carrying the owning *(AUDIT 2026-10-10: tests `stamp_delete_emits_reflow_for_owning_path`, `node_lifecycle_test.rs::tombstoning_a_promoted_stamp_reflows_with_deleted_index`)*
      path id (geometry re-flow is T2's; this is the hook + event only).

## Phase 4: Lazy node-promotion primitive (FR-5) [P1]

- [x] Task: instance→node promotion op (idempotent) + materialization to an *(AUDIT 2026-10-10: tests `promote_materializes_exactly_one_addressable_node`, `promotion_is_idempotent`, `node_lifecycle_test.rs::promote_is_authorized_and_idempotent`)*
      FR-4-addressable node; emit promotion event.
- [x] Task: row-count test — un-promoted instances add zero per-instance store *(AUDIT 2026-10-10: test `unpromoted_instances_add_zero_rows` + `promote_materializes_exactly_one_addressable_node`)*
      rows; promotion adds exactly one.

## Phase 5: Lifecycle events on sync + reporting seam (FR-6) [P1]

- [x] Task: typed create/promote/delete/reflow events on the op-log/replication *(AUDIT 2026-10-10: fe-runtime/src/messages.rs `LifecycleEvent`; `node_lifecycle_test.rs::repeat_tombstone_is_idempotent_and_emits_no_second_event` (one event per real op))*
      seam; one-event-per-op test; events carry the stable address.
- [x] Task: replication-bridge forwarding test (events reach the sync path). *(AUDIT 2026-10-10: fe-sync/src/lifecycle.rs tests `forwarded_event_reaches_the_sync_path`, `all_variants_forward`, `full_channel_drops_without_blocking`)*

## Phase 6: Docs + integrated sweep [P1]

- [x] Task: `fe-entity-store/AGENTS.md` + `fe-policy` notes — address scheme, *(AUDIT 2026-10-10: fe-entity-store/AGENTS.md + src/AGENTS.md §addressing/§lifecycle, fe-policy/AGENTS.md, fe-database/src/AGENTS.md §lifecycle, fe-sync/src/AGENTS.md §lifecycle-forwarding (c6fdf0d))*
      tombstone/cascade semantics, promotion contract (N-7).
- [x] Task: single workspace sweep — `clippy -D warnings`, `fmt --check`, *(AUDIT 2026-10-10: spine suites green @c6fdf0d; superseded by the F19 workspace sweep 3,245/0 @1e9be3a)*
      `cargo test --workspace` (N-6; use `RUST_MIN_STACK` + `-j2` if the OOM
      blocker is live).
- [x] Task: retro; flip metadata `status` when consumers have verified the ops. *(AUDIT 2026-10-10: consumers T2/T3/T4/T5 landed (56ccc3b/09e8e2e/4f765fe/cdd3291) and were integrated @623145f + 631f389; status flipped to done in this audit; in-app verification of delete/promote flows remains user-gated)*
