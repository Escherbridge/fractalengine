# fe-entity-store/src — module notes

Design rationale for the in-memory entity hot cache. Code carries terse
one-line doc comments; the "why" lives here.

## Scene-change attribution

`fe_runtime::messages::SceneChange` carries `petal_id` on every node-scoped
delta so API subscribers can filter before delivery. The desktop bridge drops
that runtime-only routing field when converting to this crate's local
`SceneChange`, because `EntityStore` already owns the node-to-petal projection.

## §node-log-cap

`EntitySnapshot::node_log` is a **last-K window**, not a full history
(track `p2p_unblock_now_20260711` FR-2). Cap is per-store:
`EntityStore::with_node_log_cap(k)`, default `DEFAULT_NODE_LOG_CAP = 1024`,
clamped to ≥1. All log pushes route through the private `push_log_capped`
helper, which trims from the front after pushing.

Why: `get()` clones the whole snapshot and `append_log`/`apply_scene_change`
clone-mutate-reinsert it, so an unbounded log made every update O(full
history) — directly hostile to IoT-frequency twin updates (decisions
§D1-T0/T1 workload). With the cap, updates are O(K).

Invariants and boundaries:

- **The durable SurrealDB `node_log`/op_log (fe-database) is untouched** and
  remains the full-history source of truth (later the WAL per decisions §D4).
  This cap applies only to the in-memory cache; `get_node_log()` on this
  store can only answer from the retained window.
- `row_version` monotonicity survives trimming: the next version is derived
  from `node_log.last()`, and trimming removes from the *front*, so the max
  row_version is always retained.
- The window is a `Vec` + front-`drain`, not a `VecDeque`: the snapshot is
  already cloned O(K) on every update, so the O(K) memmove at cap changes
  nothing asymptotically, and keeping `Vec` avoids rippling the serialized
  `EntitySnapshot` shape through fe-api consumers.

## §addressing — stable node addresses (FR-4, `address.rs`)

Track `node_lifecycle_addressing_20260725`, decision **Q-1 OVERRIDE**: the
stable node address IS the public `fe://verse/fractal/petal/node` URI, defined
here at the **data layer** (not an opaque-internal key with a later projection).

- `NodeAddress { verse_id, fractal_id, petal_id, node_id }` derives purely from
  the four hierarchy ids. It is therefore **invariant under rename** (display
  name is not an input) **and move** (transform is not an input) — the property
  the spec requires. Re-parenting across petals is not a supported node op, so
  "move" means a spatial transform move only.
- `from_scope_and_id(scope, node_id)` parses the canonical
  `VERSE#v-FRACTAL#f-PETAL#p` scope string, mirroring
  `fe_database::scope::parse_scope` boundary handling so hyphen-bearing ids
  survive. `to_uri`/`from_uri` percent-encode `%`,`/`,`#` so promoted-instance
  ids like `path1#inst-3` round-trip losslessly.
- Resolution: `EntityStore::resolve(&NodeAddress)` looks up by `node_id` (a
  globally-unique ULID for authored nodes; a deterministic `path#inst-N` for
  promoted stamps). Live resolve excludes tombstones;
  `resolve_including_deleted` sees them.

### Data-layer ↔ render-side mapping (reconciliation with `fe-renderer/src/addressing.rs`)

`fe-renderer/src/addressing.rs` is **content-addressing for assets**
(`content_address(bytes) -> AssetId` via blake3; GLB magic validation). It is
**orthogonal** to node addressing — it identifies *asset blobs* by hash, not
*nodes* by scope. There is no overlap to reconcile at the type level:

| Concern | Owner | Key |
|---|---|---|
| Node identity / endpoint | `fe-entity-store::NodeAddress` (T1, this crate) | `fe://v/f/p/n` URI (scope + node id) |
| Asset blob identity | `fe-renderer::addressing::content_address` (T5-owned file) | blake3 `AssetId` |

T5 exposes the `fe://` URI over REST/MCP and, where a node carries an asset,
joins the two by putting the node's `AssetId` in the node's reported payload.
T1 does **not** edit `addressing.rs`; only the semantic contract is documented
here.

## §lifecycle — tombstone delete, cascade, promotion (FR-1/2/3/5)

State that would have changed `EntitySnapshot`'s serialized shape is held in
**side-tables** on `EntityStore` instead (`tombstones`, `children`,
`owning_paths`), because fe-query/fe-api construct `EntitySnapshot` by literal
and must stay source-compatible.

- **Tombstone delete (FR-1, N-4).** `tombstone_node` / `cascade_tombstone`
  record a `Tombstone` in the side-table and append a distinct `Deleted`
  log op; the snapshot is *retained* in the primary map as the tombstone
  record but dropped from the petal index, so normal reads (`get`,
  `get_by_petal`, `all_snapshots`, `node_count`) skip it. **Merge-safety:**
  `upsert` and `apply_scene_change(NodeAdded/…)` are no-ops for a tombstoned
  id, so a stale/concurrent replica can never resurrect a delete (D-A7). A
  tombstone that arrives *before* the create still blocks the later create
  (delete-before-create). Retention is **unbounded** (Q-4 — GC is a filed
  follow-up needing a merge-safety proof).
- **Empty-husk fix (FR-3).** `clear_properties` empties the property bag,
  logs `PropertiesCleared`, and **keeps the node addressable** — it is a
  categorically different op from `Deleted`. The two never share a log op, so
  "clearing properties" can never be mistaken for "deleting the node".
- **Cascade (FR-2).** `cascade_tombstone` collects the whole subtree via the
  `children` adjacency **first**, then tombstones every node — all-or-nothing
  by construction, so a partial failure can never leave a half-deleted tree.
  A stamp delete emits its owning path in `TombstoneOutcome.reflow_paths`
  (the re-flow *hook*; the mesh re-flow math is T2's).
- **Lazy promotion (FR-5, N-9).** A stamp instance is not a store row until
  `promote_instance` materializes one on first select/edit. The node id is
  deterministic (`path#inst-N`) so promotion is **idempotent** and the result
  is addressable by FR-4. `node_count` proves un-promoted instances cost zero
  rows. A tombstoned instance is never re-promoted (merge-safe).

Lifecycle *events* (create/promote/delete/reflow) are the canonical
`fe_runtime::messages::LifecycleEvent` (the program-wide vocabulary); this
crate stays fe-runtime-free and returns plain outcomes
(`TombstoneOutcome`, `(NodeAddress, bool)`) that the DB thread maps to events
and forwards via `fe_sync::LifecycleForwarder` (FR-6).
