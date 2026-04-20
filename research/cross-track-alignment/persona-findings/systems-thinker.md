# Systems Thinker: Cross-Track Alignment Analysis

## Complete Data Flow, Dependency Chain, and Feedback Loop Analysis

---

## 1. SYSTEM MAP: Complete Data Flows Between Tracks

### 1A. Identity Foundation Layer (Existing)

```
fe-identity/src/keypair.rs      NodeKeypair (ed25519 seed, 32 bytes)
        |
        +---> fe-identity/src/resource.rs    NodeIdentity (Bevy Resource: keypair + did_key string)
        +---> fe-identity/src/keychain.rs    OS keychain storage (service="fractalengine", username=node_id)
        +---> fe-identity/src/did_key.rs     did:key:z6Mk... format conversion
        +---> NodeKeypair::to_iroh_secret()  iroh::SecretKey for P2P endpoint identity
```

**Critical evidence**: `NodeKeypair::to_iroh_secret()` at `fe-identity/src/keypair.rs:45-47` binds the ed25519 keypair directly to the iroh `NodeId`. The iroh identity IS the ed25519 identity. This is the single most important architectural fact for the cross-track analysis.

### 1B. Planned Cross-Track Data Flows

```
PROFILE MANAGER                         INSPECTOR SETTINGS
-------------------                     --------------------

UserProfile.display_name                PeerAccessEntry.display_name
    |                                       ^
    | UiAction::SaveProfile                 | lookup from PeerProfileCache
    v                                       |
DbCommand::UpdateProfile --> profiles   PeerProfileCache (Bevy Resource)
    |                                       ^
    v                                       |
ProfileAnnounce (gossip) ------P2P-------->  ProfileAnnounce receiver
    [pub_key, display_name,                  [verify sig, update cache]
     avatar_index, timestamp, sig]

RBAC system (shared by both tracks)
    |
    +---> fe-database/src/rbac.rs:9       get_role(db, node_id, petal_id) -> RoleId
    +---> fe-auth/src/handshake.rs:15     connect() uses get_role() for session JWT
    +---> fe-auth/src/cache.rs            SessionCache keyed on [u8; 32] pub_key
    +---> fe-ui/src/plugin.rs:159         InspectorFormState.is_admin (boolean)
    +---> fe-ui/src/role_chip.rs:6        role_chip_hud() renders "admin"/"member"
```

### 1C. Missing Data Paths (Neither Track Builds)

| Data Path | Required By | Currently Exists | Who Should Build |
|-----------|-------------|------------------|------------------|
| Per-peer online/offline state | Access tab `PeerAccessEntry.is_online`, Profile widget online dot | `VersePeers` (`fe-sync/src/verse_peers.rs`) is a stub -- always empty | **Neither track specifies this** |
| Peer count per verse | Verse Info tab (Inspector) | `SyncStatus.peer_count` exists but is global, not per-verse | Gap |
| Local user's role for active petal | Profile widget role chip, Access tab admin gating | `InspectorFormState.is_admin` exists as hardcoded boolean | Partially Inspector, partially Profile |
| `ProfileState.is_connected` | Profile widget | No resource exists yet | Profile Manager |
| Avatar rendering in peer list | Access tab peer entries | Not referenced in Inspector spec | **Unowned** |

---

## 2. DEPENDENCY CHAIN ANALYSIS

### 2A. Rendering a Single Row in the Access Tab

This is the most revealing dependency chain because it requires data from both tracks simultaneously:

```
Access Tab Row
  |
  +-- peer display_name -------> PeerProfileCache.get(peer_did) -----> ProfileAnnounce gossip received
  |                                                                      |
  |                                                 DEPENDENCY: Profile Manager Phase 4 (Task 4.5)
  |
  +-- peer role ----------------> RBAC query get_role(node_id, petal_id)
  |                               |
  |                               Currently: fe-database/src/rbac.rs:9-29
  |                               node_id = hex::encode(peer_pub_key.to_bytes())  [fe-auth/src/handshake.rs:13]
  |                               petal_id = active petal from NavigationManager
  |                               |
  |                               DEPENDENCY: Inspector Phase 4 (Task 4.3) -- DbCommand::GetEntityPeerRoles
  |
  +-- peer is_online ------------> VersePeers.get_peers(verse_id).contains(peer_node_id)
  |                               |
  |                               Currently: fe-sync/src/verse_peers.rs:14 -- STUB, always empty
  |                               Comment on line 6: "Phase F stub: always empty"
  |                               |
  |                               DEPENDENCY: **UNOWNED** -- no track builds this
  |
  +-- admin can change role -----> Local user's role >= admin for this entity
                                   |
                                   Currently: InspectorFormState.is_admin (plugin.rs:159)
                                   Set to hardcoded value, never dynamically computed
                                   |
                                   Needs: get_role(local_did, entity_petal_id) at startup/petal-switch
                                   |
                                   DEPENDENCY: Both tracks partially -- Profile provides local DID,
                                               Inspector provides the query trigger
```

