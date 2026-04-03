# Specification: Light Box — Default Lighting Rig and Light Management System

**Track ID:** `light_box_20260402`
**Type:** Feature
**Status:** Draft
**Date:** 2026-04-02

---

## Overview

Light Box provides a complete lighting subsystem for FractalEngine. Every Petal gets a professional-quality default lighting rig (3-point studio setup) out of the box, so GLTF models look good immediately without manual configuration. Operators can switch between lighting presets, inspect and edit individual lights in the UI, and all light state persists to SurrealDB.

## Background

Currently, FractalEngine has no lighting system. GLTF models loaded into Petals rely on whatever Bevy defaults exist (typically a single ambient light), which produces flat, unappealing visuals. Blender users expect a "default scene" style setup where models are well-lit from first load. This track closes that gap.

The lighting system must integrate with:
- The existing `DbCommand` / `DbResult` channel pattern for persistence (no `block_on` from Bevy)
- The existing `InspectorState` and right inspector panel for light property editing
- The SurrealDB schema in `fe-database/src/schema.rs`
- Bevy 0.18.x lighting primitives (`DirectionalLight`, `PointLight`, `SpotLight`, `AmbientLight`)

---

## Functional Requirements

### FR-1: Light Component System

**Description:** Define Bevy ECS components that represent a persistable light entity. Each light has a type, color, intensity, position/direction, and shadow settings.

**Acceptance Criteria:**
- A `LightKind` enum exists with variants: `Directional`, `Point`, `Spot`
- A `PetalLight` component bundle contains: `LightKind`, color (`Color`), intensity (`f32`), shadow enabled (`bool`), and a `light_id` (`String`) for DB correlation
- `PetalLight` derives or implements `Clone`, `Debug`, `Reflect`
- A `LightPresetTag` component marks which preset a light belongs to (e.g., "studio", "outdoor")
- All types are serializable to/from SurrealDB-compatible formats (serde `Serialize`/`Deserialize`)

**Priority:** P0

### FR-2: Default Light Box (Studio 3-Point Rig)

**Description:** A pre-configured 3-point lighting setup that spawns automatically when a Petal is loaded and contains no persisted lights.

**Acceptance Criteria:**
- Key light: warm directional light (~4500K color temperature), positioned at ~45 degrees elevation with slight azimuth offset, 100% reference intensity, shadows enabled
- Fill light: cooler directional light (~6500K), opposite side of key, ~30% of key intensity, shadows disabled
- Rim/back light: neutral directional light from behind/above, ~50% of key intensity, shadows disabled
- Ambient light: low hemisphere ambient (~10-15% of key intensity) to prevent pure-black shadows
- The full rig spawns as a set of Bevy entities with `PetalLight` components
- A `spawn_default_light_box()` function accepts a preset name and returns the entity set

**Priority:** P0

### FR-3: Light Box Presets

**Description:** Named lighting configurations that operators can switch between.

**Acceptance Criteria:**
- "Studio" preset: the 3-point rig described in FR-2
- "Outdoor" preset: strong directional sun (~80% intensity, warm white, shadows enabled) + elevated ambient (~25%)
- "Dark" preset: minimal ambient only (~5% intensity), no directional lights
- Presets are defined as pure data (no hardcoded entity spawning per preset)
- Switching preset despawns current lights and spawns the new preset's lights
- The active preset name is persisted per Petal

**Priority:** P0

### FR-4: SurrealDB Light Schema and Persistence

**Description:** A `light` table in SurrealDB for persisting light entities per Petal.

**Acceptance Criteria:**
- `DEFINE TABLE light SCHEMAFULL` with fields: `light_id`, `petal_id`, `kind` (string, asserted to directional/point/spot), `color` (array of 4 floats RGBA), `intensity` (float), `position` (array of 3 floats), `direction` (array of 3 floats), `shadows_enabled` (bool), `preset_tag` (option string), `created_at` (string)
- RBAC permissions: only admin and roles with write access can mutate lights
- Light CRUD operations go through the `DbCommand` channel (new variants: `CreateLight`, `UpdateLight`, `DeleteLight`, `LoadLights`)
- `DbResult` gets corresponding response variants
- On Petal load, lights are fetched from DB; if none exist, the default "Studio" preset is spawned and persisted

**Priority:** P0

### FR-5: Light Inspector Panel

**Description:** When a light entity is selected in the viewport, the right inspector panel shows light-specific properties.

**Acceptance Criteria:**
- Inspector detects whether the selected entity has a `PetalLight` component
- If light entity: shows a "Light" collapsible section with:
  - Light kind display (read-only label)
  - Color picker (RGB)
  - Intensity slider (0.0 to 5.0 range, default 1.0)
  - Shadow toggle checkbox
