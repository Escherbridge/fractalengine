# fe-ui/src/verse_manager — hierarchy tree + DB result draining

- `mod.rs` — `VerseEntry`/`FractalEntry`/`PetalEntry`/`NodeEntry` tree types,
  `VerseManager` resource + its query/mutate methods, `VerseManagerPlugin`,
  and the hierarchy unit tests.
- `node_index.rs` — the `node_index` fast-lookup map + the indexed
  `update_node_position`/`update_node_url`. See §node-index.
- `db_results/` — `apply_db_results` dispatcher + per-domain `DbResult`
  handlers. See §db-results.
- `spawn.rs` — GLTF-backed scene spawning (`spawn_node_entity`), the
  fallback placard sign for asset-less nodes (`spawn_fallback_sign` +
  `FallbackSign` marker component), and primitive-mesh spawning
  (`spawn_primitive_entity`, `build_primitive_mesh`, `PrimitiveNode` marker —
  FR-1). Shared by `db_results.rs`, `petal_respawn.rs`, and
  `primitive_reconcile.rs`.
- `petal_respawn.rs` — despawns/respawns scene entities in-place when
  `NavigationManager::active_petal_id` changes, using the in-memory tree
  directly (no DB round-trip for the entities). Branches per node via
  `primitive_materialize::spawn_branch`: GLTF asset → primitive (warm
  descriptor cache) → fallback sign; cache-miss asset-less nodes get one
  async `GetNodeProperties` fetch. See §primitives.
- `primitive_materialize.rs` — `PrimitiveDescriptorCache` (node_id →
  `PrimitiveDescriptor`, fed by every `NodePropertiesLoaded` broadcast) +
  `materialize_cached_primitives` (petal-wide FR-1: promotes fallback signs /
  spawns fresh / reconciles drift as cached descriptors arrive — no selection
  required). See §primitives.
- `primitive_reconcile.rs` — `reconcile_selected_primitive` (FR-1 promotion +
  FR-2 live reconcile) and `resolve_primitive_material` (FR-3 texture
  resolution via `fe_hexon::handlers::material::resolve_material_textures` +
  `FsBlobStore`). `PrimitiveMaterialAssets` holds the shared default material.
