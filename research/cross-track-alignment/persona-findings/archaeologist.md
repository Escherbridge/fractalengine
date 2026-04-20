# Archaeologist: Codebase Pattern Archaeology

## Implicit Contracts, Fossils, and Constraints for Cross-Track Alignment

---

## 1. UiAction Pattern Archaeology

**Where**: `fe-ui/src/plugin.rs` (lines 8-18, 91-98, 536-608)

**How it works**: `UiAction` enum has 4 variants: `OpenPortal`, `ClosePortal`, `PortalGoBack`, `SaveUrl`. During `EguiPrimaryContextPass`, panel code pushes actions into `UiManager.actions: Vec<UiAction>`. In `Update`, `process_ui_actions` (in `UiSet::ProcessActions`) calls `drain_actions()` via `std::mem::take()` and processes each action in a match block.

**How new variants are added**: No registration system. Add variant to enum, add match arm. Compiler enforces exhaustive matching.

**Error handling**: Invalid URLs logged via `bevy::log::warn!`. Channel-send failures checked with `.is_err()`. No user-visible error surfacing.

**Constraint for both tracks**: All new UI side effects MUST go through UiAction — never directly mutate world state during egui rendering.

---

## 2. Resource Registration Pattern

**Where**: `fe-ui/src/plugin.rs` (lines 348-384)

`GardenerConsolePlugin::build` uses `init_resource::<T>()` for Default types. Domain managers register via `add_plugins()` — each manager plugin calls `init_resource` and `add_systems` internally. Main binary uses `insert_resource()` for channel wrappers requiring constructor args.

**System ordering**: `UiSet` enum: `ProcessActions -> Selection -> PostSelection`. VerseManager's systems run `.before(UiSet::ProcessActions)`.

**Pattern for new resources**: Both tracks should follow the manager-plugin pattern: create `XxxManagerPlugin` with its own resource and systems.

---

## 3. Channel Architecture — The Fire-and-Forget Pattern

**All 4 channel pairs**: `crossbeam::channel::bounded(256)`. NO back-pressure handling beyond the bound.

**Critical finding: No request-response correlation exists.** There is no correlation ID linking a specific DbResult to a specific DbCommand. The system relies on semantic matching (e.g., `VerseCreated` corresponds to the most recent `CreateVerse`).

**Full lifecycle**:
1. UI sends `DbCommand` via channel
2. DB thread's tokio runtime processes via `rx.recv()` blocking loop
3. Handler sends `DbResult` back
4. `drain_db_results` system converts to Bevy `MessageWriter<DbResult>`
5. `apply_db_results` reads via `MessageReader<DbResult>` same frame

**Constraint**: Both tracks cannot robustly handle concurrent commands of the same type. Consider adding `request_id: u64` if confirmed-completion is needed.

---

## 4. Hierarchy Selection Pattern

**NodeManager.selected**: `Option<NodeSelection>` where `NodeSelection` = `{ entity: Entity, node_id: String, drag: Option<AxisDrag> }`. ONE node at a time. Selection via viewport raycast or sidebar click.

**NavigationManager**: Holds `active_verse_id`, `active_fractal_id`, `active_petal_id` as navigation cursor. No "selected petal for inspection" concept — navigation IS selection at the verse/fractal/petal level.

**Inspector Phase 3 (hierarchy inspection)** needs a NEW selection concept (`InspectedEntity` enum) that co-exists with both NodeManager selection and NavigationManager cursor. Conflating them would break the separation of concerns.

---

## 5. P2P Message Evolution

**Two separate messaging systems**:
- `GossipMessage<T>` in `fe-network/src/types.rs`: Generic signed envelope. Currently ONLY used for verification — broadcast/subscribe are stubs.
- `SyncCommand/SyncEvent`: Production channel. New message types = new enum variants.

**Gossip IS separate from document replication**. Gossip topics use `verse_gossip_topic()` (BLAKE3). Document replication uses `VerseReplicator` with namespace IDs.

**Adding ProfileAnnounce structurally**:
1. Define payload struct in `fe-network/src/types.rs`
2. Add `SyncCommand::BroadcastGossip` variant
3. Add `SyncEvent::GossipReceived` variant
4. Handle in sync thread's command loop
5. Update `VersePeers` from Bevy side on event

