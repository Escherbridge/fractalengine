# fe-entity-store/src — module notes

Design rationale for the in-memory entity hot cache. Code carries terse
one-line doc comments; the "why" lives here.

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
