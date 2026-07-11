# Garden Console — Live Admin & Space Manager UI

**Track ID**: `garden_console_20260322`
**Engine**: FractalEngine (Rust / Bevy 0.18 + bevy_egui 0.39)
**Status**: Active
**Date**: 2026-03-22

---

## Overview

Garden Console is the live administrative interface embedded in FractalEngine. It surfaces real
operational data from SurrealDB through six egui panels, allowing operators to inspect node health,
browse petals and rooms, manage roles, and select an active working context. This track replaces
stub placeholder text with fully-wired, data-backed UI driven by Bevy ECS resources and the
existing DbCommand/DbResult channel architecture.

---

## Context

### What Exists

| Layer | Item | State |
|---|---|---|
| UI | `NodeStatusPanel` | Stub — placeholder text only |
| UI | `PetalsPanel` | Stub — placeholder text only |
| UI | `PeerManagerPanel` | Stub — placeholder text only |
| UI | `RoleEditorPanel` | Stub — placeholder text only |
| UI | `AssetBrowserPanel` | Stub — placeholder text only |
| UI | `SessionsPanel` | Stub — placeholder text only |
| Plugin | `GardenerConsolePlugin` | Warmup guard wired, panels registered |
| Channels | `DbCommand` / `DbResult` | Working — used by existing commands |
| DB | SurrealDB tables | `petal`, `room`, `model`, `role`, `op_log` — schema live |
| Auth | `NodeIdentity` | Bevy Resource — loaded at startup |
| Cache | `SessionCache` | 60-second TTL |
| Diagnostics | `FrameTimeDiagnosticsPlugin` | Registered — data available via `DiagnosticsStore` |
| RBAC | Role query path | `fe-auth` crate — callable from DB thread |

### What Is Missing

- `DbCommand` variants for the six query types the panels need.
- `DbResult` variants carrying the data those queries return.
- Bevy Resources that hold fetched lists so egui systems can read them without blocking.
- Polling/refresh systems that send DbCommands and drain DbResults into those Resources.
- Actual panel implementations rendering from Resources instead of placeholder strings.

---

## Functional Requirements

### FR-1: DbCommand / DbResult Extensions

New command variants are added to `messages.rs` (or the equivalent message module in `fe-database`
or `fe-runtime`):

| Command | Parameters | Description |
|---|---|---|
| `ListPetals` | — | Fetch all petal records |
| `ListRooms` | — | Fetch all room records |
| `ListModels` | — | Fetch all model records |
| `ListRoles` | — | Fetch all role records |
| `CreatePetal` | `name: String, description: String` | Insert a new petal |
| `CreateRoom` | `name: String, petal_id: String` | Insert a new room under a petal |
| `AssignRole` | `node_id: String, role_id: String` | Assign a role to a node |

Corresponding `DbResult` variants carry response data:

| Result | Payload |
|---|---|
| `PetalList(Vec<PetalRecord>)` | All petal rows |
| `RoomList(Vec<RoomRecord>)` | All room rows |
| `ModelList(Vec<ModelRecord>)` | All model rows |
| `RoleList(Vec<RoleRecord>)` | All role rows |
| `PetalCreated(String)` | New petal's record ID |
| `RoomCreated(String)` | New room's record ID |
| `RoleAssigned` | Ack with no payload |

All record types (`PetalRecord`, `RoomRecord`, `ModelRecord`, `RoleRecord`) derive
`Clone, Debug, serde::Deserialize`.

### FR-2: Bevy State Resources

The following Resources are inserted via `init_resource` at plugin startup and mutated only by
Bevy systems — never by egui panel code directly:

| Resource | Fields | Consuming Panel(s) |
|---|---|---|
| `PetalListState` | `items: Vec<PetalRecord>`, `loading: bool`, `last_fetched: Option<Instant>` | Petals, ActivePetalSelector |
| `RoomListState` | `items: Vec<RoomRecord>`, `loading: bool`, `last_fetched: Option<Instant>` | AssetBrowser (Rooms tab) |
| `ModelListState` | `items: Vec<ModelRecord>`, `loading: bool`, `last_fetched: Option<Instant>` | AssetBrowser (Models tab) |
| `RoleListState` | `items: Vec<RoleRecord>`, `loading: bool`, `last_fetched: Option<Instant>` | RoleEditor |
| `NodeStatusState` | `frame_ms: f64`, `thread_statuses: Vec<ThreadStatus>`, `node_id: String`, `current_role: Option<String>` | NodeStatus |
| `ActivePetalSelection` | `selected_id: Option<String>`, `selected_name: Option<String>` | All panels — context filter |

All resources must be `Send + Sync`. No `Rc`, no raw pointers. Each list resource defaults to
`loading: false`, `items: vec![]`, `last_fetched: None`.

### FR-3: Polling and Refresh Systems

