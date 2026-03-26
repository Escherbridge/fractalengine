# Fractal Atlas — Space Manager & Metadata System

**Track:** fractal_atlas_20260322
**Stack:** Rust, Bevy 0.18, SurrealDB 3.0, bevy_egui 0.39
**Status:** Planned

---

## Overview

Fractal Atlas is the space-management and metadata layer of the Fractal Engine. It gives operators and world-builders a structured way to describe, tag, discover, and export the spaces they manage: petals, rooms, and models. It builds directly on the existing SCHEMAFULL schema and RBAC layer without replacing any existing surface.

---

## Existing Foundation

### Tables (all SCHEMAFULL)

| Table  | Key type      | Notes                       |
|--------|---------------|-----------------------------|
| petal  | PetalId(Ulid) | Top-level spatial container |
| room   | NodeId        | Child of petal              |
| model  | NodeId        | Placed inside a room        |
| role   | RoleId        | RBAC roles                  |
| op_log | auto          | Append-only audit log       |

### Existing query helpers

- `create_petal` — inserts a petal record
- `create_room` — inserts a room record
- `place_model` — places a model into a room

### Entity hierarchy

```
Fractal
  └── Node
        └── Petal
              └── Room
                    └── Model
```

### Type aliases in use

- `PetalId(Ulid)`
- `NodeId`
- `RoleId`

---

## What to Build

### 1. Schema Extensions

All fields added with `DEFINE FIELD IF NOT EXISTS` — non-destructive against live data.

#### Petal table additions

```surql
DEFINE FIELD IF NOT EXISTS description ON TABLE petal TYPE option<string>;
DEFINE FIELD IF NOT EXISTS visibility  ON TABLE petal TYPE string
    ASSERT $value IN ["public", "private", "unlisted"]
    DEFAULT "private";
DEFINE FIELD IF NOT EXISTS tags        ON TABLE petal TYPE array<string> DEFAULT [];
```

#### Room table additions

```surql
DEFINE FIELD IF NOT EXISTS description  ON TABLE room TYPE option<string>;
DEFINE FIELD IF NOT EXISTS bounds       ON TABLE room TYPE option<object>;
DEFINE FIELD IF NOT EXISTS spawn_point  ON TABLE room TYPE option<object>;
```

`bounds` shape (logical; not enforced at DB level):

```json
{
  "min": { "x": f32, "y": f32, "z": f32 },
  "max": { "x": f32, "y": f32, "z": f32 }
}
```

`spawn_point` shape:

```json
{
  "position": { "x": f32, "y": f32, "z": f32 },
  "yaw": f32
}
```

#### Model table additions

```surql
DEFINE FIELD IF NOT EXISTS display_name  ON TABLE model TYPE option<string>;
DEFINE FIELD IF NOT EXISTS description   ON TABLE model TYPE option<string>;
DEFINE FIELD IF NOT EXISTS external_url  ON TABLE model TYPE option<string>;
DEFINE FIELD IF NOT EXISTS config_url    ON TABLE model TYPE option<string>;
DEFINE FIELD IF NOT EXISTS tags          ON TABLE model TYPE array<string> DEFAULT [];
DEFINE FIELD IF NOT EXISTS metadata      ON TABLE model FLEXIBLE TYPE option<object>;
```

`metadata` uses SurrealDB's `FLEXIBLE` modifier — arbitrary key-value pairs, no fixed schema.

---

### 2. New Rust Types

Located in `fe-database/src/atlas.rs` (new file):

