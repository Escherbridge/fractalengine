# The Futurist: Cross-Track Alignment Analysis

## Forward-Looking Scale, Edge Case, and Second-Order Effects Analysis

---

## 1. Scale Scenarios

### 1.1 Fifty Peers in a Petal

**Scenario**: The Access tab renders a `PeerAccessEntry` for every peer with access. At 50 peers, 50 rows with display name, role badge, online indicator, and (for admins) a role dropdown.

**Probability**: Likely. A popular public petal could attract dozens of visitors.

**Impact**: Medium. egui immediate-mode redraws every row every frame. The real cost is the data pipeline: each entry needs display name from `PeerProfileCache` and role from the DB query.

**Current spec coverage**: Unaddressed. No pagination, no virtualized scrolling, no maximum peer count.

**Recommendation**:
- Access tab uses `egui::ScrollArea` with fixed height. Virtualize if peer count exceeds 20.
- Define `PeerProfileCache` capacity limit (e.g., 200 entries).
- `AccessTabState.entries` should be built once on tab open and refreshed on gossip events, not rebuilt every frame.

### 1.2 User with 10 Identities

**Scenario**: Power user creates 10 ed25519 identities. Each has different roles across petals. Frequent switching.

**Probability**: Edge case. Most users will have 1-2 identities.

**Impact**: High. Each switch requires confirmation, session clear, P2P reconnection. Access tab admin dropdown may appear/disappear mid-session.

**Current spec coverage**: Partially addressed. Neither spec addresses what happens to Access tab during identity switch.

**Recommendation**:
- Identity switch should force-close inspector's Access tab (reset to Info) and invalidate `AccessTabState`.
- Define maximum identity count (e.g., 20) to prevent keychain exhaustion.

### 1.3 Deep Hierarchy (Verse > 5 Fractals > 20 Petals > 100 Nodes)

**Scenario**: Inspector handles selection at any level. Rapid clicks between 20+ entities.

**Probability**: Likely.

**Impact**: Medium. Rapid switching could thrash the inspector cache. In-flight `DbCommand::GetEntityPeerRoles` queries become stale.

**Current spec coverage**: Partially addressed. NFR-1 says cache on selection change, but no stale-response handling.

**Recommendation**:
- Add generation counter to `InspectedEntity` selections. Discard stale DB responses.
- Info tab data derivable from VerseManager without DB query. Only Access tab needs async fetch.
- Consider 50ms debounce on inspector entity changes.

### 1.4 Rapid Profile Changes (50 Peers Update Simultaneously)

**Scenario**: 50 peers update display names within 5 seconds. PlumTree delivers O(n^2) = 2,500 messages.

**Probability**: Possible during initial bootstrap.

**Impact**: High. Signature verification (ed25519) + cache updates for each message.

**Current spec coverage**: Unaddressed. No rate limiting, batching, or coalescing.

**Recommendation**:
- Per-peer rate limit on `ProfileAnnounce` reception: max 1 per peer per 5 seconds.
- Debounce profile saves on sending side (2 seconds of inactivity).

---

## 2. Identity Lifecycle Edge Cases

### 2.1 Orphaned Identity Roles

**Scenario**: User switches identity. Old identity's admin role persists in `role` table forever.

**Probability**: Likely over time.

**Impact**: Medium. Stale Access tab entries, wasted storage.

**Current spec coverage**: Unaddressed. Profile eviction (7 days) only applies to gossip cache, not role table.

**Recommendation**:
- Flag roles for peers absent >30 days as "inactive" (grayed out in Access tab).
- Consider `last_seen: datetime` column on role table.

### 2.2 Identity Collision (Duplicate Import)

**Scenario**: Two users import the same exported identity. Same keypair, different display names.

**Probability**: Edge case.

**Impact**: Critical. Breaks one-key-one-peer assumption. PeerProfileCache oscillates. Role table shows one entry for both. Sessions indistinguishable.

**Current spec coverage**: Unaddressed for cross-device collision.

**Recommendation**:
- Document as known limitation.
- Add session_id (random UUID per app launch) to gossip messages for collision detection.

### 2.3 Identity Recovery and Role Restoration

**Scenario**: User exports identity, reinstalls, imports. Are roles restored?

**Probability**: Likely (core use case).

**Impact**: High. Roles ARE restored IF the host node still has the role entry. If host reinstalled too, roles are gone.

**Current spec coverage**: Not explicitly documented.

**Recommendation**:
- Document: "Roles are stored on the host node. Importing an identity restores access only if the host still has the role assignment."
- Consider backing up role assignments to iroh-docs replica.

---

## 3. RBAC at Scale

### 3.1 Permission Preset Cascade

**Scenario**: Admin sets Verse to "Admin Only." Does it cascade to all petals?

**Probability**: Likely expectation.

**Impact**: Critical. Current schema is petal-scoped only. No verse-level role representation. Cascade means N petal updates.

**Current spec coverage**: Unaddressed.

**Recommendation**:
- Either extend role table with `entity_type` + `entity_id` columns, OR keep petal-level and make presets batch-update child petals with explicit UX: "This will update roles on N petals. Continue?"

### 3.2 Role Conflict Across Petals

**Scenario**: Admin on Petal A, viewer on Petal B. Profile widget shows "current role" — what if no petal active?

**Probability**: Likely.

**Impact**: Medium. `is_admin: bool` is inadequate for a system with public/editor/owner roles.

**Recommendation**:
- Replace `is_admin: bool` with `Option<RoleId>`. `None` when no petal active.
- Display actual role string, not binary admin/member.

