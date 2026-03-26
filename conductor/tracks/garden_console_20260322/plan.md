# Garden Console — TDD Implementation Plan

**Track**: `garden_console_20260322`
**Approach**: Test-first. Every task starts with a failing test before any production code is written.
**Phases**: 5
**Verification checkpoints**: One per phase — gate on `cargo test` + `cargo clippy -- -D warnings` green.

---

## Phase 1: DB Query Commands and Result Types (`messages.rs`)

**Goal**: Extend `DbCommand` and `DbResult` with all variants the panels need. Establish the
record types they carry. No DB thread wiring yet — just the data model.

**Why first**: Every subsequent phase depends on these types compiling. Locking them in first
prevents churn in downstream phases.

---

### Task 1.1 — Define record structs

**Test first.**

```rust
// fe-database/src/messages.rs (or fe-runtime/src/messages.rs — wherever DbCommand lives)
#[cfg(test)]
mod record_struct_tests {
    use super::*;

    #[test]
    fn petal_record_derives_clone_debug_deserialize() {
        let r = PetalRecord {
            id: "petal:abc".into(),
            name: "Lobby".into(),
            description: "Main hall".into(),
            created_at: "2026-03-22T00:00:00Z".into(),
        };
        let _ = r.clone();
        let _ = format!("{:?}", r);
    }

    #[test]
    fn room_record_derives_clone_debug_deserialize() {
        let r = RoomRecord {
            id: "room:xyz".into(),
            name: "Foyer".into(),
            petal_id: "petal:abc".into(),
        };
        let _ = r.clone();
    }

    #[test]
    fn model_record_derives() {
        let r = ModelRecord {
            id: "model:1".into(),
            name: "Cube".into(),
            kind: "mesh".into(),
        };
        let _ = r.clone();
    }

    #[test]
    fn role_record_derives() {
        let r = RoleRecord {
            id: "role:1".into(),
            name: "admin".into(),
            permissions: vec!["read".into(), "write".into()],
        };
        let _ = r.clone();
    }
}
```

**Green**: Add the four structs to `messages.rs`. Each derives `Clone, Debug, serde::Deserialize`.
`RoleRecord.permissions` is `Vec<String>`.

**Refactor**: Add `///` doc comments to every field. Ensure field names match SurrealDB column names.

---

### Task 1.2 — Add new `DbCommand` variants

**Test first.**

```rust
#[cfg(test)]
mod db_command_variant_tests {
    use super::*;

    #[test]
    fn list_commands_are_unit_variants() {
        let cmds: Vec<DbCommand> = vec![
            DbCommand::ListPetals,
            DbCommand::ListRooms,
            DbCommand::ListModels,
            DbCommand::ListRoles,
        ];
        assert_eq!(cmds.len(), 4);
    }

    #[test]
    fn mutation_commands_carry_fields() {
        let _ = DbCommand::CreatePetal {
            name: "Test".into(),
            description: "Desc".into(),
        };
        let _ = DbCommand::CreateRoom {
            name: "Room A".into(),
            petal_id: "petal:1".into(),
        };
        let _ = DbCommand::AssignRole {
            node_id: "did:key:abc".into(),
            role_id: "role:1".into(),
        };
    }
}
```

**Green**: Add seven new variants to `DbCommand`. Compiler will flag any exhaustive match arms
that need updating — fix them with `todo!()` stubs for now.

**Refactor**: Verify `Ping`, `Seed`, `Shutdown` behaviour is unchanged by running existing tests.

---

### Task 1.3 — Add new `DbResult` variants

**Test first.**

```rust
#[cfg(test)]
mod db_result_variant_tests {
    use super::*;

    #[test]
    fn list_results_carry_vecs() {
        let _ = DbResult::PetalList(vec![]);
        let _ = DbResult::RoomList(vec![]);
        let _ = DbResult::ModelList(vec![]);
        let _ = DbResult::RoleList(vec![]);
    }

    #[test]
    fn creation_results_carry_id_strings() {
        let r = DbResult::PetalCreated("petal:new".into());
        if let DbResult::PetalCreated(id) = r {
            assert!(!id.is_empty());
        }
        let _ = DbResult::RoomCreated("room:new".into());
        let _ = DbResult::RoleAssigned;
    }
}
```

**Green**: Add seven new variants to `DbResult`.

