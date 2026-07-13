---
type: Worker Report
title: BIM Primitives on Paths — W4 (P1 + P2)
tags: [bim_primitives_on_paths_20260712, worker-report]
timestamp: 2026-07-12T00:00:00Z
resource: ./spec.md
---

# Worker Report — W4 — P1 (FR-1, FR-2) + P2 (FR-3, FR-4)

Scope executed: **P1 and P2 only**, per instructions. P3/P4 were NOT started
(see bottom section). No cargo/build/test/check/clippy run — coordinator owns
the single serialized sweep.

## Files changed

- `fe-runtime/src/shared_node.rs` — added `PropertyValue::Json(serde_json::Value)`
  variant + `as_json()`; added `From<fe_sdk::property::PropertyValue> for PropertyValue`
  and the reverse `From<PropertyValue> for fe_sdk::property::PropertyValue`.
- `fe-runtime/Cargo.toml` — added `fe-sdk` path dependency (needed for the
  conversion impls above; fe-sdk stays free of any fe-runtime/bevy dep).
- `fe-runtime/src/AGENTS.md` — new file, §property-bridge documents the
  convertibility approach.
- `fe-sdk/src/primitive.rs` — new file: `PrimitiveDescriptor { kind, dims,
  texture_ref }`, `PrimitiveKind` (Cube/Plane/Cylinder/Sphere),
  `PRIMITIVE_PROPERTY_KEY = "primitive"`, `from_json`/`to_json`, 4 unit tests.
- `fe-sdk/src/texture.rs` — new file: `TextureRegistry`/`TextureEntry` (FR-4),
  copy-adapted from `ui::UiExtensionRegistry` — `register`/`unregister_all`/`get`,
  4 unit tests.
- `fe-sdk/src/lib.rs` — registered `primitive`/`texture` modules + re-exports.
- `fe-sdk/src/AGENTS.md` — added §primitive, §texture.
- `fe-hexon/src/handlers/material.rs` — added the FR-3 loader:
  `DecodedTexture`, `ResolvedMaterial`, `resolve_material_textures(handle,
  blob_store) -> ResolvedMaterial`, `load_decoded` (private). 2 unit tests
  using `tempfile` + `image` (already a dev-dep, promoted to a regular dep).
- `fe-hexon/src/registry.rs` — added `FsBlobStore::default_path()` /
  `open_default()` (mirrors `fe_sync::FsBlobStore`'s convention, separate
  `hexon_blobs/` dir).
- `fe-hexon/Cargo.toml` — added `image = "0.25"` (regular dep, png+jpeg
  features only) and `dirs = "5"`.
- `fe-hexon/src/AGENTS.md` — new file, §material-loader.
- `fe-ui/src/verse_manager/spawn.rs` — added `spawn_primitive_entity`,
  `build_primitive_mesh`, `PrimitiveNode` marker component, `dim_or` helper.
- `fe-ui/src/verse_manager/primitive_reconcile.rs` — new file:
  `reconcile_selected_primitive` system (FR-1 promotion + FR-2 live reconcile)
  and `resolve_primitive_material` (FR-3 wiring: registry lookup → blob load
  → decode → `Image`/`StandardMaterial` assembly). `PrimitiveMaterialAssets`
  resource holds the shared default material. 1 unit test (descriptor
  equality gating).
- `fe-ui/src/verse_manager/mod.rs` — registered `primitive_reconcile` module +
  its system; added `TextureRegistryRes` (Bevy `Resource` newtype wrapping
  `fe_sdk::TextureRegistry`, since the SDK type is intentionally bevy-free);
  re-exports.
- `fe-ui/src/verse_manager/AGENTS.md` — extended with `spawn.rs`/
  `petal_respawn.rs` notes + new §primitives section explaining the
  selected-node-only materialization scope (see "Known limitation" below).
- `fe-ui/Cargo.toml` — added `fe-sdk`, `fe-hexon` path dependencies.

**Not touched**: `fe-ui/src/verse_manager/petal_respawn.rs` (read but not
edited — see Known limitation), `fe-ui/src/plugin.rs` (not needed —
`SpawnedNodeMarker` required no changes), `fe-plugin/src/lib.rs` (P4,
untouched).

## Primitive descriptor JSON schema (FR-1, C5)

