# Bloom Stage — 3D Scene Rendering & Object Interaction

**Track:** bloom_stage_20260322
**Project:** FractalEngine (Rust / Bevy 0.18)
**Date:** 2026-03-22
**Status:** Planning

---

## 1. Context

FractalEngine uses a BLAKE3-addressed asset pipeline. Assets are loaded via:

```rust
fn load_to_bevy(asset_id: &str, asset_server: &AssetServer) -> Handle<Gltf>
```

The entity hierarchy is:

```
Fractal → Node → Petal → Room → Model → BrowserInteraction
```

A `place_model` query and a model table with a FLEXIBLE `transform` column already exist. The `DeadReckoningPredictor` component is present for interpolated motion. The current camera is `Camera2d`.

The Bloom Stage track replaces the 2D camera with a 3D scene, adds GLTF scene spawning driven by model records, and provides admin tooling for selecting and editing model transforms via an egui panel that writes back to the database.

---

## 2. Scope

### 2.1 In Scope

| # | Feature |
|---|---------|
| 1 | 3D orbit camera (admin mode), replacing `Camera2d` |
| 2 | GLTF scene spawning system (model DB records → `SceneRoot` entities) |
| 3 | Raycasting-based click selection using Bevy picking |
| 4 | Selection highlight (outline or color tint on hover and select) |
| 5 | Transform editor (egui panel: position / rotation / scale sliders → DB write) |
| 6 | `ModelMetadata` component (`asset_id`, `petal_id`, `display_name`, `external_url`, `tags`) |
| 7 | Model inspector panel (selected model metadata + live transform readout) |
| 8 | Ground plane mesh at y = 0 for spatial reference |
| 9 | Directional light setup |

### 2.2 Out of Scope

- Runtime physics / collision
- Multi-user session sync (DeadReckoning integration is a separate track)
- Asset upload or transcoding
- Non-admin user camera modes (first-person, etc.)
- 3D viewport gizmo handles (drag arrows / rotation rings) — egui numeric editor only for v1
- Multi-select
- Model deletion or creation from the 3D viewport

---

## 3. Features

### 3.1 3D Orbit Camera

Replace `Camera2d` with `Camera3d`. The orbit camera must:

- Orbit around a focal point via mouse drag (left button).
- Zoom via scroll wheel.
- Pan via middle-mouse drag or right-mouse drag.
- Clamp pitch to prevent gimbal flip (−85° to +85°).
- Restore last camera state from a resource (`CameraState`) so hot-reloads do not reset position.
- Gate behind an `AdminMode` resource so non-admin builds are unaffected.

### 3.2 GLTF Scene Spawning

A `ModelSpawnSystem` reads model records from `place_model` (petal_id filter), calls `load_to_bevy`, and spawns a `SceneRoot` entity per record. Each entity receives:

- `ModelMetadata` component (see §3.6)
- `Transform` deserialized from the FLEXIBLE `transform` column (JSON format: `{ translation: [x,y,z], rotation: [x,y,z,w], scale: [x,y,z] }`)
- A `ModelState::Spawning` marker that transitions to `ModelState::Ready` once the GLTF asset resolves

On despawn (room change or record deletion), the system removes the corresponding entity. Duplicate spawning is prevented via a `SpawnedModels` resource that tracks in-scene `asset_id` values.

### 3.3 Raycasting Selection

Use Bevy 0.18's first-party picking via `bevy::picking`. Requirements:

- Single click selects one model; click on empty space deselects.
- `SelectedModel` resource holds the entity and its `asset_id`.
- Hover state is tracked separately in a `HoveredModel` resource.
- Selection events propagate through the entity hierarchy (clicking a child mesh selects the root `Model` entity).

**Bevy 0.18 note:** Add `DefaultPickingPlugins` to the app. Attach `PickingBehavior` to meshes. Listen for `Pointer<Click>`, `Pointer<Over>`, and `Pointer<Out>` observer events. Do not use the deprecated `RayCastMesh` / `RayCastSource` API from pre-0.15 crates.