A set of Bevy systems runs in the `Update` schedule:

- **`poll_db_results`** — drains all available `DbResult` messages from the receive channel each
  frame via `try_recv` and routes each to the appropriate Resource mutation. Runs before any panel
  system.
- **`refresh_petal_list`** — sends `DbCommand::ListPetals` if `PetalListState.last_fetched` is
  `None` or older than `CONSOLE_REFRESH_INTERVAL` (default 5 s). Sets `loading = true` immediately
  before sending; `poll_db_results` clears it when the result arrives.
- **`refresh_room_list`**, **`refresh_model_list`**, **`refresh_role_list`** — identical pattern
  for their respective types.
- **`update_node_status`** — reads `DiagnosticsStore` each frame for the latest
  `FrameTimeDiagnostics::FRAME_TIME` value (seconds × 1000 = ms), writes to
  `NodeStatusState.frame_ms`. Reads `NodeIdentity` to populate `node_id`.

No system may call `.recv()`, `.await`, or any blocking primitive. All channel reads use
`try_recv`. The `CONSOLE_REFRESH_INTERVAL` constant is defined once in the plugin module.

### FR-4: Petals Panel

The `PetalsPanel` renders:

- A scrollable table of petals: **ID** (truncated), **Name**, **Description**, **Created At**.
- A loading indicator while `PetalListState.loading == true`.
- A "New Petal" inline form: name text field, description text field, "Create" button.
  - "Create" sends `DbCommand::CreatePetal { name, description }` via the command channel.
  - After creation the panel triggers a re-fetch (sets `last_fetched = None`).
- A "Select" button per row that writes `ActivePetalSelection` with that petal's id and name.
- The currently selected row is visually highlighted (distinct background color).
- Empty state: "No petals — create one above."

### FR-5: Asset Browser Panel

The `AssetBrowserPanel` renders:

- A tab bar: **Rooms** / **Models**.
- **Rooms tab**: table with **ID** (truncated), **Name**, **Petal** columns.
  - When `ActivePetalSelection` is set, the DB query filters by petal (passed as parameter in the
    refresh command or as a client-side filter on `RoomListState.items`).
  - "New Room" inline form: name field + "Create" button. Sends `DbCommand::CreateRoom` using the
    current `ActivePetalSelection.selected_id`. Button disabled when no petal is selected.
  - Loading indicator per tab.
- **Models tab**: table with **ID** (truncated), **Name**, **Kind** columns.
- Empty states per tab.

### FR-6: Role Editor Panel

The `RoleEditorPanel` renders:

- A table listing all roles: **ID**, **Name**, **Permissions** (comma-joined).
- An "Assign Role" section:
  - Dropdown populated from `RoleListState.items`.
  - Node-ID text field.
  - "Assign" button that sends `DbCommand::AssignRole { node_id, role_id }`.
  - Assign button is disabled when either field is empty.
- Success/error feedback rendered for at most one second after the result arrives (transient
  `Local<Option<(FeedbackKind, Instant)>>` state).

### FR-7: Node Status Panel

The `NodeStatusPanel` renders:

- **Node ID** — from `NodeStatusState.node_id`.
- **Frame time** — formatted as `X.Xms`, colour-coded:
  - Green: < 8 ms
  - Yellow: 8–15 ms
  - Red: >= 16 ms
- **Current Role** — from `NodeStatusState.current_role`:
  - Green chip with role name when assigned.
  - Grey chip with "unassigned" when `None`.
- **Thread statuses** — compact list of `ThreadStatus { name: String, healthy: bool }` entries
  with a green (healthy) or red (unhealthy) dot indicator.

### FR-8: Sessions Panel

The `SessionsPanel` renders:

- A table of active sessions from `SessionCache`: **Node ID** (truncated), **Age (s)**,
  **Expires In (s)**.
- A "Purge Expired" button that calls the cache's purge method.
- A per-frame refresh button that re-reads the cache snapshot.
- The `SessionCache` is accessed directly (not via DbCommand) because it is an in-process
  resource. It is exposed to Bevy via a `SessionCacheHandle(Arc<RwLock<SessionCache>>)` resource.

### FR-9: Active Petal Selector

A persistent widget rendered at the top of the console window or in a dedicated header row:

- A combo box or chip list populated from `PetalListState`.
- Selecting an item writes `ActivePetalSelection`.
- A "Clear" button resets `ActivePetalSelection` to `{ selected_id: None, selected_name: None }`.
- When no petal is selected, panels that filter by petal show all records.
- The selector itself triggers a refresh of `PetalListState` if its data is stale.

### FR-10: Peer Manager Panel

The `PeerManagerPanel` renders:

- A table of known peers sourced from `NodeIdentity` (peer list field or equivalent).
  Each row: **Peer ID** (truncated), **Address**, **Last Seen**.
