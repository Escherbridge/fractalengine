# Specification: Shared Peer Infrastructure (Phase 0)

## Overview

Shared infrastructure that both the Inspector Settings track and the Profile Manager track depend on. Resolves phantom dependencies — infrastructure both specs assume exists but neither specifies.

This is a prerequisite track. Without it, both Inspector Settings and Profile Manager will encounter unowned blockers when they reach their Phase 4 integration. By completing this track first, both downstream tracks can execute P1-P3 fully in parallel and converge cleanly at Phase 4.

## Background

Cross-track alignment analysis (2026-04-19) deployed 10 research agents across 30+ codebase files spanning 8 crates. The analysis revealed a **phantom dependency layer**: infrastructure that both Inspector Settings and Profile Manager assume exists but neither specifies who builds it.

The ACH crucible evaluated 5 implementation strategies against 15 evidence items. The result was unambiguous: build shared infrastructure first, then both tracks in parallel, then a coordinated Phase 4.

The analysis identified 3 BLOCKERs and 5 design decisions that must be resolved before either downstream track can reach Phase 4. This track addresses all of them.

**Execution model:**

```
Phase 0 (this track)     ~1-2 sprints
    |
    +---> Inspector P1-P3   (fully independent, parallel)
    +---> Profile P1-P3     (fully independent, parallel)
    |
    v
Coordinated P4            ~1-2 sprints
```

## Functional Requirements

### FR-1: NodeIdentity Resource

**Resolves:** BLOCKER 2 (Admin Verification Dead Code)

**Description:** `NodeIdentity` must be inserted into the Bevy world in `main.rs` so that UI systems can read the local peer's DID. Currently, `NodeIdentity` is never inserted into the Bevy world — any system that tries to read it silently gets nothing, making all admin-gated UI permanently disabled regardless of the user's actual role.

The companion issue is that 14+ hardcoded `"local-node"` strings are scattered across the codebase as sentinel values. These are not peer identifiers; they are placeholders that were never replaced with actual DID values. All occurrences must be replaced with real DID lookups against `NodeIdentity`.

**Acceptance Criteria:**

- `app.insert_resource(NodeIdentity::new(keypair))` is called in `main.rs` during startup.
- `NodeIdentity` is a Bevy `Resource` holding at minimum the local keypair and the derived `did:key` string.
- A Bevy system can `Res<NodeIdentity>` to obtain the local DID without panicking.
- All 14+ hardcoded `"local-node"` strings are replaced. No literal `"local-node"` remains in source after this FR is complete.
- Tests assert the resource exists in the Bevy world after startup.

**Priority:** P0

---

### FR-2: Canonical Peer Identifier Format

**Resolves:** Decision 1

**Description:** Three peer identifier formats currently coexist in the codebase: `did:key:z6Mk...` (used in `verse_member.peer_did`), `hex::encode(pub_key)` (used in handshake and RBAC paths), and iroh `NodeId`. The `verse_member.peer_did` field establishes a clear precedent: `did:key` is the canonical format.

All peer identifier handling must be canonicalized at format boundaries. The conversion happens once, at the handshake/RBAC boundary, not scattered across the call stack.

**Acceptance Criteria:**

- A `did_key_from_public_key(pub_key: &PublicKey) -> String` conversion function exists and is tested.
- A `public_key_from_did_key(did: &str) -> Result<PublicKey>` inverse function exists and is tested.
- Hex-encoded public keys are converted to `did:key` format at the handshake/RBAC boundary. No `hex::encode(pub_key)` appears in role assignment, role lookup, or session management paths after this FR.
- All `"local-node"` sentinel values are replaced (see FR-1; this FR handles the identifier format at RBAC/handshake call sites specifically).
- The `verse_member.peer_did` column continues to hold valid `did:key` strings — this FR must not regress that field.

**Priority:** P0

---

### FR-3: PeerRegistry Resource

**Resolves:** BLOCKER 3 (PeerAccessEntry Assembly — No Owner) + Decision 3

**Description:** The Inspector Access tab needs a composite view of each peer: their role, their display name, and whether they are online. This data lives in three separate subsystems: role data from `fe-database` RBAC, display name from `PeerProfileCache` (which the Profile Manager track will build), and online status from `VersePeers`.

No spec owned the join. This FR defines it.

