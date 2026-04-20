# ACH Analysis: Cross-Track Implementation Strategy

## Central Question
**What is the correct implementation ordering and shared infrastructure strategy for the Inspector Settings and Profile Manager tracks?**

---

## Hypotheses

| ID | Strategy | Description |
|----|----------|-------------|
| H1 | Inspector First | Inspector P1-P4, then Profile P1-P4. Inspector creates stubs for profile data. |
| H2 | Profile First | Profile P1-P4, then Inspector P1-P4. Profile provides identity infrastructure. |
| H3 | Full Parallel | Both P1-P3 simultaneously, coordinated P4. |
| H4 | Phase 0 First | Shared infrastructure track first, then both tracks independently. |
| H5 | Interleaved | Alternating phases with shared infra sprint mid-way. |

---

## Evidence Catalog (15 items from 7 personas)

| ID | Evidence | Source |
|----|----------|--------|
| E1 | Single-identity assumption load-bearing (14+ "local-node" refs) | Historian, Archaeologist |
| E2 | `is_admin` is dead field (never set to true) | Contrarian, Journalist, Negative Space |
| E3 | PeerAccessEntry assembly orphaned (no owner for join) | Contrarian, Systems Thinker, Negative Space |
| E4 | Peer presence void (VersePeers always empty) | All 7 personas |
| E5 | Phase 4 coupling only (P1-P3 independent) | Systems Thinker, Historian |
| E6 | RBAC petal-scoped only (no verse/fractal roles) | Historian, Contrarian, Negative Space, Futurist |
| E7 | Fire-and-forget channels (no request-response) | Archaeologist |
| E8 | Role label mismatch (UI "admin" vs DB "owner") | Archaeologist |
| E9 | NodeIdentity not in Bevy world | Journalist |
| E10 | Gossip entirely stubbed | Journalist, Archaeologist |
| E11 | Tab system doesn't exist | Journalist |
| E12 | Profile starts from zero | Journalist |
| E13 | Identity switching cascade risk | Systems Thinker, Contrarian, Historian |
| E14 | Scale concerns at 50 peers | Futurist |
| E15 | Stale profiles vs persistent roles | Contrarian, Futurist, Analogist |

---

## Consistency Matrix

CC = Consistent, II = Inconsistent, NN = Neutral

| Evidence | H1 | H2 | H3 | H4 | H5 |
|----------|----|----|----|----|-----|
| E1 | NN | CC | NN | CC | NN |
| E2 | II | CC | II | CC | CC |
| E3 | II | II | II | CC | CC |
| E4 | II | II | II | CC | II |
| E5 | CC | CC | CC | CC | CC |
| E6 | II | NN | II | CC | CC |
| E7 | NN | NN | NN | NN | NN |
| E8 | II | NN | II | CC | CC |
| E9 | NN | II | II | CC | CC |
| E10 | NN | II | NN | CC | NN |
| E11 | CC | CC | CC | CC | CC |
| E12 | NN | CC | CC | CC | CC |
| E13 | NN | II | NN | CC | NN |
| E14 | NN | NN | NN | NN | NN |
| E15 | NN | NN | II | CC | CC |

---

## Inconsistency Count

| Rank | Hypothesis | II Count | CC Count | Assessment |
|------|-----------|----------|----------|------------|
| **1** | **H4: Phase 0 First** | **0** | **12** | No evidence contradicts it. Resolves all shared gaps. |
| 2 | H5: Interleaved | 1 | 8 | Nearly as good. Presence (E4) is the risk. |
| 3 | H1: Inspector First | 5 | 2 | Dead is_admin, no presence, orphaned assembly, RBAC gap |
| 3 | H2: Profile First | 5 | 4 | No NodeIdentity, stubbed gossip, no presence, cascade risk |
| 5 | H3: Full Parallel | 6 | 3 | **Worst.** Maximizes gaps falling through cracks. |

---

## Winner: H4 (Phase 0 First)

**Confidence: HIGH (0 inconsistencies / 15 evidence items)**

### Phase 0 Scope

| # | Work Item | Evidence Resolved | Size |
|---|-----------|-------------------|------|
| 0.1 | Insert NodeIdentity into Bevy world | E9 | Small |
| 0.2 | Replace 14+ "local-node" strings with actual DID | E1 | Medium |
| 0.3 | Define PeerRegistry resource (unified per-peer data) | E3, E15 | Medium |
| 0.4 | Build presence: SyncEvent::PeerConnected/Disconnected, populate VersePeers | E4 | Large |
| 0.5 | Wire is_admin from RBAC query (local DID + active petal) | E2, E8 | Medium |
| 0.6 | Decide RBAC scope: petal-only with inheritance OR entity_type/entity_id | E6 | Design decision |
| 0.7 | Canonicalize identity format on did:key across RBAC, handshake, session | E1, E8 | Medium |

### Execution Order

```
Phase 0 (shared infra)     ~1-2 sprints
    |
    +---> Inspector P1-P3   (parallel) ~3 sprints
    +---> Profile P1-P3     (parallel) ~3 sprints
    |
    v
Coordinated P4 integration  ~1-2 sprints
```

### Fallback: H5 if Phase 0 is rejected

If business needs demand visible progress first:
1. Inspector P1 → Profile P1 → Inspector P2 → Profile P2
2. **Explicit shared infra sprint** (presence, PeerRegistry, admin wiring, RBAC scope)
3. Both P3 in parallel → Coordinated P4

Critical: the shared infra sprint MUST be explicitly planned. If skipped, H5 degrades to H3 (worst option).

### Firm Rejection: H3 (Full Parallel)

6 inconsistencies. Maximizes integration risk. All 7 personas converge on shared infrastructure deficits that parallel execution cannot resolve.

---

## Caveats

1. **Phase 0 does NOT solve identity switching** (E13). That remains Profile P3's responsibility. Phase 0 only designs the IdentitySwitchEvent type.
2. **RBAC scope (E6) is a design question** that must produce a written decision before Inspector P3.
3. **Gossip unstubbing (E10) can be partial** — Phase 0 unstubs VersePeers via iroh events. Full ProfileAnnounce gossip remains in Profile P4.