### 2B. Profile Sync Round-Trip Chain

```
1. User edits display name in profile panel
2. validate_display_name() -- Profile Phase 2, Task 2.2
3. UiAction::SaveProfile -- Profile Phase 2, Task 2.5
4. process_ui_actions handles SaveProfile -- NEW: not in current UiAction enum (plugin.rs:8-18)
5. DbCommand::UpdateProfile sent to DB thread -- NEW: not in current DbCommand enum (messages.rs:17-70)
6. DB thread persists to `profiles` table -- NEW: table does not exist in schema.rs
7. ProfileAnnounce gossip broadcast via iroh-gossip -- Profile Phase 4, Task 4.4
8. Peer receives ProfileAnnounce -- Profile Phase 4, Task 4.5
9. PeerProfileCache updated -- NEW: resource does not exist
10. Access tab re-renders with new name -- Inspector Phase 4 + Profile Phase 4 Task 4.6
```

Steps 4-10 all require new infrastructure. The critical insight is that step 7 depends on a gossip broadcast mechanism that currently only exists as a stub (`fe-network/src/gossip.rs:38-42` -- `broadcast()` is a no-op that logs and returns `Ok`).

### 2C. Temporal Ordering Constraints

The dependency graph reveals a strict partial ordering:

```
Profile P1 (ProfileState, UserProfile)  ---must precede---  Inspector P4 (Access tab rendering)
Profile P2 (SaveProfile action)         ---must precede---  Profile P4 (DB persist + gossip)
Profile P4 (PeerProfileCache)           ---must precede---  Inspector P4 Task 4.4 (render peer names)

Inspector P1 (URL persistence)          ---independent of---  Profile P1-P3
Inspector P2 (tab system)               ---independent of---  Profile P1-P3
Inspector P3 (hierarchy inspection)     ---independent of---  Profile P1-P3
Inspector P4 (Access tab)               ---depends on---     Profile P4 (PeerProfileCache) for display names

Profile P3 (identity management)        ---independent of---  Inspector P1-P3
```

The two tracks are fully independent through their first three phases. The coupling concentrates entirely in Phase 4 of both tracks.

---

## 3. FEEDBACK LOOPS

### 3A. Reinforcing Loop: Profile-Access Sync

```
Profile Edit --> SaveProfile --> DB persist --> ProfileAnnounce gossip
                                                     |
                                                     v
                                            Peers update PeerProfileCache
                                                     |
                                                     v
                                            Access tab re-renders with new name
                                                     |
                                                     v
                                            Admin sees updated name, might change role
                                                     |
                                                     v (reinforcing)
                                            Role change improves/restricts peer access
```

This is a **reinforcing** loop: better profile data leads to more informed access decisions, which leads to a better-managed system. The risk is that the loop has a **broken link** at the gossip broadcast step (currently a no-op stub).

### 3B. Balancing Loop: Role Change Propagation Failure

```
Admin changes peer role in Access tab
    |
    v
DbCommand::UpdatePeerRole --> local DB updated
    |
    v
??? P2P propagation --> EXPLICITLY OUT OF SCOPE (Inspector spec line 153)
    |
    v
Remote peer's Access tab does NOT update
    |
    v
Remote peer sees stale role data --> acts on incorrect permissions
    |
    v (balancing/destabilizing)
System state diverges between peers
```

This is a **destabilizing** feedback loop. The Inspector spec explicitly defers P2P role propagation (line 153: "P2P propagation of role changes (deferred to the Mycelium Live track)"), but the Profile Manager spec builds P2P profile propagation. This creates an asymmetry: profiles sync across peers, but roles do not. Users will see their peers' correct names but incorrect roles.

### 3C. Catastrophic Loop: Identity Switch Cascade

```
User switches identity (Profile P3)
    |
    v
New keypair --> new did:key --> new iroh::SecretKey (keypair.rs:45-47)
    |
    +---> iroh endpoint node_id changes --> ALL peer connections drop
    +---> Session JWT invalid (signed by old key) --> handshake.rs must re-auth
    +---> SessionCache entries (keyed on [u8;32] pub_key, cache.rs:8) --> all stale
    +---> RBAC role table (schema.rs:132, keyed on node_id=hex(old_pub_key)) --> orphaned
    +---> VersePeers entries (verse_peers.rs:26, keyed on old node_id string) --> stale
    +---> PeerProfileCache entries (not yet built) --> old identity evicted, new one unknown
    +---> Other peers' Access tabs --> show old identity as offline, new one as unknown newcomer
```