```rust
pub enum Visibility { Public, Private, Unlisted }

pub struct PetalMetadata {
    pub description: Option<String>,
    pub visibility: Visibility,
    pub tags: Vec<String>,
}

pub struct RoomBounds {
    pub min: [f32; 3],   // [x, y, z]
    pub max: [f32; 3],
}

pub struct SpawnPoint {
    pub position: [f32; 3],
    pub yaw: f32,
}

pub struct RoomMetadata {
    pub description: Option<String>,
    pub bounds: Option<RoomBounds>,
    pub spawn_point: Option<SpawnPoint>,
}

pub struct ModelMetadataUpdate {
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub external_url: Option<String>,
    pub config_url: Option<String>,
    pub tags: Vec<String>,
    pub metadata: Option<serde_json::Value>,
}

pub struct SpaceOverview {
    pub petal_count: u64,
    pub room_count: u64,
    pub model_count: u64,
    pub peer_count: u64,
    pub estimated_storage_bytes: u64,
}
```

---

### 3. Query Functions (fe-database)

New functions in `fe-database/src/` following the existing async pattern (`surrealdb::Result<T>`).

#### Petal queries

- `update_petal_metadata(db, id, PetalMetadata)` — partial update; writes op-log entry
- `set_petal_visibility(db, id, Visibility)` — targeted update; leaves other fields unchanged
- `list_petals_by_tag(db, tag: &str)` — returns petals whose `tags` array contains the exact value
- `search_petals(db, query: &str)` — case-insensitive CONTAINS on name, description, tags

#### Room queries

- `update_room_metadata(db, id, RoomMetadata)` — partial update
- `get_room_detail(db, id)` — returns all fields including new metadata

#### Model queries

- `update_model_metadata(db, id, ModelMetadataUpdate)` — partial update; writes op-log entry
- `list_models_by_tag(db, tag: &str)` — returns models whose `tags` array contains the exact value
- `search_models(db, query: &str)` — case-insensitive CONTAINS on display_name, description, tags
- `upsert_model_kv(db, id, key: &str, value: serde_json::Value)` — merges one key into `metadata`

#### Aggregate queries

- `space_overview(db, petal_id: Option<PetalId>)` — returns `SpaceOverview` counts; `None` = global scope

---

### 4. UI Modules (fe-ui/src/atlas/)

New directory:

```
fe-ui/src/atlas/
  mod.rs
  petal_wizard.rs       — step-by-step petal creation wizard
  room_editor.rs        — bounds, spawn point, description editor
  model_editor.rs       — name, description, URLs, tags, key-value pairs
  tag_panel.rs          — shared tag input / chip display component
  visibility_control.rs — radio group: public / private / unlisted
  dashboard.rs          — space overview stats panel
  search_bar.rs         — unified name/tag/metadata search + filter chips
  import_export.rs      — JSON import/export of petal config bundles
```

All panels are Bevy systems receiving `ResMut<EguiContexts>`. State lives in Bevy `Resource`s. UI-to-DB communication uses the existing channel pattern (new `DbCommand`/`DbResult` variants).

#### 4.1 Petal Creation Wizard

Multi-step egui window. Resource: `PetalWizardState`.

Steps:

1. **Identity** — name (required, max 64 chars), description (optional)
2. **Visibility** — radio: Public / Private / Unlisted
3. **Tags** — tag chip input (add / remove)
4. **Confirm** — summary of all values; "Create" button fires `CreatePetalEvent`

Validation rules:

- Name: non-empty, max 64 characters
- Tags: max 20, each max 32 characters, no duplicates (case-insensitive)

Navigation: Back / Next buttons disabled appropriately per step and validation state. Wizard resets to step 1 after a successful create.

#### 4.2 Room Editor

Opened from the room list. Single egui window.

Fields:

- Description (multi-line text area)
- Bounds: six f32 inputs (min x/y/z, max x/y/z) — live AABB validation (min < max per axis)
- Spawn point: three f32 position inputs + yaw slider (–180 to 180 degrees)
- "Save" fires `UpdateRoomEvent`

#### 4.3 Model Metadata Editor

Opened from the model list. egui window.

Fields:

