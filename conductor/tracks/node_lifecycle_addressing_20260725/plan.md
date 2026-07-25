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

- [ ] Task: define the stable node address type + deterministic derivation from
      scope + id; prove stability across move/rename with a pure test.
- [ ] Task: resolver (address → node handle) + serialization round-trip test.
- [ ] Task: reconcile with `fe-renderer/src/addressing.rs` semantics (document
      the data-layer ↔ render-side mapping in `fe-entity-store/AGENTS.md`).

## Phase 2: Sync-safe delete op + authz (FR-1, FR-3) [P0]

- [ ] Task: add the `delete(node)` op as a tombstone op-log entry (no raw drop);
      failing merge test first (stale replica must not resurrect).
- [ ] Task: authorize delete through `fe-policy` (Editor+ on scope); reject test.
- [ ] Task: **empty-husk regression** — prove clear-properties keeps an
      addressable node and delete removes it; assert they are distinct ops.

## Phase 3: Cascade + re-flow lifecycle (FR-2) [P1]

- [ ] Task: cascade tombstones across a subtree as one logical op; atomicity
      test on a 3-level tree; partial-failure leaves no half-deleted subtree.
- [ ] Task: emit `PathReflow`/stamp-delete lifecycle event carrying the owning
      path id (geometry re-flow is T2's; this is the hook + event only).

## Phase 4: Lazy node-promotion primitive (FR-5) [P1]

- [ ] Task: instance→node promotion op (idempotent) + materialization to an
      FR-4-addressable node; emit promotion event.
- [ ] Task: row-count test — un-promoted instances add zero per-instance store
      rows; promotion adds exactly one.

## Phase 5: Lifecycle events on sync + reporting seam (FR-6) [P1]

- [ ] Task: typed create/promote/delete/reflow events on the op-log/replication
      seam; one-event-per-op test; events carry the stable address.
- [ ] Task: replication-bridge forwarding test (events reach the sync path).

## Phase 6: Docs + integrated sweep [P1]

- [ ] Task: `fe-entity-store/AGENTS.md` + `fe-policy` notes — address scheme,
      tombstone/cascade semantics, promotion contract (N-7).
- [ ] Task: single workspace sweep — `clippy -D warnings`, `fmt --check`,
      `cargo test --workspace` (N-6; use `RUST_MIN_STACK` + `-j2` if the OOM
      blocker is live).
- [ ] Task: retro; flip metadata `status` when consumers have verified the ops.