- Changes in the inspector update the Bevy component immediately (live preview)
- A "Save" action persists the updated light to SurrealDB via `DbCommand::UpdateLight`
- If the selected entity is not a light, the Light section is hidden

**Priority:** P1

### FR-6: Default Light Spawning on Petal Load

**Description:** Automatic light spawning logic on Petal load.

**Acceptance Criteria:**
- When a Petal is loaded and `DbResult::LightsLoaded` returns an empty list, the system spawns the default "Studio" light box
- The spawned default lights are immediately persisted to SurrealDB
- When lights exist in the DB, they are spawned as Bevy entities from the persisted data
- Light entities are despawned when switching Petals (cleanup system)

**Priority:** P0

---

## Non-Functional Requirements

### NFR-1: Performance

- Light spawning must complete within a single frame (no async spawning for lights)
- Maximum 16 active light entities per Petal (Bevy forward renderer shadow map limit)
- Preset switching must complete within 2 frames (despawn + spawn)

### NFR-2: Compatibility

- All light types must use Bevy 0.18.x native lighting components (no custom shaders)
- Light color values stored as linear RGBA (not sRGB) to match Bevy internals

### NFR-3: Persistence Safety

- All DB writes go through `DbCommand` channel; no `block_on()` from Bevy systems
- RBAC enforced at SurrealDB layer, not in Bevy systems
- Light mutations logged to op_log for sync compatibility

---

## User Stories

### US-1: First-Time Petal Experience

**As** an operator loading a new Petal for the first time,
**I want** models to be well-lit automatically,
**So that** I get a professional-looking scene without manual setup.

**Given** a Petal with no persisted lights
**When** the Petal loads
**Then** a Studio 3-point lighting rig is spawned and the scene is visibly well-lit

### US-2: Preset Switching

**As** an operator building a scene,
**I want** to switch between lighting presets,
**So that** I can quickly find the right mood for my Petal.

**Given** a Petal with active Studio lighting
**When** the operator selects the "Outdoor" preset
**Then** studio lights are removed, outdoor lights appear, and the change persists

### US-3: Light Fine-Tuning

**As** an operator,
**I want** to select a light and adjust its color, intensity, and shadows,
**So that** I can customize the lighting beyond presets.

**Given** a light entity is selected in the viewport
**When** the operator adjusts intensity from 1.0 to 0.5 in the inspector
**Then** the scene updates immediately and the change can be saved to DB

### US-4: Light Persistence Across Sessions

**As** an operator,
**I want** my custom light settings to persist,
**So that** I see the same lighting when I reopen a Petal.

**Given** the operator has customized a light and saved
**When** the Petal is closed and reopened
**Then** the customized light values are restored from SurrealDB

---

## Technical Considerations

- **Crate placement:** Light components and spawn logic in `fe-renderer` (new `lighting` module). Inspector UI in `fe-ui`. Schema in `fe-database`. Message types in `fe-runtime`.
- **Bevy 0.18.x API:** Use `DirectionalLight`, `PointLight`, `SpotLight` bundles with `Transform` for positioning. Use `AmbientLight` resource for hemisphere fill.
- **Color temperature to RGB:** Provide a utility function to convert approximate Kelvin values to linear RGB (simple approximation, not full blackbody curve).
- **Shadow maps:** Only enable shadows on the key light by default to limit GPU cost. Bevy 0.18 supports cascaded shadow maps for directional lights.
- **Preset data structure:** Use a `LightPreset` struct containing a `Vec<LightConfig>` (pure data, no ECS types). A `spawn_from_preset()` function converts data to entities.
- **DbCommand extension:** Add light-specific variants to the existing `DbCommand` enum in `fe-runtime/src/messages.rs`.

---

## Out of Scope

- Volumetric lighting / god rays
- Light baking or global illumination
- Area lights (not supported in Bevy 0.18)
- Per-room lighting zones (future track)
- Light animation / keyframes
- Custom operator-defined presets (future; only built-in presets for now)
- Emissive material glow from GLTF models

---

## Open Questions

1. **Point lights and spot lights in presets:** The 3-point rig uses directional lights. Should any preset include point or spot lights, or defer those to manual placement only?
2. **Light gizmos:** Should lights render a visible gizmo/icon in the viewport for selection, or rely on a sidebar list? (Bevy 0.18 has built-in light gizmos.)
3. **Maximum light count enforcement:** Should the system hard-reject light creation above 16, or warn and allow with degraded shadow quality?