**Refactor**: Confirm all existing `match` arms on `DbResult` in `fe-runtime` and `fe-ui` compile
without warnings (add `_` arms where the new variants are not yet handled).

---

### Phase 1 Verification Checkpoint

```
cargo test -p fe-database 2>&1 | tail -5
cargo test -p fe-runtime 2>&1 | tail -5
cargo clippy --workspace -- -D warnings 2>&1 | grep "^error" | wc -l
# Expected: 0 errors, all tests pass
```

Gate: Zero clippy errors. All existing tests green. New struct/variant tests green.

---

## Phase 2: DB Thread Handlers for New Commands

**Goal**: The `spawn_db_thread` command loop in `fe-database/src/lib.rs` executes each new
`DbCommand` variant against SurrealDB and sends the matching `DbResult`.

**Why here**: Query logic is isolated to the DB thread. Testing it independently (with in-memory
SurrealDB) prevents contamination of ECS/egui layers.

---

### Task 2.1 — Query functions in `fe-database/src/queries.rs` (or equivalent)

**Test first** — use in-memory SurrealDB or mock.

```rust
#[cfg(test)]
mod query_tests {
    use super::*;

    #[tokio::test]
    async fn list_petals_returns_empty_on_fresh_db() {
        let db = setup_test_db().await;
        let result = list_petals(&db).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn create_then_list_petal() {
        let db = setup_test_db().await;
        create_petal(&db, "Alpha", "First petal").await.unwrap();
        let petals = list_petals(&db).await.unwrap();
        assert_eq!(petals.len(), 1);
        assert_eq!(petals[0].name, "Alpha");
    }

    #[tokio::test]
    async fn create_room_links_to_petal() {
        let db = setup_test_db().await;
        let petal_id = create_petal(&db, "Beta", "").await.unwrap();
        create_room(&db, "Foyer", &petal_id).await.unwrap();
        let rooms = list_rooms(&db).await.unwrap();
        assert_eq!(rooms[0].petal_id, petal_id);
    }

    #[tokio::test]
    async fn assign_role_round_trips() {
        let db = setup_test_db().await;
        assign_role(&db, "did:key:abc", "role:1").await.unwrap();
        let roles = list_roles(&db).await.unwrap();
        assert!(roles.iter().any(|r| r.id.contains("did:key:abc") || r.name == "role:1"));
    }
}
```

**Green**: Implement `list_petals`, `list_rooms`, `list_models`, `list_roles`, `create_petal`,
`create_room`, `assign_role` as async functions. Each returns `Result<Vec<Record>, DbError>` or
`Result<String, DbError>` for creation.

**Refactor**: Extract a `setup_test_db` helper. Remove any duplication with existing query
functions (`create_petal` may already exist — extend rather than duplicate).

---

### Task 2.2 — Wire commands into `spawn_db_thread` loop

**Test first** — channel-level round-trip.

```rust
#[cfg(test)]
mod thread_handler_tests {
    use super::*;

    #[tokio::test]
    async fn list_petals_command_returns_petal_list() {
        let (cmd_tx, result_rx) = spawn_test_db_thread().await;
        cmd_tx.send(DbCommand::ListPetals).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(result, DbResult::PetalList(_)));
    }

    #[tokio::test]
    async fn create_petal_command_returns_created_id() {
        let (cmd_tx, result_rx) = spawn_test_db_thread().await;
        cmd_tx.send(DbCommand::CreatePetal {
            name: "New".into(),
            description: "Desc".into(),
        }).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(result, DbResult::PetalCreated(_)));
    }

    #[tokio::test]
    async fn assign_role_command_returns_ack() {
        let (cmd_tx, result_rx) = spawn_test_db_thread().await;
        cmd_tx.send(DbCommand::AssignRole {
            node_id: "did:key:abc".into(),
            role_id: "role:1".into(),
        }).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(result, DbResult::RoleAssigned));
    }

    #[tokio::test]
    async fn db_errors_return_error_result_not_panic() {
        let (cmd_tx, result_rx) = spawn_test_db_thread_with_bad_db().await;
        cmd_tx.send(DbCommand::ListPetals).unwrap();
        let result = result_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(result, DbResult::Error(_)));
    }
}
```