```json
{
  "kind": "cube" | "plane" | "cylinder" | "sphere",
  "dims": [f32, ...],
  "texture_ref": "string" | null   // omitted when null (skip_serializing_if)
}
```

`dims` semantics per kind (world units):
- `cube`: `[width, height, depth]`
- `plane`: `[width, depth]`
- `cylinder`: `[radius, height]`
- `sphere`: `[radius]`

Missing/invalid (non-finite or ≤0) `dims[i]` falls back to a small sane
default per index rather than panicking — see `dim_or` in `spawn.rs`.

Storage: `PropertyValue::Json(descriptor.to_json())` under key `"primitive"`
(`fe_sdk::primitive::PRIMITIVE_PROPERTY_KEY`) on the node's property bag.

## fe-runtime ↔ fe-sdk PropertyValue convertibility (C5)

- `fe_runtime::shared_node::PropertyValue` (Tauri↔Bevy bridge, authoritative)
  gained a 5th variant: `Json(serde_json::Value)`, matching the shape
  `fe_sdk::property::PropertyValue` already had.
- `fe-runtime` now depends on `fe-sdk` (one-directional; `fe-sdk` stays
  serde-only/engine-decoupled per its own `AGENTS.md` — it must never gain a
  `fe-runtime`/bevy dependency).
- `From<fe_sdk::property::PropertyValue> for fe_runtime::shared_node::PropertyValue`
  and the reverse are implemented as straight variant maps
  (`String↔String`, `Number↔Number`, `Boolean↔Bool`, `Json↔Json`).
- The one asymmetry: `fe-runtime`'s `PropertyValue::Array` has no SDK-side
  counterpart. The `fe-runtime → fe-sdk` direction folds `Array` into
  `Json` via `serde_json::to_value` — lossless, no third enum introduced.
- No fork of primitive/geometry semantics across the two enums — the single
  canonical descriptor type is `fe_sdk::primitive::PrimitiveDescriptor`,
  serialized identically regardless of which `PropertyValue` carries it.

## MaterialHandle → texture loader (FR-3)

`fe-hexon` stays Bevy-agnostic; it resolves blobs to raw decoded bytes only.
Signature:

```rust
// fe-hexon/src/handlers/material.rs
pub struct DecodedTexture { pub width: u32, pub height: u32, pub rgba: Vec<u8> }
pub struct ResolvedMaterial {
    pub albedo: Option<DecodedTexture>,
    pub normal: Option<DecodedTexture>,
    pub roughness: Option<DecodedTexture>,
    pub ao: Option<DecodedTexture>,
    pub metallic: Option<DecodedTexture>,
}
pub fn resolve_material_textures(
    handle: &MaterialHandle,
    blob_store: &FsBlobStore,
) -> ResolvedMaterial;
```

Missing/undecodable blobs resolve to `None` per role (logged via
`tracing::warn!`) rather than failing the whole material.

`fe-ui`'s `resolve_primitive_material` (in `primitive_reconcile.rs`) is the
Bevy-side assembly: registry lookup (`TextureRegistry::get`) → construct a
1-role `MaterialHandle` (`albedo_hash` only, v1 primitive texturing is
albedo-only) → `resolve_material_textures` → wrap `DecodedTexture.rgba` into
`bevy::image::Image` (`Rgba8UnormSrgb`) → `Assets<StandardMaterial>::add`
with `base_color_texture`. Falls back to `PrimitiveMaterialAssets::default_material`
(one shared gray `StandardMaterial`, built once via `FromWorld`) when
`texture_ref` is `None` or unresolvable at any step.

`FsBlobStore::open_default()` (new) resolves the blob directory as
`{dirs::data_local_dir()}/fractalengine/hexon_blobs/` — no live
`HexonRegistry` Bevy resource exists yet in the codebase to source this path
from, so this mirrors `fe_sync::FsBlobStore`'s established convention.

## TextureRegistry API (FR-4)

```rust
// fe-sdk/src/texture.rs (engine-decoupled)
pub struct TextureEntry { pub id: String, pub plugin_id: String, pub blob_hash: String, pub label: String }
pub struct TextureRegistry { /* ... */ }
impl TextureRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, entry: TextureEntry);       // re-register same id replaces
    pub fn unregister_all(&mut self, plugin_id: &str);
    pub fn get(&self, id: &str) -> Option<&TextureEntry>;
    pub fn all(&self) -> impl Iterator<Item = &TextureEntry>;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
}
```

