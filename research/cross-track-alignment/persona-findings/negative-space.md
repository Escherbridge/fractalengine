# Negative Space Explorer: Cross-Track Alignment Gaps

## What's Missing — 27 Gaps, Silences, and Orphaned Requirements

---

## 1. Orphaned Requirements (No Owner)

### 1.1 Peer Presence System

- **What's missing**: Inspector Access tab requires per-peer `is_online`. Profile widget requires local online/offline indicator. Both consume presence data. Neither specifies who PRODUCES it.
- **Evidence**: `SyncStatus` tracks only global `online: bool` and `peer_count: usize`. `VersePeers` is "Phase F stub: always empty."
- **Why it matters**: Access tab's online/offline indicator is unimplementable without this.
- **Owner**: NEITHER — needs new shared infrastructure track.
- **Severity**: BLOCKER
- **Recommendation**: Define `PeerPresence` Bevy resource in `fe-sync` with per-peer last-seen timestamps, populated by gossip heartbeats.

### 1.2 PeerAccessEntry Assembly Logic

- **What's missing**: `PeerAccessEntry { peer_id, display_name, role, is_online }` needs data from 3 subsystems (RBAC, PeerProfileCache, Presence). No spec describes the join point.
- **Evidence**: PeerAccessEntry does not exist in codebase. PeerProfileCache does not exist.
- **Owner**: Inspector Settings (defines and renders), but must declare deps on Profile Manager and Presence.
- **Severity**: HIGH
- **Recommendation**: Add Inspector Phase 4 task: "Implement PeerAccessEntryAssembler joining RBAC + PeerProfileCache + PeerPresence."

### 1.3 Peer List Maintenance (Multi-Level RBAC)

- **What's missing**: Access tab needs "all peers with access to entity" at ALL hierarchy levels. Role table only has `(node_id, petal_id)` — petal-scoped. No fractal or verse-level roles.
- **Evidence**: `verse_member` exists for verse-level, `role` for petal-level. Nothing for fractals or nodes.
- **Owner**: Shared decision needed.
- **Severity**: HIGH
- **Recommendation**: Decide: inheritance model (nodes inherit petal access) vs. per-entity ACLs.

---

## 2. Missing Specs/Requirements

### 2.1 No Error Recovery / Offline Behavior

Neither spec addresses SurrealDB unavailability, sync offline mode, or DB command failures. Users see blank panels.

**Severity**: HIGH. **Recommendation**: Add NFR: "When DB ops fail, show error state with retry affordance."

### 2.2 No Migration Spec

Profile Manager adds `profiles` table. Must update `apply_all()`, `KnownTable` enum, `ALL_TABLE_NAMES`. No migration test exists. `admin::load_from_file()` skips unknown tables — data loss on restore.

**Severity**: MEDIUM. **Recommendation**: Add Profile Phase 4 task for schema registration.

### 2.3 No Accessibility Spec

Zero mentions of keyboard navigation, screen reader, tab order, or focus management in either spec.

**Severity**: MEDIUM. **Recommendation**: Add NFR for keyboard navigability.

### 2.4 No In-App Notification System

No mechanism to inform user of role changes or profile updates. No toast/notification infrastructure exists.

**Severity**: MEDIUM. **Recommendation**: Create lightweight `ToastNotification` system.

### 2.5 No Audit Trail for Role Changes

`OpType::AssignRole` exists in the enum but Inspector spec doesn't mention writing to op_log. Existing patterns (create_petal, place_model) always write op_log.

**Severity**: HIGH. **Recommendation**: Inspector Phase 4 Task 4.5 must write op_log entry with `OpType::AssignRole`.

---

## 3. Questions Neither Spec Asks

### 3.1 Inspector Behavior in Offline Mode
Show locally cached roles with "may be stale" banner. **Severity**: LOW.

### 3.2 Access Tab for Non-Owner Entities
Can non-admins see the full peer list? Privacy concern. **Severity**: MEDIUM. Decide: full visibility vs. own-role-only.

### 3.3 Concurrent Role Changes
Two admins change same peer's role simultaneously. Last-write-wins with no defined ordering. **Severity**: MEDIUM. Use lamport-clock ordering.

### 3.4 Exported Identity Revocation
No way to revoke a compromised exported identity. ed25519 key IS the identity. **Severity**: HIGH (security). Add signed revocation broadcast.

### 3.5 Identity Count Limits
OS keychains have varying limits. No soft limit specified. **Severity**: LOW. Add 16-identity limit.

### 3.6 Identity/Profile Deletion
No delete operation in spec. `delete_keypair()` EXISTS in code but no UI references it. **Severity**: MEDIUM. Add "Delete Identity" to FR-3.

---

## 4. Missing Integration Points

### 4.1 Profile Manager to NavigationManager Data Flow