```rust
commands.entity(entity).observe(on_click_select);
commands.entity(entity).observe(on_hover_highlight);

fn on_click_select(
    trigger: Trigger<Pointer<Click>>,
    mut selected: ResMut<SelectedModel>,
) { ... }
```

### 3.4 Selection Highlight

On hover: apply a `StandardMaterial` emissive tint (`Color::srgba(1.0, 0.9, 0.3, 0.4)`).

On select: apply a stronger tint (`Color::srgba(0.2, 0.7, 1.0, 0.6)`).

Implementation strategy:

- Store original material handles in an `OriginalMaterials(Vec<(Entity, Handle<StandardMaterial>)>)` component added at spawn time.
- Swap materials on hover/select events; restore on unhover/deselect.
- Only highlight direct child meshes of the selected `Model` entity, not nested scene children from other models.
- Highlights are removed when the model is deselected or no longer hovered. The base material is never permanently modified.

### 3.5 Transform Editor (egui)

An egui side panel labeled "Transform" opens when `SelectedModel` is `Some`. It contains:

- `DragValue` sliders for position X / Y / Z (world space, step 0.01)
- `DragValue` sliders for rotation X / Y / Z in degrees (Euler ZYX order, step 0.1°)
- `DragValue` sliders for scale X / Y / Z (step 0.01, min 0.001)
- A "Reset Transform" button restoring identity transform
- An "Apply" button that serializes the transform to JSON and writes to the DB via the `DbCommand` channel

The system debounces DB writes: only write if the transform has changed by more than an epsilon and at least 200 ms have elapsed since the last write. Transform changes in the 3D viewport are reflected immediately on drag.

### 3.6 ModelMetadata Component

```rust
#[derive(Component, Debug, Clone, Reflect)]
pub struct ModelMetadata {
    pub asset_id: String,          // BLAKE3 content address
    pub petal_id: i64,
    pub display_name: String,
    pub external_url: Option<String>,
    pub tags: Vec<String>,
}
```

Populated from the model DB record at spawn time. The component is immutable during a session (metadata editing is out of scope for this track).

### 3.7 Model Inspector Panel (egui)

A second egui panel labeled "Inspector" shows:

- `display_name` as a heading
- `asset_id` (truncated to 16 chars + "…") with a copy-to-clipboard button
- `petal_id`
- `external_url` as a clickable hyperlink if present
- `tags` as a chip list
- Live transform readout (position / rotation / scale, read-only mirror of the Transform editor values)

When nothing is selected, the panel shows "No model selected."

### 3.8 Ground Plane

Spawn a `Mesh3d` (plane mesh) at y = 0, size 50 × 50 units, with a `MeshMaterial3d<StandardMaterial>` using:

- `base_color`: `Color::srgb(0.3, 0.3, 0.3)`
- `perceptual_roughness`: `1.0`
- `PickingBehavior::IGNORE` so it does not interfere with model selection

The plane exists solely for spatial reference and shadow catching.

### 3.9 Directional Light

Spawn one `DirectionalLight` with:

- `illuminance`: `10_000.0` lux
- `shadows_enabled`: `true`
- Transform rotated to approximately 45° elevation, 30° azimuth

Add an `AmbientLight` resource at `200.0` lux to soften shadow harshness.

---

## 4. Acceptance Criteria

### AC-1 Camera

- [ ] Application starts with `Camera3d` active; `Camera2d` is absent.
- [ ] Left-drag orbits; scroll zooms; right-drag or middle-drag pans.
- [ ] Pitch is clamped; no gimbal flip at extreme angles.
- [ ] Camera state persists across hot-reloads via `CameraState` resource.

### AC-2 GLTF Spawning

- [ ] Given a model record in the DB, a `SceneRoot` entity with `ModelMetadata` is present in the world within one asset-load cycle.
- [ ] Removing the DB record despawns the entity on the next poll.
- [ ] `Transform` on the spawned entity matches the serialized value in the DB.
- [ ] Duplicate records do not produce duplicate entities.

### AC-3 Selection

- [ ] Clicking a model sets `SelectedModel`; clicking empty space clears it.
- [ ] Hover and select produce visually distinct material tints.
- [ ] Clicking a child mesh selects the root `Model` entity.
- [ ] Only one model can be selected at a time.

