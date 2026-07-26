# fe-entity-store — crate notes

The in-memory entity hot cache plus the **data-layer node-addressing and
lifecycle spine** for the Spatial Builder Program
(`conductor/tracks/node_lifecycle_addressing_20260725`).

Substantive module rationale lives beside the code in
[`src/AGENTS.md`](src/AGENTS.md):

- **§addressing** — the stable public `fe://verse/fractal/petal/node` address
  (FR-4, Q-1 override) and its reconciliation with the render-side
  content-addressing in `fe-renderer/src/addressing.rs`.
- **§lifecycle** — tombstone delete (FR-1), the empty-husk fix (FR-3), cascade +
  re-flow hook (FR-2), and lazy stamp promotion (FR-5), all held in
  `EntityStore` side-tables so `EntitySnapshot` stays source-compatible for
  fe-query/fe-api.
- **§node-log-cap** — the last-K in-memory op-log window.

Program vocabulary (`DbCommand` op variants + `LifecycleEvent`) is owned by
`fe-runtime/src/messages.rs`; delete/promote authorization by `fe-policy`
(`authorize_node_delete`); durable tombstone/promotion persistence by
`fe-database` (`handlers::crud::{tombstone_node_handler, cascade_tombstone_node_handler, promote_instance_handler}`);
FR-6 event forwarding by `fe-sync::LifecycleForwarder`.

## Source of truth: durable path, not a parallel model

The **durable `fe-database` path is authoritative.** `EntityStore` is a *mirror*
of it, not an independent world the engine can diverge from:

- **Delete** — the DB thread's tombstone/cascade handlers soft-delete the
  SurrealDB row (durable) and emit `SceneChange::NodeRemoved`;
  `EntityStore::apply_scene_change` turns that into an in-memory tombstone, so
  both sides agree the node is gone. The store's `upsert`/`apply_scene_change`
  tombstone guard is the in-memory twin of `fe_database::merge::apply_replicated_node`'s
  durable non-resurrection guard (N-4) — identical semantics on both sides.
- **Create / promote** — mirrored via `SceneChange::NodeAdded` and the
  `LifecycleEvent::{NodeCreated, NodePromoted}` seam.

The durable-path behaviour (soft delete survives reload, cascade is atomic, merge
refuses resurrection, promotion is idempotent) is tested against real SurrealDB
rows in `fe-database` (`durable_lifecycle_tests`, `merge::tests`,
`runtime_lifecycle_tests`) — the in-memory `EntityStore` tests assert the mirror
stays in step, they are no longer the *only* validation of the model.