**Green**: Add match arms in `spawn_db_thread` for all seven new `DbCommand` variants. Each arm
calls the corresponding query function from Task 2.1, then sends the matching `DbResult`. All
errors send `DbResult::Error(msg.to_string())` — no panics, no unwraps.

**Refactor**: Extract a `dispatch_command` helper if the match arm is getting long. Keep each arm
to a single `send(result)` call maximum.

---

### Phase 2 Verification Checkpoint

```
cargo test -p fe-database 2>&1 | tail -10
cargo clippy -p fe-database -- -D warnings
# All 7 new commands round-trip. No panics on DB error path.
```

Gate: All DB thread round-trip tests pass. Error path test passes. Zero clippy warnings in
`fe-database`.

---

## Phase 3: Bevy State Resources and Polling Systems

**Goal**: Define the six Bevy Resources, register them in `GardenerConsolePlugin`, and implement
the systems that keep them up to date from the channel.

**Why here**: Resources and systems are the data contract between DB layer and UI layer. Defining
them before panels ensures panel code is written against a stable interface.

---

### Task 3.1 — Define state resources in `fe-ui/src/state.rs`

**Test first** — headless `App` construction.

```rust
#[cfg(test)]
mod resource_definition_tests {
    use bevy::prelude::*;
    use super::*;

    fn build_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<PetalListState>();
        app.init_resource::<RoomListState>();
        app.init_resource::<ModelListState>();
        app.init_resource::<RoleListState>();
        app.init_resource::<NodeStatusState>();
        app.init_resource::<ActivePetalSelection>();
        app
    }

    #[test]
    fn all_resources_default_insert_without_panic() {
        let app = build_app();
        assert!(app.world().get_resource::<PetalListState>().is_some());
        assert!(app.world().get_resource::<ActivePetalSelection>().is_some());
    }

    #[test]
    fn petal_list_state_defaults_to_empty_not_loading() {
        let app = build_app();
        let state = app.world().resource::<PetalListState>();
        assert!(state.items.is_empty());
        assert!(!state.loading);
        assert!(state.last_fetched.is_none());
    }

    #[test]
    fn active_petal_selection_defaults_to_none() {
        let app = build_app();
        let sel = app.world().resource::<ActivePetalSelection>();
        assert!(sel.selected_id.is_none());
    }
}
```

**Green**: Create `fe-ui/src/state.rs` with all six resources. Implement `Default` for each.

**Refactor**: Add `ThreadStatus { name: String, healthy: bool }` struct to the same file.

---

### Task 3.2 — Register resources in `GardenerConsolePlugin`

**Test first.**

```rust
#[test]
fn gardener_console_plugin_registers_all_resources() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(GardenerConsolePlugin);
    assert!(app.world().get_resource::<PetalListState>().is_some());
    assert!(app.world().get_resource::<NodeStatusState>().is_some());
}
```

**Green**: Add `app.init_resource::<X>()` calls for all six resources in `GardenerConsolePlugin::build`.

**Refactor**: Group them under a comment block for readability.

---

### Task 3.3 — Implement `poll_db_results` system

**Test first.**

```rust
#[test]
fn poll_db_results_routes_petal_list_to_resource() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<PetalListState>();
    // Insert a fake DbResultReceiver with one message pre-loaded
    let (tx, rx) = crossbeam_channel::unbounded::<DbResult>();
    tx.send(DbResult::PetalList(vec![
        PetalRecord { id: "p1".into(), name: "Alpha".into(), description: "".into(), created_at: "".into() }
    ])).unwrap();
    app.insert_resource(DbResultReceiver(rx));
    app.add_systems(Update, poll_db_results);
    app.update();
    let state = app.world().resource::<PetalListState>();
    assert_eq!(state.items.len(), 1);
    assert!(!state.loading);
}

#[test]
fn poll_db_results_routes_role_assigned_ack() {
    // Similar structure — send DbResult::RoleAssigned, confirm no panic, no state corruption.
}
```

**Green**: Implement `poll_db_results` system. It `try_recv`s in a loop until `Empty` or `Disconnected`,
matches each `DbResult` variant, and writes to the corresponding resource. Sets `loading = false`
on list results.

**Refactor**: Ensure `RoleAssigned` and `PetalCreated`/`RoomCreated` trigger `last_fetched = None`
on the relevant list resource to force a re-fetch.

---

### Task 3.4 — Implement refresh systems

**Test first** — one test per refresh system.

