# Contradiction Mapping: Cross-Track Alignment Crucible

## Top 5 Cross-Track Alignment Issues (Ranked)

1. **Presence System Gap** — BLOCKER. All 7 personas agree. Neither track builds it.
2. **Admin Verification Dead Code** — BLOCKER. `is_admin` never set to true. `NodeIdentity` not in Bevy world.
3. **PeerAccessEntry Assembly Orphan** — HIGH. The join between 3 subsystems has no owner.
4. **Identity Switching Cascade** — HIGH. Requires sync thread teardown, role orphaning. No infra exists.
5. **Canonical Peer Identifier Format** — HIGH. Three string formats for same key. Impedance mismatch.

---

## 1. Contradictions With Resolution

### C1: Ownership of PeerAccessEntry Assembly

| Persona | Position |
|---------|----------|
| Systems Thinker | Unified `PeerRegistry` resource. Both tracks write; Access tab reads composite. |
| Negative Space | Inspector owns assembly; declares deps on Profile + Presence. |
| Contrarian | Profile Manager owns (closest to most data sources). |

**Resolution: Systems Thinker wins.** `PeerRegistry` follows existing VerseManager consolidation pattern. Avoids circular dependencies.

### C2: Identity Switching Severity

| Persona | Rating |
|---------|--------|
| Historian | CRITICAL ("most architecturally disruptive change") |
| Contrarian | HIGH ("identity switching bomb") |
| Systems Thinker | HIGH ("most dangerous emergent property") |
| Futurist | MEDIUM ("edge case", most users have 1-2 identities) |

**Resolution: HIGH wins.** Futurist correct about frequency, wrong about severity. Code evidence shows iroh endpoint can't be hot-swapped, sync thread has no identity-change command, roles orphaned on key change.

### C3: RBAC Scope Strategy

| Persona | Position |
|---------|----------|
| Futurist | Extend role table: `entity_type` + `entity_id` columns |
| Contrarian | Petal-only for v1; show "managed at petal level" for verse/fractal |
| Negative Space | Inheritance model (nodes inherit petal access) |

**Resolution: v1 = Contrarian (petal-only). v2+ = Futurist (schema extension). Inheritance model rejected (no evidence, hard to audit).**

### C4: Profile Cache Eviction

| Persona | Position |
|---------|----------|
| Contrarian | 7 days too aggressive; persist in SurrealDB |
| Futurist | Persist in DB; 200-entry capacity limit |
| Negative Space | (silent — oversight) |

**Resolution: Contrarian + Futurist converge.** Volatile 7-day gossip cache for freshness + persistent `profiles` table for display names. Names survive eviction.

### C5: Admin Verification Layers

| Persona | Focus |
|---------|-------|
| Contrarian | Security black box — no server-side validation of role changes |
| Negative Space | BLOCKER — `is_admin` dead, proposes `LocalUserRole` resource |
| Journalist | Handshake JWT minting works; infrastructure exists but disconnected from UI |

**Resolution: Not a contradiction — three complementary views of a three-layer gap:**
1. Layer 1 (exists): Handshake mints JWT with role
2. Layer 2 (missing): No system reads SessionCache → writes to is_admin
3. Layer 3 (missing): NodeIdentity not in Bevy world → can't query local DID

### C6: Profile Widget Location

Historian: sidebar auto-collapses when inspector opens → profile disappears.
Futurist: move to top toolbar (always visible).

**Resolution: Futurist wins.** Top toolbar or status bar.

### C7: Role Label Mismatch

Archaeologist (only one to flag): UI shows "admin"/"member" but DB stores "owner"/"editor"/"public".

**Resolution: Display actual DB role strings.** All personas should have caught this.

---

## 2. Points of Unanimous Agreement

1. **`is_admin` is dead code** — defaults to false, never set to true. All 7 personas.
2. **Presence system is unowned** — VersePeers always empty, no PeerConnected event. All 7.
3. **Phase 4 coupling only** — P1-P3 fully independent. Historian, Systems Thinker, Futurist.
4. **"local-node" must be replaced** — 14+ hardcoded locations. Archaeologist, Historian, grep verified.
5. **NodeIdentity not in Bevy world** — Resource exists but never inserted. Journalist, Negative Space.
6. **Gossip fully stubbed** — broadcast() is no-op. Journalist, Archaeologist.
7. **Three identity string formats** — did:key vs hex vs NodeId. Historian, Contrarian, Archaeologist, Systems Thinker.

---

## 3. Severity Calibration

| Issue | Contrarian | Neg. Space | Sys. Thinker | Futurist | Calibrated |
|-------|-----------|------------|--------------|----------|------------|
| Presence gap | HIGH | BLOCKER | HIGH | P0 | **BLOCKER** |
| Admin verification | HIGH | BLOCKER | MEDIUM | P1 | **BLOCKER** |
| Identity switch | HIGH | — | HIGH | MEDIUM | **HIGH** |
| RBAC scope | MEDIUM | HIGH | — | P0 | **HIGH** |
| Cache eviction | MEDIUM | — | — | P2 | **MEDIUM** |
| PeerAccessEntry | BLOCKER | HIGH | HIGH | P0 | **BLOCKER** |

---

## 4. Design Decisions Required

| Decision | Options | Impact | Blocking |
|----------|---------|--------|----------|
| RBAC scope | Inheritance / entity_type+entity_id / petal-only v1 | Access tab query, presets | Inspector P3-4 |
| Profile exchange | Gossip-only / handshake extension / both | Raw-key window after connect | Profile P4 |
| Data architecture | PeerRegistry (unified) / three separate resources | Phase 4 assembly | Both P4 |
| Tab persistence | Reset on change / per entity type / per instance | Admin UX | Inspector P2-3 |
| Canonical peer ID | did:key / hex bytes / keep all three | Every cross-track join | Both P4 |

---

## 5. Evidence Verification Log

All claims verified against codebase:

| Claim | Result |
|-------|--------|
| is_admin never set to true | 4 grep hits: decl, default, 2 reads. Zero writes. CONFIRMED. |
| NodeIdentity not in Bevy world | 0 grep hits in main.rs. CONFIRMED. |
| VersePeers never populated | 10 hits: definition, init, tests. Zero add_peer() calls. CONFIRMED. |
| "local-node" hardcoded | 14 hits across fe-database, fe-test-harness, fe-sync. CONFIRMED. |
| Role table petal-scoped only | schema.rs:130-137: node_id, petal_id, role. No entity_type. CONFIRMED. |
| Handshake uses hex key | handshake.rs:13: hex::encode(pub_key.to_bytes()). CONFIRMED. |
| verse_member uses peer_did | schema.rs:163-167: peer_did: String. CONFIRMED. |
| Role chip renders "admin"/"member" | plugin.rs:451-456: conditional on is_admin boolean. CONFIRMED. |
| DB stores "owner"/"editor" | rbac.rs:32: WRITE_ROLES = ["owner", "editor"]. CONFIRMED. |
