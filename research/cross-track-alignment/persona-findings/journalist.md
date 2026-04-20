# Journalist: Current State Factual Inventory

## Cross-Track Alignment — What Actually Exists Today

---

### 1. CURRENT INSPECTOR STATE

**InspectorFormState** (`fe-ui/src/plugin.rs` lines 157-178):
- Fields: `is_admin: bool`, `external_url: String`, `config_url: String`, `pos: [String; 3]`, `rot: [String; 3]`, `scale: [String; 3]`
- Defaults: `is_admin: false`, all strings empty/zero

**UiAction variants** (`fe-ui/src/plugin.rs` lines 8-18):
- `OpenPortal { url }`, `ClosePortal`, `PortalGoBack`, `SaveUrl`
- No `GetNodeUrl`, no `GetRoles`, no `AssignRole` variants

**ActiveDialog variants** (`fe-ui/src/plugin.rs` lines 33-65):
- `None`, `CreateEntity`, `ContextMenu`, `GltfImport`, `NodeOptions`, `InviteDialog`, `JoinDialog`, `PeerDebug`
- No `ProfilePanel`, no `IdentityManager` variants

**Inspector rendering** (`fe-ui/src/panels.rs` lines 730-785):
- NO TAB SYSTEM. The inspector renders three collapsing sections sequentially: Entity (line 868), Transform (line 894), Portal URLs (line 940)
- Inspector only opens when `node_mgr.selected_entity().is_some()` -- nodes only
- No concept of `InspectedEntity` enum, no petal/fractal/verse inspection

**Existing tab system**: DOES NOT EXIST. No `InspectorTab`, `InspectedEntity`, or `active_tab` anywhere in the codebase.

---

### 2. CURRENT PROFILE / IDENTITY STATE

**NodeIdentity** (`fe-identity/src/resource.rs` lines 5-28):
- Fields: `keypair: NodeKeypair`, `did_key: String`
- Methods: `did_key()`, `verifying_key()`, `sign()`
- NO display name field. NO avatar field. NO profile data whatsoever.
- Is a Bevy `Resource`, but is never inserted into the Bevy world in `main.rs`

**NodeKeypair** (`fe-identity/src/keypair.rs` lines 4-57):
- Fields: `signing_key: SigningKey`
- Not `Clone`. Recreated from seed bytes when a second copy is needed (see `main.rs` line 35)
- Methods: `generate()`, `from_bytes()`, `sign()`, `verifying_key()`, `to_did_key()`, `seed_bytes()`, `to_iroh_secret()`, `pkcs8_der()`

**Keypair persistence** (`fractalengine/src/main.rs` lines 124-154):
- OS keyring via `keyring` crate, service `"fractalengine"`, username `"node_keypair"`
- Stored as 64-char hex of 32-byte seed
- Single identity only. Falls back to ephemeral generation on error

**Keychain helpers** (`fe-identity/src/keychain.rs`):
- `store_keypair(node_id, keypair)` -- stores under service `"fractalengine"`, username = `node_id`
- `load_keypair(node_id)` -- loads by node_id
- `delete_keypair(node_id)` -- deletes by node_id
- These exist but are NOT called from `main.rs`. Main uses `keyring::Entry` directly with hardcoded `"node_keypair"` username.

**UserProfile / ProfileState**: DOES NOT EXIST. No struct, no table, no resource.

**Multi-identity support**: DOES NOT EXIST. Single keypair stored under fixed key `"node_keypair"`.

---

### 3. CURRENT RBAC STATE

**Role table schema** (`fe-database/src/schema.rs` lines 131-137):
```
table "role" => Role { node_id: String, petal_id: String, role: String }
```
- Compound key: node_id + petal_id
- No verse-level roles. No fractal-level roles. Roles are scoped to (node_id, petal_id) pairs only.

**Role checking** (`fe-database/src/rbac.rs`):
- `get_role(db, node_id, petal_id)` -- returns `RoleId`, defaults to `"public"` when no row found
- `require_write_role(db, node_id, petal_id)` -- checks if role is in `WRITE_ROLES = ["owner", "editor"]`
- No `list_roles_for_petal()`, no `assign_role()` function, no `get_all_peers_for_entity()`