- `path_asset_reconcile.rs` — `reconcile_path_asset` (stamps a hexon model
  along a track's GPX path) + the fe-ui-local arc-length sampler. See
  §path-asset-stamp.

`find_petal_mut` on `VerseManager` stays private (not `pub(super)`) — Rust's
privacy rule already makes private items of a module visible to all of its
descendants, so the `db_results/` handlers can call it without widening the
type's public API.

## §db-results (`code_review_20260430_mega_function`)

`apply_db_results` used to be a single ~620-line match with ~30 arms. It is
now a thin dispatcher (`db_results/mod.rs`) over one `handle_*` function per
`DbResult` variant, grouped by domain:

- `hierarchy.rs` — tree structure: `Seeded`, `HierarchyLoaded`,
  `DatabaseReset`, `VerseJoined`, `Verse/Fractal/PetalCreated`,
  `EntityRenamed`, `EntityDeleted`.
- `nodes.rs` — single-node lifecycle: `GltfImported`, `NodeCreated` (pen
  auto-create flush + Paths-tab re-query), `NodeDeleted`.
- `roles.rs` — invites, peer roles, local role, log-only acks.
- `tokens.rs` — API token mint/revoke/list + `tokens_to_entries` /
  `refresh_inspector_tokens`.
- `properties.rs` — `NodeProperties{Loaded,Set,Deleted}` +
  `is_for_selected_node`. These three return `bool`: `false` means the
  dispatcher must `continue`, which **skips `pending_api.try_deliver` for
  that result** — this preserves the original mega-match's `continue`
  control flow exactly (stale/unselected property results were never
  delivered to pending API requests).
- `query.rs` — the shared untagged `QueryResult`/`Error` channel with its
  GIS-panel > Paths-tab > Query-tab claim priority.
- `fields.rs` / `terrain.rs` — field-def lists, petal terrain docs.

Handlers take the minimal `&`/`&mut` param set (no Bevy system params), so
they unit-test without spinning up ECS — the smoke tests live in
`db_results/mod.rs`. Handlers are `pub(super)`: visible to the dispatcher
and its tests only. The dispatcher keeps the `_ => {}` catch-all (variants
like `ScopeResolved` are consumed solely via `pending_api.try_deliver`).

## §node-index (`code_review_20260430_performance_hotpaths`)

`VerseManager.node_index` maps `node_id → (verse_idx, fractal_idx,
petal_idx)` so drag-commit updates (`update_node_position`,
`update_node_url` — called every gimbal release via
`node_manager/transform_broadcast.rs`) are O(1) average instead of the old
O(n³) tree walk. Maintenance sites:

- `rebuild_node_index()` — after `HierarchyLoaded` (full tree replace),
  `EntityDeleted` (retains shift indices), `DatabaseReset`.
- `add_node(petal_id, node)` / `remove_node(petal_id, node_id)` —
  incremental upkeep used by `GltfImported` / `NodeCreated` / `NodeDeleted`.
- Verse/fractal/petal *creates* append at the end of their Vecs and cannot
  shift existing node indices — no rebuild needed there.

The index is an accelerator, never an authority: lookups verify the indexed
petal actually contains the node and otherwise fall back to the full walk,
healing the entry. Code that mutates `verses` directly (it is still `pub`)
therefore degrades to the old behavior instead of silently missing nodes.
The field is `pub(crate)` only so in-crate test helpers can construct
`VerseManager` literals with `..Default::default()`; do not write to it
outside `node_index.rs`.

## §primitives (FR-1..FR-4, `bim_primitives_on_paths_20260712`)

A primitive descriptor (`fe_sdk::primitive::PrimitiveDescriptor`) rides on a
node's `primitive` property as `PropertyValue::Json` (C5). The hierarchy
payload (`NodeHierarchyData`, fe-runtime — outside this crate's edit scope)
carries no properties, so petal-wide FR-1 runs on a fe-ui-local
**`PrimitiveDescriptorCache`** (`primitive_materialize.rs`) instead of a
`NodeEntry.properties` field:

- **Feed**: every `NodePropertiesLoaded` broadcast calls
  `cache.note_properties` in `db_results/properties.rs` — *before* the
  inspector's selection gate, so unselected results still land. A
  `NodePropertySet`/`Deleted` on the `primitive` key invalidates the entry
  (and `Set` re-issues `GetNodeProperties` regardless of selection);
  `DatabaseReset` clears the cache.
- **Petal (re)spawn** (`petal_respawn.rs`): `spawn_branch` picks GLTF →
  cached primitive → fallback sign. Warm cache materializes immediately; a
  cold asset-less node spawns its sign and sends one deduped
  `GetNodeProperties` (`mark_requested`).
- **Promotion**: `materialize_cached_primitives` (chained after
  `apply_db_results` + `respawn_on_petal_change`; the chain's sync points
  make their deferred spawns visible) walks the active petal on cache
  change: reconciles descriptor drift on spawned `PrimitiveNode`s, promotes
  `FallbackSign`s (undoing the +0.5 hover offset), or spawns fresh from the
  tree position. No selection involved.
- `reconcile_selected_primitive` remains the **inspector-driven update path**
  (FR-2 live edits off `InspectorFormState.node_properties` for the selected
  node, plus legacy FR-1 promotion).

`TextureRegistryRes` wraps the engine-decoupled `fe_sdk::TextureRegistry` as
a Bevy `Resource` (the SDK type itself has no bevy dependency by design).

## §path-asset-stamp (`hexon_path_asset_stamp_20260713`)

Stamps a hexon's model asset repeatedly along a **track** node's GPX path —
the core "hexon-as-path-asset" feature. A `path_asset` descriptor
(`fe_sdk::path_asset::PathAssetDescriptor`, see `fe-sdk/src/AGENTS.md`
§path-asset) rides the track node's property bag; the Tools panel writes it
via `UiAction::PathAssetApply` → `SetNodeProperty`.

`reconcile_path_asset` is the consuming system. It keys off the **Paths-tab**
selection — `PathEditorState.editing_track_id` — NOT the viewport/tree
selection (`NodeManager.selected`). The Tools panel stamps the Paths-tab
track, so the original gating on `NodeManager` + `InspectorFormState.node_properties`
silently dropped every stamp whenever the two selections diverged (the common
case: stamp from the Paths tab without also viewport-selecting the ribbon).
It now reads the descriptor from `PathEditorState.edited_track_path_asset` and
the points from `PathEditorState.points` directly — both seeded by the
`PathSelectTrack` read-back and refreshed after a `PathAssetApply` write (see
below). Still active-petal-gated (`nav.active_petal_id`).

`edited_track_path_asset` is populated in `db_results`'s `NodePropertiesLoaded`
arm alongside `points`/`edited_track_style` (parsed from the `path_asset` prop
via `PathAssetDescriptor::from_json`), and reset in `start_editing`/`stop_editing`.
After a `PathAssetApply` writes the property, `db_results`'s `NodePropertySet`
arm — when the written node is the editing track — re-arms `points_pending` and
re-issues `GetNodeProperties`, so the fresh descriptor flows back through
`NodePropertiesLoaded` and drives the next reconcile (`hexon_path_asset_stamp_20260713`
in-app fix).

The stamp is a per-instance `SceneRoot` (one shared `Handle<Scene>` across all
instances — cheap) spawned by `spawn::spawn_stamped_entity`, an additive
sibling of `spawn_node_entity` that takes a full `Transform` (so the tangent
rotation bakes in) and tags each entity with a `PathAssetInstance` marker
carrying the source track id + petal. That marker lets the system despawn and
rebuild the whole stamped group as a unit.

Change-gating: `PathAssetApplied` (a `Resource`) records the last-applied
`(track_id, descriptor, points-fingerprint)`. `points_fingerprint` is an
FNV-1a hash over the bit-cast coordinates + length (order-sensitive), so a
changed path or descriptor re-stamps and an unchanged one is a no-op — the
same equality-gate discipline as the primitive reconcile's `descriptor ==`
check. The system runs each frame in the `.before(UiSet::ProcessActions)`
group in `VerseManagerPlugin::build`, gated by the cheap `matches()` check.

The arc-length sampler is a focused, `[f32;3]`-based port of
`fe_terrain::iot::PathTracker` (fe-ui must **not** depend on fe-terrain):
`cumulative_distances` / `position_at_progress` / `sample_progresses` /
`tangent_yaw`. `FixedSpacing` places instances every `spacing_value`
world-units (guarding non-positive spacing → endpoints only, no
div-by-zero); `FixedCount` distributes `count` instances evenly (count 0 →
none, 1 → start only). `tangent_yaw` returns the `Quat::from_rotation_y`
angle (`atan2(dx, dz)`, aiming the model's -Z forward down the path) applied
only when `descriptor.tangent_align`.
