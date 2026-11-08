# fe-ui/src/verse_manager — hierarchy tree + DB result draining

## Admitted replica current state

`replica_projection.rs` consumes only the DB's typed, Viewer-fenced resulting
current row. It does not convert a remote clock, edit sequence or old local
journal fact into a local mutation receipt. The manager validates loaded stored
ancestry, complete renderable TRS and the actual asset identity. It updates
resident ordinary entities in place, rematerializes changed asset paths through
the existing residency budgets, and leaves new births to the same spawner.
Missing parents request one coalesced hierarchy refresh followed by a fresh
owned current read; the original incomplete projection is never replayed later.

`replica_reads.rs` owns at most256 current-read intents. Accepted reads bind a
fresh nonce, navigation intent, actual Petal and node-cache epoch. Full channels
retain unsent reads; five-second timeouts and disconnected/failing reads use
capped backoff. A hierarchy replacement retires old nonces. An observed related
role change clears scene source bindings and waits for a fresh effective role;
the DB's actual Viewer check remains authoritative even before UI role feedback.
These queues belong to VerseManager, so terminal reset replaces their entire
state along with the scene manager. No mutation is retried.

The matching accepted LocalRoleResolved path initializes an absent access/source
generation in accept_scene_role, after the exact current-scope check. It preserves
any epoch already minted by actual access invalidation; role resolution never
replaces that epoch. Initialization is not a role grant: unknown/None roles remain
blocked and stale-scope replies change neither epoch nor cached role. The shared
crate-private apply_local_role_result seam applies the existing manager/cache/Pen
effects together and is used by the dispatcher and native navigation journey.
The latter starts from the real default manager and actual product_guard with no
field override; the existing index journey still checks invalidation preservation.

`scene_admission.rs` gives admitted render sources a manager incarnation and a
checked monotonic epoch. Actual local receipts additionally bind their positive
local journal version; replica/current reads do not. Canonical BLAKE3 hashes bind
the full property bag and exact path rows without retaining another full bag per
node. A full current read whose render data is already identical is idempotent:
it does not cancel a newer preview or clear a draft. Local-history numbers are
retained only as stale-receipt floors, never attributed to a changed peer row.

The host's local path queues must bind the exact admitted local receipt version
and manager incarnation. A reader that runs before that admission waits under a
bounded deadline; it cannot attach the manager's arbitrary newer epoch to an old
queue item. Current path work carries its admission token at production and is
discarded when a later admitted source replaces it. Plain property replies never
mint this proof. A matching completed-save receipt remains the save outcome;
CurrentRead/UncommittedRead recovery refreshes through the separate full-current
read protocol rather than inventing a committed journal version.

Complete current bags feed the shared primitive/stamp/visibility/callout/Inspector
hydration seam. Conflicting local path drafts are retained with an explicit
stored-state-change advisory. Tombstones remove tree/resident/cache/selection
ownership, and queue route teardown under the same current-source fence. Asset
metadata with unavailable bytes retains its identity and no usable path; this
adapter never claims a successful download or peer historical convergence.

The existing DB-result/residency App journey now also covers remote TRS and rich
null-bearing data, an idempotent read during a newer preview, actual asset-branch
rematerialization, current-read nonce/epoch rejection, ordinary births and node
tombstone cleanup. The existing index journey checks local/current source
separation, hierarchy invalidation and manager replacement. These are authored
component checks; no source-author execution or real peer/GPU claim is implied.

## §committed-object-materialization

Owned editor-batch results validate the full request outcome and preflight every member
before projecting any. Group registry, properties and descriptor caches are updated only
after that complete admission. Route rows enter the host's existing confirmed path
projection seam. Pure legacy current snapshots without journal versions request normal
hierarchy/property hydration; no fictitious committed version is inserted. See
`../editor_batch/AGENTS.md` for ownership and reconciliation behavior.

`db_results/committed.rs` adapts only current `NodeMaterialized` and `NodeMutationCommitted`
snapshots. Exact-version history results have no scene-write path. The manager rejects regressing
commit versions and unexpected parent changes, upserts one tree entry, and retains complete transforms.
New active-petal objects materialize through the existing residency and spawn budgets; existing resident
entities receive the confirmed transform in place. A newer current commit cancels an unfinished local
axis drag. Complete property bags feed the existing primitive, path, visibility and selected-Inspector
hydration path, retaining the typed editor's draft/conflict decisions.

Hierarchy transport includes unit quaternion rotation and scale. Legacy quaternion and ordinary Euler
storage share `fe_runtime::node_history::stored_rotation_quaternion`; all respawn branches retain them.
Fallback placards encode their half-unit display offset in mesh vertices, so the marker entity,
Inspector, gimbal and confirmed transform all retain the logical node position. Unknown asset-backed
receipts request authoritative hierarchy hydration instead of creating a pathless replacement.