- Display name (text field)
- Description (multi-line text area)
- External URL (text field; accepts http/https or empty; format hint shown)
- Config URL (text field; same)
- Tags (shared `tag_input_ui` component)
- Key-value pairs: scrollable table; each row has string key + JSON-value text field + "Delete" button; "Add row" appends a blank row
- "Save" fires `UpdateModelEvent`; invalid JSON in any value field shows inline error and disables save

#### 4.4 Tag System

`tag_panel.rs` exports:

```rust
pub fn tag_input_ui(ui: &mut egui::Ui, tags: &mut Vec<String>)
```

Tags render as chip-style labels with an `×` close button. An inline text field plus "Add" button appends new tags. Validation: duplicate (case-insensitive), length > 32, and count > 20 each produce inline error messages. The "Add" button is disabled when any validation rule is violated.

Tags are searchable: the search bar queries `list_petals_by_tag` and `list_models_by_tag` in parallel.

#### 4.5 Petal Visibility Controls

`visibility_control.rs` exports:

```rust
pub fn visibility_radio_ui(ui: &mut egui::Ui, current: &mut Visibility)
```

Used in the petal wizard (step 2) and as a standalone control in petal detail views.

#### 4.6 Space Overview Dashboard

`dashboard.rs` renders a compact egui panel with:

- Total petals
- Total rooms
- Total models
- Connected peers (from swarm state)
- Estimated storage (from DB aggregate)

Data is fetched via a Bevy async task calling `space_overview(None)` on a configurable polling interval (default: 10 s). Displayed values animate via linear interpolation on update. If the DB is unreachable, stale values are shown with an "offline" indicator; the panel must not panic.

#### 4.7 JSON Import / Export

Export format — per-petal config bundle:

```json
{
  "schema_version": 1,
  "exported_at": "<ISO8601>",
  "petal": {
    "id": "<ulid>",
    "name": "<string>",
    "description": "<string|null>",
    "visibility": "public|private|unlisted",
    "tags": ["<string>"],
    "rooms": [
      {
        "id": "<node_id>",
        "name": "<string>",
        "description": "<string|null>",
        "bounds": { "min": { "x": 0, "y": 0, "z": 0 }, "max": { "x": 0, "y": 0, "z": 0 } },
        "spawn_point": { "position": { "x": 0, "y": 0, "z": 0 }, "yaw": 0.0 },
        "models": [
          {
            "id": "<node_id>",
            "display_name": "<string|null>",
            "description": "<string|null>",
            "external_url": "<string|null>",
            "config_url": "<string|null>",
            "tags": ["<string>"],
            "metadata": {},
            "asset_refs": [
              { "path": "<string>", "blake3": "<hex>" }
            ]
          }
        ]
      }
    ]
  }
}
```

Asset references carry BLAKE3 hashes only (no file embedding). Thumbnail generation is deferred; `asset_refs` provides path + hash for external resolution.

**Import flow:**

1. Parse JSON; reject on invalid JSON or `schema_version` > 1 with descriptive error
2. For each item (petal, rooms, models): check if ID already exists in DB
3. Conflict resolution panel: per-item choice to skip (leave existing unchanged) or overwrite (replace all fields)
4. Apply inserts/updates; show summary of created / skipped / overwritten counts

#### 4.8 Search / Filter

`search_bar.rs` binds a single text input to `SearchQuery` resource. On change (debounced 300 ms):

- Calls `search_petals(query)` and `search_models(query)` concurrently
- Results stored in `SearchResults` resource
- Other panels observe `SearchResults` to highlight / filter their lists

Filter chips narrow results by:

- Entity type: petals / rooms / models (toggle)
- Visibility (petals only): public / private / unlisted (multi-select)
- Tag (multi-select dropdown populated from all tags returned by DB)

Filter chips compose with AND semantics.

---

## Acceptance Criteria

### Schema