```rust
#[test]
fn refresh_petal_list_sends_command_when_last_fetched_is_none() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<PetalListState>(); // last_fetched = None by default
    let (tx, rx) = crossbeam_channel::unbounded::<DbCommand>();
    app.insert_resource(DbCommandSender(tx));
    app.add_systems(Update, refresh_petal_list);
    app.update();
    assert!(matches!(rx.try_recv().unwrap(), DbCommand::ListPetals));
    // loading should be set to true
    assert!(app.world().resource::<PetalListState>().loading);
}

#[test]
fn refresh_petal_list_does_not_double_send_when_already_loading() {
    // Set loading = true, run system twice, assert only one command was sent.
}
```

**Green**: Implement `refresh_petal_list`, `refresh_room_list`, `refresh_model_list`,
`refresh_role_list`. Each checks `!state.loading` AND (`last_fetched.is_none()` OR
`last_fetched.elapsed() > CONSOLE_REFRESH_INTERVAL`) before sending.

**Refactor**: Extract a `should_refresh(loading: bool, last_fetched: Option<Instant>) -> bool`
helper used by all four refresh systems.

---

### Task 3.5 — Implement `update_node_status` system

**Test first** — mock DiagnosticsStore is tricky; test the field assignment logic in isolation.

```rust
#[test]
fn node_status_state_frame_ms_field_can_be_written() {
    let mut state = NodeStatusState::default();
    state.frame_ms = 4.2;
    assert!((state.frame_ms - 4.2).abs() < f64::EPSILON);
}

#[test]
fn update_node_status_reads_node_identity() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<NodeStatusState>();
    app.insert_resource(NodeIdentity::test_fixture("did:key:test123"));
    app.add_systems(Update, update_node_status);
    app.update();
    let state = app.world().resource::<NodeStatusState>();
    assert_eq!(state.node_id, "did:key:test123");
}
```

**Green**: Implement `update_node_status`. Read `DiagnosticsStore` for frame time; gracefully
default to `0.0` when no measurement exists. Read `NodeIdentity` for `node_id`.

**Refactor**: Confirm `NodeIdentity` has a method or field for the DID string; adapt as needed.

---

### Phase 3 Verification Checkpoint

```
cargo test -p fe-ui 2>&1 | tail -15
cargo clippy -p fe-ui -- -D warnings
# All resource init, poll, and refresh tests pass.
```

Gate: All six resources register without panic. `poll_db_results` correctly routes at least
`PetalList` and `RoleAssigned`. Refresh systems send commands on stale state and skip when
already loading.

---

## Phase 4: Live Panel Implementations

**Goal**: Replace each stub panel with a real egui implementation that reads from Resources.
Panel logic is separated from rendering where testable (data-preparation functions that can be
tested headlessly; egui draw calls that cannot).

**Why this order**: Resources are stable from Phase 3. Panel implementations can now be written
against a known data contract.

---

### Task 4.1 — Refactor panel system signatures

**Test first** — compile-time check.

```rust
// Verify the top-level gardener_console_system accepts needed resources as Bevy params.
// If using a monolithic system, write a compile-time test that instantiates the system fn signature.
#[test]
fn gardener_console_system_compiles_with_resource_params() {
    fn _check(
        _ctx: bevy_egui::EguiContexts,
        _petals: Res<PetalListState>,
        _rooms: Res<RoomListState>,
        _models: Res<ModelListState>,
        _roles: Res<RoleListState>,
        _status: Res<NodeStatusState>,
        _active: ResMut<ActivePetalSelection>,
        _cmd_tx: Res<DbCommandSender>,
    ) {}
    // Existence of this function without compile errors is the test.
}
```

**Green**: Refactor `GardenerConsolePlugin`'s UI system to accept the resources above. Pass data
into each panel function as parameters (not as global statics).

**Refactor**: Decide on monolithic vs. per-panel systems. Monolithic (one `egui::Context` scope)
is simpler with bevy_egui — prefer it unless panel count causes readability issues.

---

### Task 4.2 — Implement live Petals panel

**Test first** — data preparation.