- A "Disconnect" button per row that logs a message (full disconnect implementation is
  out of scope for this track — stub only).

---

## Acceptance Criteria

| ID | Criterion |
|---|---|
| AC-1 | All six panels render real data sourced from Bevy Resources; zero hardcoded placeholder strings remain. |
| AC-2 | `ListPetals`, `ListRooms`, `ListModels`, `ListRoles` commands round-trip through the DB thread and populate their Resources within one refresh interval of startup. |
| AC-3 | "Create Petal" form: submitting valid name + description inserts a record, triggers a re-fetch, and the new record appears in the Petals table within the next refresh interval. |
| AC-4 | "Create Room" form: submitting a name with an active petal selected inserts a room linked to that petal and it appears in the Rooms tab. |
| AC-5 | "Assign Role" form: selecting a role + entering a node ID dispatches `AssignRole`; success feedback is displayed for ~1 second. |
| AC-6 | `NodeStatusPanel` frame time value visually changes as application load varies. |
| AC-7 | `NodeStatusPanel` current-role chip reads the actual RBAC role for the local node identity, not a hardcoded string. |
| AC-8 | Selecting a petal in the Active Petal Selector causes the `AssetBrowserPanel` Rooms tab to filter to that petal within the same frame. |
| AC-9 | All polling systems use `try_recv`; no frame stalls occur regardless of DB latency. |
| AC-10 | `cargo test --workspace` passes with new unit tests covering command construction and resource update logic. |
| AC-11 | `cargo clippy --workspace -- -D warnings` produces zero new warnings introduced by this track. |
| AC-12 | Sessions panel shows accurate TTL countdown based on `SessionCache` snapshot. |

---

## Non-Goals

The following are explicitly out of scope for this track:

- Full peer disconnect / network control (Peer Manager shows stubs only).
- Op-log viewer panel (separate track).
- Authentication flow changes.
- Role permission set editing (assignment is in scope; editing what a role can do is not).
- Custom egui theming beyond colour-coded status chips.
- Any new network transport for DbCommands — all DB work stays on the existing in-process DB thread.
- Mobile or non-desktop rendering targets.
- Petal deletion, room deletion, or any other destructive operations.
- Undo/redo.

---

## Technical Considerations

### Channel Architecture

All DB communication uses the existing `crossbeam-channel` (or `std::sync::mpsc`) channels already
threaded through `GardenerConsolePlugin`. The new commands and results are additive enum variants —
no structural change to the channel setup or thread lifecycle.

### Refresh Interval

`CONSOLE_REFRESH_INTERVAL: Duration = Duration::from_secs(5)` is defined once in the plugin
module and referenced by every refresh system. Panels must not hardcode their own intervals.

### ECS System Ordering

`poll_db_results` must be ordered before every panel egui system so same-frame updates are visible.
If ordering ambiguity arises, use a `SystemSet` label (e.g., `ConsoleSet::PollResults` before
`ConsoleSet::RenderPanels`).

### SurrealDB Query Patterns

List queries use `SELECT * FROM <table>`. Optional petal-scoped filtering (`WHERE petal = $pid`)
is performed in the DB thread, keeping the async boundary fully inside `fe-database`. Panel systems
never touch async code.

### egui Transient State

Per-panel state that is not shared between systems (form field buffers, selected tab index, feedback
timers) is stored in `Local<T>` system parameters. This keeps the shared Bevy Resources clean and
deterministic.

### DiagnosticsStore API (Bevy 0.18)

Use `DiagnosticsStore::get_measurement(&FrameTimeDiagnostics::FRAME_TIME)` to obtain the latest
sample in seconds, then multiply by 1000 for milliseconds. Fall back to `0.0` if no sample is
available (first few frames).

### RBAC Role for Node Status

`NodeStatusState.current_role` is populated by querying RBAC using `NodeIdentity.did_key()` as
the node identifier. If `NodeIdentity` already carries a role field populated during handshake,
prefer that to avoid an extra DB round-trip. Otherwise add a minimal `GetNodeRole(node_id: String)`
DbCommand variant.

### Testing Strategy

- Unit tests for enum construction and `#[derive]` completeness live in `messages.rs`.
- Resource update logic (routing `DbResult` → Resource mutation) is tested with a headless
  `bevy::app::App` — no egui context required.
- Panel rendering correctness is validated via acceptance criteria (AC-1 through AC-12) through
  manual inspection and integration runs.
- DB thread handlers are tested with an in-memory SurrealDB instance where available.

### Dependencies

No new crate dependencies are required. All functionality is achievable with crates already in the
workspace:

- `bevy` 0.18 (ECS, diagnostics)
- `bevy_egui` 0.39 (UI rendering)
- `surrealdb` (DB queries — existing)
- `serde` / `serde_json` (record deserialization — existing)
- `crossbeam-channel` or `std::sync::mpsc` (existing channels)
