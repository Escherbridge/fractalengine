# Dataflows

ASCII diagrams of the major data flows through FractalEngine.

---

## 1. DB → UI Dataflow (GLTF Import)

User picks a GLTF file via the import dialog. The path flows down to the DB thread,
the DB imports it and fires a result event, which the UI handles to update state and
spawn a scene entity.

```
User selects GLTF file in dialog
        │
        ▼
GltfImportState { file_path_buf, name_buf, position }
        │  (user clicks "Import")
        ▼
db_sender.send(DbCommand::ImportGltf {
    file_path, name, petal_id, position
})
        │  (crossbeam channel)
        ▼
[DB thread — fe-database]
  import_gltf_to_db(...)
  → copy asset to assets/ dir
  → INSERT node row with asset_path
  → return node_id
        │
        ▼
MessageReader<DbResult::GltfImported {
    node_id, name, petal_id, asset_path, position
}>
        │  (Bevy event, read next Update)
        ▼
[apply_db_results — verse_manager.rs]
  verse_mgr.find_petal_mut(petal_id)
    .nodes.push(NodeEntry { id, name, has_asset: true,
                            position, asset_path: Some(...) })
  if nav.active_petal_id == petal_id:
    spawn_node_entity(commands, asset_server,
                      node_id, petal_id, name, position, asset_path)
        │
        ▼
Bevy scene entity spawned:
  SceneRoot(asset_server.load("path/to/model.glb#Scene0"))
  + Transform::from_xyz(x, y, z)
  + SpawnedNodeMarker { node_id, petal_id }
  + Name::new(name)
        │
        ▼
Model appears in 3D viewport
```

---

## 2. Node Transform Dataflow (Gimbal Drag)

User selects a node (via click or sidebar), switches to Move/Rotate/Scale tool,
then drags a gimbal axis. On release the transform is committed to DB and P2P peers.

```
User clicks gimbal axis handle
        │
        ▼
[handle_gimbal_interaction — node_manager.rs]
  axis hit-test: segment_dist_2d(cursor, center_screen, tip_screen) < PICK_PX
  NodeManager.selected.drag = Some(AxisDrag {
    axis, start_cursor, axis_screen_dir, start_pos, start_rot, start_scale
  })
        │  (every frame while mouse held)
        ▼
[handle_gimbal_interaction — apply drag]
  movement = (cursor - start_cursor).dot(axis_screen_dir)
  scale_factor = distance_to_camera * 0.002
  transform.translation = start_pos + axis_dir * movement * scale_factor
        │
        ▼
Transform component updated on entity
  → Bevy renders updated position
  → Changed<Transform> detected by sync_manager_to_inspector
        │
        ▼
[sync_manager_to_inspector]
  InspectorState.pos/rot/scale = formatted transform values
  → Inspector panel shows live values
        │  (on mouse release)
        ▼
[handle_gimbal_interaction — commit]
  sel.drag = None
  sel.drag_committed = true
        │  (same frame, later in chain)
        ▼
[broadcast_transform — node_manager.rs]
  sel.drag_committed = false  (consume flag)
  read (Transform, SpawnedNodeMarker) from entity
  verse_mgr.update_node_position(node_id, [x,y,z])  ← in-memory sync
  db_sender.send(DbCommand::UpdateNodeTransform {    ← persist
    node_id, position, rotation, scale
  })
  sync_sender.send(SyncCommand::UpdateNodeTransform {← broadcast to peers
    verse_id, node_id, position, rotation, scale
  })
```

---

## 3. Navigation State Machine