- [ ] All `DEFINE FIELD IF NOT EXISTS` statements apply cleanly on a DB that already has the five base tables.
- [ ] Existing records (lacking new fields) remain readable; new fields default correctly.
- [ ] `visibility` ASSERT rejects values outside `["public", "private", "unlisted"]` at the DB level.
- [ ] `metadata` on model accepts arbitrary JSON objects including nested maps and arrays.
- [ ] Schema re-application is idempotent (running `apply_schema` twice produces no errors).

### Query Layer

- [ ] `update_petal_metadata` round-trips: write then read returns identical values for all fields.
- [ ] `set_petal_visibility` changes only `visibility`; all other petal fields are unchanged.
- [ ] `list_petals_by_tag("foo")` returns only petals whose `tags` array contains exactly `"foo"`.
- [ ] `search_petals("tes")` matches on name, description, and tags substrings (case-insensitive).
- [ ] `space_overview(None)` returns correct counts after a sequence of inserts.
- [ ] `upsert_model_kv` merges the given key into `metadata` without clobbering other existing keys.
- [ ] All new query functions are covered by integration tests against a test DB instance.
- [ ] All mutations write an op-log entry with the appropriate operation type.

### UI — Petal Wizard

- [ ] Cannot advance past step 1 with an empty name field.
- [ ] Cannot add a tag that is already present in the list (case-insensitive).
- [ ] Confirm step (step 4) displays all entered values accurately before creation.
- [ ] "Create" button fires `CreatePetalEvent` with the correct payload.
- [ ] Wizard resets to step 1 after a successful creation.
- [ ] Name > 64 characters: "Next" button disabled with inline character-count error.

### UI — Room Editor

- [ ] Bounds validation blocks save when min >= max on any axis; error shown inline.
- [ ] Yaw slider is clamped to [–180, 180].
- [ ] "Save" fires `UpdateRoomEvent` with correct payload.

### UI — Model Metadata Editor

- [ ] Key-value table handles add and delete rows without panic.
- [ ] Invalid JSON in a value field shows an inline error and disables the "Save" button.
- [ ] URL fields accept http/https prefixes or empty string; no reachability check.

### Tag System

- [ ] `tag_input_ui` prevents duplicate tags (case-insensitive comparison).
- [ ] Tags longer than 32 characters are rejected with an inline message; "Add" button disabled.
- [ ] More than 20 tags disables "Add" with an inline message.

### Dashboard

- [ ] Counts update within two polling intervals of a new record being inserted.
- [ ] Panel renders without panic when DB is unreachable; stale data shown with "offline" indicator.

### Import / Export

- [ ] Exported JSON parses back without error on re-import.
- [ ] Import rejects `schema_version` > 1 with a descriptive error; does not panic.
- [ ] BLAKE3 hashes in `asset_refs` survive export → import byte-for-byte.
- [ ] Skip conflict resolution: existing record is unchanged after import.
- [ ] Overwrite conflict resolution: all fields of the existing record are replaced.

### Search

- [ ] Empty query string clears `SearchResults` without issuing a DB call.
- [ ] Rapid keystrokes produce at most one DB call per 300 ms window (debounce).
- [ ] Tag filter and entity-type filter compose with AND semantics.

---

## Out of Scope (This Track)

- Thumbnail generation and display
- Full-text indexing (SurrealDB `CONTAINS` is sufficient at current scale)
- Petal deletion or archival
- Role-based visibility enforcement at query time (separate RBAC track)
- Real-time collaborative metadata editing between peers
- Asset file upload or CDN integration
- Bulk operations (batch tag, batch delete)

---

## Dependencies

| Crate / Feature | Version / Note                              |
|-----------------|---------------------------------------------|
| surrealdb       | 3.0 — `FLEXIBLE` field modifier required    |
| bevy            | 0.18                                        |
| bevy_egui       | 0.39                                        |
| serde / serde_json | existing                                 |
| blake3          | existing (asset hashing)                    |
| ulid            | existing (PetalId)                          |
| tokio           | existing (async runtime)                    |

No new external dependencies are required.
