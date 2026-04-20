# Deep Research Report: Cross-Track Alignment Analysis

## Inspector Settings x Profile Manager — FractalEngine Conductor Tracks

**Date**: 2026-04-19
**Methodology**: Asymmetric Research Squad (8 personas, ACH crucible, contradiction mapping)
**Confidence Level**: HIGH (evidence verified against codebase by 10 independent agents)

---

## Executive Synthesis

Both tracks are well-designed individually but share a **phantom dependency layer** — infrastructure that both specs assume exists but neither specifies. The ACH analysis evaluated 5 implementation strategies against 15 evidence items from 7 personas. The result is unambiguous:

**Build shared infrastructure first (Phase 0), then both tracks in parallel through P1-P3, with a coordinated Phase 4 integration.**

Phase 0 resolves the 3 BLOCKERs and 2 HIGH-priority gaps that would otherwise derail either track at Phase 4. Without Phase 0, whichever track reaches Phase 4 first will discover it needs data from subsystems that don't exist and aren't owned by either track.

---

## The 3 BLOCKERs

### BLOCKER 1: Peer Presence System (Unowned)

Both tracks consume per-peer online/offline state. Neither produces it.

- Inspector Access tab: `PeerAccessEntry.is_online` per peer
- Profile widget: green/gray dot for local connectivity
- **Current state**: `VersePeers` at `fe-sync/src/verse_peers.rs` has the data structure but is "Phase F stub: always empty." `SyncEvent` has no `PeerConnected`/`PeerDisconnected` variants.
- **Fix**: Add `SyncEvent::PeerConnected { peer_id }` / `PeerDisconnected { peer_id }`. Populate `VersePeers` from iroh connection events. Feed into a shared `PeerRegistry` resource.

### BLOCKER 2: Admin Verification Dead Code

The entire admin-gated UI is permanently disabled.

- `InspectorFormState.is_admin` at `plugin.rs:167` defaults to `false`, is **never set to true**.
- `NodeIdentity` is never inserted into the Bevy world — UI systems can't read the local DID.
- **Fix**: (1) `app.insert_resource(NodeIdentity::new(keypair))` in `main.rs`. (2) System that queries `rbac::get_role(local_did, active_petal)` and writes to a `LocalUserRole` resource. (3) Replace `is_admin: bool` with `Option<RoleId>`.

### BLOCKER 3: PeerAccessEntry Assembly (No Owner)

The composite peer view needs data from 3 subsystems. No spec describes the join.

- Role data: from `fe-database` RBAC (`role` table)
- Display name: from Profile Manager's `PeerProfileCache` (doesn't exist yet)
- Online status: from `VersePeers` (empty stub)
- **Fix**: Create a unified `PeerRegistry` Bevy resource (following VerseManager consolidation precedent). Both tracks write to it; Access tab reads the composite.

---

## 5 Design Decisions Required Before Implementation

### Decision 1: Canonical Peer Identifier Format

Three formats coexist: `did:key:z6Mk...` (verse_member), `hex::encode(pub_key)` (handshake), iroh `NodeId`. Plus 14+ hardcoded `"local-node"` strings.

**Recommendation**: Canonicalize on `did:key` (precedent from `verse_member.peer_did`). Replace all "local-node" refs. Convert hex→did:key in handshake/RBAC boundary.

### Decision 2: RBAC Scope Model

Role table is petal-scoped only. Inspector wants hierarchy-level access tabs.

**Recommendation**: Petal-only for v1. Access tab at verse/fractal levels shows "Access managed at petal level." Schema extension (`entity_type` + `entity_id`) for v2.

### Decision 3: Data Architecture (PeerRegistry vs. Separate Resources)

**Recommendation**: Unified `PeerRegistry` resource. Systems Thinker proposal — follows VerseManager precedent, eliminates three-way render-time lookups.

### Decision 4: Profile Cache Eviction

7-day hard eviction is too aggressive — peers with persistent roles lose display names.