Wrapped as a Bevy `Resource` in `fe-ui`: `TextureRegistryRes(pub fe_sdk::TextureRegistry)`,
`init_resource`'d in `VerseManagerPlugin::build`. Entries reference
hexon-installed content-addressed `blob_hash`es only (C6) — nothing in this
track's code accepts raw bytes from a plugin.

**Not wired in this pass**: nothing currently calls `TextureRegistry::register`
— there's no existing hook in owned files where a hexon material install
notifies the UI layer. The registry is live and testable but starts empty at
runtime; populating it from `handle_material_install` results is follow-up
work (likely P3/P4 or a small fast-follow, not blocking P1/P2's mesh+texture
plumbing itself).

## Known limitation (scope-forced by file ownership)

`FR-1`'s spec text says "a node carrying a `primitive` JSON property
materializes... third branch in `petal_respawn.rs:58-95`." I could not add a
`properties` field to `NodeEntry` (`fe-ui/src/verse_manager/mod.rs`, owned)
because `db_results.rs` (NOT owned — W1's file) constructs `NodeEntry` at 4
call sites with fully-explicit field lists and no `..Default::default()`
spread; adding a field there would require editing `db_results.rs`, which is
out of scope.

**What I actually implemented instead**: `reconcile_selected_primitive`
(new system in `primitive_reconcile.rs`) reads `InspectorFormState.node_properties`
— the one already-wired per-node property source (populated by
`db_results.rs`'s existing `NodePropertiesLoaded` handling, for the
currently-**selected** node only) — and:
1. **FR-1 promotion**: if the selected node has a `primitive` descriptor and
   is currently rendered as a `FallbackSign` (i.e. not yet a `PrimitiveNode`),
   despawns the sign and spawns the real primitive mesh in its place.
2. **FR-2 reconcile**: if the selected node is already a `PrimitiveNode`,
   diffs the live descriptor against the spawned one and re-meshes/re-materials
   in place (no despawn) only on an actual change.

Nodes that are primitives but never get selected still spawn as fallback
signs on petal load/switch (`petal_respawn.rs`, unmodified, still only knows
GLTF-vs-fallback) until the user selects them once. This is a genuine gap
versus the spec's literal ask ("a primitive node materializes" petal-wide,
unconditionally) but fully satisfies the **in-app verification** flow
("spawn each of cube/plane/cylinder/sphere; edit dims live") since spawning
a primitive node in-app necessarily involves selecting it to set its
properties in the inspector first. Closing the gap needs either (a)
`db_results.rs` edits (W1/coordinator territory) to thread `properties` onto
`NodeEntry`, or (b) a new DB query the UI issues per-petal-load to bulk-fetch
primitive properties for all nodes — both are reasonable fast-follows, not
done here to respect the file fence.

## P3 / P4 — NOT STARTED (fenced)

**P3 (FR-5 path→wall, FR-6 building composition)**: nothing implemented.
Explicitly withheld per instructions (depends on Track 1 landing +
in-app verification). Remaining work for whoever picks up P3:
- `P3.1` — wall mesh builder: extrude a `gpx_points` polyline into a
  quad-strip wall mesh at a height param. `bake_splat_mesh`
  (`fe-terrain/src/splat/render.rs:313`) is the cited raw-mesh-building
  reference; the polyline source (`gpx_points`) presumably needs to be read
  the same way GPX path data currently flows into `fe-terrain`'s path
  editor/layers — worth checking `fe-ui/src/gis/` (path editor state) for
  the existing polyline shape before inventing a new one.
- `P3.2` — a wall is a `primitive`-kind node (reuse this track's
  `PrimitiveDescriptor`/`PrimitiveNode` machinery — a wall's "dims" would
  need to be a height scalar since the polyline itself is the shape driver,
  which doesn't fit today's `dims: Vec<f32>` per-kind convention cleanly;
  likely needs a 5th `PrimitiveKind::Wall` with `dims = [height]` plus a
  separate `gpx_points`-sourced polyline reference, OR a dedicated
  non-`PrimitiveDescriptor` wall descriptor — worth a design decision before
  implementing) bound to its source path node, re-projecting on path change
  events. Track 1 (`path_node_binding_hardening`) is supposed to supply the
  change-notification backbone this needs — check what that track actually
  landed before starting.