**Session struct** (`fe-auth/src/session.rs`):
- Fields: `pub_key: [u8; 32]`, `role: RoleId`, `token: String`
- Minimal. No display name, no metadata.

**SessionCache** (`fe-auth/src/cache.rs`):
- TTL-based (60 seconds), keyed by `[u8; 32]` (public key bytes)
- Maps to `RoleId` only. No profile data cached.

**Role assignment currently**:
- Roles are assigned at seed time (`fe-database/src/lib.rs` line 671-675): `Repo::<Role>::create` with role `"owner"` for the local node DID
- On petal creation (`fe-database/src/lib.rs` line 1155-1159): role `"owner"` assigned to `"local-node"` string
- No UI for role assignment. No DbCommand for assigning/changing roles.

**is_admin determination**:
- `InspectorFormState.is_admin` defaults to `false` (line 170)
- There is NO code that ever sets it to `true` at runtime. The field is completely dead -- it is read for the role chip label (line 443) and for showing the config URL field (line 965), but never written.

---

### 4. CURRENT P2P MESSAGE TYPES

**SyncCommand variants** (`fe-sync/src/messages.rs` lines 10-50):
- `FetchBlob { hash, verse_id }`
- `OpenVerseReplica { verse_id, namespace_id, namespace_secret }`
- `CloseVerseReplica { verse_id }`
- `WriteRowEntry { verse_id, table, record_id, content_hash }`
- `Shutdown`
- `UpdateNodeTransform { verse_id, node_id, position, rotation, scale }`
- NO `ProfileAnnounce`. NO `PeerPresence`. NO `AssignRole`. NO `UpdateProfile`.

**SyncEvent variants** (`fe-sync/src/messages.rs` lines 53-65):
- `Started { online }`, `BlobReady { hash }`, `Stopped`
- NO `PeerConnected`. NO `PeerDisconnected`. NO `ProfileReceived`.

**GossipMessage** (`fe-network/src/types.rs` lines 1-6):
- Generic `GossipMessage<T> { payload: T, sig: Vec<u8>, pub_key: Vec<u8> }`
- This is a signed envelope. No concrete payload types are defined for profiles.

**Gossip functions** (`fe-network/src/gossip.rs`):
- `verify_gossip_message()` -- works, verifies ed25519 signature
- `subscribe_petal()` -- stub (logs only)
- `broadcast()` -- stub (logs only, discards message)

**Peer presence tracking**: DOES NOT EXIST.
- `VersePeers` (`fe-sync/src/verse_peers.rs`) has the data structure (HashMap<verse_id, HashSet<peer_id>>) and methods (add_peer, remove_peer, total_unique_peers, verse_gossip_topic)
- But it is NEVER POPULATED. Init as default (empty). No system ever calls `add_peer()`.
- `SyncStatus.peer_count` is always 0 -- `drain_sync_events` never updates it (no `PeerConnected` event type exists).

**Profile announcement mechanism**: DOES NOT EXIST. No `ProfileAnnounce` message type defined anywhere.

---

### 5. CURRENT DB COMMANDS

**DbCommand variants** (`fe-runtime/src/messages.rs` lines 17-70):
- `Ping`, `Seed`, `Shutdown`
- `CreateVerse { name }`, `CreateFractal { verse_id, name }`, `CreatePetal { fractal_id, name }`, `CreateNode { petal_id, name, position }`
- `ImportGltf { petal_id, name, file_path, position }`
- `LoadHierarchy`
- `GenerateVerseInvite { verse_id, include_write_cap, expiry_hours }`
- `JoinVerseByInvite { invite_string }`
- `ResetDatabase`
- `UpdateNodeTransform { node_id, position, rotation, scale }`
- `UpdateNodeUrl { node_id, url }`
- NO `GetRoles`, `AssignRole`, `GetPeerAccess`, `SaveProfile`, `LoadProfile`, `GetNodeInfo`, `GetPetalInfo`