```
                     ┌─────────────────────────────────┐
                     │        No Verse Selected         │
                     │  active_verse_id  = None         │
                     │  active_fractal_id = None        │
                     │  active_petal_id  = None         │
                     └──────────────┬──────────────────┘
                                    │ navigate_to_verse(id, name)
                                    ▼
                     ┌─────────────────────────────────┐
                     │        Verse Selected            │
                     │  active_verse_id  = Some(v)      │
                     │  active_fractal_id = None        │
                     │  active_petal_id  = None         │
                     └──────┬─────────────────┬────────┘
                            │                 │ back_from_verse()
       navigate_to_fractal  │                 ▲ (clears all 3 fields)
       (id, name)           │                 │
                            ▼                 │
                     ┌─────────────────────────────────┐
                     │       Fractal Selected           │
                     │  active_verse_id  = Some(v)      │
                     │  active_fractal_id = Some(f)     │
                     │  active_petal_id  = None         │
                     └──────┬─────────────────┬────────┘
                            │                 │ back_from_fractal()
        navigate_to_petal   │                 ▲ (clears fractal + petal)
        (id)                │                 │
                            ▼                 │
                     ┌─────────────────────────────────┐
                     │        Petal Selected            │
                     │  active_verse_id  = Some(v)      │
                     │  active_fractal_id = Some(f)     │
                     │  active_petal_id  = Some(p)      │
                     └─────────────────────────────────┘
                                    │ back_from_petal()
                                    ▼ (clears only petal)
                           returns to Fractal Selected state

Side effect on active_verse_id change:
  close old P2P replica → open new P2P replica
  (handled by handle_verse_replica_lifecycle)
```

---

## 4. P2P Sync Flow

```
User navigates to a Verse that has a namespace_id
        │
        ▼
NavigationManager.active_verse_id changes
        │
        ▼
[handle_verse_replica_lifecycle — navigation_manager.rs]
  verse_mgr.find verse by id → get namespace_id
  fe_database::get_namespace_secret_from_keyring(verse_id) → namespace_secret (Option)
  sync_sender.send(SyncCommand::OpenVerseReplica {
    verse_id,
    namespace_id,
    namespace_secret,   ← Some = read-write, None = read-only
  })
        │  (crossbeam channel to sync thread)
        ▼
[fe-sync thread]
  iroh-docs: open or create replica for namespace_id
  subscribe to replica changes from peers
        │
        ▼
Peer sends updated node transform
        │
        ▼
[fe-sync thread receives iroh-docs update]
  parse node_id + transform from document entry
  (future: send SyncResult back to Bevy via MessageReader)
        │
User drags a node (see Transform Dataflow above)
        │
        ▼
broadcast_transform sends:
  SyncCommand::UpdateNodeTransform {
    verse_id, node_id,
    position: [x, y, z],
    rotation: [rx, ry, rz],
    scale:    [sx, sy, sz],
  }
        │
        ▼
[fe-sync thread]
  serialise to JSON
  write to iroh-docs replica entry keyed by node_id
        │
        ▼
iroh protocol replicates entry to connected peers
        │
        ▼
Peers see transform update in their viewport

Invite / Join flow:
  db_sender.send(DbCommand::GenerateVerseInvite { verse_id, ... })
      → DB thread: iroh ticket for namespace
      → DbResult::VerseInviteGenerated { invite_string }
      → InviteDialogState.invite_string shown in UI

  db_sender.send(DbCommand::JoinVerse { invite_string })
      → DB thread: parse ticket → get namespace_id → store
      → DbResult::VerseJoined
      → apply_db_results sends DbCommand::LoadHierarchy to refresh tree
```

---

## 5. Petal Switch Dataflow

```
User clicks a different Petal in the sidebar
        │
        ▼
panels::gardener_console() calls
  nav.navigate_to_petal(petal_id)
        │
        ▼
NavigationManager.active_petal_id = Some(new_petal_id)
        │  (detected next Update frame)
        ▼
[respawn_on_petal_change — verse_manager.rs]
  Local<last_petal_id> != nav.active_petal_id  → change detected

  Phase 1 — collect kept node ids:
    for (entity, marker) in spawned.iter():
      if marker.petal_id == new_petal_id:
        kept_node_ids.insert(marker.node_id)
        (these are entities already in the scene for the new petal)

  Phase 2 — despawn old entities:
    for (entity, marker) in spawned.iter():
      if marker.petal_id != new_petal_id:
        commands.entity(entity).despawn()
        (deferred — entity still in query this frame)

  Phase 3 — spawn new entities from in-memory tree (NO DB round-trip):
    verse_mgr.find_petal(new_petal_id)
      for node in petal.nodes:
        if node.asset_path.is_some() AND node.id not in kept_node_ids:
          spawn_node_entity(commands, asset_server,
                            node.id, petal.id, node.name,
                            node.position, node.asset_path)

  Phase 4 — update last:
    *last = nav.active_petal_id.clone()
        │
        ▼
Old petal entities removed from scene
New petal entities loaded and rendered
```
