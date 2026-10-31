---
type: Track Spec
title: Node Lifecycle & Addressing — Tombstone Delete, Cascade + Re-flow, Stable Addresses, Lazy Promotion
description: The data-layer spine of the Spatial Builder Program. Adds a sync-safe delete op (tombstone + cascade-to-children + path re-flow) that fixes the "empty husk" bug, a stable per-node addressing scheme that is the substrate for the read/write API (T5) and stamp-as-node (T2), and a lazy node-promotion primitive so tens-of-thousands of addressable stamp nodes stay smooth. Owns the data core exclusively (fe-entity-store, fe-database, fe-policy, fe-sync); every Wave-1 track builds on it.
tags: [feature, node_lifecycle_addressing_20260725, pending]
timestamp: 2026-07-25T00:00:00Z
resource: ./metadata.json
---

# Specification: Node Lifecycle & Addressing (Program Spine)

**Track ID:** `node_lifecycle_addressing_20260725`
**Type:** feature (data-layer foundation)
**Wave:** 0 · **Blocks:** stamped_asset_nodes, sculpt_earthwork_regions, contextual_controls, endpoint_api_surface
**Crates (exclusive):** `fe-entity-store`, `fe-database`, `fe-policy`, `fe-sync`

Anchor: [`../../decisions/spatial-builder-program-20260725.md`](../../decisions/spatial-builder-program-20260725.md)
(thesis, decision register D-A1..A11, shared NFR pool N-1..N-10, file partition).

## Overview

This track is the spine. Three of the program's decisions are meaningless
without it: delete must be real and sync-safe (D-A7), every object must be
addressable so it can carry an API (D-A4), and stamp-as-node must scale to tens
of thousands without a full node per stamp on the hot path (D-A5 + D-A6). It
touches only the data core, so it runs in Wave 0 fully parallel with the shell
UX track, and must land before the Wave-1 tracks that consume its ops.

### Ground truth (2026-07-25)

- Delete surfaces today are partial and not sync-safe: property edits go through
  `fe-ui actions/node_props.rs`, but there is no cascade-safe node delete — the
  reported **"empty husk" bug** is exactly this (clearing properties leaves the
  node). Nodes live in `fe-entity-store` / `fe-database`; sync is op-log-based
  via `fe-sync` (auth never LWW, log-first WAL — hexon-p2p-commons D1-D6).
- An addressing concept already exists render-side at
  `fe-renderer/src/addressing.rs`; the canonical scope hierarchy is
  `VERSE#v-FRACTAL#f-PETAL#p` + node id. This track defines the **data-layer**
  stable address; T5 exposes it and reconciles the render-side view.
- `fe-policy` (deny-by-default) is the home of `RoleLevel` and is already wired
  into the hexon + sync write path — delete authorization plugs in here.

## Functional Requirements