**DbResult variants** (`fe-runtime/src/messages.rs` lines 72-128):
- `Pong`, `Seeded`, `Started`, `Stopped`, `Error(String)`
- `VerseCreated`, `FractalCreated`, `PetalCreated`, `NodeCreated`
- `GltfImported { node_id, asset_id, petal_id, name, asset_path, position }`
- `HierarchyLoaded { verses: Vec<VerseHierarchyData> }`
- `VerseInviteGenerated`, `VerseJoined`
- `DatabaseReset`
- NO `RolesLoaded`, `ProfileLoaded`, `PeerAccessList`

**UpdateNodeUrl implementation**: YES, FULLY IMPLEMENTED.
- `DbCommand::UpdateNodeUrl` is defined (line 65-69)
- Handled in DB thread (`fe-database/src/lib.rs` line 431-435)
- `update_node_url_handler()` (lines 489-504) runs SurrealQL to update the model record
- UI sends it via `UiAction::SaveUrl` -> `process_ui_actions` (plugin.rs lines 582-604)
- Round-trip: URLs load from DB via `HierarchyLoaded` -> `VerseManager` -> `NodeEntry.webpage_url` -> `sync_manager_to_inspector` populates `InspectorFormState.external_url` on selection change

---

### 6. CROSS-TRACK GAP REPORT

| Capability | Needed By | Exists? | Location | What's Missing |
|---|---|---|---|---|
| **InspectorFormState** | Inspector | YES | `fe-ui/src/plugin.rs:157-178` | No tab state, no InspectedEntity |
| **Inspector tab system** | Inspector FR-2 | NO | -- | Entirely new. No InspectorTab enum, no active_tab field |
| **InspectedEntity enum** | Inspector FR-3 | NO | -- | No hierarchy-level selection concept exists |
| **Petal/Fractal/Verse inspection** | Inspector FR-3 | NO | -- | Inspector only handles nodes |
| **Peer access list for entity** | Inspector FR-4, Profile FR-4 | NO | -- | No DbCommand to fetch peers+roles for an entity |
| **Role assignment UI** | Inspector FR-4 | NO | -- | No DbCommand::AssignRole, no UI |
| **is_admin determination** | Inspector FR-4 | PARTIAL | `fe-ui/src/plugin.rs:159` | Field exists but is NEVER set to true at runtime |
| **UpdateNodeUrl** | Inspector FR-1 | YES | `fe-database/src/lib.rs:431-504` | Fully implemented end-to-end |
| **URL round-trip persistence** | Inspector FR-1 | YES | Multiple files | Works: save -> DB -> hierarchy load -> VerseManager -> inspector |
| **NodeIdentity resource** | Profile FR-1 | YES | `fe-identity/src/resource.rs` | Exists but NOT inserted into Bevy world |
| **Display name** | Profile FR-1, Inspector FR-4 | NO | -- | No field on NodeIdentity, no DB table, no UI |
| **Avatar/icon** | Profile FR-2 | NO | -- | Nothing exists |
| **Profile DB table** | Profile FR-4 | NO | -- | No `profiles` table in schema.rs |
| **Profile editing UI** | Profile FR-2 | NO | -- | No dialog, no panel |
| **Multi-identity support** | Profile FR-3 | NO | -- | Single hardcoded key `"node_keypair"` in keyring |
| **Identity export/import** | Profile FR-3 | NO | -- | No encryption, no file format |
| **Identity switching** | Profile FR-3 | NO | -- | No concept of identity list |
| **Keypair keychain helpers** | Profile FR-3 | YES | `fe-identity/src/keychain.rs` | Exist but unused by main.rs |
| **ProfileAnnounce P2P message** | Profile FR-4 | NO | -- | No message type defined |
| **Peer presence tracking** | Profile FR-4, Inspector FR-4 | PARTIAL | `fe-sync/src/verse_peers.rs` | Data structure exists, never populated |
| **Gossip broadcast** | Profile FR-4 | PARTIAL | `fe-network/src/gossip.rs` | Function exists, is a stub (no-op) |
| **GossipMessage envelope** | Profile FR-4 | YES | `fe-network/src/types.rs:1-6` | Signed envelope works, no app-level payload types |
| **SyncStatus.peer_count** | Profile FR-1 | PARTIAL | `fe-sync/src/status.rs:27` | Field exists, always 0 |
| **VersePeers** | Inspector FR-4, Profile FR-4 | PARTIAL | `fe-sync/src/verse_peers.rs` | Struct + methods exist, empty at runtime |
| **RBAC get_role()** | Inspector FR-4 | YES | `fe-database/src/rbac.rs:9-29` | Works for (node_id, petal_id) scope |
| **RBAC require_write_role()** | Inspector FR-4 | YES | `fe-database/src/rbac.rs:36-47` | Works, checks owner/editor |
| **Role table schema** | Inspector FR-4 | YES | `fe-database/src/schema.rs:131-137` | Petal-scoped only. No verse/fractal scope |
| **Role assignment command** | Inspector FR-4 | NO | -- | No DbCommand::AssignRole variant |
| **Role listing command** | Inspector FR-4 | NO | -- | No DbCommand::GetRoles / ListPeerRoles |
| **SessionCache** | Both | YES | `fe-auth/src/cache.rs` | Works but only caches RoleId, no profile data |
| **Handshake** | Both | YES | `fe-auth/src/handshake.rs` | Works: role lookup + JWT mint. No profile exchange |
| **Clipboard (arboard)** | Profile FR-3 | PARTIAL | Transitive via egui | Not directly depended on |
| **File dialog (rfd)** | Profile FR-3 | YES | `fe-ui/Cargo.toml:18`, `fe-ui/src/dialogs.rs:259` | Already used for GLTF import |
| **Online/offline status** | Profile FR-1 | YES | `fe-sync/src/status.rs:23` | Works via SyncEvent::Started |
| **Verse gossip topic** | Both (P2P) | YES | `fe-sync/src/verse_peers.rs:56-58` | blake3 hash, deterministic |