Profile widget shows "current role for active petal" but never references NavigationManager. Plan says "wire to `InspectorFormState.is_admin` (or the active petal's role)" — source unclear.

**Severity**: HIGH. **Recommendation**: Read role keyed by active petal from NavigationManager.

### 4.2 Inspector to NodeIdentity: Admin Verification

`is_admin` exists, defaults to `false`, never populated from RBAC. Entire Access tab admin UI is dead without this.

**Severity**: BLOCKER. **Recommendation**: Add system: query `rbac::get_role()` using `NodeIdentity.did_key()` + inspected entity scope → `LocalUserRole` resource.

### 4.3 Profile P2P Sync vs. Auth Handshake

Spec says "sent as part of the handshake or as a gossip message" — ambiguous "or". Window after connection before first gossip where Access tab shows raw keys.

**Severity**: MEDIUM. **Recommendation**: Decide: gossip-only (1 second delay acceptable) — do NOT modify handshake.

### 4.4 Identity Switch to Session Revocation

`revoke_session()` exists in fe-auth but Profile Manager plan never references it. Old sessions valid for 60s after switch.

**Severity**: MEDIUM. **Recommendation**: Task 3.5 must call `revoke_session()` for all active peers.

---

## 5. Missing UI States

### 5.1 Loading State
What shows during initial data fetch? Empty tab = ambiguous (loading vs. no data). **Severity**: MEDIUM.

### 5.2 Error State
Role query fails? Keychain locked? No error UI defined. **Severity**: MEDIUM.

### 5.3 Empty State
No peers on freshly created petal. Tab shows nothing? **Severity**: LOW.

### 5.4 Conflict State
Identity switch succeeds locally but reconnect fails. What shows? **Severity**: MEDIUM.

**Recommendation for all**: Define distinct loading/error/empty/conflict states for every data-dependent panel.

---

## 6. Missing Non-Functional Requirements

### 6.1 Consistency Model
Profiles = eventual consistency via gossip. Roles = local DB. Neither spec states this. **Severity**: MEDIUM.

### 6.2 Display Name Conflict Resolution
Clock skew could cause wrong profile to win. No tiebreaker specified. **Severity**: LOW.

### 6.3 Rate Limiting P2P Profile Messages
No throttling on ProfileAnnounce. Malicious peer could flood gossip. **Severity**: MEDIUM.

### 6.4 Data Portability for Role Assignments
Identity export includes keypair but not roles. Out of scope for v1. **Severity**: LOW.

---

## Consolidated Priority Matrix

| # | Gap | Severity | Owner | Blocks |
|---|-----|----------|-------|--------|
| 1.1 | Peer Presence System | BLOCKER | New track / fe-sync | Inspector P4, Profile P1 |
| 4.2 | Admin Verification (is_admin wiring) | BLOCKER | Inspector | Inspector P4 |
| 1.2 | PeerAccessEntry Assembly | HIGH | Inspector (with deps) | Inspector P4 |
| 1.3 | Multi-level RBAC scope | HIGH | Shared decision | Inspector P3-4 |
| 2.1 | Error Recovery Spec | HIGH | Both tracks | All phases |
| 2.5 | Audit Trail for Role Changes | HIGH | Inspector | Inspector P4 |
| 3.4 | Identity Revocation | HIGH | Profile Manager | Profile P3 |
| 4.1 | Profile-to-NavigationManager flow | HIGH | Profile Manager | Profile P1 |
| 2.2 | Schema Migration | MEDIUM | Profile Manager | Profile P4 |
| 2.4 | Notification System | MEDIUM | Shared | Both P4 |
| 3.2 | Access Tab Privacy Model | MEDIUM | Inspector | Inspector P4 |
| 3.3 | Concurrent Role Changes | MEDIUM | Inspector | Inspector P4 |
| 3.6 | Identity/Profile Deletion | MEDIUM | Profile Manager | Profile P3 |
| 4.3 | P2P Sync vs Handshake | MEDIUM | Profile Manager | Profile P4 |
| 4.4 | Identity Switch Revocation | MEDIUM | Profile Manager | Profile P3 |
| 5.1-5.4 | UI States (loading/error/empty/conflict) | MEDIUM | Both | All data phases |
| 6.1 | Consistency Model | MEDIUM | Both | P4 of both |
| 6.3 | Rate Limiting | MEDIUM | Profile Manager | Profile P4 |

---

## Top 3 Recommendations

1. **Create a Presence Infrastructure track** before either track enters Phase 4. `VersePeers` in `fe-sync/src/verse_peers.rs` is the natural home.

2. **Resolve the RBAC scope question now**: petal-only or hierarchy-scoped. Affects both tracks' Phase 3-4 fundamentally.

3. **Wire `is_admin` to actual RBAC data**: single most impactful missing integration. Without it, the entire Access tab admin UI is dead code.