- **FR-1 — Sync-safe delete op (tombstone).** A first-class `delete(node)` op
  writes a tombstone into the op-log rather than dropping a row, so a delete
  survives P2P/HLC merge and cannot be resurrected by a concurrent replica
  (N-4). The op is authorized through `fe-policy` (Editor+ on the node's scope).
  *Acceptance:* deleting a node then merging a stale replica that still holds it
  keeps it deleted (unit test on the merge path); an unauthorized caller is
  rejected; no raw row drop remains on the delete path.

- **FR-2 — Cascade + path re-flow.** Deleting a parent cascades tombstones to
  its descendants as one logical op (confirm-gated at the UI layer, T4).
  Deleting a single stamp node re-flows the owning path so the remaining stamps
  re-distribute (the geometry re-flow contract is consumed by T2; this track
  provides the lifecycle hook + event, not the mesh math). *Acceptance:*
  cascade tombstones every descendant atomically (test on a 3-level subtree);
  a stamp delete emits a `PathReflow`/lifecycle event carrying the owning path
  id; partial-failure leaves no half-deleted subtree.

- **FR-3 — "Empty husk" fix (delete ≠ clear-props).** Clearing a node's
  properties must never be confused with deleting it, and there must be a real
  path to remove a node entirely. Property-clear keeps the node; delete removes
  it (via FR-1). *Acceptance:* a regression test proves clear-properties leaves
  an addressable node and delete removes it; the two operations are distinct ops
  in the op-log.

- **FR-4 — Stable node addressing scheme.** Every node has a canonical, stable
  address derived from its scope + id that survives rename/move and is the
  substrate T5 turns into a read/write endpoint (D-A4) and T2 uses to address
  individual stamps (D-A5). *Acceptance:* address is deterministic and stable
  across a move/rename (unit test); resolvable back to a node handle;
  round-trips through serialization; documented in `fe-entity-store/AGENTS.md`.

- **FR-5 — Lazy node-promotion primitive.** A lightweight instance (e.g. one of
  122 stamps) can be **promoted** to a full addressable node on demand
  (first individual selection/edit), and until then costs no per-node store
  row. This is the data-model contract T2's stamp instancing depends on; this
  track ships the promotion primitive + its op-log event, T2 ships the stamp
  producer/consumer. *Acceptance:* promoting an instance materializes a node
  addressable by FR-4 and emits a lifecycle event; un-promoted instances add no
  per-instance store rows (test asserts row count); promotion is idempotent.

- **FR-6 — Lifecycle events on the sync + reporting seam.** create / promote /
  delete(tombstone) / reflow each emit a typed lifecycle event on the existing
  op-log/replication seam so sync stays consistent and reporting (T5) sees the
  same truth. *Acceptance:* each op produces exactly one lifecycle event;
  events carry the stable address (FR-4); replication bridge forwards them.

## Non-Functional Requirements

Inherits the shared pool (N-1..N-10). Load-bearing here: **N-4** (sync-safe,
no raw drops), **N-5** (authz in fe-policy, no UI authz, no `block_on`),
**N-9** (no per-instance store cost until promotion), **N-10** (deleted +
promoted nodes remain consistent for reporting). No new crate dependencies.

## Dependencies & concurrency

- **depends_on:** none. **blocks:** all four Wave-1 tracks.
- **Owns exclusively:** the four data-core crates — zero file overlap with any
  other track, so Wave 0 runs fully parallel with `shell_ux_sidebar`.
- **Consumers:** T2 (FR-2 reflow event + FR-5 promotion), T4 (FR-1 delete op +
  FR-2 cascade confirm), T5 (FR-4 address + FR-6 events), T3 (region nodes are
  ordinary nodes created via this lifecycle).

## Open questions (ratify before build)

- **Q-1 — Address form.** Extend the existing scope-string node id with an
  opaque-but-stable data-layer key (recommended — minimal churn, reconciles
  with `fe-renderer/addressing.rs`), or introduce a public `fe://verse/fractal/
  petal/node` URI now? *Recommended:* stable internal key now; T5 owns the
  public URI projection.
- **Q-2 — Promotion trigger.** Promote on first individual **selection/edit**
  (recommended — cheapest, matches "materialize when addressed") or on any
  read/address (heavier, simpler mental model)?
- **Q-3 — Cascade confirm.** Always confirm on any cascade (recommended) or only
  above a child-count threshold?
- **Q-4 — Tombstone GC.** Do tombstones ever compact, or is retention unbounded
  (log-first WAL)? *Recommended:* unbounded for now; file a follow-up — GC needs
  a merge-safety proof.

## Ratified decisions (2026-07-25)

User ratified via AskUserQuestion 2026-07-25. Normative for the phases they gate.

- **Q-1 → RATIFIED (OVERRIDE of the spec default): public `fe://verse/fractal/
  petal/node` URI at the data layer now.** The stable address is the public,
  human-readable URI itself (materialized from scope + id), not an
  opaque-internal key with a later T5 projection. This settles T5 Q-1 too and
  shifts the T1↔T5 boundary: **T1 *defines* the public URI; T5 *exposes* it over
  REST/MCP and reconciles `fe-renderer/src/addressing.rs`** (no partition change —
  `addressing.rs` stays T5's). Gates FR-4.
- **Q-2 → RATIFIED: promote on first individual select/edit.** Materialize a node
  only when the instance is individually addressed; un-promoted instances add
  zero store rows (N-9). Gates FR-5.
- **Q-3 → RATIFIED: always confirm on any cascade** (confirm is UI-side, T4). No
  child-count threshold. Consistent with T4 Q-2. Gates FR-2.
- **Q-4 → RATIFIED: tombstone retention unbounded for now** (log-first WAL);
  compaction/GC is a filed follow-up needing a merge-safety proof. Gates FR-1
  scope boundary.

## Out of scope

- The UI for delete/cascade confirm and context-menu wiring (T4).
- Stamp mesh re-flow math and instanced rendering (T2 consumes the FR-2 event).
- The public REST/MCP endpoint + query surface over addresses (T5).
- Tombstone compaction/GC (Q-4 follow-up).