---

### CRITICAL OBSERVATIONS

1. **is_admin is dead code.** The `InspectorFormState.is_admin` field defaults to `false` and is never set to `true` anywhere in the codebase. The role chip always shows "member". Both tracks depend on admin detection -- neither can function without fixing this.

2. **NodeIdentity is not in the Bevy world.** `NodeIdentity` is a Bevy `Resource` by derive, but `main.rs` never calls `app.insert_resource(NodeIdentity::new(kp))`. The keypair is created, used to derive the iroh secret and DB keypair, but the identity resource is not available to UI systems. Profile display (FR-1) cannot read the local DID without this.

3. **Roles are petal-scoped only.** The `role` table has `(node_id, petal_id)` as its implicit compound key. Inspector FR-3 wants per-verse and per-fractal inspection with access control. The schema has no verse_id or fractal_id in the role table. This is a schema gap.

4. **VersePeers is a hollow shell.** The struct, methods, and tests all exist and work. But nothing ever calls `add_peer()` at runtime. No SyncEvent carries peer connection/disconnection data. The Access tab has no source of truth for "who is connected."

5. **The keychain module is orphaned.** `fe-identity/src/keychain.rs` has fully functional `store_keypair`/`load_keypair`/`delete_keypair` functions parameterized by node_id. Main.rs ignores these and uses `keyring::Entry` directly with a hardcoded username. The Profile track's multi-identity support should use the keychain module, but first needs to reconcile with the existing main.rs approach.

6. **No profile data anywhere.** Zero profile infrastructure exists: no DB table, no Bevy resource, no P2P message type, no UI panel. The Profile track starts from absolute zero for profile-specific work.

7. **Gossip is fully stubbed.** `broadcast()` and `subscribe_petal()` are no-ops. Both tracks need gossip for profile announcements and potentially role change propagation. The signed `GossipMessage<T>` envelope is ready but has no concrete payload types.