### 3.3 Admin Bootstrapping

**Scenario**: Who is first admin? Verse creator. But if they switch identity, original key retains admin.

**Probability**: Likely for every verse.

**Impact**: High. Must guarantee atomic petal creation + owner role self-assignment.

**Recommendation**:
- `CreatePetal` handler must atomically create petal row AND role row with "owner".
- Access tab should display verse creator specially (crown icon).

---

## 4. P2P Profile Sync Edge Cases

### 4.1 Gossip Network Partition

**Scenario**: 20 peers split into two groups. Profile updates during partition. Partition heals.

**Probability**: Possible over WAN.

**Impact**: High. iroh-gossip does NOT guarantee delivery during partition. Clock-skew risks wrong profile winning.

**Recommendation**:
- Use lamport_clock instead of wall-clock timestamps. Op_log already uses lamport clocks.
- Define profile reconciliation protocol (bloom filter exchange on connect).

### 4.2 ProfileAnnounce Size Budget (512 Bytes)

**Evidence-based calculation**:
- Worst case (JSON): 56 + 128 + 1 + 24 + 88 + 100 = **397 bytes**. Under 512 but tight.
- Binary encoding: ~160-200 bytes. Comfortable.

**Recommendation**:
- Use binary encoding (bincode/MessagePack) for gossip messages.
- Validate display name byte length (max 128 UTF-8 bytes), not just character count.
- Store raw 32-byte public key in gossip, not DID string. Receivers reconstruct locally.

### 4.3 Replay Attack on ProfileAnnounce

**Scenario**: Malicious peer replays old ProfileAnnounce. Valid signature, outdated data.

**Probability**: Possible.

**Impact**: High. Victim's display name reverts across network.

**Recommendation**:
- Enforce monotonic timestamps per public key in PeerProfileCache.
- Include monotonic sequence number in ProfileAnnounce.

---

## 5. UI Integration at Scale

### 5.1 Access Tab + Profile Sync Race

**Scenario**: User viewing Access tab when ProfileAnnounce arrives with new name.

**Probability**: Likely.

**Impact**: Medium. If entries cached on open, they go stale. If resolved at render time, they live-update.

**Current spec coverage**: Contradicts. Inspector NFR-1 says "refreshed only on selection change." Profile spec implies liveness.

**Recommendation (MOST IMPORTANT CROSS-TRACK DECISION)**:
- `AccessTabState` stores `peer_id` and `role` (from DB, refreshed on selection change). Display name and online status resolved at render time from `PeerProfileCache` and `VersePeers`. This way DB queries are infrequent but display data stays live.

### 5.2 Inspector + Profile Panel Space

**Scenario**: Left sidebar auto-collapses when inspector opens. Profile widget in sidebar disappears.

**Probability**: Likely.

**Impact**: Low but annoying. Can't see own profile while configuring access.

**Recommendation**:
- Move profile widget to top toolbar (always visible).
- Or add miniature profile indicator to status bar.

### 5.3 Tab Persistence vs. Entity Change

**Scenario**: Admin comparing Access tabs across petals. Tab resets to Info on each entity change.

**Recommendation**:
- Remember active tab per entity TYPE. Selecting different petal keeps Access tab. Selecting node resets to Info.
- Store `last_tab_per_type: HashMap<EntityType, InspectorTab>`.

---

## 6. Future Track Dependencies

### What Depends on the Combined System?

1. **Seedling Onboarding** -- needs identity bootstrap + verse admin role.
2. **Mycelium Scaling** -- amplifies ProfileAnnounce thundering herd.
3. **Marketplace/cross-verse** -- needs asset-level permissions + portable identity.

### Architecture Flexibility Assessment

- **Real-time collaboration**: Partially ready. Op_log with lamport clocks is CRDT-compatible. Inspector state is single-user.
- **Asset marketplace**: Not yet. RBAC is petal-scoped. Would need `(node_id, entity_type, entity_id)`.
- **Cross-verse identity**: Yes. `did:key` is not verse-bound. Profile would re-announce per verse.

### Critical Architectural Risk

The **split data ownership** for Access tab entries:
- Roles from SurrealDB (persistent, host-local)
- Profiles from gossip (ephemeral, eventually consistent)
- Online status from VersePeers (real-time, Bevy resource)

Three data sources with different consistency guarantees. The join at render time is the correct approach but neither spec fully specifies it.

---

## Priority Summary

| Priority | Recommendation | Tracks |
|----------|---------------|--------|
| P0 | Define Access tab data join: roles from DB, profiles from gossip, status from VersePeers at render time | Both |
| P0 | Extend role schema for hierarchy scoping or define cascade batch semantics | Inspector |
| P1 | Replace `is_admin: bool` with `Option<RoleId>` | Both |
| P1 | Monotonic timestamp enforcement for ProfileAnnounce replay protection | Profile |
| P1 | Move profile widget to top toolbar | Profile |
| P1 | Generation counter for stale DB response rejection | Inspector |
| P2 | Rate limiting on ProfileAnnounce reception | Profile |
| P2 | PeerProfileCache capacity limit and GC policy | Profile |
| P2 | Binary encoding for gossip messages | Profile |
| P2 | Remember active tab per entity TYPE | Inspector |
| P3 | Session_id for identity collision detection | Profile |
| P3 | Role GC for absent peers | Inspector |
| P3 | Atomic petal creation + owner role | Inspector |
| P3 | Profile reconciliation protocol on connect | Profile |
