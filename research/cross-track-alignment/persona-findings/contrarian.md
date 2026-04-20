# Contrarian Critique: Cross-Track Alignment Analysis

## Inspector Settings (inspector_settings_20260419) x Profile Manager (profile_manager_20260419)

---

## 1. TEMPORAL PARADOX: Profile Widget Needs a Role That Nobody Computes

**Severity: BLOCKER**

Profile Manager FR-1 states:

> "Current role for the active petal (if a petal is selected), e.g., 'admin', 'viewer'."

Profile Manager Plan Task 1.4 says:

> "Wire the role to `InspectorFormState.is_admin` (or the active petal's role)."

There are two fatal problems here.

**Problem A: `is_admin` is a dead field.** In the actual codebase, `InspectorFormState.is_admin` (`fe-ui/src/plugin.rs:159`) defaults to `false` and is **never written to `true` anywhere**. A grep for `.is_admin` across the entire codebase returns only reads at `plugin.rs:443` and `panels.rs:965`, plus the `false` default at `plugin.rs:170`. No system, handshake handler, or startup logic ever sets it. The Profile Manager plan proposes wiring to a field that is permanently `false`. This is not a "wire it up" task -- this is a "design and implement an entire role-resolution pipeline from scratch" task that neither spec acknowledges.

**Problem B: `get_role()` takes `node_id`, not `peer_id`.** The RBAC function at `fe-database/src/rbac.rs:9` has signature `get_role(db: &Db, node_id: &str, petal_id: &str)`. The `node_id` parameter in the role table (`fe-database/src/schema.rs:132-136`) refers to the entity node, not the peer/user. Meanwhile, the handshake at `fe-auth/src/handshake.rs:13` passes `hex::encode(peer_pub_key.to_bytes())` as the `node_id`. This is a naming collision between "node" as an entity in the scene graph and "node" as a peer identity. The Profile Manager spec assumes it can call something like `get_role(db, my_did_key, active_petal_id)` but the existing function uses hex-encoded raw public key bytes, not DID key format. The Profile Manager defines identity as `did:key:z6Mk...` strings. These are different string formats for the same key -- an impedance mismatch that will silently return "public" for every lookup.

**Resolution direction:** Both specs need a shared "role resolution service" design. This service must: (a) canonicalize identity format (hex bytes vs. DID key), (b) be callable synchronously from the UI thread (the current `get_role` is async and lives behind a DB channel), and (c) actually wire `is_admin` or a replacement to the result. This is a shared infrastructure task that belongs to neither track individually.

---

## 2. DATA OWNERSHIP CONFLICT: The Orphaned Join

**Severity: BLOCKER**

Inspector Phase 4 defines:

> `PeerAccessEntry { peer_id, display_name, role, is_online }`

Profile Manager Phase 4 defines:

> `UserProfile { display_name, public_key, avatar_index }` and `PeerProfileCache`

Profile Manager Plan Task 4.6 says:

> "Modify the Access tab rendering (from Track: Inspector Settings) to look up display names from `PeerProfileCache`."

But Inspector Plan Task 4.3 says:

> "Define `DbCommand::GetEntityPeerRoles { entity_type, entity_id }` ... Implement the command send on Access tab activation."

This means Inspector produces a list of `(peer_id, role)` tuples from the role table, and Profile Manager produces a `PeerProfileCache` of `(public_key, display_name, avatar_index)`. But **nobody specifies the code that joins these two data sources into `PeerAccessEntry`**.

Inspector Task 4.4 says "Render each `PeerAccessEntry` as a row" -- but Task 4.1-4.3 only define the struct and the DB query for roles. The struct includes `display_name` and `is_online`, but those fields are not populated by the DB query. The Profile Manager's Task 4.6 says "modify the Access tab rendering to look up display names" -- but this is a rendering modification, not a data assembly step. Who constructs the `PeerAccessEntry` with all four fields populated?

This is an orphan requirement. The join logic -- "for each role entry, look up the peer's display name in `PeerProfileCache` and determine online status" -- exists in neither plan. It falls through the gap between "Inspector builds the Access tab" and "Profile Manager wires in display names."

**Resolution direction:** One track (Profile Manager is the natural owner since it owns `PeerProfileCache`) must define an explicit "assemble `PeerAccessEntry` list" system/function that takes role data from RBAC + profile data from cache + presence data from somewhere, and produces the fully populated list. This should be a named task, not an implied side-effect of Task 4.6's vague "modify the rendering."

---

## 3. ONLINE/OFFLINE STATE VOID: Nobody Owns Per-Peer Presence

**Severity: HIGH**

Inspector spec FR-4 says:

