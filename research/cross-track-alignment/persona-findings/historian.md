# Historian: Cross-Track Alignment Analysis

## Executive Summary

The FractalEngine codebase has evolved through a series of architectural phases (Seed Runtime, Garden Console, Petal Portal, P2P Mycelium A-F) that established deeply load-bearing patterns: a single-identity-per-process model, an at-most-one-dialog mutual exclusion system, bounded crossbeam channels for thread communication, and an in-memory hierarchy mirror (VerseManager) that avoids DB round-trips. Both the Inspector Settings and Profile Manager tracks must extend these patterns without disrupting them, but the Profile Manager's multi-identity proposal (Phase 3) represents the single most architecturally disruptive change in the system's history -- it invalidates assumptions wired throughout main.rs, the sync thread, and the keychain layer. The Inspector Settings track follows existing patterns more closely but creates a new selection model (InspectedEntity) that must coexist with the existing NodeManager selection without creating dual-ownership confusion.

---

## Pattern Genealogy

### 1. UiAction: The Queued-Event Pattern

**Origin:** Introduced during the UI Manager Refactor track (ui_manager_refactor_20260419) to replace scattered one-frame signal fields.

**Current state (fe-ui/src/plugin.rs:7-18):**
```rust
pub enum UiAction {
    OpenPortal { url: String },
    ClosePortal,
    PortalGoBack,
    SaveUrl,
}
```

**Architecture:** Actions are pushed during the egui render pass (which runs in `EguiPrimaryContextPass` schedule), then drained in a separate `Update` system (`process_ui_actions` at `fe-ui/src/plugin.rs:537`). This two-phase model exists because egui rendering is immediate-mode and runs before Bevy's Update stage.

**Load-bearing constraint:** Both tracks will add new UiAction variants. Inspector Settings will add `SaveProfile` (or role changes); Profile Manager will add `SaveProfile`, `SwitchIdentity`, etc. The drain loop in `process_ui_actions` is a single function that handles all variants. As this grows, it becomes a coupling bottleneck. The pattern itself is sound, but the single `process_ui_actions` function at `fe-ui/src/plugin.rs:537-608` will need to be refactored or split as variant count grows.

### 2. ActiveDialog: Mutual Exclusion

**Origin:** Consolidated during the UI Manager Refactor from "7 separate dialog-state resources" (comment at `fe-ui/src/plugin.rs:32`).

**Current state (fe-ui/src/plugin.rs:33-65):** Seven variants plus `None`. Dialog state is stored as a field on `UiManager`.

**Load-bearing constraint:** The at-most-one-dialog invariant is enforced by `open_dialog()` replacing the current dialog (plugin.rs:115-117) and `close_dialog()` resetting to `None` (plugin.rs:119-121). The mutual exclusion test at plugin.rs:324-333 explicitly verifies this. Both tracks must decide whether their new UI surfaces are *dialogs* (subject to mutual exclusion) or *always-visible panels* (exempt). The Profile Manager spec explicitly states the profile panel should be "always-visible sidebar element, NOT a dialog" -- this is the correct choice given the constraint, but it creates a new UI surface category that doesn't exist in the current architecture.

### 3. InspectorFormState: The Form Buffer Pattern

**Current state (fe-ui/src/plugin.rs:157-178):** Holds `is_admin`, URL strings, and 3x3 transform text buffers. This is a *display buffer* resource -- it does NOT own the truth (VerseManager and ECS Transform do).

**Data flow:** `sync_manager_to_inspector` (node_manager.rs:467-525) writes transform data from the ECS Transform component into InspectorFormState. URL is synced from VerseManager on selection change (node_manager.rs:491-499).

**Load-bearing constraint:** Inspector Settings Phase 2 adds `active_tab` to this or a new resource. Phase 3 adds `InspectedEntity`. The question is whether InspectorFormState remains node-specific or becomes entity-agnostic. Currently it is tightly coupled to nodes (transform fields have no meaning for petals/fractals/verses). The Inspector Settings plan correctly identifies this (plan.md Task 3.1-3.2) but the actual refactoring touch surface is large.

### 4. Channel Architecture: Bounded Queues and Thread Topology

**Current state (fe-runtime/src/channels.rs:1-37):** Four bounded(256) crossbeam channels: net_cmd, net_evt, db_cmd, db_res.