`PeerRegistry` is a unified Bevy resource following the `VerseManager` consolidation precedent. It holds a map of `peer_did -> PeerEntry`. Both the Inspector Settings track (writing role data) and the Profile Manager track (writing display name data) write to it. The Access tab reads the composite without performing any per-frame lookups.

**Acceptance Criteria:**

- `PeerRegistry` is a Bevy `Resource` containing a map of `peer_did: String -> PeerEntry`.
- `PeerEntry` holds: `peer_did: String`, `role: Option<RoleId>`, `display_name: Option<String>`, `is_online: bool`.
- `PeerRegistry` exposes `insert`, `update`, `remove`, and `get` operations.
- The resource is inserted into the Bevy world during startup with an empty map.
- `display_name` defaults to `None` — the Profile Manager track will populate it later. The Access tab must handle `None` gracefully (showing truncated `peer_did` as fallback).
- No per-frame DB queries originate from `PeerRegistry` or any system that reads it.
- Tests cover insert, update, remove, and get operations.

**Priority:** P0

---

### FR-4: Peer Presence System

**Resolves:** BLOCKER 1 (Peer Presence System — Unowned)

**Description:** Both the Inspector Access tab (`PeerAccessEntry.is_online`) and the Profile widget (online/offline indicator) consume per-peer presence data. Neither track produces it.

The current state: `VersePeers` at `fe-sync/src/verse_peers.rs` has the correct data structure but is documented as a "Phase F stub: always empty." `SyncEvent` has no `PeerConnected` or `PeerDisconnected` variants.

This FR adds the missing event variants and wires iroh connection events into the existing event/resource pattern. The stub becomes functional.

**Acceptance Criteria:**

- `SyncEvent::PeerConnected { peer_id: String }` variant is added.
- `SyncEvent::PeerDisconnected { peer_id: String }` variant is added.
- The iroh connection event handler fires `SyncEvent::PeerConnected` when a peer connects and `SyncEvent::PeerDisconnected` when a peer disconnects.
- A Bevy system consumes these events and updates `VersePeers` accordingly.
- `VersePeers` updates feed into `PeerRegistry.is_online` for the affected peer entries.
- `PeerRegistry` entries for peers that have never connected initialize with `is_online: false`.
- Tests verify: `PeerConnected` adds to `VersePeers`; `PeerDisconnected` removes; `PeerRegistry.is_online` reflects the current state.
- No per-frame polling. All updates are event-driven.

**Priority:** P0

---

### FR-5: LocalUserRole Resource

**Resolves:** BLOCKER 2 (Admin Verification Dead Code — continued)

**Description:** `InspectorFormState.is_admin` at `plugin.rs:167` defaults to `false` and is never set to `true`. Admin-gated UI is dead code. The root cause is that no system ever queries RBAC for the local user's role and writes the result anywhere readable by UI systems.

This FR adds that system and replaces the dead `bool` with a typed resource. The role lookup uses the hierarchical resolution chain provided by `RoleManager` (FR-8), resolving the effective role for the local user at whatever scope is currently active.

**Acceptance Criteria:**

- A Bevy system reads `Res<NodeIdentity>` for the local DID and the currently active scope (from `InspectedEntity` or active petal), then calls `RoleManager::resolve_role(local_did, scope)` to get the effective role.
- The result is written to a `LocalUserRole` Bevy resource holding `Option<RoleLevel>` — `None` means the role has not yet been resolved.
- `InspectorFormState.is_admin: bool` is replaced with `Option<RoleLevel>` (or removed in favor of a direct `Res<LocalUserRole>` read).
- The system re-queries when the active scope changes (petal, fractal, or verse selection).
- Tests assert that `LocalUserRole` is populated with the expected role after the system runs with a mock RBAC.
- Admin-gated UI reads from `LocalUserRole` and correctly enables/disables based on the actual resolved role.

**Priority:** P1

---

### FR-6: Hierarchical RBAC Scope Model

**Resolves:** Decision 2, expanded from petal-only to full hierarchy

**Description:** The RBAC system must support role assignments at every level of the entity hierarchy: verse, fractal, and petal. Roles cascade downward using a **most-specific-scope-wins** resolution strategy, with one exception: the verse owner always has full control and cannot be restricted at lower levels.

This follows the GitHub organization model: org owners are admin everywhere, team-level permissions apply to groups of repos, repo-level permissions are the most specific.

**Hierarchical Scope ID Format (Resource Descriptor):**