> "Each entry shows: peer display name (or truncated key if no name), role, **online/offline indicator**."

Profile Manager FR-1 says:

> "An **online/offline** status indicator (green dot = connected to at least one peer, gray dot = no peers)."

These are fundamentally different concepts. The Profile Manager's online/offline is **self-status** (am *I* connected to anyone?). The Inspector's online/offline is **per-peer status** (is *this specific peer* currently connected?).

The existing codebase has:
- `SyncStatus` (`fe-sync/src/status.rs:20-28`): tracks `online: bool` (local node) and `peer_count: usize` (aggregate).
- `SyncEvent` (`fe-sync/src/messages.rs:54-65`): only `Started { online }`, `BlobReady`, `Stopped`. No per-peer connect/disconnect events.
- `VersePeers` (`fe-sync/src/verse_peers.rs:14-17`): has a `HashMap<String, HashSet<String>>` of verse-to-peer-set, but the comment on line 13 says "Phase F stub: always empty. Will be populated by gossip presence messages."

So `VersePeers` is an empty stub. There is no mechanism to detect when a specific peer connects or disconnects. The `SyncEvent` enum has no `PeerConnected`/`PeerDisconnected` variants. The gossip system (`fe-network/src/gossip.rs:1`) has a TODO comment: "Wire gossip to the running swarm handle once it is accessible from this context."

Both specs assume per-peer presence data will exist, but neither spec owns creating it. The Profile Manager spec says P2P sync happens "within 5 seconds" but does not define heartbeat, presence, or disconnect detection. The Inspector spec just reads the `is_online` field without specifying its source.

**Resolution direction:** A presence tracking system must be designed as explicit shared infrastructure. This includes: (a) `SyncEvent::PeerConnected { node_id }` and `SyncEvent::PeerDisconnected { node_id }` variants, (b) a `PeerPresence` Bevy resource mapping peer IDs to last-seen timestamps, (c) timeout-based disconnect detection. This is a prerequisite for both tracks' Phase 4 and must be built first or specified as a dependency.

---

## 4. IDENTITY SWITCHING BOMB: Roles, Replicas, and Iroh NodeId Cascade

**Severity: HIGH**

Profile Manager FR-3 says:

> "Switching identity requires confirmation: 'This will disconnect from all peers. Continue?'"
> "After switching, the application reconnects with the new identity."

Profile Manager NFR-1 says:

> "Identity switching clears all session state (JWT tokens, cached peer sessions)."

The existing code reveals the depth of this problem:

- `NodeIdentity` (`fe-identity/src/resource.rs:6-8`) is a Bevy `Resource` (singleton). It stores one keypair and one `did_key` string.
- `NodeKeypair::to_iroh_secret()` (`fe-identity/src/keypair.rs:45-47`) derives the iroh `SecretKey` from the ed25519 seed. The iroh `NodeId` (which identifies this node in the P2P network) is derived from this secret.
- The sync thread (`fe-sync/src/sync_thread.rs`) uses this iroh secret to create the iroh endpoint at startup.

Switching identity means: a new ed25519 keypair, a new iroh `SecretKey`, a new iroh `NodeId`, and therefore all active iroh-docs replicas become invalid (they are bound to the original `NodeId`). The spec says "reconnects" but this is not a reconnect -- it is a **complete teardown and rebuild** of the iroh endpoint and all replicas.

But more critically: **roles assigned to the old identity are orphaned**. The role table (`fe-database/src/schema.rs:132-136`) stores `(node_id, petal_id, role)` where `node_id` is the hex-encoded public key (per `fe-auth/src/handshake.rs:13`). When the user switches to a new keypair, their new `node_id` has no role entries. They lose admin access to their own petals. The spec does not address this at all.

Are roles meant to follow the *person* or the *key*? If the person, you need a migration mechanism. If the key, the user needs to be warned that switching identity means losing all permissions. The spec says "requires confirmation" but the confirmation message only mentions peer disconnection, not permission loss.

**Resolution direction:** The spec must decide: (a) identity switching is rare and destructive (document the permission loss clearly, warn the user), or (b) roles should be migratable (add a "transfer roles to new identity" operation, which itself requires proving ownership of both keys). Option (a) is simpler and probably correct for v1, but the confirmation dialog text must enumerate the consequences.

---

## 5. STALE PROFILES VS. PERSISTENT ROLES: The Ghost Peer Problem

**Severity: MEDIUM**

Profile Manager FR-4 says:

> "Stale peer profiles (from peers not seen in 7 days) are evicted from the cache."

But roles in SurrealDB persist indefinitely. The role table has no TTL, no eviction, no "last seen" field (`fe-database/src/schema.rs:132-136`).