```rust
#[test]
fn petals_panel_prep_formats_id_truncated() {
    let records = vec![PetalRecord {
        id: "petal:averylonginternalid".into(),
        name: "Lobby".into(),
        description: "Main entrance".into(),
        created_at: "2026-03-22".into(),
    }];
    let rows = prepare_petal_rows(&records);
    // ID column truncated to 12 chars
    assert!(rows[0].display_id.len() <= 14); // e.g. "petal:avery…"
    assert_eq!(rows[0].name, "Lobby");
}

#[test]
fn petals_panel_prep_empty_state_text() {
    let rows = prepare_petal_rows(&[]);
    assert!(rows.is_empty());
    // Caller is responsible for "No petals" message — not this fn
}
```

**Green**: Replace the stub `PetalsPanel` egui block with:
- `ScrollArea` over `PetalListState.items`.
- Each row: truncated ID, name, description, created_at, "Select" button.
- "Select" mutates `ActivePetalSelection` via `ResMut`.
- "New Petal" form: two `TextEdit` fields in `Local<PetalFormState>`, "Create" button sends
  `DbCommand::CreatePetal` and clears `last_fetched` on `PetalListState`.
- Loading label when `loading == true`.
- "No petals — create one above." when `items.is_empty() && !loading`.

**Refactor**: Highlight selected row via `egui::SelectableLabel` or background colour override.

---

### Task 4.3 — Implement live Asset Browser panel

**Test first** — room filter logic.

```rust
#[test]
fn asset_browser_filters_rooms_by_active_petal() {
    let rooms = vec![
        RoomRecord { id: "r1".into(), name: "Hall".into(), petal_id: "p1".into() },
        RoomRecord { id: "r2".into(), name: "Gallery".into(), petal_id: "p2".into() },
    ];
    let filtered = filter_rooms_for_petal(&rooms, Some("p1"));
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].name, "Hall");
}

#[test]
fn asset_browser_shows_all_rooms_when_no_petal_selected() {
    let rooms = vec![
        RoomRecord { id: "r1".into(), name: "Hall".into(), petal_id: "p1".into() },
        RoomRecord { id: "r2".into(), name: "Gallery".into(), petal_id: "p2".into() },
    ];
    let filtered = filter_rooms_for_petal(&rooms, None);
    assert_eq!(filtered.len(), 2);
}
```

**Green**: Replace `AssetBrowserPanel` stub with tab bar (Rooms / Models). Rooms tab reads
`RoomListState`, filters via `filter_rooms_for_petal`. "New Room" button disabled when
`ActivePetalSelection.selected_id.is_none()`. Models tab reads `ModelListState`.

**Refactor**: `filter_rooms_for_petal` is a pure function — keep it testable by not embedding it
inside the egui closure.

---

### Task 4.4 — Implement live Role Editor panel

**Test first** — validation logic.

```rust
#[test]
fn assign_role_disabled_when_node_id_empty() {
    assert!(!can_assign_role("", "role:1"));
}

#[test]
fn assign_role_disabled_when_role_id_empty() {
    assert!(!can_assign_role("did:key:abc", ""));
}

#[test]
fn assign_role_enabled_when_both_fields_filled() {
    assert!(can_assign_role("did:key:abc", "role:1"));
}
```

**Green**: Replace `RoleEditorPanel` stub with:
- Table of `RoleListState.items`.
- `Local<RoleEditorFormState>` holding `node_id_buf: String`, `selected_role_idx: usize`.
- `ComboBox` populated from role names.
- "Assign" button gated on `can_assign_role(&node_id_buf, &selected_role_id)`.
- `Local<Option<(FeedbackKind, Instant)>>` for temporary success/error display.

**Refactor**: `FeedbackKind { Success(String), Error(String) }` as a local enum.

---

### Task 4.5 — Implement live Node Status panel

**Test first** — colour threshold logic.

```rust
#[test]
fn frame_time_color_green_below_8ms() {
    assert_eq!(frame_time_color(7.9), FrameTimeColor::Green);
}

#[test]
fn frame_time_color_yellow_between_8_and_16ms() {
    assert_eq!(frame_time_color(12.0), FrameTimeColor::Yellow);
}

#[test]
fn frame_time_color_red_at_or_above_16ms() {
    assert_eq!(frame_time_color(16.0), FrameTimeColor::Red);
    assert_eq!(frame_time_color(33.0), FrameTimeColor::Red);
}
```

**Green**: Replace `NodeStatusPanel` stub with:
- Node ID label.
- Frame time value coloured via `frame_time_color` result.
- Current role chip: green label when `Some`, grey "unassigned" when `None`.
- Thread status list: green/red dot + name per `ThreadStatus` entry.