### AC-4 Transform Editor

- [ ] Dragging a position slider moves the entity in the world immediately.
- [ ] Releasing the drag (after 200 ms debounce) persists the new transform to the DB.
- [ ] "Reset Transform" restores identity transform and writes to DB.
- [ ] No DB writes fire while the user is mid-drag (debounce is respected).

### AC-5 Inspector

- [ ] Inspector panel shows the correct `display_name`, truncated `asset_id`, `petal_id`, and tags for the selected model.
- [ ] Copy button places `asset_id` on the system clipboard.
- [ ] `external_url` renders as a clickable hyperlink when present.
- [ ] Panel shows "No model selected" when `SelectedModel` is `None`.

### AC-6 Ground Plane and Lighting

- [ ] Ground plane is visible at y = 0 and not selectable via picking.
- [ ] Directional shadows are cast by models onto the ground plane.
- [ ] Ambient light prevents fully-black shadow areas.

---

## 5. Technical Notes

### 5.1 Bevy 0.18 Picking API

Bevy 0.18 ships first-party picking under `bevy::picking`. The minimal setup is:

```rust
app.add_plugins(DefaultPickingPlugins);
```

Meshes are pickable by default. To exclude a mesh (e.g. ground plane):

```rust
commands.spawn((mesh, material, PickingBehavior::IGNORE));
```

Use observer syntax to attach per-entity event handlers. Avoid `MeshPickingPlugin` from third-party crates unless the first-party plugin is insufficient.

### 5.2 Orbit Camera

`bevy_panorbit_camera` (version tracking Bevy 0.18) is the recommended approach. If vendored, implement with `Transform::looking_at` updated each frame from spherical coordinates `(radius, yaw, pitch)` stored in an `OrbitCameraState` resource:

```rust
let x = state.radius * state.pitch.cos() * state.yaw.sin();
let y = state.radius * state.pitch.sin();
let z = state.radius * state.pitch.cos() * state.yaw.cos();
transform.translation = state.focus + Vec3::new(x, y, z);
*transform = transform.looking_at(state.focus, Vec3::Y);
```

### 5.3 GLTF Loading in Bevy 0.18

`load_to_bevy` returns `Handle<Gltf>`. Wait for the asset to reach `LoadState::Loaded`, then access `gltf.scenes[0]` and spawn:

```rust
commands.spawn((
    SceneRoot(gltf.scenes[0].clone()),
    transform,
    metadata,
));
```

`SceneBundle` was removed in Bevy 0.15. Use `SceneRoot` as a standalone component.

### 5.4 egui Integration

Use `bevy_egui` 0.31+ (tracks Bevy 0.18). Access via the `EguiContexts` system parameter. Schedule egui systems in `Update` after `EguiSet::BeginPass`.

### 5.5 DB Write Path

Transform persistence goes through the existing `DbCommand` / `DbResult` channel, not direct DB access from Bevy systems:

```sql
UPDATE models SET transform = $1 WHERE id = $2
```

where `$1` is JSON `{ "translation": [x,y,z], "rotation": [x,y,z,w], "scale": [x,y,z] }`.

### 5.6 Material Swapping Strategy

At spawn, traverse immediate mesh children and cache their `Handle<StandardMaterial>` in an `OriginalMaterials(Vec<(Entity, Handle<StandardMaterial>)>)` component on the root entity. On highlight, clone the material, mutate its emissive field, and insert the clone. On restore, re-insert the original handle. Never mutate the asset directly (that would affect all users of the shared handle).

---

## 6. Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `bevy` | 0.18 | Engine |
| `bevy_egui` | 0.31 | egui integration |
| `bevy_panorbit_camera` | compatible with 0.18 | Orbit camera (optional) |
| `serde_json` | 1 | Transform serialization |

---

## 7. Open Questions

1. Should the ground plane tile a texture or remain solid-color? (Default: solid grey for now.)
2. Should the transform editor support non-uniform scale, or lock X / Y / Z scale together by default?
3. Is `bevy_outline` available in the vendor set, or should we use material tinting only?
4. What polling interval is appropriate for `place_model` record sync (suggested: 2 s)?
