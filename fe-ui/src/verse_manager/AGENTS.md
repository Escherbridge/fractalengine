# fe-ui/src/verse_manager — hierarchy tree + DB result draining

- `mod.rs` — `VerseEntry`/`FractalEntry`/`PetalEntry`/`NodeEntry` tree types,
  `VerseManager` resource + its query/mutate methods, `VerseManagerPlugin`,
  and the hierarchy unit tests.
- `db_results.rs` — `apply_db_results`, the large `DbResult` match that
  updates the in-memory tree, dialog state, and inspector state in response
  to every DB thread reply. Also owns `tokens_to_entries` /
  `refresh_inspector_tokens` (API token list bookkeeping) and
  `is_for_selected_node` — the `NodePropertiesLoaded`/`NodePropertySet`/
  `NodePropertyDeleted` arms gate on this (dropping stale results for a
  node that's no longer selected) as part of the annotation-save fix; see
  root `AGENTS.md` §gis-query-ui.
- `spawn.rs` — GLTF-backed scene spawning (`spawn_node_entity`) and the
  fallback placard sign for asset-less nodes (`spawn_fallback_sign` +
  `FallbackSign` marker component). Shared by both `db_results.rs` (spawn on
  load/import) and `petal_respawn.rs` (spawn on petal switch).
- `petal_respawn.rs` — despawns/respawns scene entities in-place when
  `NavigationManager::active_petal_id` changes, using the in-memory tree
  directly (no DB round-trip).

`find_petal_mut` on `VerseManager` stays private (not `pub(super)`) — Rust's
privacy rule already makes private items of a module visible to all of its
descendants, so `db_results.rs` can call it without widening the type's
public API.