**Refactor**: `frame_time_color` is a pure function — testable independently of egui.

---

### Task 4.6 — Implement Sessions panel

**Test first** — session age formatting.

```rust
#[test]
fn session_age_formats_correctly() {
    let age_secs = 45u64;
    let ttl_secs = 60u64;
    let formatted = format_session_ttl(age_secs, ttl_secs);
    assert_eq!(formatted.expires_in, "15s");
    assert_eq!(formatted.age, "45s");
}
```

**Green**: Replace `SessionsPanel` stub with:
- Lock `SessionCacheHandle`, call `prune_expired()`, iterate entries.
- Table: truncated node ID, age (s), expires in (s).
- "Purge Expired" button calls `prune_expired()` again.
- "Refresh" button forces a re-read snapshot.

Note: If `SessionCacheHandle` does not yet exist, introduce
`SessionCacheHandle(Arc<RwLock<SessionCache>>)` and register it in the plugin before implementing
this panel.

**Refactor**: `format_session_ttl` is a pure function.

---

### Task 4.7 — Implement Peer Manager panel (stub)

No data preparation logic to test. This panel is a stub by design (see Non-Goals in spec).

**Green**: Replace the placeholder string in `PeerManagerPanel` with a static table header
("Peer ID / Address / Last Seen") and an explanatory label:
"Live peer management coming in a future track. Disconnect buttons are stubs."

This satisfies AC-1 (no hardcoded placeholder strings) while keeping the panel visually coherent.

---

### Phase 4 Verification Checkpoint

```
cargo test --workspace 2>&1 | tail -20
cargo clippy --workspace -- -D warnings
# Launch app manually: all six panels show data or meaningful empty states.
# No placeholder text visible in any panel.
```

Gate: All unit tests pass. Zero clippy warnings. Visual inspection confirms AC-1 is met.

---

## Phase 5: Active Petal Selector + Role Chip + Node Status Integration

**Goal**: Wire the Active Petal Selector header widget, populate `NodeStatusState.current_role`
from RBAC, and confirm all cross-panel filtering works end-to-end.

---

### Task 5.1 — Implement Active Petal Selector widget

**Test first** — selection state mutation.

```rust
#[test]
fn selecting_petal_writes_active_petal_selection() {
    let mut selection = ActivePetalSelection::default();
    apply_petal_selection(&mut selection, "petal:1", "Lobby");
    assert_eq!(selection.selected_id.as_deref(), Some("petal:1"));
    assert_eq!(selection.selected_name.as_deref(), Some("Lobby"));
}

#[test]
fn clearing_selection_resets_to_none() {
    let mut selection = ActivePetalSelection {
        selected_id: Some("petal:1".into()),
        selected_name: Some("Lobby".into()),
    };
    clear_petal_selection(&mut selection);
    assert!(selection.selected_id.is_none());
    assert!(selection.selected_name.is_none());
}
```

**Green**: Render the selector as a persistent `egui::ComboBox` or horizontal chip strip at the
top of the console window, populated from `PetalListState.items`. "Clear" button calls
`clear_petal_selection`. Selection writes `ActivePetalSelection` via the pure functions above.

**Refactor**: `apply_petal_selection` and `clear_petal_selection` are pure — keep them outside
egui closures so they remain testable.

---

### Task 5.2 — Cascade system for `ActivePetalSelection` changes

**Test first.**

```rust
#[test]
fn changing_active_petal_marks_room_list_stale() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.init_resource::<ActivePetalSelection>();
    app.init_resource::<RoomListState>();
    app.init_resource::<ModelListState>();
    app.init_resource::<RoleListState>();
    app.add_systems(Update, cascade_active_petal_change);

    // Pre-populate with data and mark not-stale
    {
        let mut rooms = app.world_mut().resource_mut::<RoomListState>();
        rooms.items.push(RoomRecord { id: "r1".into(), name: "Hall".into(), petal_id: "p1".into() });
        rooms.last_fetched = Some(Instant::now());
    }

    // Change the active petal
    app.world_mut().resource_mut::<ActivePetalSelection>().selected_id = Some("p2".into());
    app.update();

    let rooms = app.world().resource::<RoomListState>();
    assert!(rooms.last_fetched.is_none(), "should be reset to force re-fetch");
    assert!(rooms.items.is_empty(), "should be cleared on petal change");
}
```