```
VERSE#<verse_id>                                         → verse scope
VERSE#<verse_id>-FRACTAL#<fractal_id>                    → fractal scope
VERSE#<verse_id>-FRACTAL#<fractal_id>-PETAL#<petal_id>   → petal scope
```

**Role Levels (ordered by privilege):**

| Role | Verse | Fractal | Petal | Capabilities |
|------|-------|---------|-------|-------------|
| **owner** | Yes | — | — | Full control everywhere, irrevocable, sets verse defaults |
| **manager** | — | Yes | Yes | Create child entities, assign roles within scope |
| **editor** | Yes | Yes | Yes | Edit content within scope |
| **viewer** | Yes | Yes | Yes | View-only access within scope |
| **none** | — | Yes | Yes | Explicit restriction — overrides inherited access |

**Resolution Algorithm:**

```
resolve_role(peer_did, petal_scope):
  if peer_did == verse.owner_did → owner (always, cannot be overridden)
  if petal-level role exists for peer_did → use it
  if fractal-level role exists for peer_did → use it
  if verse-level role exists for peer_did → use it
  → verse.default_access (configurable: "viewer" or "none")
```

**Configurable Default Access:**

The verse owner can set a `default_access` for the verse: either `viewer` (new members can see everything) or `none` (new members need explicit grants at fractal or petal level). This is stored on the verse record.

**DB Schema Change:**

The current `role(node_id, petal_id, role)` table is replaced with:

```
role(peer_did, scope, role)
```

Where `scope` is the hierarchical Resource Descriptor string (e.g., `VERSE#abc123-FRACTAL#def456-PETAL#ghi789`). This single table supports all hierarchy levels.

**Acceptance Criteria:**

- Role assignments can be created at verse, fractal, and petal scope levels.
- `resolve_role(peer_did, scope)` walks up the hierarchy: petal → fractal → verse → default.
- Verse owner always resolves to `owner` regardless of lower-level assignments.
- A `none` role at a lower level blocks inherited access (except for verse owner).
- The verse record stores a `default_access` field (`viewer` or `none`).
- The old `role(node_id, petal_id, role)` table is migrated to `role(peer_did, scope, role)`.
- `RoleLevel` enum is defined with `Owner > Manager > Editor > Viewer > None` ordering.
- Tests cover: cascade resolution, verse owner exception, explicit restriction via `none`, default access fallback.

**Priority:** P0

---

### FR-7: Role Label Reconciliation

**Resolves:** Archaeologist finding (UI displays hardcoded strings instead of actual DB values)

**Description:** The UI renders hardcoded role display strings that do not match the actual values stored in SurrealDB. With the expanded 5-level role system (`owner`, `manager`, `editor`, `viewer`, `none`), display strings must be derived from the `RoleLevel` enum, not independently hardcoded.

**Acceptance Criteria:**

- `RoleLevel` enum with variants: `Owner`, `Manager`, `Editor`, `Viewer`, `None`.
- `RoleLevel` implements `Display` for rendering and `Ord` for privilege comparison.
- `RoleLevel` implements `From<&str>` and `Into<String>` for DB serialization.
- UI displays actual DB role values as authoritative labels via `RoleLevel::display()`.
- Any existing hardcoded role label strings in UI code are replaced.
- Adding a new role level requires only adding a new enum variant — not hunting for hardcoded strings.

**Priority:** P1

---

### FR-8: RoleManager Resource

**Description:** A dedicated Bevy resource (following the VerseManager/NodeManager pattern) that owns all role resolution, assignment, and query logic. Other systems interact with roles exclusively through `RoleManager` — they never query the role table directly.

**Responsibilities:**

- **resolve_role(peer_did, scope)** — Walks the hierarchy to determine effective role. Returns `RoleLevel`.
- **assign_role(peer_did, scope, role)** — Assigns a role at a specific scope. Validates the caller has sufficient privilege (must be owner or manager at or above the target scope). Writes to DB + op_log.
- **revoke_role(peer_did, scope)** — Removes a role assignment at a specific scope. Peer falls back to inherited role.
- **get_scope_members(scope)** — Returns all peers with explicit role assignments at the given scope, plus inherited members with their resolved roles.
- **set_default_access(verse_id, default)** — Sets the verse-level default access (viewer or none).
- **parse_scope(scope_str)** — Parses a Resource Descriptor string into its hierarchy components.
- **build_scope(verse_id, fractal_id?, petal_id?)** — Builds a Resource Descriptor string from components.

