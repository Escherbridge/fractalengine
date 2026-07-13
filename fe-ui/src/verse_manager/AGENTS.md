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
- `spawn.rs` — GLTF-backed scene spawning (`spawn_node_entity`), the
  fallback placard sign for asset-less nodes (`spawn_fallback_sign` +
  `FallbackSign` marker component), and primitive-mesh spawning
  (`spawn_primitive_entity`, `build_primitive_mesh`, `PrimitiveNode` marker —
  FR-1). Shared by `db_results.rs`, `petal_respawn.rs`, and
  `primitive_reconcile.rs`.
- `petal_respawn.rs` — despawns/respawns scene entities in-place when
  `NavigationManager::active_petal_id` changes, using the in-memory tree
  directly (no DB round-trip). `NodeEntry` carries no `properties` field
  (adding one would break `db_results.rs`'s explicit-field `NodeEntry`
  construction, which is out of this crate's edit scope — see §primitives),
  so per-petal-switch materialization still spawns primitive nodes as
  fallback signs; `primitive_reconcile.rs` promotes them once selected.
- `primitive_reconcile.rs` — `reconcile_selected_primitive` (FR-1 promotion +
  FR-2 live reconcile) and `resolve_primitive_material` (FR-3 texture
  resolution via `fe_hexon::handlers::material::resolve_material_textures` +
  `FsBlobStore`). `PrimitiveMaterialAssets` holds the shared default material.

`find_petal_mut` on `VerseManager` stays private (not `pub(super)`) — Rust's
privacy rule already makes private items of a module visible to all of its
descendants, so `db_results.rs` can call it without widening the type's
public API.

## §primitives (FR-1..FR-4, `bim_primitives_on_paths_20260712`)

A primitive descriptor (`fe_sdk::primitive::PrimitiveDescriptor`) rides on a
node's `primitive` property as `PropertyValue::Json` (C5). The **only**
currently-wired source of per-node properties in owned files is
`InspectorFormState.node_properties` — populated for the **selected** node
via the existing `NodePropertiesLoaded` DB round-trip (owned by
`db_results.rs`, read-only here). `reconcile_selected_primitive` therefore
drives both materialization (FR-1) and live-edit reconcile (FR-2) off the
selected node only; petal-wide materialization for *all* primitive nodes on
petal switch requires `NodeEntry` to carry `properties`, which is fenced to
a future pass once `db_results.rs`'s `NodeEntry` construction sites can be
touched. `TextureRegistryRes` wraps the engine-decoupled
`fe_sdk::TextureRegistry` as a Bevy `Resource` (the SDK type itself has no
bevy dependency by design).