Inspector FR-4 says:

> "The Access tab lists **all peers with access** to the selected entity."

This means the Access tab queries the role table, which returns peers who were assigned roles months ago. For each such peer, it attempts to look up the display name in `PeerProfileCache`. After 7 days of absence, the cached profile is evicted. The result: the Access tab shows entries with truncated keys instead of names for any peer who has not been online recently.

The Inspector spec says "peer display name (or truncated key if no name)" -- so there is a fallback. But this is a **degraded experience by default** for any peer list older than 7 days. In practice, most Access tab entries will show truncated keys because profile eviction is aggressive (7 days) while role persistence is permanent.

Worse: there is no way to distinguish "peer has no display name" from "peer had a display name but the cache expired." The user sees the same truncated key in both cases.

**Resolution direction:** Either (a) increase the profile cache TTL significantly (30+ days) or make it role-aware (never evict profiles for peers who have active role assignments), or (b) persist profile data in SurrealDB alongside roles (not just as a volatile cache), so display names survive eviction. Option (b) is cleaner -- a `peer_profiles` table that stores the last-known display name and is updated on `ProfileAnnounce` receipt, never evicted.

---

## 6. ADMIN VERIFICATION BLACK BOX: Trust Without Verification

**Severity: HIGH**

Inspector FR-4 says:

> "Admins see a dropdown next to each peer to change their role."
> "Non-admin users see the role list as read-only (no dropdowns)."

The current admin check: `InspectorFormState.is_admin` (`fe-ui/src/plugin.rs:159`) is a plain `bool` that defaults to `false` and is never set to `true` (as established in Issue 1).

But even if it were set, the mechanism is deeply suspect. The handshake (`fe-auth/src/handshake.rs:6-22`) works like this: when a peer connects, the **host** looks up the peer's role via `get_role()` and mints a JWT token containing that role. The host sends the token back to the peer. The peer's role is determined **by the host**, not by the peer.

Now consider the Inspector Access tab scenario: the local user opens the Access tab for their own petal. To determine if *they* are admin, the system needs to check the local user's role for that petal in the local database. This is straightforward for the petal owner -- but the spec does not distinguish between "inspecting my own petal" (where I query my local DB) and "inspecting someone else's petal" (where I received my role via JWT from their host).

For remote petals, the local user's admin status was determined by the remote host during handshake. But the JWT token is in `SessionCache` (`fe-auth/src/cache.rs`), which maps `[u8; 32]` (peer public key) to `RoleId`. The `is_admin` field on `InspectorFormState` has no connection to `SessionCache`. There is no system that reads the cache, extracts the role, and sets `is_admin`.

More critically: can a malicious peer claim admin by sending a forged JWT? The host issues JWTs, but if a peer modifies their own `Session.role` locally (it is just a `pub` field on a struct at `fe-auth/src/session.rs:3-7`), the UI will show admin controls. Role changes sent from this "fake admin" would need to be **validated server-side** (by the host), but neither spec mentions server-side validation of role-change commands.

**Resolution direction:** (a) Implement the system that populates `is_admin` from either the local DB (for owned entities) or the session cache (for remote entities). (b) All role-change operations must be validated by the entity host, not trusted from the requesting peer. The Inspector spec should explicitly state that `DbCommand::UpdatePeerRole` is validated against the requester's role before execution.

---

## 7. AVATAR GAP: Tracked but Never Displayed Where It Matters

**Severity: LOW**

Profile Manager defines `UserProfile { display_name, public_key, avatar_index: u8 }` and `ProfileAnnounce { public_key, display_name, avatar_index, timestamp, signature }`.

Inspector FR-4 defines `PeerAccessEntry { peer_id, display_name, role, is_online }`.

Notice: **no `avatar_index` in `PeerAccessEntry`**. The Profile Manager broadcasts avatars to all peers. The Inspector never displays them. The Profile Manager's own widget shows the local user's avatar, but the place where you actually see *other people* (the Access tab) does not use it.

This is either a spec omission in Inspector (if avatars should show in the Access tab) or wasted bandwidth in Profile Manager (if they should not). The 512-byte P2P message limit (`ProfileAnnounce`) includes `avatar_index` -- it is small, but the design intent is unclear.

**Resolution direction:** Either add `avatar_index: Option<u8>` to `PeerAccessEntry` and render it in the Access tab, or document that avatar is local-UI-only and remove it from `ProfileAnnounce` to save complexity. The former is almost certainly the right call -- showing a peer's chosen avatar in the Access tab is a natural UX expectation.

---

