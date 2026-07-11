---
type: Decision Record
title: Hexon P2P Commons — Ratified Architecture Decisions
timestamp: 2026-07-11T00:00:00Z
status: ratified
source: ../../research/hexon-p2p-commons/report.md
tags: [decision-record, p2p, hexon, consistency, auth, transport]
---

# Decision Record: Hexon P2P Commons (2026-07-11)

Ratified by the project owner in a grilling session against the verified findings of the
hexon-p2p-commons research round (`research/hexon-p2p-commons/report.md`, 6/6 codebase
spot-checks passed). Each decision states the owner's originating position, the caveats the
evidence forced, and the resulting commitment. Track specs cite these as **§D1–§D6** rather
than restating them.

---

## D1 — Consistency contract: three tiers, auth carved out

**Originating position:** "Eventually consistent is enough at scale; at the local operational
level it should be near-realtime, and that's fine."

**Decision:**

| Tier | Scope | Target | Notes |
|---|---|---|---|
| T0 | Device-local (SurrealDB materialized view → Bevy ECS) | milliseconds | already the write path |
| T1 | Verse-live, **including the relay/registry seam** | best-effort sub-second p50 | tail explicitly **unbounded under churn** — documented, not promised |
| T2 | Federation / commons | eventual; seconds-to-minutes staleness | the honest hexon niche |

**Caveats that shaped it:**
- Choosing to include the seam in T1 (owner's call, ambitious end) pulls the gossip receive
  loop, BBR configuration, relay latency budgeting, and both local hot-path perf fixes
  *inside* the near-realtime contract (report §3 P1, P2, P5–P8).
- **Auth state is exempt from LWW at every tier.** Grants/revokes are signed causal-DAG ops
  with a strong-removal resolver. Plain LWW on auth reinvents the Matrix state-reset CVE
  (report §4.2, §5c-10). This is the single most important design invariant in the record.
- Digital-twin scope at T2 is the **observational shadow** — never closed-loop control
  (sub-2s isochronous deadlines are categorically incompatible with churned gossip+NAT+CRDT
  jitter, report §4.7).

## D2 — Transport: handshake-then-swarm

**Originating position:** "See the transport of shared assets through the lens of a handshake
and a *them* (an identified counterparty)."

**Decision:** The handshake authorizes **once, at the membership/capability boundary**
(ticket-invite, registry discovery — the machinery that already exists). Within the
authorized set, transfer is anonymous **content-addressed swarm fetch + relay-as-seeder**.
No per-asset pairwise negotiation.

**Caveats that shaped it:**
- Per-asset handshakes forfeit swarming and concentrate load on origin peers, worsening the
  seeding-economics problem (report §4.4).
- The "them" is often reachable only via relay anyway: ~70% independently-measured NAT
  hole-punch success (plan to the pessimistic end), and browser peers are relay-only by
  construction (report §3 P9, §6).
- Identified-counterparty semantics are preserved where they matter: authorization, refusal
  to serve (moderation, §4.6), and audit at the membership boundary.
- PII/erasable payloads are encrypted regardless of lane — erasure = key destruction
  (crypto-shredding, §4.3).

## D3 — Verse services: opt-in centralization, accelerator-only

**Originating position:** "Plugins should allow for opt-in per-verse centralization."

**Decision:** Per-verse centralization ships as a **plugin service class** that may seed,
cache, order-hint, and host presence — but the signed op-log remains the state of record and
**any member can reconstruct the verse without the service**. No sequencer authority in v1.
The relay and registry are re-framed as the first instances of this class.

**Caveats that shaped it:**
- The Hubs survival condition: local-first bytes + export is what lets a community outlive
  its operator; a service that becomes the sole authority rebuilds the dependency that killed
  Third Room (report §1, stage 4 §1.1).
- Every surviving comparable system kept a deliberate seam (Croquet reflector, Decentraland
  catalysts, ATProto relays) — this makes the seam explicit and elected rather than denied.
- Requires a new plugin capability class (long-running network service) — a real security
  surface; production HostEnv wiring is still open (analytics_extension_api residual).
- An electable *sequencer* (authoritative intra-verse ordering, which would soften the
  ACL-in-CRDT concurrent-removal problem) was considered and **deferred**, not rejected —
  revisit once accelerator-only services are proven.

## D4 — Storage/compute separation; serverless as an operating mode

**Originating position:** "Fabric/Snowflake storage-separated-from-compute, and the serverless
model as an operating mode for the loop."

**Decision (owner-refined):** The **signed op-log is the source of truth (WAL)**. SurrealDB
**remains the realtime operational representation** — the hot query surface — as a
materialized view rebuildable from the log. **fe-query routes by intent:** live/operational
queries → SurrealDB; history/time-travel/audit queries → op-log. The serverless operating
mode is a stateless worker that materializes from content-addressed hexon deltas and runs at
the seam as a D3 verse service.

**Caveats that shaped it:**
- Today the op-log cannot be the WAL: writes are DB-first, op-log writes are best-effort and
  non-fatal (replay history silently under-represents edits), and all signature sites are
  placeholders (report §2, stage 1). Inversion is committed work, not a rename — it lands
  with the delta-hexon format track.
- Separation does not solve seeding; it sharpens the need for relay-as-paid-seeder (§4.4) —
  which is also where the serverless compute runs. D3 and D4 are the same seam.
- The model applies to the **federation layer**. Local client peers stay fused
  storage+compute (Bevy needs materialized state in RAM).

## D5 — Preconditions and sequencing (not covered by the four positions)

1. **Real per-op ed25519 signing** replaces the 13 placeholder `sig: "00".repeat(64)` sites
   **before** auth work — auth verifies those signatures (report §5b-6, verification SC1).
2. **iroh 0.35 → 1.0 upgrade before Dec 31, 2026** (0.35 relay EOL; iroh 1.0 shipped
   2026-06-15). The `VerseReplicator` trait is the isolation seam (report §5d-12, §6).
3. **Unblock-now perf fixes ship first** and are independent of all format/auth decisions:
   try_send replication bridge, BBR config, node_log ring-buffer, sync-thread blocking read
   (report §5a).

## D6 — Documented non-promises

- **No time-bounded revocation** under partition (UCAN reality, §4.1); mitigate with
  short-lived capabilities + routine re-delegation.
- **No GDPR-hard erasure** for open-gossip hexons; crypto-shredding + local tombstones are
  the mechanism (§4.3).
- **Staleness per D1**; T2 twins are shadows, not control loops (§4.7).
- Every peer/relay keeps an **unconditional local denylist** and discretion to refuse to
  replicate (§4.6) — legal survival, not policy nicety.

---

*Drill-down: evidence and mechanism for every claim above lives in
`research/hexon-p2p-commons/report.md` (§ references) and its `stages/` files.*