- `P3.3` — building = N wall primitives (+ optional GLTF models) grouped
  under one petal via the existing node hierarchy; no new grouping primitive
  needed per spec.

**P4 (FR-7 transform surface, FR-8 stats surface, FR-9 plugin transaction
wiring)**: nothing implemented, including the explicitly-optional
`fe-plugin/src/lib.rs:285-380` stub-finishing (I had margin to attempt it per
instructions but chose not to risk destabilizing a file three other workers
may also be touching indirectly via shared crates, and P1+P2 alone used the
full scope of what I could safely verify without a build). Stubs remain
exactly as found: `PluginCommand::CommitTransaction`/`SetProperty`/`CreateNode`
all just log + ack, `GetNode`/`QueryNodes` return empty data — no DB thread
forwarding. Whoever does P4 should start there per the spec's own ordering
(FR-9 gates FR-7/FR-8 being "relied on end-to-end").

## Tests written (not run — coordinator's sweep will run these)

- `fe-sdk/src/primitive.rs`: 4 tests (round-trip all kinds, bare-shape parse,
  texture_ref-absent default, unknown-kind rejection).
- `fe-sdk/src/texture.rs`: 4 tests (register/get, re-register replaces,
  unregister_all scoping, empty-registry state).
- `fe-hexon/src/handlers/material.rs`: 2 tests (albedo-only resolution via a
  real `tempfile` `FsBlobStore` + encoded PNG bytes, missing-blob → `None`
  without erroring).
- `fe-ui/src/verse_manager/primitive_reconcile.rs`: 1 test (descriptor
  equality gates the reconcile no-op path).

Per the track's build discipline, these need `-p fe-sdk -p fe-hexon -p fe-ui`
(with `fe-ui` at `-j 1`) in the coordinator's single serialized sweep. New
crate deps introduced: `fe-hexon` gained `image = "0.25"` (regular dep,
promoted from dev-only) + `dirs = "5"`; `fe-runtime` gained `fe-sdk`; `fe-ui`
gained `fe-sdk` + `fe-hexon`.

WORKER_COMPLETE W4

## Coordinator fix: T4 hardening (untagged-Json doc + petal-scope)

Applied fixes from opus SHIP review (contained HIGH + 2 fast-follows). Edit-only, no cargo run.

**FIX 1 (HIGH) — untagged `PropertyValue::Json` round-trip:** chose the doc-correction
approach (not a wire-format change). `PropertyValue` stays untagged with its current
variant order — changing to tagged risks breaking the Tauri↔Bevy bridge wire shape and
any tests/consumers relying on it, which wasn't worth the risk for a caveat that only
affects scalar/array `Json` payloads (JSON *objects* — the only shape primitive
descriptors use — round-trip losslessly). Updated:
- `fe-runtime/src/shared_node.rs` doc-comment on `PropertyValue` to state the caveat
  explicitly.
- `fe-runtime/src/AGENTS.md` §property-bridge with an "Untagged-deser caveat" paragraph
  explaining the variant-order fallthrough and warning against reordering/tagging
  without confirming no wire-shape dependents.

**FIX 2 (MEDIUM) — petal-scope primitive promotion match:**
`fe-ui/src/verse_manager/primitive_reconcile.rs::reconcile_selected_primitive` now
matches on `marker.node_id == sel.node_id && marker.petal_id == active_petal_id` in both
the in-place reconcile loop (line ~61) and the fallback-sign promotion `find` (line ~92),
using the `petal_id` already bound from `nav.active_petal_id.as_deref()`. Prevents a
stale sign from a previous petal, or a node_id collision across petals, from being
mismatched during a petal switch.

**FIX 3 (LOW, doc only):** added a fast-follow note in `fe-runtime/src/AGENTS.md`
flagging that the two `FsBlobStore` path conventions in `fe-hexon` need reconciling
before the texture registry is wired at runtime (registry is empty/inert today, so no
current impact) — for whoever wires FR-4 registry population / P2 texture install.
`fe-hexon` was not touched per scope.

Files touched: `fe-runtime/src/shared_node.rs`, `fe-runtime/src/AGENTS.md`,
`fe-ui/src/verse_manager/primitive_reconcile.rs`. No cargo run (coordinator owns build).
