# Implementation Plan: Shared Peer Infrastructure (Phase 0)

## Overview

Two-phase TDD implementation. Phase 1 establishes identity (FR-1, FR-2, FR-5, FR-7). Phase 2 builds the peer registry and presence system (FR-3, FR-4, FR-6). Both phases gate downstream track work: downstream tracks must not start their Phase 4 until both phases here are verified.

**TDD discipline applies throughout.** Every system gets a Red test before the Green implementation. Tests are written to fail first (asserting on behavior that doesn't exist yet), then the implementation makes them pass.

---

## Phase 1: Identity Foundation

**Resolves:** BLOCKER 2 (Admin Verification Dead Code), Decision 1 (Canonical Peer Identifier Format)
**Unblocks:** FR-1, FR-2, FR-5, FR-6 (schema), FR-7 in spec

---

### Task 1.1 — NodeIdentity resource existence test (TDD Red)

Write a test asserting that the `NodeIdentity` resource exists in the Bevy world after application startup.

- Create a minimal Bevy `App` with only the systems under test.
- Call `app.update()`.
- Assert `app.world.get_resource::<NodeIdentity>()` returns `Some(...)`.
- Test must fail (Red) because `NodeIdentity` is not yet inserted.

**Files touched:** new test file, e.g. `fractalengine/src/identity_tests.rs` or inline module.

---

### Task 1.2 — Insert NodeIdentity in main.rs (TDD Green)

Implement `NodeIdentity` as a Bevy `Resource` and insert it during startup.

- Define `NodeIdentity { did: String, keypair: <KeypairType> }` in `fractalengine` crate.
- Add `app.insert_resource(NodeIdentity::new(keypair))` in `main.rs` after the keypair is loaded.
- `NodeIdentity::new` derives the `did:key` string from the public key (using the conversion from Task 1.4 — stub the conversion until Task 1.4 lands, or implement a minimal version here and refine in 1.4).
- Task 1.1 test must now pass (Green).

**Files touched:** `fractalengine/src/main.rs`, new `NodeIdentity` type (location TBD — `fractalengine/src/identity.rs` or a shared crate).

---

### Task 1.3 — did:key conversion function test (TDD Red)

Write tests for the canonical format conversion functions.

- Test: `did_key_from_public_key(known_pubkey)` returns the expected `did:key:z6Mk...` string.
- Test: `public_key_from_did_key("did:key:z6Mk...")` returns the expected public key bytes.
- Test: round-trip — `public_key_from_did_key(did_key_from_public_key(key)) == key`.
- Test: `public_key_from_did_key` returns `Err` on malformed input (not a `did:key`, truncated, etc.).
- Tests must fail (Red) because the functions do not exist yet.

**Files touched:** new test module alongside the conversion implementation location.

---

### Task 1.4 — Implement did:key conversion functions (TDD Green)

Implement `did_key_from_public_key` and `public_key_from_did_key`.

- `did_key_from_public_key(pub_key: &PublicKey) -> String`: encodes the raw public key bytes with multicodec prefix (`0xed01` for Ed25519) then base58btc-encodes with `z` prefix to produce `did:key:z6Mk...`.
- `public_key_from_did_key(did: &str) -> Result<PublicKey>`: reverses the encoding.
- Locate these in a place accessible to both `fe-sync` and `fractalengine` crates (e.g., a `fe-identity` utility crate or an existing shared module).
- All Task 1.3 tests must now pass (Green).
- Update `NodeIdentity::new` from Task 1.2 to use the real implementation.

**Files touched:** new conversion module, `Cargo.toml` dependency updates if a new crate is introduced.

---

### Task 1.5 — Replace all "local-node" strings (Refactor)

Grep the entire workspace for the literal string `"local-node"` and replace every occurrence with a real DID lookup via `NodeIdentity`.

- Run: `grep -rn '"local-node"' --include="*.rs"` to enumerate all sites.
- At each site, determine the correct access pattern: most will need `Res<NodeIdentity>` injected into the system, then `node_identity.did.clone()`.
- After replacement, `grep -rn '"local-node"'` must return zero results.
- Existing tests that used `"local-node"` as a placeholder must be updated to use a real (test-generated) DID string.

**Files touched:** multiple — all sites returned by the grep.

---

### Task 1.6 — Convert hex pub keys to did:key at RBAC/handshake boundaries (Refactor)

Identify all call sites where `hex::encode(pub_key)` is passed into role assignment, role lookup, or session management code, and replace with `did_key_from_public_key(pub_key)`.

- Run: `grep -rn 'hex::encode' --include="*.rs"` and filter to RBAC/handshake/session paths.
- At each site, replace `hex::encode(pub_key)` with `did_key_from_public_key(&pub_key)` (or equivalent).
- Verify: `verse_member.peer_did` column values continue to be valid `did:key` strings (not hex).
- After refactor, no `hex::encode` calls appear in RBAC, handshake, or session paths.

**Files touched:** `fe-sync` handshake code, RBAC call sites, session management — specific files determined by grep results.

---

### Task 1.7 — LocalUserRole resource population test (TDD Red)

Write a test asserting that `LocalUserRole` is populated from an RBAC query after startup.

- Set up a test Bevy app with a mock RBAC that returns a known role for the local DID.
- Run the `LocalUserRole`-populating system.
- Assert `app.world.get_resource::<LocalUserRole>()` returns `Some(LocalUserRole { role: Some(RoleId::Owner) })` (or whatever the mock returns).
- Test must fail (Red) because the system and resource do not exist yet.

**Files touched:** new test module.

---

### Task 1.8 — Implement LocalUserRole with hierarchical resolution and replace is_admin (TDD Green)

Implement the RBAC query system using hierarchical resolution, and replace the dead `is_admin: bool`.

- Define `LocalUserRole { role: Option<RoleLevel> }` as a Bevy `Resource`.
- Implement a Bevy system that: reads `Res<NodeIdentity>` for the local DID, determines the currently active scope (from `InspectedEntity` or active petal), calls `RoleManager::resolve_role(local_did, scope)` (stubbed initially — full RoleManager in Phase 2), and writes the result into `ResMut<LocalUserRole>`.
- Insert `LocalUserRole` into the Bevy world at startup with `role: None`.
- The system re-runs when the active scope changes (petal, fractal, or verse selection).
- In `InspectorFormState` (`fe-ui/src/plugin.rs`): remove `is_admin: bool`. Replace usages with a read of `Res<LocalUserRole>`.
- All Task 1.7 tests must now pass (Green).

**Files touched:** new `LocalUserRole` type, `fractalengine/src/main.rs` (resource insertion), `fe-ui/src/plugin.rs` (`is_admin` removal), RBAC system location.

---

### Task 1.9 — Define RoleLevel enum and reconcile role labels (Refactor)

Define the 5-level role hierarchy and ensure all UI displays derive from it.

- Define `RoleLevel` enum with variants: `Owner`, `Manager`, `Editor`, `Viewer`, `None`.
- Implement `Display` for UI rendering (`"owner"`, `"manager"`, `"editor"`, `"viewer"`, `"none"`).
- Implement `Ord` so `Owner > Manager > Editor > Viewer > None` for privilege comparison.
- Implement `From<&str>` and `Into<String>` for DB serialization.
- Audit UI code for hardcoded role label strings (the old `"owner"`, `"editor"`, `"public"` set). Replace with `role_level.to_string()`.
- Note: the old `"public"` role is renamed to `"viewer"` in the new model. Update any existing DB entries.
- Verify: role labels rendered in the UI match the enum `Display` output.

**Files touched:** `RoleLevel` type definition (shared module), UI rendering code, any code referencing the old `"public"` role string.

---

### Task 1.10 — Migrate role table schema to hierarchical scope (TDD Red -> Green)

Migrate the DB schema from `role(node_id, petal_id, role)` to `role(peer_did, scope, role)`.

- Write tests asserting:
  - A role can be created with scope `VERSE#abc123` (verse level).
  - A role can be created with scope `VERSE#abc123-FRACTAL#def456` (fractal level).
  - A role can be created with scope `VERSE#abc123-FRACTAL#def456-PETAL#ghi789` (petal level).
  - Query by `(peer_did, scope)` returns the correct role.
  - Old `(node_id, petal_id)` compound key queries no longer work.
- Implement the schema migration:
  - Replace `node_id` column with `peer_did` (using `did:key` format from Task 1.4).
  - Replace `petal_id` column with `scope` (Resource Descriptor string).
  - Migrate existing role entries: `(node_id, petal_id, role)` → `(did_key_from_hex(node_id), "VERSE#<verse_id>-FRACTAL#<fractal_id>-PETAL#<petal_id>", role)`.
  - Rename role value `"public"` → `"viewer"` in all existing entries.
- Update `fe-database/src/schema.rs` table definition.
- Update `rbac::get_role` and `rbac::require_write_role` to use the new schema (these will be replaced by RoleManager in Phase 2, but must still work during migration).
- All tests pass (Green).

**Files touched:** `fe-database/src/schema.rs`, `fe-database/src/rbac.rs`, migration code, `fe-auth/src/handshake.rs` (update role query call sites).

---

### Task 1.11 — Implement scope string parsing and building (TDD Red -> Green)

Implement the Resource Descriptor format utilities.

- Write tests:
  - `build_scope("verse123", None, None)` → `"VERSE#verse123"`.
  - `build_scope("verse123", Some("fractal456"), None)` → `"VERSE#verse123-FRACTAL#fractal456"`.
  - `build_scope("verse123", Some("fractal456"), Some("petal789"))` → `"VERSE#verse123-FRACTAL#fractal456-PETAL#petal789"`.
  - `parse_scope("VERSE#verse123-FRACTAL#fractal456-PETAL#petal789")` → `ScopeParts { verse_id: "verse123", fractal_id: Some("fractal456"), petal_id: Some("petal789") }`.
  - `parse_scope("invalid")` → `Err(...)`.
  - `parent_scope("VERSE#v-FRACTAL#f-PETAL#p")` → `"VERSE#v-FRACTAL#f"`.
  - `parent_scope("VERSE#v-FRACTAL#f")` → `"VERSE#v"`.
  - `parent_scope("VERSE#v")` → `None` (top level).
- Implement `build_scope`, `parse_scope`, `parent_scope` functions.
- All tests pass (Green).

**Files touched:** new scope module (e.g., `fe-database/src/scope.rs` or shared crate).

---

### Phase 1 Verification Checkpoint

Before moving to Phase 2:

1. `cargo test --workspace` passes with zero failures.
2. `grep -rn '"local-node"' --include="*.rs"` returns zero results.
3. `NodeIdentity` resource is inserted and readable in a running app (verify via test or manual startup log).
4. `LocalUserRole` is populated from RBAC query after active scope loads (verify via test).
5. No `hex::encode` calls remain in RBAC, handshake, or session management paths.
6. `RoleLevel` enum has 5 variants with `Display`, `Ord`, serialization.
7. Role table schema is migrated to `role(peer_did, scope, role)`.
8. Scope string parsing/building round-trips correctly.
9. Old `"public"` role values migrated to `"viewer"`.

---

## Phase 2: Peer Registry and Presence

**Resolves:** BLOCKER 1 (Peer Presence System), BLOCKER 3 (PeerAccessEntry Assembly), Decision 3 (PeerRegistry vs. Separate Resources)
**Unblocks:** FR-3, FR-4, FR-8, FR-9 in spec; Inspector Settings P4 Access tab with full peer management; Profile Manager P4 display name wiring

---

### Task 2.1 — PeerRegistry resource tests (TDD Red)

Write tests for the `PeerRegistry` resource and `PeerEntry` type.

- Test: `PeerRegistry::insert(entry)` stores a `PeerEntry` retrievable by `peer_did`.
- Test: `PeerRegistry::update(peer_did, |entry| ...)` modifies an existing entry.
- Test: `PeerRegistry::remove(peer_did)` removes the entry; subsequent `get` returns `None`.
- Test: `PeerRegistry::get(peer_did)` returns `None` for unknown peers.
- Test: default `PeerEntry` for a newly connected peer has `role: None`, `display_name: None`, `is_online: false`.
- Tests must fail (Red) — types do not exist yet.

**Files touched:** new test module.

---

### Task 2.2 — Implement PeerRegistry resource (TDD Green)

Implement `PeerRegistry` and `PeerEntry`.

- `PeerEntry { peer_did: String, role: Option<RoleId>, display_name: Option<String>, is_online: bool }`.
- `PeerRegistry` wraps a `HashMap<String, PeerEntry>` and exposes: `insert`, `update`, `remove`, `get`, `iter`.
- Implement as a Bevy `Resource`.
- Insert `PeerRegistry` into the Bevy world at startup with an empty map.
- All Task 2.1 tests must now pass (Green).

**Files touched:** new `PeerRegistry` type (location: shared crate or `fractalengine/src/peer_registry.rs`), `fractalengine/src/main.rs` (resource insertion).

---

### Task 2.3 — SyncEvent presence variant tests (TDD Red)

Write tests for the new `SyncEvent` variants.

- Test: `SyncEvent::PeerConnected { peer_id: String }` can be constructed and pattern-matched.
- Test: `SyncEvent::PeerDisconnected { peer_id: String }` can be constructed and pattern-matched.
- Test: a system consuming `EventReader<SyncEvent>` can distinguish `PeerConnected` from `PeerDisconnected`.
- Tests must fail (Red) — variants do not exist yet.

**Files touched:** new test module alongside `fe-sync`.

---

### Task 2.4 — Implement SyncEvent variants and iroh wiring (TDD Green)

Add the new event variants and hook them into iroh connection events.

- Add `PeerConnected { peer_id: String }` and `PeerDisconnected { peer_id: String }` to the `SyncEvent` enum in `fe-sync`.
- In the iroh connection event handler: fire `SyncEvent::PeerConnected { peer_id }` when a peer connects; fire `SyncEvent::PeerDisconnected { peer_id }` when a peer disconnects.
- The `peer_id` value must be a `did:key` string (use `did_key_from_public_key` from Task 1.4).
- All Task 2.3 tests must now pass (Green).
- No existing `SyncEvent` match arms should be broken — add `#[non_exhaustive]` or update all match sites.

**Files touched:** `fe-sync/src/lib.rs` or wherever `SyncEvent` is defined, iroh connection event handler.

---

### Task 2.5 — VersePeers population system tests (TDD Red)

Write tests for the system that populates `VersePeers` from `SyncEvent`.

- Test: when `SyncEvent::PeerConnected { peer_id }` fires, the system adds `peer_id` to `VersePeers`.
- Test: when `SyncEvent::PeerDisconnected { peer_id }` fires, the system removes `peer_id` from `VersePeers`.
- Test: `VersePeers` is empty before any events fire.
- Tests must fail (Red) — the population system does not exist yet (`VersePeers` is a stub).

**Files touched:** new test module.

---

### Task 2.6 — Implement VersePeers population and PeerRegistry.is_online feed (TDD Green)

Implement the system that drives `VersePeers` from presence events and feeds `PeerRegistry`.

- Implement a Bevy system that reads `EventReader<SyncEvent>` and for each:
  - `PeerConnected { peer_id }`: inserts `peer_id` into `VersePeers`, sets `PeerRegistry[peer_id].is_online = true` (creating the entry if absent with `role: None, display_name: None`).
  - `PeerDisconnected { peer_id }`: removes `peer_id` from `VersePeers`, sets `PeerRegistry[peer_id].is_online = false`.
- Remove (or document the removal of) the "Phase F stub: always empty" comment from `VersePeers`.
- All Task 2.5 tests must now pass (Green).

**Files touched:** `fe-sync/src/verse_peers.rs` (stub removal), new presence system, `PeerRegistry` update calls.

---

### Task 2.7 — PeerRegistry role population tests (TDD Red)

Write tests for the system that writes RBAC role data into `PeerRegistry`.

- Test: when RBAC data loads for a petal, peers with assigned roles have their `PeerRegistry[peer_did].role` set to the correct `RoleId`.
- Test: a peer with no assigned role has `role: None` in `PeerRegistry`.
- Test: when RBAC data reloads (petal change), `PeerRegistry` role values are updated, not duplicated.
- Tests must fail (Red) — the RBAC-to-PeerRegistry population system does not exist yet.

**Files touched:** new test module.

---

### Task 2.8 — Implement RBAC to PeerRegistry role population system (TDD Green)

Implement the system that reads RBAC data and writes it into `PeerRegistry`.

- Implement a Bevy system that: queries `fe-database` RBAC for all peer roles in the active petal scope, iterates the results, and for each `(peer_did, role)` pair calls `PeerRegistry.update` or `PeerRegistry.insert` to set `role: Some(role_id)`.
- System runs on petal load and on petal change.
- Peers in `PeerRegistry` from presence events (Task 2.6) but not in RBAC data keep `role: None` — they are online but have no assigned role.
- All Task 2.7 tests must now pass (Green).

**Files touched:** new RBAC-to-PeerRegistry system, RBAC query calls.

---

### Task 2.9 — Implement RoleManager resource (TDD Red -> Green)

Build the `RoleManager` Bevy resource that owns all role resolution and assignment logic.

- Write tests for:
  - `resolve_role(peer_did, petal_scope)`: walks petal → fractal → verse → default.
  - `resolve_role` returns `Owner` for `verse.owner_did` regardless of other assignments.
  - `resolve_role` returns `None` role when explicit `None` is set at a lower scope, even if higher scope grants access.
  - `resolve_role` falls back to `verse.default_access` when no explicit role exists.
  - `assign_role(caller_did, peer_did, scope, role)`: succeeds when caller is owner/manager at or above scope.
  - `assign_role` fails when caller has insufficient privilege.
  - `assign_role` writes to both `role` table and `op_log`.
  - `revoke_role(peer_did, scope)`: removes the role entry, peer falls back to inherited role.
  - `get_scope_members(scope)`: returns explicit + inherited members with resolved roles.
- Implement `RoleManager` with all methods.
- Insert as a Bevy `Resource` at startup.
- All tests pass (Green).

**Files touched:** new `RoleManager` type, `fractalengine/src/main.rs` (resource insertion), `fe-database/src/rbac.rs` (RoleManager uses the RBAC query functions internally).

---

### Task 2.10 — Implement invite generation with scope encoding (TDD Red -> Green)

Build scope-encoded invite generation and acceptance.

- Write tests for:
  - `generate_invite(creator_did, scope, role, expiry)` produces a valid invite payload.
  - Invite payload contains: scope (Resource Descriptor), role, creator_did, expiry, Ed25519 signature.
  - `accept_invite(invite_payload, acceptor_did)` creates a role assignment at the specified scope.
  - Expired invites are rejected.
  - Invalid signatures are rejected.
  - Creator must have `manager` or `owner` role at or above the target scope (verified via `RoleManager`).
  - Invite for verse scope grants verse-level role.
  - Invite for fractal scope grants fractal-level role.
  - Invite for petal scope grants petal-level role.
- Implement invite generation and acceptance in `RoleManager` (or a companion `InviteManager`).
- Link format: `fractalengine://invite/<base64url-encoded-payload>`.
- All tests pass (Green).

**Files touched:** new invite module, `fe-auth/src/verse_invite.rs` (extend or replace existing invite format), `fe-database/src/invite.rs`.

---

### Task 2.11 — Add verse default_access field (TDD Red -> Green)

Add the configurable default access setting to verses.

- Write tests:
  - Verse record has a `default_access` field (either `"viewer"` or `"none"`).
  - New verses default to `default_access: "viewer"`.
  - `RoleManager::resolve_role` uses `default_access` as final fallback when no explicit role exists.
  - `RoleManager::set_default_access(verse_id, "none")` changes the default.
- Implement `default_access` field on the verse record/schema.
- Update `RoleManager::resolve_role` to read `default_access` from the verse.
- All tests pass (Green).

**Files touched:** `fe-database/src/verse.rs` (schema update), `RoleManager`, verse creation code.

---

### Phase 2 Verification Checkpoint

Before declaring this track complete:

1. `cargo test --workspace` passes with zero failures.
2. `PeerRegistry` is populated correctly in tests: presence events set `is_online`, RBAC load sets `role`.
3. `SyncEvent::PeerConnected` and `SyncEvent::PeerDisconnected` variants exist and compile.
4. `VersePeers` "always empty" stub is replaced — presence events now populate it.
5. `RoleManager::resolve_role` correctly walks petal → fractal → verse → default.
6. Verse owner always resolves to `Owner` regardless of lower-level assignments.
7. `RoleLevel::None` at lower scope correctly blocks inherited access.
8. Invite generation produces scope-encoded links; acceptance creates correct role entries.
9. Verse `default_access` is configurable and used as fallback in resolution.
10. No per-frame DB queries in any system introduced by this track.

---

## Dependency Order

```
Phase 1:
1.1 → 1.2
1.3 → 1.4 → 1.2 (1.2 refines once 1.4 exists)
         └→ 1.5
         └→ 1.6
1.7 → 1.8 (requires NodeIdentity from 1.2)
1.9 (requires RoleLevel, builds on 1.8)
1.10 (requires did:key from 1.4, RoleLevel from 1.9)
1.11 (scope parsing — independent, can parallel with 1.10)

Phase 1 checkpoint

Phase 2:
2.1 → 2.2
2.3 → 2.4 (requires did:key from 1.4)
2.5 → 2.6 (requires SyncEvent from 2.4, PeerRegistry from 2.2)
2.7 → 2.8 (requires PeerRegistry from 2.2, RoleLevel from 1.9)
2.9 (RoleManager — requires scope from 1.11, schema from 1.10, RoleLevel from 1.9)
2.10 (invites — requires RoleManager from 2.9)
2.11 (default_access — requires RoleManager from 2.9)

Phase 2 checkpoint
```

---

## Handoff Contract to Downstream Tracks

When both phase checkpoints pass, the following are available for downstream tracks:

| Resource / Type | Available To |
|-----------------|-------------|
| `Res<NodeIdentity>` | Inspector Settings, Profile Manager |
| `Res<LocalUserRole>` | Inspector Settings (gates admin UI, uses hierarchical resolution) |
| `Res<PeerRegistry>` | Inspector Settings P4 (Access tab reads), Profile Manager P4 (writes display_name) |
| `Res<RoleManager>` | Inspector Settings P4 (role assignment, invite generation, peer management) |
| `SyncEvent::PeerConnected/Disconnected` | Profile Manager (presence-aware profile loading) |
| `RoleLevel` enum + `Display` + `Ord` | Inspector Settings (role label rendering, privilege comparison) |
| `did_key_from_public_key` | Both tracks (any new peer identifier handling) |
| `build_scope` / `parse_scope` / `parent_scope` | Inspector Settings (scope-aware Access tab), RoleManager internals |
| Invite generation/acceptance | Inspector Settings P4 (invite UI at each hierarchy level) |

Inspector Settings and Profile Manager may begin their P1-P3 work immediately — they do not depend on Phase 2 for early phases. They depend on both phases only at their respective Phase 4 integration tasks.