This cascade is the single most dangerous emergent property of the combined system. The Profile Manager spec acknowledges this partially (FR-3 line 82: "Switching identity requires confirmation: 'This will disconnect from all peers.'") but does not address the role orphaning problem.

---

## 4. EMERGENT PROPERTIES

### 4A. The Composite Peer View Problem

Neither track fully owns the "peer entry" that the Access tab renders. The `PeerAccessEntry` struct defined in the Inspector spec contains:

```rust
PeerAccessEntry {
    peer_id: String,       // From: iroh NodeId / did:key  (fe-identity)
    display_name: String,  // From: PeerProfileCache       (Profile Manager)
    role: String,          // From: RBAC table              (fe-database)
    is_online: bool,       // From: presence system         (UNOWNED)
}
```

This struct depends on three separate subsystems, one of which does not exist. The assembly logic -- who creates these entries and keeps them current -- is specified by neither track.

### 4B. The Phantom Presence System

Both tracks assume a presence system exists:
- Inspector spec FR-4 (line 83): "Each entry shows: peer display name, role, **online/offline indicator**"
- Profile spec FR-1 (line 25): "An **online/offline status indicator** (green dot = connected to at least one peer)"
- `VersePeers` (`fe-sync/src/verse_peers.rs:14`) is declared as a Resource but documented as "Phase F stub: always empty"
- `SyncStatus.online` (`fe-sync/src/status.rs:23`) tracks whether the LOCAL node is online, not per-peer state

Neither track builds a per-peer presence tracker. The `VersePeers` stub would need to be populated by gossip heartbeats, connection events, or iroh endpoint peer discovery -- none of which are implemented or specified.

### 4C. The Multi-Identity Role Fragmentation

When the Profile Manager's Phase 3 (IdentityList) is combined with the RBAC system:

- The `role` table (`schema.rs:132-137`) keys on `node_id` (a string, which is `hex::encode(pub_key.to_bytes())` per `handshake.rs:13`)
- Switching identity changes the public key, which changes the `node_id`
- Previous role assignments are keyed on the OLD `node_id` and remain in the database
- The new identity starts as "public" (default role, `rbac.rs:28`)
- There is no mechanism to transfer roles from one identity to another

This means: an admin who switches identity loses their admin role. An admin who creates a second identity cannot make it admin without help from another admin.

---

## 5. LEVERAGE POINTS

### Leverage Point 1: Unified `PeerRegistry` Resource

A single Bevy resource combining all per-peer data would resolve the ownership gap:

```rust
#[derive(Resource, Default)]
pub struct PeerRegistry {
    peers: HashMap<String, PeerInfo>,  // keyed on did:key or hex(pub_key)
}

pub struct PeerInfo {
    pub did_key: String,
    pub display_name: Option<String>,
    pub avatar_index: Option<u8>,
    pub role: Option<RoleId>,        // per current entity scope
    pub is_online: bool,
    pub last_seen: Instant,
}
```

**Impact**: This resolves the "who assembles PeerAccessEntry" question, eliminates the need for Access tab code to query three separate systems, and provides a natural home for presence state. Both tracks would write to this resource; only the Access tab reads the composite.

### Leverage Point 2: Presence as a Gossip Heartbeat

Adding a `PresenceHeartbeat` gossip message type alongside `ProfileAnnounce` solves presence for both tracks with minimal marginal cost, since the Profile Manager already builds the gossip infrastructure.

**Evidence**: `fe-network/src/gossip.rs:38-42` shows the broadcast stub already accepts `GossipMessage<T>` -- the type parameter means adding a new message type is structurally trivial. The `verse_gossip_topic()` function at `fe-sync/src/verse_peers.rs:56-58` already computes per-verse topics.

### Leverage Point 3: Schema Design with `peer_did` Foreign Key

The current `role` table (`schema.rs:132-137`) uses `node_id` as a bare string. If both `profiles` and `role` tables use a shared `peer_did` column:

```sql
DEFINE TABLE profiles SCHEMAFULL;
DEFINE FIELD peer_did ON TABLE profiles TYPE string;  -- did:key:z6Mk...

DEFINE TABLE role SCHEMAFULL;
DEFINE FIELD peer_did ON TABLE role TYPE string;       -- same key space
```

Then a single query can join profiles and roles for the Access tab. The `verse_member` table already uses `peer_did` (`schema.rs:167`), establishing the naming convention.