**Acceptance Criteria:**

- `RoleManager` is a Bevy `Resource` inserted at startup.
- All role queries go through `RoleManager`, not direct DB queries.
- `resolve_role` implements the cascade algorithm with verse owner exception.
- `assign_role` validates caller privilege and writes to both the `role` table and `op_log`.
- `get_scope_members` returns the union of explicit and inherited members with resolved roles.
- Scope string parsing/building is tested with round-trip assertions.
- Tests cover: assignment, resolution, revocation, privilege validation, scope parsing.

**Priority:** P0

---

### FR-9: Invite and Link Generation

**Description:** Invite links encode the scope they grant access to using the same Resource Descriptor format. A verse-level invite grants the verse default access to all petals. A fractal-level invite grants access only to petals within that fractal. A petal-level invite grants access to that specific petal.

**Invite Link Format:**

```
fractalengine://invite/<base64url-encoded-payload>
```

Payload contains:
- `scope`: Resource Descriptor string (e.g., `VERSE#abc123-FRACTAL#def456`)
- `role`: The role being granted (e.g., `editor`)
- `creator_did`: The DID of the invite creator
- `expiry`: Timestamp
- `signature`: Ed25519 signature by the creator

**Acceptance Criteria:**

- Invite links can be generated at verse, fractal, or petal scope.
- The invite creator must have `manager` or `owner` role at or above the target scope.
- Accepting an invite creates a role assignment at the specified scope with the specified role.
- Expired invites are rejected.
- Invalid signatures are rejected.
- Invite generation and acceptance are handled by `RoleManager`.
- Tests cover: generation, acceptance, expiry, signature verification, privilege validation.

**Priority:** P1

---

## Non-Functional Requirements

### NFR-1: Performance

- No per-frame DB queries. `PeerRegistry` updates on events only (presence events from iroh, role load events from DB query completion).
- `LocalUserRole` re-queries on petal change events, not on every frame.
- The Access tab reads `PeerRegistry` directly — no render-time DB lookups.

### NFR-2: Code Coverage

- Greater than 80% test coverage for: `PeerRegistry` operations, the peer presence system (SyncEvent variants + VersePeers population), and identity resolution (`did_key_from_public_key`, `NodeIdentity` startup insertion, `LocalUserRole` population).

---

## Out of Scope

The following are explicitly not owned by this track:

- **Profile editing UI** — owned by the Profile Manager track. `PeerRegistry.display_name` will be `None` until Profile Manager P4 wires in `PeerProfileCache`.
- **Inspector tabs implementation** — owned by the Inspector Settings track. This track provides the data resources; Inspector Settings builds the UI that reads them.
- **P2P gossip for profile sync** — owned by Profile Manager P4. This track unstubs `VersePeers` via iroh connection events only; full gossip is Profile Manager's responsibility.
- **RBAC management UI** (role assignment dropdowns, permission presets, peer management panels) — owned by Inspector Settings P4. This track provides `RoleManager` as the backend; Inspector Settings builds the UI layer on top.
- **Identity revocation** — noted as a gap in the cross-track analysis, deferred to a future security track.
- **Profile cache eviction policy** (7-day/30-day stale model) — owned by Profile Manager track.

---

## Dependencies

| Depends On | Why |
|------------|-----|
| `fe-database` RBAC schema (`role` table) | FR-5, FR-7 read from it |
| `fe-sync` `SyncEvent` enum | FR-4 extends it |
| `fe-sync` `VersePeers` | FR-4 populates it |
| iroh connection event hooks | FR-4 wires into them |
| `NodeIdentity` keypair (already exists in `main.rs` init) | FR-1 inserts it as a resource |
| `verse` table (SurrealDB) | FR-6 reads `default_access` and `owner_did` from it |
| `fe-auth` invite system | FR-9 extends the existing invite format |

## Enables (Downstream Unblocked)

| Track | Unblocked Capability |
|-------|---------------------|
| Inspector Settings P1-P3 | Can proceed immediately after Phase 0 |
| Inspector Settings P4 | Access tab reads `PeerRegistry`; `RoleManager` handles assignment; `LocalUserRole` gates admin UI; peer management at all hierarchy levels |
| Profile Manager P1-P3 | Can proceed immediately after Phase 0 |
| Profile Manager P4 | Writes `display_name` into `PeerRegistry`; reads `NodeIdentity` for local profile |