**Green**: Implement `cascade_active_petal_change` system using a `Local<Option<String>>` to
track the previous `ActivePetalSelection.selected_id`. On change: clear `items` and set
`last_fetched = None` on `RoomListState`, `ModelListState`, `RoleListState`.

**Refactor**: Confirm `refresh_room_list` / `refresh_model_list` / `refresh_role_list` pick up
the stale state in the same frame or next frame (depending on system ordering).

---

### Task 5.3 — Populate `NodeStatusState.current_role` from RBAC

**Test first** — role lookup routing.

```rust
#[test]
fn poll_db_results_routes_role_list_to_node_status_current_role() {
    // If using a GetNodeRole command + result:
    // Send DbResult::NodeRole("admin"), assert NodeStatusState.current_role == Some("admin")
    //
    // If deriving from NodeIdentity directly:
    // Assert NodeStatusState.current_role is populated after update_node_status runs
    // with a NodeIdentity that carries a role field.
}
```

**Green**: Choose approach (see spec FR-9 / Technical Considerations):
- If `NodeIdentity` already carries a role string: read it in `update_node_status`.
- Otherwise: add minimal `DbCommand::GetNodeRole(node_id)` + `DbResult::NodeRole(String)` and
  handle in Phase 2 handlers and `poll_db_results`.

Set `NodeStatusState.current_role` accordingly.

**Refactor**: Ensure role chip in the Node Status panel reads from `current_role` — no hardcoded
strings.

---

### Task 5.4 — End-to-end integration smoke test

**Test first** — headless app with seeded DB.

```rust
#[tokio::test]
async fn gardener_console_end_to_end_smoke() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(GardenerConsolePlugin::with_test_db().await);

    // Seed one petal in the test DB.
    // Run several update cycles.
    for _ in 0..10 { app.update(); }

    let petals = app.world().resource::<PetalListState>();
    assert!(!petals.items.is_empty(), "seeded petal should appear after refresh cycles");
}
```

**Green**: Implement `GardenerConsolePlugin::with_test_db` (or equivalent test constructor) if
it doesn't exist. Otherwise approximate with pre-seeded channels.

**Refactor**: Extract test helpers into a `tests/` module or `test_utils.rs`.

---

### Task 5.5 — Final polish: doc comments, clippy, fmt

**Test first** — documentation completeness.

```
cargo doc --no-deps --workspace 2>&1 | grep "warning\[missing_docs\]"
# Target: zero missing_docs warnings on public items in fe-ui/src/state.rs and messages.rs
```

**Green**: Add `///` doc comments to all public types and functions introduced in this track.

**Refactor**:
- `cargo fmt --all -- --check` — fix any formatting issues.
- `cargo clippy --workspace -- -D warnings` — zero warnings.
- Review all `unwrap()` / `expect()` calls in new code; replace with `?` or explicit error
  handling with `DbResult::Error`.

---

### Phase 5 Verification Checkpoint (Final Gate)

```bash
cargo test --workspace 2>&1 | tail -20
cargo clippy --workspace -- -D warnings 2>&1 | grep "^error" | wc -l
cargo fmt --all -- --check
```

Manual acceptance check against spec acceptance criteria:

| AC | Check |
|---|---|
| AC-1 | Open each of the 6 panels — confirm no placeholder text |
| AC-2 | Wait 5s after launch — confirm Petals/Rooms/Models/Roles tables populate |
| AC-3 | Create a petal — confirm it appears after next refresh |
| AC-4 | Create a room under an active petal — confirm it appears in Rooms tab |
| AC-5 | Assign a role — confirm success feedback shown |
| AC-6 | Vary load — confirm frame time value changes |
| AC-7 | Node status role chip — confirm it matches RBAC, not hardcoded |
| AC-8 | Select a petal — confirm Rooms tab filters within the same frame |
| AC-9 | Introduce artificial DB delay — confirm no frame stall |
| AC-10 | `cargo test --workspace` green |
| AC-11 | `cargo clippy --workspace -- -D warnings` zero errors |
| AC-12 | Sessions panel — confirm TTL countdown is live |

**Track complete when**: All 12 ACs pass, all tests green, zero clippy errors.