**Recommendation** (from Analogist's DNS serve-stale pattern): Mark stale after 7 days, hard evict after 30 days. Persist display names in SurrealDB `profiles` table (survives cache eviction).

### Decision 5: PeerAccessEntry Field Design

**Recommendation** (from Analogist's Matrix/Discord pattern): `PeerAccessEntry` stores only `{ peer_did, role }`. Display name and online status resolved at render time from `PeerProfileCache` and `VersePeers`. Self-asserted data (profile) and other-asserted data (roles) stay in separate schemas.

---

## Implementation Strategy

### Phase 0: Shared Infrastructure (~1-2 sprints)

| # | Work Item | Resolves |
|---|-----------|----------|
| 0.1 | Insert NodeIdentity into Bevy world in main.rs | BLOCKER 2 |
| 0.2 | Replace 14+ "local-node" strings with actual DID | Canonical ID |
| 0.3 | Canonicalize on did:key format across RBAC, handshake, session | Decision 1 |
| 0.4 | Define PeerRegistry resource | BLOCKER 3, Decision 3 |
| 0.5 | Build presence: SyncEvent::PeerConnected/Disconnected, populate VersePeers | BLOCKER 1 |
| 0.6 | Wire admin determination from RBAC query → LocalUserRole resource | BLOCKER 2 |
| 0.7 | Reconcile role labels: UI displays actual DB values (owner/editor/public) | Archaeologist finding |
| 0.8 | Decide RBAC scope model (petal-only v1) | Decision 2 |

### Phase 1-3: Parallel Track Execution (~3 sprints each)

Inspector P1-P3 and Profile P1-P3 are **fully independent** (confirmed by Systems Thinker temporal analysis). Both can proceed in parallel once Phase 0 is complete.

### Phase 4: Coordinated Integration (~1-2 sprints)

Inspector P4 (Access tab) + Profile P4 (P2P sync + PeerProfileCache wiring). The `PeerRegistry` from Phase 0 serves as the integration contract.

```
Phase 0 (shared infra)     ~1-2 sprints
    |
    +---> Inspector P1-P3   (parallel)
    +---> Profile P1-P3     (parallel)
    |
    v
Coordinated P4              ~1-2 sprints
```

---

## Additional Findings by Priority

### HIGH Priority

- **Identity switching cascade**: Switching identity requires full sync thread teardown, iroh endpoint rebuild, session clearing. Roles orphaned on old key. Treat as "destructive reset" for v1 with clear user warning. (Historian, Contrarian, Systems Thinker)
- **Gossip fully stubbed**: `broadcast()` is a no-op. Profile P4 needs working gossip. Phase 0 should unstub VersePeers via iroh events; full gossip in Profile P4. (Journalist)
- **No audit trail for role changes**: OpType::AssignRole exists but Inspector plan doesn't write op_log. Must follow existing pattern from queries.rs. (Negative Space)
- **Identity revocation gap**: No way to revoke a compromised exported identity. Add signed revocation broadcast. (Negative Space)

### MEDIUM Priority

- **Profile widget location**: Sidebar auto-collapses when inspector opens. Move to top toolbar. (Historian, Futurist)
- **Tab persistence**: Remember active tab per entity TYPE, not per instance. (Futurist)
- **ProfileAnnounce replay protection**: Enforce monotonic timestamps per public key. (Futurist)
- **Rate limiting on gossip**: Max 1 ProfileAnnounce per peer per 10 seconds. (Negative Space, Futurist)
- **Profile exchange method**: Gossip-only (not handshake). Accept ~1s delay for name resolution. (Negative Space)
- **Input validation on ProfileAnnounce**: Treat as untrusted input. Validate length, charset, signature. (Analogist, Mastodon pattern)
- **Loading/error/empty UI states**: Define for all data-dependent panels. (Negative Space)

### LOW Priority

- **Avatar in Access tab**: Profile tracks avatar_index but Inspector Access tab doesn't display it. Add avatar to PeerRegistry. (Contrarian)
- **Identity count limit**: Soft cap at 16 identities (OS keychain limits). (Negative Space)
- **Identity/profile deletion**: delete_keypair() exists in code but no UI. Add to Profile FR-3. (Negative Space)

---

## Cross-Domain Patterns Applied

| Pattern | Source | Application |
|---------|--------|-------------|
| Render-time assembly | Matrix | Access tab joins role + profile + presence at draw time |
| Self-asserted vs other-asserted | SSB | Profile data (gossip) stays separate from role data (DB) |
| Three-TTL model | OAuth2 + DNS | Presence: 60s, Profile: 7-30 days, Roles: persistent |
| Composite view model | Discord | PeerDisplayRow assembled at boundary, not per-frame |
| Serve-stale | DNS RFC 8767 | Stale profiles shown dimmed, not evicted |
| Identity is a key | Keybase | Switching identity orphans roles — document explicitly |

---

## Research Artifacts

| Phase | Files | Agents |
|-------|-------|--------|
| Phase 1: Persona Findings | 8 files in `persona-findings/` | Historian, Contrarian, Analogist, Systems Thinker, Journalist, Archaeologist, Futurist, Negative Space |
| Phase 2: Crucible Analysis | 2 files in `synthesis/` | ACH Analyst, Contradiction Mapper |
| Objective | `objective.md` | — |
| Final Report | `report.md` (this file) | — |

**Total research agents deployed**: 10
**Total evidence items evaluated**: 15 (ACH) + 27 gaps (Negative Space) + 7 contradictions resolved
**Codebase files examined**: 30+ across 8 crates
**Confidence in primary recommendation**: HIGH (0 inconsistencies in ACH)