---

## 6. Schema Migration — Additive Only

**No migration system.** Schema uses `DEFINE TABLE/FIELD IF NOT EXISTS`. Purely additive and idempotent.

**Only migration precedent**: `migrate_base64_assets_to_blob_store` in `fe-database/src/lib.rs` (lines 745-819). One-time eager migration on startup.

**Pattern**: Add `define_table!` definitions, register in `apply_all()` and `ALL_TABLE_NAMES`. Cannot rename or remove fields without manual intervention. `ResetDatabase` command is the only destructive migration path.

---

## 7. CRITICAL IMPLICIT CONTRACTS

### A. The "local-node" Singleton — The Deepest Fossil

The hardcoded string `"local-node"` appears in **14+ locations**:

- `fe-database/src/lib.rs` line 591: `NodeId("local-node")` in seed data
- `fe-database/src/lib.rs` line 848: `created_by: "local-node"` in create_verse
- `fe-database/src/lib.rs` lines 1078, 1105, 1149, 1156: create_fractal, create_petal, join-verse
- `fe-sync/src/sync_thread.rs` line 195: replicator author_id
- `fe-test-harness/src/peer.rs`: 8+ occurrences in test fixtures

**Impact**: Profile Manager MUST replace ALL these with actual DID strings before multi-identity works. Inspector's Access tab would show "local-node" as the local peer's identity otherwise.

### B. Role Value Mismatch — The UI/DB Disconnect

**DB stores**: `"owner"`, `"editor"`, `"member"`, `"public"` (from `WRITE_ROLES` and defaults in `rbac.rs`)
**UI displays**: `"admin"` / `"member"` (from `role_chip.rs` and `plugin.rs:443-447`)

These do NOT match. The UI says "admin" but the DB has "owner". Both tracks must reconcile this before building RBAC UI.

### C. ID Format Assumptions

- Entity IDs: `ulid::Ulid::new().to_string()` — 26-char Crockford Base32
- DID format: `"did:key:z6Mk..."` (multicodec + base58btc)
- Handshake identity: `hex::encode(pub_key.to_bytes())` — 64-char hex
- `VerseMember.peer_did` uses DID format (established convention)

**Three different representations of the same 32-byte key**. Both tracks must choose one canonical form. `did:key` is the precedent from `verse_member`.

### D. Channel Message Ordering

System assumes results arrive approximately in command order. `apply_db_results` processes sequentially, no out-of-order check. Single-threaded DB loop makes this safe in practice, but neither track should assume strict ordering for concurrent commands.

### E. Namespace Secret Ownership

Keyring entries (`"fractalengine:verse:{id}:ns_secret"`) are bound to the MACHINE, not the identity. Identity switching means all identities on the same machine share verse access — a potential security concern.

### F. The "Phase F" TODO Pattern

Comments like `// TODO(Phase F): use actual node DID` throughout codebase. Both tracks are "Phase G+" work building on partially-implemented Phase F stubs. `VersePeers` is the most significant stub both tracks depend on.

---

## Summary of Fossils and Their Constraints

| Fossil | Location | Impact on Inspector | Impact on Profile |
|--------|----------|--------------------|--------------------|
| `"local-node"` singleton | 14+ locations | Access tab shows wrong identity | Must replace before multi-identity |
| Role label mismatch (admin vs owner) | `role_chip.rs`, `rbac.rs` | RBAC UI uses wrong labels | Profile widget uses wrong labels |
| Fire-and-forget channels | All channel pairs | No confirmed-completion for role changes | No confirmed-completion for profile saves |
| No `NodeIdentity` in Bevy world | `main.rs` | Can't query local DID for admin check | Can't display local profile |
| `VersePeers` empty stub | `verse_peers.rs` | Access tab can't show online/offline | Profile widget can't show connection status |
| Additive-only schema | `schema.rs` | Can add tables, can't modify role table | Can add profiles table |
| Single selection model | `node_manager.rs` | Must add InspectedEntity alongside, not replace | N/A |
| Navigation ≠ Inspection | `navigation_manager.rs` | Must create new selection concept | Reads active petal from NavigationManager |