**Thread topology (main.rs):**
1. Main/Bevy thread: runs the ECS, egui rendering, processes drained events
2. Network thread: net_cmd_rx -> net_evt_tx
3. DB thread: db_cmd_rx -> db_res_tx (with optional repl_tx for sync)
4. Sync thread: sync_cmd_rx -> sync_evt_tx
5. Replication bridge thread: repl_rx -> sync_cmd_tx

**Load-bearing constraint:** The bounded(256) queue depth means that if any consumer thread falls behind, the sender blocks. Both tracks add new command/result variants to DbCommand and DbResult. The Profile Manager's `SaveProfile` / `LoadProfile` commands and the Inspector's `GetEntityPeerRoles` commands add load to the DB thread's command loop. The existing loop (fe-database/src/lib.rs:170-244+) is already a massive match block; both tracks extend it further.

### 5. VerseManager: The In-Memory Hierarchy Mirror

**Origin:** Replaced the old `VerseHierarchy` resource to eliminate DB round-trips on petal navigation (verse_manager.rs docstring, lines 1-10).

**Current state (fe-ui/src/verse_manager.rs):** Owns `Vec<VerseEntry>` with nested FractalEntry -> PetalEntry -> NodeEntry. Updated by `apply_db_results` system which processes DbResult messages.

**Load-bearing constraint:** Inspector Settings Phase 3 reads from VerseManager to populate petal/fractal/verse info tabs. This is fine -- it follows the existing read pattern. Profile Manager does NOT directly interact with VerseManager for hierarchy data, but the Access tab (Inspector Phase 4) needs to correlate peer DIDs with display names, which requires a *separate* data source (PeerProfileCache from Profile Manager). This is a new cross-resource dependency.

---

## Path Dependencies

### Critical: The Single-Identity Assumption

The deepest path dependency in the codebase is the assumption of a single ed25519 keypair per process instance.

**Evidence chain:**

1. **main.rs:24-30** -- `load_or_generate_keypair()` loads ONE keypair. The function name, return type (single `NodeKeypair`), and keyring entry (`"fractalengine"`, `"node_keypair"`) all encode the single-identity assumption.