Initial hierarchy navigation selects one populated petal with its actual ancestors, falling back to
the first empty verse. Delayed role replies are discarded when their scope no longer matches the valid
current cursor. See `../AGENTS.md` §navigation-ancestry.

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
  `primitive_reconcile.rs`. All three tree-node spawners tag `ResidentNode`
  (the residency ledger owns those entities' lifetime, §chain-order); also
  hosts `ResidencyBudget` and the stamp-budget taper `distance_ranked_allowance`.
- `petal_respawn.rs` — despawns/respawns scene entities in-place when
  `NavigationManager::active_petal_id` changes, using the in-memory tree
  directly (no DB round-trip for the entities), spawning only the nodes the
  residency ledger admits. Owns `NodeSpawner`, the shared per-node branch
  (`primitive_materialize::spawn_branch`: GLTF asset → primitive (warm
  descriptor cache) → fallback sign; cache-miss asset-less nodes get one
  async `GetNodeProperties` fetch) that `apply_node_residency` reuses. See
  §primitives.
- `residency_ledger.rs` — `ResidencyLedger` (D-74 node + stamp-group
  pointer-set), `update_residency_ledger` and `apply_node_residency`. See
  §chain-order and fe-ui `src/AGENTS.md` §residency-ledger.
- `primitive_materialize.rs` — `PrimitiveDescriptorCache` (node_id →
  `PrimitiveDescriptor`, fed by every `NodePropertiesLoaded` broadcast) +
  `materialize_cached_primitives` (petal-wide FR-1: promotes fallback signs /
  spawns fresh / reconciles drift as cached descriptors arrive — no selection
  required). See §primitives.
- `primitive_reconcile.rs` — `reconcile_selected_primitive` (FR-1 promotion +
  FR-2 live reconcile) and `resolve_primitive_material` (FR-3 texture
  resolution via `fe_hexon::handlers::material::resolve_material_textures` +
  `FsBlobStore`). `PrimitiveMaterialAssets` holds the shared default material.
- `path_asset_reconcile.rs` — the fe-ui-local arc-length sampler
  (`sample_transforms` + metric `sanitize_world_scale`) plus
  `reconcile_path_asset`, which no longer spawns: it FEEDS the shared
  `PathAssetCache` with the Paths-tab edited track's live descriptor + points.
  See §path-asset-materialization.
- `path_asset_materialize.rs` — `PathAssetCache` (track_id →
  descriptor/points/petal, fed by `NodePropertiesLoaded` + the live feeder),
  the per-track `PathAssetApplied` change gate, and `materialize_path_assets`
  (the single system that does ALL stamp spawning — petal-wide FR-1). See
  §path-asset-materialization.

## §chain-order (p2p_asset_streaming S8)

`VerseManagerPlugin`'s `Update` chain, in order:

1. `apply_lifecycle_events`
2. `apply_db_results`
3. `update_residency_ledger`
4. `respawn_on_petal_change`
5. `apply_node_residency`
6. `materialize_cached_primitives`
7. `reconcile_selected_primitive`
8. `reconcile_path_asset`
9. `materialize_path_assets`

The ledger has to see this frame's tree, so it runs after `apply_db_results`,
and it has to run before every spawner, so it precedes the rest.
`apply_node_residency` follows the petal-entry batch. The chain's automatic
sync points make that batch's spawns visible to it, which is why it never
double-spawns a node. Tree-node entities carry `spawn::ResidentNode`. Only those
are reconciled; stamps, ribbons and track icons are not.

`find_petal_mut` on `VerseManager` stays private (not `pub(super)`) — Rust's
privacy rule already makes private items of a module visible to all of its
descendants, so the `db_results/` handlers can call it without widening the
type's public API.

## §db-results (`code_review_20260430_mega_function`)

`apply_db_results` used to be a single ~620-line match with ~30 arms. It is
now a thin dispatcher (`db_results/mod.rs`) over one `handle_*` function per
`DbResult` variant, grouped by domain:

- `hierarchy.rs` — tree structure: `Seeded`, `HierarchyLoaded`,
  `DatabaseReset`, `VerseJoined`, `JoinVerseFailed`, `Verse/Fractal/PetalCreated`,
  `EntityRenamed`, `EntityDeleted`, and `ElevatedQueryResult` (an API
  `/query/elevated` write with `changed_rows > 0` has no typed result the
  tree could apply, so it reloads; the result still reaches `try_deliver`).
  `VerseJoined`/`JoinVerseFailed` (polish_lane_20261031 OD-2/M-03) negotiate
  with `UiManager.join_request` (W1-S1's contract): while `pending`, success
  closes the dialog + navigates + toasts, and a failure writes
  `join_request.error` for the dialog to render inline instead of a toast;
  without a pending dialog a failure still reaches the user as a general
  error notice (`ui_mgr.show_error`) rather than being dropped. The same
  `DialogRequestState` shape closes the glTF Import dialog:
  `mod.rs::finish_gltf_import_request` runs after `nodes.rs`'s
  `GltfImported` handler (`dialogs/AGENTS.md` §dialog-requests).
- `nodes.rs` — single-node lifecycle: `GltfImported`, `NodeCreated` (pen
  auto-create flush + Paths-tab re-query), `NodeDeleted`.
- `roles.rs` — invites, peer roles, local role, log-only acks, and (M2/F6)
  `VerseTimeseriesSettingsSet` — the authoritative echo that updates
  `TimeseriesSettingsState` so the settings surface never shows an
  optimistic guess.
- `tokens.rs` — API token mint/revoke/list + `tokens_to_entries` /
  `refresh_inspector_tokens`.
- `properties.rs` — `NodeProperties{Loaded,Set,Deleted}` +
  `is_for_selected_node`. These three return `bool`: `false` means the
  dispatcher must `continue`, which **skips `pending_api.try_deliver` for
  that result** — this preserves the original mega-match's `continue`
  control flow exactly (stale/unselected property results were never
  delivered to pending API requests).
- `query.rs` — GUI `QueryResult`/`QueryFailed` routed by request id
  (`route_query_reply`; claimed → `continue`, unclaimed → `try_deliver`),
  the legacy uncorrelated flag-priority claim, and `Error`'s pen-create
  cleanup (fe-ui/src/AGENTS.md §gis-query-ui). X-01 (polish_lane_20261031):
  `handle_error` ALSO turns every bare `DbResult::Error` into
  `ui_mgr.show_error(humanize_db_error(msg, &ctx))` (`ctx` built from
  `LocalUserRole` + the active verse name) — previously these only reached
  the log, so a denied command (e.g. a GUI `SetPetalTerrain`) silently
  failed with no UI trace beyond a vanishing toast at the old call site.
- `fields.rs` / `terrain.rs` — field-def lists, petal terrain docs. `terrain.rs`
  also rehydrates `ProposalEditState` (5th param via DescriptorCaches.proposal_state)
  and, since p2p_asset_streaming S6, handles `PetalRenderDistanceLoaded`
  (writes `VerseManager::update_petal_render_distance`, consumed by every
  spawner through `ResidencyBudget::render_distance_for`). `mod.rs`'s own
  `ReplicatedRowApplied { table: "petal", outcome: Applied, .. }` arm
  re-requests `GetPetalRenderDistance` so a peer's change lands without a
  full hierarchy reload. QA-0142: a replicated `fractal` or `petal` row whose
  id is NOT yet in the tree (a peer created it) requests one coalesced
  authoritative `LoadHierarchy` via `request_committed_hierarchy` — the echo
  carries no name/parent, `LoadHierarchy` applies no access filter, and the
  sidebar's "Open fractal/petal X" rows read only the tree. Known ids never
  reload, so steady petal churn (terrain, render distance) costs nothing and
  the reload cannot loop. M-03 (polish_lane_20261031): `mod.rs` now threads
  `correlation_id.as_deref()` into `handle_petal_terrain_loaded` (6th param)
  and dispatches the new `PetalTerrainSetFailed` to
  `handle_petal_terrain_set_failed` — both calls into W1-S3's contract
  shape; the rollback/`PetalMapState.pending_set` logic lives in
  `terrain_map/AGENTS.md`, not here.

**`DescriptorCaches` is also the SystemParam-overflow valve (M2/F6).**
`apply_db_results` sits at Bevy's 16-`SystemParam` ceiling, so any new
resource it needs goes INTO the `DescriptorCaches` bundle rather than onto
the system's parameter list: `proposal_state` (terrain rehydration) and
`ts_settings` (`TimeseriesSettingsState` — `hierarchy.rs` folds each verse's
`timeseries` into it on `HierarchyLoaded`; `roles.rs` writes the echo into
it). A 17th top-level param is a compile error (E0599 on `.chain()`), not a
style issue — route through the bundle.

Handlers take the minimal `&`/`&mut` param set (no Bevy system params), so
they unit-test without spinning up ECS — the smoke tests live in
`db_results/mod.rs`. Handlers are `pub(super)`: visible to the dispatcher
and its tests only. The dispatcher keeps the `_ => {}` catch-all (variants
like `ScopeResolved` are consumed solely via `pending_api.try_deliver`).

**§fresh-create namespace id (F22/M2).** CreateVerse does NOT trigger a
hierarchy reload — the only `LoadHierarchy` triggers are Seeded / VerseJoined
/ DatabaseReset / an elevated write that changed rows / an external API poll. So `handle_verse_created` is the only
place a freshly created verse's `namespace_id` enters the tree, and it now
takes it from the `DbResult::VerseCreated` payload (the DB handler computes
it) and appends/updates the `VerseEntry` with it. Without this the entry had
`namespace_id: None`, `NavigationManager::open_replica` silently skipped the
verse on navigation, and the sync thread's retained verse-manifest write was
dropped at shutdown — permanent manifest loss (the F21/F22 shape). Keep the
id flowing through the result: do not re-derive it UI-side and do not add a
`LoadHierarchy` round-trip on create. A verse already present in the tree
(e.g. an API poll raced the create) is updated in place, not duplicated.

## §node-index (`code_review_20260430_performance_hotpaths`)

Same-Petal current snapshots replace their existing node slot in place. A Save,
Undo or Redo must not append selected rows beneath their siblings or invalidate
visible-range selection order. Births and explicit parent moves append normally;
every replacement still retires the previous URL/property cache identity.

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

`reconcile_path_asset` is the Paths-tab **live-edit feeder** (it no longer
spawns — see §path-asset-materialization). It keys off the **Paths-tab**
selection — `PathEditorState.editing_track_id` — NOT the viewport/tree
selection (`NodeManager.selected`). The Tools panel stamps the Paths-tab
track, so the original gating on `NodeManager` + `InspectorFormState.node_properties`
silently dropped every stamp whenever the two selections diverged (the common
case: stamp from the Paths tab without also viewport-selecting the ribbon).
It reads the descriptor from `PathEditorState.edited_track_path_asset` and
the points from `PathEditorState.points` directly — both seeded by the
`PathSelectTrack` read-back and refreshed after a `PathAssetApply` write (see
below) — and upserts them into `PathAssetCache`. Still active-petal-gated
(`nav.active_petal_id`).

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

The arc-length sampler is a focused, `[f32;3]`-based port of
`fe_terrain::iot::PathTracker` (fe-ui must **not** depend on fe-terrain):
`cumulative_distances` / `position_at_progress` / `sample_progresses` /
`tangent_yaw` / `sample_transforms`. `FixedSpacing` places instances every
`spacing_value` **meters** (converted to world units via `world_scale` — see
§path-asset-materialization; guards non-positive spacing → endpoints only, no
div-by-zero); `FixedCount` distributes `count` instances evenly (count 0 →
none, 1 → start only, scale-invariant). `tangent_yaw` returns the
`Quat::from_rotation_y` angle (`atan2(dx, dz)`, aiming the model's -Z forward
down the path) applied only when `descriptor.tangent_align`.

## §path-asset-materialization (`gpx_stamp_persistence_20260716`)

The bug: stamped instances (`spawn::spawn_stamped_entity`, tagged
`PathAssetInstance`) are ephemeral scene entities. The old reconcile spawned
them only while the Paths tab had the track selected and gated them behind a
**single-slot** `PathAssetApplied` that was never invalidated when
`respawn_on_petal_change` despawned them — so stamps vanished on petal change
and never came back, and one track's applied state stomped another's.

The fix mirrors §primitives: a petal-wide **`PathAssetCache`**
(`path_asset_materialize.rs`, keyed `track_id → {petal_id, descriptor, points,
fingerprint}`) fed independently of selection, plus ONE materializer.

- **Feed (petal-load persistence):** every `NodePropertiesLoaded` broadcast
  carrying BOTH a `path_asset` descriptor AND non-empty `gpx_points` upserts
  the cache in `db_results/properties.rs` (`cache.note_properties`), tagged
  with the node's owning petal (`VerseManager::petal_id_of`). The gpx bridge's
  `request_petal_gpx_materialization` already fetches `GetNodeProperties` for
  every node on petal (re)entry, so this fires without any fe-ui-side fetch. A
  `path_asset` `NodePropertySet` re-fetches (regardless of selection); a
  `NodePropertyDeleted` or an empty/absent path evicts; `DatabaseReset` clears.
- **Feed (live edit):** `reconcile_path_asset` upserts the Paths-tab edited
  track's live descriptor + `PathEditorState.points` (the freshest buffer while
  dragging). A **conditional** upsert (only when the content actually differs)
  keeps the cache — and thus the materializer — idle when nothing changed.
- **Materialize (all spawning):** `materialize_path_assets` is the ONLY system
  that spawns stamps. It runs when the cache changed, the petal changed, or the
  petal `world_scale` changed; for each active-petal cached track whose
  `(descriptor, points, world_scale)` differs from the **per-track**
  `PathAssetApplied` gate, it despawns the old group and restamps. Orphan groups
  (track no longer cached) are torn down. Chained LAST (after
  `respawn_on_petal_change` + `reconcile_path_asset`) so it observes their
  despawns, the gate clear, and both feeds — and because only it spawns, double
  stamping is structurally impossible.
- **Delete cascade (FR-4, `terrain_editor_overhaul_20260718`):** the bridge
  first calls public `invalidate_path_stamp_projection` after a successful
  `DeleteNode` enqueue, invalidating cache/applied/pick-index state so orphan
  entities disappear on the next materializer pass. A channel failure leaves
  every projection intact. Promoted stamp nodes remain durable rows; projection
  teardown never deletes `owning_path_id` records. The eventual `NodeDeleted`
  (`db_results/nodes.rs::handle_node_deleted`) calls `PathAssetCache::invalidate`
  + `PathAssetApplied::invalidate` on the deleted track. Invalidating the cache
  both wakes the materializer (it's `is_changed`) and makes the track's stamps
  match `stamp_is_orphaned` (no cache entry for their `source_track_id`), so the
  orphan pass despawns every stamped glTF and it can't resurrect on petal
  re-entry. Idempotent / no-op for a track that never stamped (NFR-5): invalidate
  on an absent key is a no-op and there is no `PathAssetInstance` to tear down.
  Realizes "a path's lifecycle owns its stamped assets" with no new data model —
  the `PathAssetInstance { source_track_id }` back-reference already existed.

Why petal-wide + property-driven: the hierarchy payload carries no properties
(same constraint as §primitives), and stamps must survive petal switches even
when the Paths tab isn't open — so materialization keys on the cache + active
petal, never on selection.

Why a per-track gate: the single slot let one track's applied fingerprint hide
another's need to restamp. The gate is keyed `track_id → (descriptor,
points-fingerprint, world_scale-bits)` and **cleared wholesale** by
`respawn_on_petal_change` on petal change (the old petal's stamps are despawned
there), so re-entering a petal always restamps every active-petal cached track.

Metric spacing (FR-3): `PathAssetDescriptor.spacing_value` is real METERS.
`sample_transforms` converts `spacing_world = spacing_m * world_scale` where
`world_scale` (world units per meter) comes from `PetalMapState.world_scale`;
`sanitize_world_scale` collapses a non-finite/≤0 scale to `1.0` so a bad petal
config never zeroes or flips spacing. `world_scale` is folded into the applied
gate so a scale change restamps. Pre-existing saved descriptors re-interpret in
meters (identical at human scale `1.0`). `FixedCount` is scale-invariant.

### T2 stamped-asset integration (`stamped_asset_nodes_20260725`, Wave-1 pass)

**Curve-follow sampling:** both cache feeders now FLATTEN the bezier anchors
before caching — `PathAssetCacheEntry.points` is the dense polyline from
`node_manager::curve::flatten_anchor_path` (a byte-exact mirror of fe-terrain
`flatten_route`, 16 samples per handle-carrying segment; fe-ui must not depend
on fe-terrain), so stamps sit ON the visible curve. `note_properties` keeps the
decoded rows' handles; `reconcile_path_asset` flattens the live
`PathEditorState.points` buffer. The fingerprint is computed over the FLATTENED
polyline, so a handle-only edit re-fingerprints and restamps. All-corner tracks
flatten passthrough → legacy fingerprints/spacing unchanged.

**Per-stamp overrides (FR-3):** `materialize_path_assets` reads
`StampInteractionState` (third selection authority, N-3 — never
`NodeManager.selected`). Sparse overrides win over descriptor defaults:
`arc_offset_m` re-samples position+yaw at the absolute arc length (meters,
clamped — `path_asset_reconcile::transform_at_arc_length`); `rotation`
(absolute quat xyzw) replaces tangent yaw; `scale` sets the transform scale
(`stamp_transform`, pure). The applied gate gains a 4th component — a
per-track `overrides_fingerprint` — and the run gate adds
`stamp_state.is_changed()`, so an override gesture restamps ONLY its track.

**Reflow (`lifecycle_events.rs`):** `LifecycleEvent::PathReflow` (pumped from
the DB thread by fe-runtime `drain_lifecycle_events`) shifts surviving stamp
identities down one index (`StampInteractionState::reflow_after_delete`),
invalidates the track in `PathAssetCache`/`PathAssetApplied` (the
`handle_node_deleted` idiom) and re-fetches `GetNodeProperties` so the reload
re-feeds the cache with the reflowed truth. Runs FIRST in the plugin chain so
the invalidation lands before the same-frame materializer pass. Other
lifecycle variants are ignored here — `NodePromoted` rides `DbResult`.

**Pick index + instanced-draw seam (FR-4):** each restamp (inside the applied
gate — never per frame) rebuilds the track's entry in the `StampRenderIndex`
resource: `tracks: HashMap<track_node_id, StampTrackRenderData { petal_id,
index: StampSpatialIndex, batches: Vec<InstanceBatch> }>` (types from
`fe_renderer::instancing`). T4's right-click classification iterates
active-petal entries and calls `StampTrackRenderData::pick_stamp(x, z,
radius)`. That maps the pick-index hit through `stamp_indices`, the stamp index
of each spawned position. The mapping is needed because, since S8, a saturated
stamp budget truncates farthest-first, so the spawned stamps are no longer a
prefix. The result is `(track_node_id, stamp_index)` for `HitTarget::Stamp`. Entries drop on orphan teardown and clear on petal change.
The instanced DRAW currently rides Bevy auto-instancing over the shared GLB
asset handles from `spawn_stamped_entity` (same `Mesh3d`+material = one batch);
`InstanceBatch` (with overrides folded into its matrices) is the seam for a
future custom pipeline.

## Property API delivery and UI hydration

The dispatcher forwards correlated property replies before selected-node guards, then still runs the existing cache/Inspector handlers. Selection filters affect Inspector buffers, never the API acknowledgment. Unmatched and duplicate correlated replies have no fallback; the final dispatcher delivery skips replies already forwarded. The existing unselected-properties journey runs the actual Bevy dispatcher, proving both API delivery and untouched UI state.

## Entity settings details adoption

EntityDetailsLoaded routes to dialogs/entity_details::apply with NoticeClock.
The active dialog's target, request nonce, canonical stored scope and ancestry
must all match before any form buffer is hydrated. Matching rename, default-
access and description acknowledgements request a fresh stored-details read;
late acknowledgements for another open target do not change that form.

## Plugin removals

The GUI-only PluginNodeRemoved result shares the existing committed node-removal
manager/cache adapter. It deliberately has no API reply family, so background
plugin soft deletes cannot satisfy a pending legacy DeleteNode request.


## Undo recovery projections

PathAssetCache.note_properties is crate-visible so the checked editor's authorized
current-snapshot recovery can reuse the normal property projection after a lost
or refused save. It preserves the descriptor/curve fingerprint and applied gates;
Undo introduces no independent stamp sampler or cache format.

PetalRequirementsLoaded and exact RequestFailed replies route into the open
requirements dialog state. They never mutate Node/Verse scene caches, residency
or selection. The target scope/request is checked by the dialog controller.

## Inspector token result ownership

The Inspector's mint uses an optional correlation ID and its form-owned
request state. Result handlers receive the current NodeManager selection,
NavigationManager scope and LocalUserRole before accepting a secret. Unknown,
late, timed-out or mismatched replies owned by this UI namespace are consumed,
never passed to another form or API waiter. Handle Inspector-owned failures
before generic feedback; render safe static text without backend payload echo.
The shared mint_command helper takes correlation_id last: Inspector supplies
Some, legacy Entity Settings supplies None. The runtime mint reply family
preserves exact-ID routing and the DB door preserves Manager authorization.

Mint acceptance also requires the actual Inspector API tab. Target synchronization
discards pending ownership outside that tab before handling either success or
failure, even if the selected node and scope have not changed.

## Stamp editor projection

`stamp_editor_defaults` reuses sample_progresses and tangent_yaw to initialize editable fields
without moving an unmodified stamp. Metric slide overrides convert through sanitized world_scale
inside stamp_transform before sampling the world-coordinate path. The same source path/Petal
configuration contributes to the database's atomic save fence and meter bounds.

## URL receipts and cache identity

NodeUrlUpdated is consumed before generic API delivery. Its current form owner may show URL
saved, while separate latest per-node write ownership schedules a fresh Viewer-fenced ReadNodeUrl
even after selection changes or a notice timeout. No write receipt directly changes webpage_url.
NodeUrlRead has exact nonce/target/scope/asset guards and updates the global current cache
independently of the selected form. A clean unchanged form follows the read; a newer draft stays.

Each manager-owned add/replacement or hierarchy-index rebuild assigns a fresh per-node cache
incarnation. Removal discards it. Counter exhaustion leaves that node without an admissible URL
cache identity, rather than reusing a stamp. A stale read cannot survive an identical-looking
same-ID/path rebuild. Accepted URL reads retain the actual model asset identity; the committed
node adapter preserves a URL only for that matching asset. An unknown/replaced model gets a
fresh typed read, carrying the previous clean form baseline while its URL is pending. Paths and
asset presence are invalidation hints, never proof that two asset IDs identify the same model.

Existing node URL journey covers save A/select B/ack/reselect A, clean and dirty form reads,
timeout recovery, newer-write/older-read ordering, foreign nonce/asset, model replacement and
same-looking hierarchy rebuild. These source extensions remain unexecuted until the integrated
sweep. No LoadHierarchy or API reply fallback is used to refresh Portal URLs.

## Portal storage availability

The exact-node URL read also confirms NodeUrlStorage alongside the actual asset identity.
Its map is bounded by cached nodes and shares the existing rebuild/add/remove invalidation.
Committed snapshots preserve it only for the same actual model identity. Terminal reset through
VerseManager::default clears this state. No standalone Resource or unbounded request registry
is added. Ordinary missing/shared/assetless metadata is inline state, not an advisory per selection.


## Stamp identity sampler

The renderer uses fe_sdk::path_asset::sample_progresses, shared with checked GUI promotion. This
keeps rendered index existence identical to the DB validator across metric spacing and degenerate
paths. Materialization budgets may omit a valid sampler index; GUI selection also requires its
actual live owned scene entity.


## Live stamp source proof

Actual materialization shares one immutable descriptor/flattened-points/world-scale proof across
its spawned stamp group. Selection compares this proof with the authoritative property cache and
loaded editor context before admitting an entity. Cache updates preserve the last authoritative
properties separately from live render edits; source revision changes gate catalog rebuilds. An
old entity cannot be captured under a reflowed descriptor before the renderer replaces it.

## Imported and placed object materialization

GltfImported's qualified complete birth enters the existing committed snapshot
adapter. Its routed node/asset/Petal/name/path/position must agree with the typed
snapshot, which carries initial rotation, nonuniform scale, properties and v1.
The manager and ordinary residency/spawn chain own materialization; there is no
second identity-only import spawner. Repeated receipts stay one tree/entity and
a late v1 cannot replace a newer committed version. A legacy/mismatched receipt
requests authoritative hierarchy hydration rather than inventing a pose.

The existing full App result/correlation journey now delivers actual typed asset
birth messages through apply_db_results and the residency systems. It checks
nonidentity initial TRS on the resident SceneRoot, duplicate and delayed receipts,
API correlation and unqualified/mismatched fallback. This is source-authored
component/adapter coverage, not GPU/model-load or executed QA evidence.

Only a qualified GUI-origin completion can finish the native import dialog; an
API-correlated placement retains its own waiter and cannot consume that request.

Native import completion additionally requires its exact nonce, active Import
dialog and captured current navigation. Stale successful births still hydrate
above this completion gate; stale failure never changes a replacement request.
The existing receipt App journey includes an unknown-outcome replacement, stale
committed birth/failure, and late matching completion without another create.

Committed-receipt hierarchy fallback retains one coalesced Local read request.
The result system tries it once at the end of each frame, including frames with
no replies. Full retains it for a later frame; accepted or disconnected clears it.
No mutation is retried. The existing App receipt journey fills the bounded queue,
processes multiple unsupported births, then checks one refresh after space opens.

Receipt-triggered hierarchy hydration suppresses NavigationManager's startup
auto-selection gate before queueing the read. It does not change the cursor or
advance async user navigation intent. The existing full App journey covers normal
startup, exit between read and late hierarchy reply, stale receipts after exit,
repeated reloads, and hydration arriving before any explicit navigation.

## Stable promoted-stamp projection

An addressed stamp remains a member of its owning path's rendered group. Its
durable NodeEntry is metadata, not a second ordinary scene entity at the stored
origin. NodeSpawner and the primitive materializer recognize only canonical stamp
identity plus the complete loaded stamp bag; an ID suffix alone hides nothing.
Cold assetless nodes first request one bounded property classification. Full queues
retry on later residency frames, without blocking the UI or inventing a fallback.
The shared primitive cache records successful classification even for ordinary
nonprimitive bags. Identity-property writes invalidate that classification.
Any already-resident ordinary projection is removed after typed stamp hydration.

Empty sparse overrides do not affect a path's render fingerprint. Override-only
changes update the same per-index entities and rebuild their spatial/draw data
only when descriptor, complete points, world scale, admission, live source proof
and rendered index set still match. Source reflow, residency change, missing or
duplicate entities rebuild the group; no selection is transferred across them.
The live editor feeder and materializer independently require the hierarchy's
actual track owner. An old editor surviving until Selection cannot relabel its
cache or spawn stamps in the newly active Petal.

The existing residency App journey exercises production materialization through
promotion metadata, scale/rotation/slide projection, ordinary-spawn exclusion,
Petal leave/re-entry with a late old editor, and a real descriptor reflow. Existing
sidebar and native selector journeys cover their separate producer boundaries.
These are authored source checks; root owns execution and runtime acceptance.

## Scene-property reads

The existing descriptor cache owns one bounded read coordinator shared by cold
scene classification and property-write refreshes. Handlers enqueue intent; only
the per-frame pump sends GetNodeProperties with try_send. Full retains one queued
read per node. Accepted reads bind a fresh correlation ID to the actual Petal and
hierarchy node epoch, with a five-second deadline. Superseded, timed-out and
foreign replies cannot classify a node or feed the property projections.

Matching failure, missing reply and disconnected channel visibly fail that read
attempt, retain the need for fresh properties, and retry after capped exponential
backoff from two to 32 seconds. A failure never becomes an ordinary fallback. The
coordinator holds at most 256 reads; capacity refusal is visible, and cold residency
retries admission as slots open. Successful replies release their slot. Pending
read bookkeeping bypasses render change detection, so an accepted or backoff read
does not force a complete residency scan every frame. Property writes supersede
older reads and use this same coordinator rather than a blocking channel send.

The existing full DB-result/residency App journey now begins an ordinary node
cold and covers correlated failure, missing reply and deadline, stale success,
Full retention, disconnected recovery, then successful fallback classification.
The existing primitive/property journeys check the retained read boundary. These
checks are authored only; root owns execution.

The addressed invitation phase replies are consumed by their separate correlated
workflow reader. The ordinary tree/result projection ignores them before generic
API fallback, so a prepared source or pending membership cannot navigate, create
a scene, satisfy an unrelated API waiter or masquerade as Joined.


Addressed Join phase results belong to the invitation coordinator. Legacy uncorrelated join
results cannot finish its request. Hierarchy rebuilding suppresses first-load auto-navigation
while that coordinator owns the cursor, so intermediate admitted rows do not imply Ready.

## Selection replay admission and draft preservation

The editor-batch fresh-read branch rechecks original live target identity, transforms,
path drafts and rendering immediately before submitting an inverse. DescriptorCaches
reuses its existing mutable cache/state borrows and a read-only RenderedSources query;
there is no duplicate resource borrow or current-selection requirement. Complete valid
receipts still materialize authoritative snapshots. If a local path draft changed after
submission, batch projection protects its rows from pending property hydration, and the
host repeats the before-baseline comparison before touching editor rows. Retaining a
draft does not negate a committed save or drop exact pending-write ownership.

## Current replica identity and editor baselines

A repeated current read preserves an active preview only when its full stored row,
original scene HLC/publisher, scope and resolved asset state have the same canonical
fingerprint as the previously admitted replica source. Equal visible geometry is
insufficient. Local receipts have their own positive-history binding; a new peer
admission always retires that authority, even for a no-op peer edit. Legacy apply
provenance cannot prove repeated admission and therefore refreshes conservatively.

Each binding retains the prior admitted gpx point baseline. A complete peer update
compares the open editor with that baseline before replacing it: clean rows advance,
while rows changed locally are retained. This occurs during FIFO admission and again
when the host consumes its captured projection, so several queued peer updates do
not turn a clean older editor into a fictional draft. The same rule handles a row
that stops being a path. Initial unknown baselines remain conservative.

Private current-read replies check their deadline at routing, before acceptance,
independently of the later pump. Expired matching nonces retain only read need and
back off before a fresh nonce; they never apply stale state or retry a mutation.

Private host-path creation outcomes may update the known tree, but the generic
NodeCreated handler does not continue another UI workflow for those IDs. Pen
first-anchor intent is unsent work: role invalidation or denied edit access clears
that intent, and a matching late birth cannot restart editing outside its original
active Petal. Accepted host command outcomes remain with the host's separate owner.


## Scene asset descriptor and failed-loader recovery

SceneSources retains the exact admitted asset availability/provenance beside
its current-source token. Local journal receipts do not fabricate this remote
asset descriptor. SceneAssetNeeded is routed after the corresponding complete
current DTO in DB channel order and must match this descriptor before any fetch.

A fresh authorized LocalBlob may recover a prior Bevy failed root or labelled
Scene handle. The adapter invokes AssetServer::reload on the actual failed URI
and rematerializes the resident through normal residency. Retry records are
bounded by known nodes, coalesced by URI and survive same-node cache updates;
deletion, hierarchy/navigation/access retirement and terminal reset clear them.
Repeated recovery reads cannot create a reload storm for a permanently invalid
model. File availability is not a claim that GPU decoding/rendering succeeded.

## Earthwork hydration identity

NodeCreated binds its actual Petal plus region to the echoed node, and writes
identity/material metadata only. Volume properties start absent. Ordinary property
hydration uses the known owning Petal and removes old aliases before rebinding;
it never seeds the background save-confirmation cache from an unrelated read.
Before mutation, exact owned direct and resolved pair receipts recheck the original
nonretired demand and node source/epoch against current terrain/height/access.
They pass the ordinary whole committed preflight, then take
a property-only projection: unchanged stored TRS never overwrites a live gesture,
selects a node, cancels a drag or queues user Undo. Invalid owned packets are
withheld for outcome reconciliation, not generic full-row projection. Navigation
or access loss withholds this current projection while the owner still drains
the actual accepted outcome. The independent Resource drains exact owned reads,
commits, Resolve replies and failures in Update. No generic properties/replica
refresh follows this background save. Other receipt producers keep their existing
projection path.
