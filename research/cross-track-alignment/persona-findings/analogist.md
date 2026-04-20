# Analogist: Structural Parallels from Existing Systems

## Cross-Domain Patterns for Inspector-Profile Track Alignment

---

## Executive Summary

The most transferable patterns for FractalEngine come from **Matrix** (event-type separation with assembly at render time), **Discord** (composite Member object from User + Guild data), **SSB** (self-published profile with separate access control), and **DNS/OAuth** (dual-TTL caching for data with different staleness tolerances). Every system examined separates identity/profile data from permission/role data, but each chooses a different seam.

---

## 1. Matrix / Element: Three-Layer Separation (HIGH)

**How it works:** Matrix separates peer information into three event types:
1. `m.presence` — Global online/offline. TTL-based.
2. `m.room.member` — Per-room membership with snapshotted display name/avatar.
3. `m.room.power_levels` — Per-room permission map `{user_id: power_level}`.

Client assembles member list by joining all three at render time. Missing data degrades gracefully.

**Transferable pattern:** **Render-time assembly from separate data sources.** Keep profile and permission data in separate tables/caches. UI assembles the composite view. Neither track "owns" the assembly.

**Application:** `PeerAccessEntry` should store `peer_did` + `role` only. `display_name` resolved from `PeerProfileCache` at render time. `is_online` from `VersePeers` at render time.

---

## 2. Secure Scuttlebutt: Self-Asserted vs. Other-Asserted (HIGH)

**How it works:** Identity = ed25519 keypair (same as FractalEngine). Profile = self-published `about` message. Access control = other-published `contact` messages (follow/block).

**Transferable pattern:** **Self-asserted profile vs. other-asserted permission.** These are different trust domains. Profile is a claim by the peer about themselves (eventually consistent). Role is a claim by the entity owner about the peer (authoritative, persistent).

**Application:** Profile data flows through P2P gossip (ephemeral). Role data persists in SurrealDB (authoritative). These two paths should never merge.

---

## 3. Discord: Composite Member Object (HIGH)

**How it works:** Three tiers: **User** (global identity), **Member** (per-guild: embedded User + role IDs + nickname), **Role** (per-guild definitions).

**Transferable pattern:** **Denormalize at the assembly boundary.** Create a `PeerDisplayRow` assembled once when data changes (not per-frame):

```rust
struct PeerDisplayRow {
    peer_did: String,
    display_name: String,  // from PeerProfileCache
    role: String,          // from Role table
    is_online: bool,       // from VersePeers
}
```

**Application:** A shared assembly function takes `(&[RoleEntry], &PeerProfileCache, &VersePeers) -> Vec<PeerDisplayRow>`. Owned by neither track — shared rendering utility.

---

## 4. DNS Serve-Stale: Dual-TTL Caching (HIGH)

**How it works:** DNS records have per-type TTL. RFC 8767 "serve-stale": expired records served if authoritative server unreachable, with 7-day stale cap.

**Transferable pattern:** **Track freshness per-field, not per-row.** Profile data (7-day stale), role data (persistent), presence data (60s heartbeat). Three different TTLs for three data layers.

**Application:** Change Profile Manager's 7-day eviction from hard-delete to **stale-mark**. Evicted profile + active role = raw key in Access tab (worse UX). Stale profile + active role = dimmed name (better UX).

| Data | Volatility | TTL | Storage |
|------|-----------|-----|---------|
| Presence | Seconds | 60s heartbeat | VersePeers in-memory |
| Profile | Days | 7-day stale, 30-day evict | PeerProfileCache + SurrealDB |
| Role | Weeks/Months | Persistent | Role table |
| Identity | Permanent | Never | fe-identity keychain |

---

## 5. Keybase: Identity Is a Key, Not a Person (HIGH)

**How it works:** Per-device keypairs linked by signature chain. Multiple keys → one identity via verifiable delegation.

**Transferable pattern:** FractalEngine has NO identity-linking mechanism. Switching identity creates a new DID. Roles don't transfer. This is the **correct behavior** for privacy (identity unlinkability), but both specs must document it.

**Application:** Both specs should state: "Switching identity creates a new DID. Role assignments belong to the previous DID and are not transferred."

---

## 6. OAuth2: Short-Lived Proof + Long-Lived Grant (HIGH)

**How it works:** Access token (minutes) = ephemeral proof of current state. Refresh token (days) = persistent capability grant.

**Parallel:** `ProfileAnnounce` gossip = access token (ephemeral state). `Role` table = refresh token (persistent grant). `SessionCache` TTL of 60s already implements this pattern.

---

## 7. GitHub: Three-Level Permission Hierarchy (MEDIUM)

**How it works:** Organization → Team → Repository roles. Permissions are additive. API returns effective permission.

**Application:** FractalEngine's `Role` table is petal-scoped only. To show roles at Verse/Fractal levels, either add `scope_type`/`scope_id` columns or define inheritance (nodes inherit petal access).

---

## 8. Mastodon: Profile Crosses Trust Boundaries (MEDIUM)

**Transferable pattern:** Profile data from remote peers = untrusted input. Must validate: display name length, character set, avatar index, signature.

---

## 9. libp2p: Three-Layer Identity Architecture (MEDIUM)

| Layer | libp2p | FractalEngine | Crate |
|-------|--------|---------------|-------|
| Identity | PeerId | NodeKeypair/did:key | fe-identity |
| Transport Profile | Identify protocol | ProfileAnnounce | fe-sync |
| Application Profile | (app builds it) | UserProfile/PeerProfileCache | fe-ui |
| Permissions | (app builds it) | Role table | fe-database |

**Application:** `ProfileAnnounce` belongs in `fe-sync/src/messages.rs` as `SyncEvent::ProfileReceived`, drained via existing `drain_sync_events` pattern.

---

## Concrete Spec Amendment Recommendations

### R1: Split PeerAccessEntry
**Current:** `PeerAccessEntry { peer_id, display_name, role, is_online }`
**Proposed:** `PeerAccessEntry { peer_did, role }`. Display name and online status assembled at render time from PeerProfileCache and VersePeers.

### R2: Stale-Then-Evict for Profile Cache
**Current:** Hard evict after 7 days.
**Proposed:** Mark stale after 7 days (visual indicator). Hard evict after 30 days.

### R3: Add SyncEvent::ProfileReceived
Reuse existing `drain_sync_events` channel pattern. Don't create a separate channel.

### R4: Shared PeerDisplayRow Assembly Function
`fn assemble_peer_rows(roles, profiles, peers) -> Vec<PeerDisplayRow>`. Owned by neither track.

### R5: Document Identity-Switching Role Impact
Both specs: "Switching identity orphans all role assignments under the previous DID."

### R6: Add Scope to Role Table
Add `scope_type` + `scope_id` or use inheritance model for verse/fractal-level roles.

### R7: Validate Received ProfileAnnounce
Treat as untrusted input. Validate length, charset, signature. Reject malformed.