### Leverage Point 4: Identity Switch as a System-Wide Event

Rather than each subsystem independently discovering a keypair change, an `IdentitySwitchEvent` (Bevy event) could trigger coordinated cleanup:

```rust
pub struct IdentitySwitchEvent {
    pub old_did: String,
    pub new_did: String,
}
```

Systems would listen for this event:
- `SessionCache` -- clear all entries
- `PeerRegistry` -- remove old identity entries
- `NavigationManager` -- close verse replicas
- `SyncEndpoint` -- rebuild with new `iroh::SecretKey`

**Evidence**: The `SyncEndpoint` (`fe-sync/src/endpoint.rs:19`) takes a `secret_key` at construction time -- there is no way to change it without rebuilding the endpoint.

---

## 6. SYSTEMIC RISKS

### Risk 1: Eventual Consistency Split-Brain (Severity: HIGH)

**Mechanism**: Profile updates propagate via iroh-gossip (Profile Manager Phase 4). Role changes persist only locally (Inspector Phase 4, out-of-scope for P2P propagation per Inspector spec line 153). Two peers can see different profile+role combinations for the same third peer.

**Concrete scenario**: Alice (admin on Peer A) changes Bob's role from viewer to editor. Peer B, where Carol is admin, still shows Bob as viewer. Carol independently changes Bob to admin on Peer B. Now Peer A thinks Bob is editor, Peer B thinks Bob is admin, and Bob himself has no single authoritative role.

**Evidence**: The RBAC `get_role()` function (`rbac.rs:9`) queries the local database only. There is no mechanism for role table replication.

### Risk 2: Identity Switch Cascade Failure (Severity: HIGH)

**Mechanism**: Detailed in Section 3C above. The iroh endpoint cannot be hot-swapped -- `SyncEndpoint::new()` (`endpoint.rs:19`) is async and binds a UDP socket. The entire sync thread would need to be torn down and restarted.

**Evidence**: `spawn_sync_thread()` (`sync_thread.rs:28-33`) takes `secret_key` as an owned parameter. The thread loop has no mechanism to receive a "change identity" command.

### Risk 3: Orphaned Role Records (Severity: MEDIUM)

**Mechanism**: When a user switches identity, their old `role` records persist in the database forever. No eviction or migration mechanism exists.

**Evidence**: The `role` table has no TTL, no expiry field, and no cascade delete. The `delete_keypair()` function only removes the keychain entry.

### Risk 4: Channel Capacity Exhaustion Under Profile Storm (Severity: LOW-MEDIUM)

**Mechanism**: Many peers updating profiles simultaneously floods `SyncEvent` messages. The channel is bounded at 256. Profile processing competes with blob-ready events and transform broadcasts.

### Risk 5: Admin Verification Bootstrapping (Severity: MEDIUM)

**Mechanism**: The Access tab gates role-change dropdowns on `InspectorFormState.is_admin` which is never dynamically set. Neither track clearly specifies HOW the local user's admin status is determined at runtime.

---

## 7. KEY INSIGHTS

**1. The coupling between tracks is a Phase 4 problem exclusively.** Inspector Phases 1-3 and Profile Phases 1-3 are fully independent. The tight coupling concentrates entirely in Phase 4 of both tracks. This means parallel development is safe for 75% of the work, but the final phase requires explicit coordination or a shared integration sprint.

**2. Presence is the unowned dependency that blocks both tracks' core promise.** Both the "online/offline indicator" in the profile widget and the "is_online" field in PeerAccessEntry depend on per-peer presence tracking. `VersePeers` is the obvious home, but it is an empty stub with no track assigned to populate it. This should be extracted as a prerequisite task.

**3. The identity model has an unresolved architectural contradiction.** The RBAC system identifies peers by `hex(pub_key)`, the iroh layer by `NodeId`, and the profile system by `did:key:z6Mk...`. These are three different string representations of the same 32-byte public key. Without a canonical peer identifier type, join operations between profiles, roles, and presence data will require conversion at every boundary. The `verse_member` table already uses `peer_did`, suggesting that `did:key` should be the canonical form.

**4. Multi-identity fundamentally breaks the single-keypair assumption embedded throughout the architecture.** The iroh endpoint, session cache, RBAC lookup, and sync thread are all built around a single identity. Identity switching needs to be treated as a full application state reset, not as a profile-level operation.

**5. A `PeerRegistry` resource is the highest-leverage single design decision.** By introducing one shared Bevy resource that aggregates profile data, role data, and presence data per peer, both tracks gain a single point of truth for "who is this peer and what can they do." This eliminates three separate lookup paths in the Access tab renderer and provides a natural home for stale data eviction.