2. **main.rs:31** -- `let iroh_secret = node_kp.to_iroh_secret();` -- The iroh secret key (which determines the node's P2P identity) is derived from this single keypair.

3. **main.rs:35-36** -- `db_keypair` is recreated from the same seed bytes because `NodeKeypair` is not `Clone`. The DB thread gets its own copy of the SAME identity.

4. **main.rs:62-63** -- `fe_sync::spawn_sync_thread(iroh_secret, ...)` -- The sync thread's lifetime iroh endpoint identity is set once at startup and cannot be changed.

5. **fe-identity/src/resource.rs:6-9** -- `NodeIdentity` wraps a single `NodeKeypair` and caches a single `did_key`. It is a Bevy `Resource` (singleton).

6. **fe-identity/src/keychain.rs:5-20** -- `store_keypair` and `load_keypair` take a `node_id` string, so they *could* support multiple entries, but the caller always passes `"node_keypair"`.

7. **fe-auth/src/session.rs:3-7** -- `Session` has a single `pub_key: [u8; 32]`. No concept of session per identity.

**Impact on Profile Manager Phase 3 (Identity Switching):**

The spec says: "Switching identity requires confirmation: 'This will disconnect from all peers. Continue?'" and "After switching, the application reconnects with the new identity."

This is not a simple UI change. It requires:
- Shutting down the sync thread (which owns the iroh endpoint)
- Re-creating the iroh endpoint with a new secret key
- Re-opening all verse replicas
- Clearing all JWT sessions and auth caches
- Re-populating the DB thread's keypair
- Updating the `NodeIdentity` Bevy resource

None of this infrastructure exists. The sync thread's `spawn_sync_thread` creates the endpoint once and enters a run loop. There is no `SyncCommand::SwitchIdentity` or shutdown-and-restart mechanism.

### Secondary: The node_id Ambiguity

The codebase uses `node_id` to mean two different things:
1. **In the DB schema** (schema.rs:132-137, rbac.rs:9): `node_id` in the `role` table refers to the *peer's* identity (DID or keypair-derived ID).
2. **In the hierarchy** (schema.rs:194-207, verse_manager.rs): `node_id` in the `node` table is a ULID identifying a placed 3D object.

Both tracks touch `node_id`:
- Inspector Settings Phase 4 needs `role.node_id` (peer identity) to display access lists
- Inspector Settings Phase 3 creates `InspectedEntity::Node { id }` where `id` is the entity ULID

This is a naming collision, not a functional bug, but it creates confusion risk during implementation.

### Tertiary: Selection Model Divergence

The current selection model is `NodeManager.selected: Option<NodeSelection>` (node_manager.rs:28-43). This is ECS-entity-based: it tracks a `bevy::prelude::Entity` and a `node_id` string.

Inspector Settings Phase 3 introduces `InspectedEntity` enum with `Node`, `Petal`, `Fractal`, `Verse` variants. But petals, fractals, and verses are NOT ECS entities -- they exist only in VerseManager's in-memory tree.

This means:
- `NodeManager` continues to own ECS-entity-based selection (nodes in the 3D viewport)
- A new selection concept must own hierarchy-level selection (sidebar clicks on petal/fractal/verse names)
- These two selections can coexist (spec says: "Selection at a higher hierarchy level does not deselect the current node")

This dual-selection model is new and has no precedent in the codebase.

---

## Schema Evolution Risks

### Current Schema (10 tables, fe-database/src/schema.rs)

| Table | Key | Notes |
|-------|-----|-------|
| verse | verse_id | Top-level container |
| verse_member | member_id | Peer-to-verse membership |
| fractal | fractal_id | Groups petals under verse |
| petal | petal_id | Space containing nodes |
| room | petal_id | Subdivision of petal |
| model | asset_id | 3D model placement |
| node | node_id | Interactive object |
| asset | asset_id | Binary asset metadata |
| role | node_id (peer!) | RBAC per petal |
| op_log | lamport_clock | CRDT append-only log |

### Inspector Settings Schema Impact

**Phase 4 (Access Tab):** Needs to query `role` table by entity scope. Currently roles are scoped to `(node_id, petal_id)` -- meaning peer `node_id` has role `X` in petal `petal_id`. But the Inspector wants to show roles for Nodes, Petals, Fractals, and Verses. The current schema only supports petal-scoped roles. To show "who has access to this Verse," you would need either:
- A new role scope column (e.g., `scope_type`, `scope_id`)
- Separate role tables per entity type
- Inferring verse-level access from petal-level roles (which loses information)

This is not addressed in either spec.

### Profile Manager Schema Impact

**Phase 4 (Persistence):** Needs a new `profiles` table. The spec suggests:
```
{ id: did_key, display_name: option<string>, avatar_index: u8, created_at: datetime }
```

This is additive -- no conflict with existing tables. However, the `apply_all()` function in schema.rs:232-244 must be updated to include `Repo::<Profile>::apply_schema(db).await?`. The `ALL_TABLE_NAMES` constant (schema.rs:248-259) must also be updated.

### Interaction Risk: Profile + Access Tab

The Inspector's Access tab (Phase 4) wants to display `PeerAccessEntry { peer_id, display_name, role, is_online }`. The `display_name` comes from the Profile Manager's `PeerProfileCache`. This means:
- If Inspector Settings is implemented first, the Access tab shows raw keys (no display names)
- If Profile Manager is implemented first, there's no Access tab to consume the profiles
- Either way, Task 4.6 of the Profile Manager plan ("Wire peer profiles into inspector Access tab") creates a hard dependency on Inspector Settings Phase 4 being at least stub-complete

---

## Historical Lessons

### Lesson 1: The Phase-Comment Trail

The codebase is rich with phase markers: "Phase B", "Phase C", "Phase D", "Phase E", "Phase F". These map to the P2P Mycelium track's incremental delivery. This pattern of phase-marked code has worked well -- it allows later phases to easily find and extend earlier stubs.

**Implication for new tracks:** Both tracks should adopt the same phase-comment convention. `// Profile Manager Phase 1: profile display` and `// Inspector Settings Phase 2: tab system` will make cross-track code navigation possible.

### Lesson 2: The "TODO(tracked)" Antipattern

There are `TODO` comments that are never resolved (e.g., dialogs.rs:54: `// TODO(tracked): CreateEmptyNode at cursor world pos`). These accumulate.

**Implication:** Both tracks should avoid leaving TODOs for the other track. Instead, define explicit stub interfaces (empty functions with correct signatures) that the other track will fill in.

### Lesson 3: The VerseManager Refactoring Precedent

The `VerseManager` module explicitly states it "Replaces the old `VerseHierarchy` resource and the `apply_db_results_to_hierarchy` + `despawn_and_spawn_on_petal_change` systems in plugin.rs" (verse_manager.rs:1-5). This was a significant refactoring that moved logic from a monolithic plugin.rs into a dedicated manager module.

**Implication:** Both tracks should follow this precedent. `ProfileManager` should be a separate module (not bolted onto plugin.rs). Inspector tab state should be a dedicated module, not stuffed into InspectorFormState.

### Lesson 4: The Bounded Channel Backpressure Risk

Channels are bounded at 256 (channels.rs:16-19). The gossip.rs module is entirely stubbed (`// TODO: Wire gossip to the running swarm handle`). When Profile Manager Phase 4 adds `ProfileAnnounce` gossip messages, each profile change generates a broadcast that flows through the sync thread. If many peers are changing profiles simultaneously (e.g., on initial connection), the 256 bound could create backpressure.

**Evidence:** The on_blob_miss callback at main.rs:85-94 already works around a verse_id limitation: "We don't know the verse_id at the asset-reader level, so use a placeholder." This shows the channel architecture creates information-loss pressure.

### Lesson 5: The keyring::Entry Pattern

The keychain integration uses `keyring::Entry::new(SERVICE, username)`. Currently there are two usage patterns:
- `("fractalengine", "node_keypair")` -- main.rs:129 -- the single identity
- `("fractalengine:verse:{id}:ns_secret", "fractalengine")` -- lib.rs:907 -- per-verse namespace secrets

Profile Manager Phase 3 needs to store multiple identities. The keychain.rs module already supports this via `store_keypair(node_id, &keypair)` where `node_id` is the keyring username. But the main.rs entry point only ever stores/loads one entry under `"node_keypair"`. Multi-identity requires changing the keyring strategy from one entry to a registry of entries.

---

## Key Insights

1. **The single-identity assumption is load-bearing infrastructure, not a UI concern.** Profile Manager Phase 3 (identity switching) touches main.rs thread startup, sync thread lifetime, iroh endpoint identity, and session state -- all of which are currently initialized once and assumed immutable. This is not a feature that can be added at the UI layer alone. It requires a new lifecycle management layer that does not exist.

2. **The Inspector and Profile Manager have a temporal dependency at Phase 4.** Inspector Settings Phase 4 (Access Tab) needs peer display names from Profile Manager Phase 4 (PeerProfileCache). Profile Manager Phase 4, Task 4.6 explicitly says "Wire peer profiles into inspector Access tab." Neither track's earlier phases depend on the other, but both Phase 4s create a circular integration point.

3. **The dual-selection model (ECS entity vs. hierarchy level) is unprecedented.** No existing pattern in the codebase supports "inspecting" a non-ECS-entity like a Petal or Verse. The Inspector Settings track creates a new selection concept that must coexist with -- but not conflict with -- NodeManager's entity-based selection. The spec correctly notes this ("Selection at a higher hierarchy level does not deselect the current node") but the implementation requires careful state machine design.

4. **The RBAC schema (role table) is petal-scoped, but the Inspector Access tab wants entity-scoped roles.** The current `role` table has `(node_id, petal_id, role)` -- it cannot express "peer X has role Y on verse Z" without schema evolution. This gap exists between the Inspector Settings spec (which assumes entity-level access control) and the actual schema (which only supports petal-level roles).

5. **The Profile widget as "always-visible sidebar element" establishes a new UI surface category.** The existing UI architecture has three categories: toolbar (top), sidebar panels (left/right), and floating dialogs (mutual exclusion). A persistent profile widget at the top of the sidebar is none of these. It is closest to a sidebar header, like `sidebar_verse_header` (panels.rs:310-393), which provides a precedent for the rendering approach but not for the state management pattern.

---

## Contradictions & Uncertainties

### Contradiction 1: Profile Location
The Profile Manager spec (FR-1) says "top of the left sidebar or a top toolbar section." But the existing left sidebar auto-collapses when the inspector or portal is open (panels.rs:56-57: `sidebar.open = !right_panel_open;`). If the profile is at the top of the left sidebar, it disappears when the user inspects a node -- precisely when they might want to see their role.

### Contradiction 2: Role Display
Both tracks display role information:
- Profile Manager FR-1: "Current role for the active petal (if a petal is selected)"
- Inspector Settings FR-4: "Each entry shows: peer display name, role, online/offline indicator"
- Existing code: `role_chip_hud` (role_chip.rs:6-12) already renders a role chip at bottom-right, currently hardcoded from `inspector.is_admin`.

Three different role display mechanisms with no clear ownership.

### Contradiction 3: Online/Offline Indicator
Both tracks show online/offline status:
- Profile Manager FR-1: "green dot = connected to at least one peer, gray dot = no peers"
- Inspector Settings spec implicitly via the Access tab's "online/offline indicator" per peer
- Existing: status bar already shows online/offline (panels.rs:188-208) from `SyncStatus`

The source of truth for "is this specific peer online" does not exist. `SyncStatus` (status.rs:21-28) tracks `peer_count: usize` but not per-peer online status. `VersePeers` (verse_peers.rs:14-17) tracks peer node IDs per verse but is "Phase F stub: always empty."

### Uncertainty 1: NodeKeypair Not Being Clone
`NodeKeypair` is not `Clone` (main.rs:34 comment: "NodeKeypair is not Clone, so we recreate from the same seed bytes"). This design choice forces seed-byte sharing rather than keypair sharing. For multi-identity, this means the identity list must store seed bytes (or keyring references), not `NodeKeypair` instances.

### Uncertainty 2: InspectedEntity Ownership
The Inspector Settings plan (Task 3.2) says: "Add `inspected: Option<InspectedEntity>` to `UiManager` or `InspectorFormState`." The "or" is an unresolved design decision. Putting it on `UiManager` (which already owns `ActiveDialog` and `PortalState`) follows the centralization precedent. Putting it on `InspectorFormState` co-locates it with the form buffers. The answer depends on whether InspectedEntity drives form buffer content (favoring InspectorFormState) or is a broader UI state concern (favoring UiManager).

---

## File Reference Index

| File | Key Evidence |
|------|-------------|
| `fe-ui/src/plugin.rs:7-18` | UiAction enum definition |
| `fe-ui/src/plugin.rs:33-65` | ActiveDialog enum (mutual exclusion) |
| `fe-ui/src/plugin.rs:68-122` | UiManager with dialog/portal state |
| `fe-ui/src/plugin.rs:157-178` | InspectorFormState (form buffers) |
| `fe-ui/src/plugin.rs:537-608` | process_ui_actions drain loop |
| `fe-ui/src/panels.rs:26-91` | gardener_console layout entry point |
| `fe-ui/src/panels.rs:56-57` | Sidebar auto-collapse on right panel open |
| `fe-ui/src/panels.rs:730-785` | right_inspector panel rendering |
| `fe-ui/src/verse_manager.rs:22-69` | Hierarchy types and VerseManager resource |
| `fe-ui/src/node_manager.rs:28-88` | NodeManager selection model |
| `fe-ui/src/node_manager.rs:467-525` | sync_manager_to_inspector |
| `fe-ui/src/role_chip.rs:6-12` | Role chip HUD rendering |
| `fe-identity/src/resource.rs:6-28` | NodeIdentity (single keypair wrapper) |
| `fe-identity/src/keypair.rs:4-57` | NodeKeypair (not Clone) |
| `fe-identity/src/keychain.rs:1-26` | Keychain store/load/delete |
| `fe-runtime/src/channels.rs:1-37` | Bounded(256) channel architecture |
| `fe-runtime/src/messages.rs:17-70` | DbCommand variants |
| `fe-runtime/src/messages.rs:72-128` | DbResult variants |
| `fe-database/src/schema.rs:88-223` | All 10 table definitions |
| `fe-database/src/rbac.rs:9-29` | get_role() query (petal-scoped) |
| `fe-auth/src/session.rs:3-7` | Session struct (single pub_key) |
| `fe-network/src/types.rs:1-6` | GossipMessage generic type |
| `fe-sync/src/messages.rs:9-50` | SyncCommand variants |
| `fe-sync/src/status.rs:20-28` | SyncStatus resource |
| `fe-sync/src/verse_peers.rs:14-58` | VersePeers (stub, always empty) |
| `fractalengine/src/main.rs:8-122` | Application entry point, single-identity wiring |
| `fractalengine/src/main.rs:128-154` | load_or_generate_keypair (single entry) |