## 8. PERMISSION PRESETS SCOPE AMBIGUITY: Cascade Semantics Undefined

**Severity: MEDIUM**

Inspector FR-4 says:

> "Quick-set buttons for common configurations: 'Public' (everyone can view), 'Members Only' (only assigned peers), 'Admin Only' (only admins)."
> "Presets modify role assignments for the entity scope."

Inspector FR-3 establishes that entities can be inspected at four hierarchy levels: Node, Petal, Fractal, Verse.

The role table (`fe-database/src/schema.rs:132-136`) stores `(node_id, petal_id, role)` -- it only has a `petal_id` scope. There is no `fractal_id` or `verse_id` column. You cannot express "this peer has admin role at the Verse level" in the current schema.

So when the user selects a Verse in the inspector and opens the Access tab, what roles are shown? The spec says "all peers with access to the selected entity" -- but the role table only stores petal-level roles. Does Verse-level access mean "has a role in any petal within this verse"? Does Fractal-level mean "has a role in any petal within this fractal"?

And when the user clicks "Admin Only" on a Verse, what happens? Does it:
(a) Set all petals in the verse to admin-only? (cascade down)
(b) Set a verse-level role that does not exist in the current schema?
(c) Do nothing useful because the schema does not support it?

The plan's Task 4.6 says "clicking 'Public' preset sets all peer roles to viewer-equivalent" -- but for which entity? For a petal, this is clear. For a verse, it is undefined.

**Resolution direction:** Either (a) restrict presets to petal-level only (the only level the role schema supports), and show "Access control is managed at the petal level" for verse/fractal inspection, or (b) extend the role table schema to include `entity_type` and `entity_id` columns, enabling role assignments at any hierarchy level. Option (a) is pragmatic for v1; option (b) is correct long-term but is a schema migration that affects both tracks.

---

## Summary of Findings

| # | Issue | Severity | Core Problem |
|---|-------|----------|--------------|
| 1 | Temporal Paradox: Role display before role resolution exists | BLOCKER | `is_admin` is dead code; role format mismatch (hex vs DID) |
| 2 | Data Ownership: PeerAccessEntry assembly is unspecified | BLOCKER | Neither track owns the join between roles + profiles + presence |
| 3 | Presence Void: No per-peer online/offline tracking | HIGH | `SyncEvent` has no peer-level events; `VersePeers` is an empty stub |
| 4 | Identity Switching: Roles orphaned on keypair change | HIGH | Role table keyed on public key; new key = no permissions |
| 5 | Stale Profiles: 7-day cache eviction vs. permanent roles | MEDIUM | Access tab degrades to truncated keys after one week |
| 6 | Admin Verification: No actual admin check; no server-side validation | HIGH | `is_admin` is never set; role changes not validated at host |
| 7 | Avatar Gap: Tracked in profiles, absent from Access tab | LOW | Spec inconsistency; wasted P2P bandwidth |
| 8 | Preset Scope: Role schema is petal-only; presets imply hierarchy | MEDIUM | Cannot express verse/fractal-level roles in current schema |

**Bottom line:** These two tracks share a phantom dependency layer that neither fully specifies. The specs assume infrastructure (role resolution, presence tracking, data assembly, admin verification) that does not exist and is not owned by either track. Before either track reaches Phase 4, a shared "Identity & Presence Infrastructure" task must be scoped and built.

---

**Key files referenced in this analysis:**

- `/fractalengine/fe-ui/src/plugin.rs:159` -- `InspectorFormState.is_admin` (dead field)
- `/fractalengine/fe-database/src/rbac.rs:9-29` -- `get_role()` function and its `node_id` parameter
- `/fractalengine/fe-database/src/schema.rs:132-136` -- Role table schema (petal-scoped only)
- `/fractalengine/fe-auth/src/handshake.rs:6-22` -- Handshake flow using hex-encoded public key
- `/fractalengine/fe-auth/src/session.rs:3-7` -- Session struct with `pub` role field
- `/fractalengine/fe-identity/src/resource.rs:6-28` -- `NodeIdentity` singleton resource
- `/fractalengine/fe-identity/src/keypair.rs:45-47` -- `to_iroh_secret()` coupling keypair to iroh NodeId
- `/fractalengine/fe-sync/src/messages.rs:54-65` -- `SyncEvent` enum (no per-peer events)
- `/fractalengine/fe-sync/src/status.rs:20-28` -- `SyncStatus` (aggregate peer_count only)
- `/fractalengine/fe-sync/src/verse_peers.rs:13-17` -- `VersePeers` (empty stub)
- `/fractalengine/fe-network/src/gossip.rs:1` -- Gossip module (TODO stub)
