---
type: research-report
date: 2026-07-11
methodology: 4 parallel stages (2 codebase sonnet, 2 external sonnet/opus) + opus verification
builds_on: p2p-mycelium (2026-04), cross-track-alignment (2026-04)
title: "Hexon as a P2P Digital-Twin Format — Performance, Fundamental Limits, and a Mitigation Roadmap"
---

# Hexon as a P2P Digital-Twin Format: Performance, Limits, and How to Get Around Them

*Synthesis and verification lead report. Drills down into four stage files under
`research/hexon-p2p-commons/stages/` and the prior round `research/p2p-mycelium/findings.md`.
Every claim carries an evidence pointer (file:line or URL) and a confidence rating.*

---

## 1. Executive synthesis — an honest verdict on the premise

The premise is: *the hexon is a P2P digital-twin format, and FractalEngine is the browser and
client peer-server for a fully distributed, resilient, self-permissioned and self-operated
federated 3D commons.* After verifying the codebase stages against source and cross-checking the
external stages against published data, the verdict splits cleanly into three bands.

**Achievable, and genuinely the premise's edge — *survivability*.** The one property that saved
Mozilla Hubs' community after its sponsor died was that users *already held their bytes* plus an
export path (`stages/stage4-3dworlds-twins-federated-governance.md` section 1.1, HIGH). FractalEngine's
local-first substrate — a real SurrealDB store, a real content-addressed blob store, a real HLC
clock (`fe-database/src/op_log.rs:29-149`, verified HIGH) — delivers exactly this. "Fully
distributed" is defensible when it means *no single point whose death kills the commons*, not *no
infrastructure at all*. The event-sourced-twin framing (append-only op-log to deterministic
materialized view) maps cleanly onto the hexon-delta vision and is where the twin story earns
strong convergence for free (`stages/stage4...` section 3.2, MEDIUM-HIGH).

**Aspiration, not present reality — the "distributed" and "self-permissioned" parts.** This is the
uncomfortable core finding. The CRDT replication layer that is supposed to carry authoritative
Verse/Petal state is **100% mocked**: `IrohDocsReplicator` and `IrohPetalReplicator` both delegate
to an in-memory `MockVerseReplicator`, and the availability flag is hardcoded `false`
(`fe-sync/src/replicator.rs:235-237, 286-304`, verified HIGH). Gossip has a real *send* path but
**no receive loop** (`fe-sync/src/sync_thread.rs` — `topic.broadcast` at 538/678, no `topic.next`;
verified HIGH). Peer presence never populates at runtime. Per-op signatures are all
`sig: "00".repeat(64)` placeholders at **13** construction sites (verified by grep, HIGH — stage 1
said 11, the reality is worse). There is **zero** RBAC on the sync write path
(grep `role_manager|require_role|evaluate` in `fe-sync/src` returns no matches, verified HIGH). So today,
two peers on the same Verse exchange *nothing* authoritative; each is an island that silently
diverges. "Self-permissioned" currently means one thing only: possession of the namespace secret
grants unrestricted write to every table in the Verse.

**Fundamentally constrained — things no amount of implementation removes.** Revocation cannot be
time-bounded in an offline-first system (UCAN: "no temporal guarantees," HIGH). Convergent ACLs are
pre-alpha unaudited research (Keyhive carries "DO NOT use in production," HIGH). Content addressing
is incompatible with hard right-to-erasure (IPFS erasure literature, HIGH). Hard real-time control
loops (sub-2s isochronous deadlines) are categorically incompatible with churned gossip+NAT+CRDT
jitter (HIGH). Browser peers are relay-only *by construction* of the browser sandbox, not by a
fixable limitation (HIGH). None of these is a bug to close; each is a boundary to design against.

**Bottom line:** the commons is buildable and its durability edge is real, but the current codebase
is a well-architected *local-first single-player substrate with the P2P seams stubbed*, not a
distributed system. The gap between the platform-vision language ("sovereign authorship," "fully
distributed") and the code is large and must be stated plainly before any resilience claim is made
to users.

---

## 2. The gap between premise and present — the real-vs-mocked inventory

Before cataloguing performance walls, the baseline reality check. Much of the "P2P" surface is
scaffolding. This inventory is the load-bearing context for everything after it: *you cannot
benchmark a bottleneck in a code path that does not yet carry traffic.*

| Subsystem | Premise claims | Verified reality | Evidence | Confidence |
|---|---|---|---|---|
| Verse/Petal CRDT replication | Distributed authoritative state | Mocked — in-memory `HashMap`, no network I/O, availability flag hardcoded `false` | `fe-sync/src/replicator.rs:235-237, 286-304` | HIGH |
| Gossip | Live peer sync | Send-only; **no receive loop**; inbound transforms never reach Bevy | `fe-sync/src/sync_thread.rs:538,678` (broadcast); no `topic.next` | HIGH |
| Peer presence | "fully functional" (doc comment) | `PeerConnected`/`Disconnected` constructed **only in tests**; `peer_count` permanently 0 | `stages/stage2...` section 1.4 (exhaustive grep) | HIGH |
| Per-op signature | "Exists (`OpLogEntry.sig`)" | `sig: "00".repeat(64)` at **13** sites; zero real signing | grep verified (section 8 appendix) | HIGH |
| Sync-path authz | "self-permissioned" | No RBAC/policy consulted in `fe-sync`; namespace-secret = god-mode write | grep `fe-sync/src` returns 0 matches | HIGH |
| Revocation | Peer permissioning | Local-only; does not propagate; secret not rotated | `fe-auth/src/revocation.rs:22` deferred comment | HIGH |
| Blob fetch on miss | Fetch from peers | Stub — logs and returns nothing | `stages/stage2...` section 1.1 | HIGH |
| libp2p stack | (implied second transport) | Kademlia DHT builds but carries **zero** application traffic; separate identity | `stages/stage2...` section 3 | HIGH / MEDIUM (intent) |

The honest framing for users during this window (from `stages/stage2...` section 8 candidate mitigation 2):
multi-peer convergence is *aspirational, not implemented*. Everything in section 3 below is either a
bottleneck that already bites the single-user substrate, or a bottleneck the mock is currently
*masking* — real iroh-docs writes (network I/O, disk-backed redb, peer acks) will expose the
blocking replication chain the moment they replace the mock.

---

## 3. Performance issues catalog

Each row gives mechanism, evidence, severity, and the scale at which it bites. "Bites at" columns
mark single-user (1), and 10 / 100 / 1000 peers. A dash means "not the binding constraint at that
scale." A bold X marks where it becomes the *binding* constraint.

| # | Issue | Mechanism | Evidence | Severity | 1 | 10 | 100 | 1000 |
|---|---|---|---|---|---|---|---|---|
| P1 | EntityStore O(N) clone-on-write | `get()` clones full snapshot incl. unbounded `node_log` Vec; every append re-clones + reinserts | `fe-entity-store/src/lib.rs:136,198-207` | HIGH | **X** | X | X | X |
| P2 | 3x write amplification per transform | 1 SELECT + op_log insert + node UPDATE, all sync in DB thread | `fe-database/src/handlers/transform.rs:17-60` | HIGH | **X** | X | X | X |
| P3 | ZIP no-streaming / full-unzip-into-RAM | central dir at EOF -> whole hexon downloaded before manifest parses; assets read before sig check | `fe-hexon/src/package.rs:120-238` | HIGH | X | X | X | X |
| P4 | HexonTileSource all-tiles-in-RAM, no eviction | continent tileset held as `HashMap<String,Vec<u8>>` for process lifetime | `stages/stage1...` s3 (`hexon_source.rs:30-44`) | HIGH/MED | X | X | X | X |
| P5 | Blocking replication bridge | Two `bounded(256)` hops, blocking `.send()`, no `try_send`/drop | `fe-database/src/lib.rs:155`; `main.rs:113-120` | HIGH | - | - | **X** | X |
| P6 | Single-core sync runtime | `new_current_thread` runs *all* gossip/replica/blob I/O for the node on one OS thread | `fe-sync/src/sync_thread.rs:49` | HIGH | - | - | X | **X** |
| P7 | Sync fs::read on async runtime | synchronous `std::fs::read` inside async fn on the single-thread runtime stalls all queued work | `fe-sync/src/sync_thread.rs:377` | HIGH/MED | - | X | X | X |
| P8 | CUBIC-vs-BBR 30x throughput cliff | CUBIC ~1-1.5% of link vs BBR ~40%; defaults may leak CUBIC | iroh#4286 (`stages/stage3...` s1.2) | HIGH | X | X | X | X |
| P9 | Relay-only browser peers | browser sandbox forbids UDP -> 100% relay cost, always | docs.iroh.computer/deployment/wasm-browser-support | HIGH | - | X | X | **X** |
| P10 | Gossip fan-out / topic-count scaling | HyParView/Plumtree ~few-thousand *claimed*; each topic adds connection + routing-table cost | `stages/stage3...` s5 | MED/LOW | - | - | X | X |
| P11 | Tile manifest vs content-address friction | BLAKE3 hash not computable from spatial position -> one manifest hop before first tile | `stages/stage4...` s2.2 | MED | X | X | X | X |
| P12 | SurrealKV / redb growth under replication | append-only + versioned writes; redb vendor benches disclaim concurrent/scale case | `stages/stage3...` s7 | MED | - | - | X | X |

**The two that bite even a single user (P1, P2).** These are the most concrete perf bugs found and
they are independent of the entire P2P question. P1 is the sharper: `EntityStore::get` clones the
whole snapshot including an unbounded append-only `node_log`, `append_log` mutates the clone and
reinserts the whole thing (`fe-entity-store/src/lib.rs:136, 198-207`). For a digital-twin node under
continuous IoT position streaming — the exact workload the premise cares about — a single update is
O(N) in prior log length, and N grows without bound because nothing compacts `node_log` in the hot
cache. This is strictly worse than the durable-side write amplification P2 (which is O(1) per edit,
just 3x the operations). Both live in the *live* twin write path, so a busy sensor feed pays them on
every sample.

**The ones the mock is masking (P5, P6, P7).** The replication bridge chains two `bounded(256)`
crossbeam hops with blocking sends and no drop semantics (`fe-database/src/lib.rs:155` -> `main.rs:113`
-> sync thread). Verified nuance: the code is `tx.send(...).ok()` — crossbeam's bounded `send` blocks
when the buffer is full and only returns `Err` on *disconnect*, so `.ok()` swallows the disconnect
case but the *blocking* characterization is correct. Two sibling bridges in the same file correctly
use `try_send`+drop; the replication path is the one hop that can turn a stalled sync thread into a
frozen database. Today the mock's `HashMap::insert` is instant so it self-heals — but once real
iroh-docs writes (network round-trips, redb disk, live-sync acks) sit behind that hop, a burst of
~256 writes fills the buffer and every subsequent `CREATE`/`UPDATE` blocks the DB thread
mid-handler. P6 compounds it: the *entire* sync subsystem for a node runs on exactly one OS thread
(`new_current_thread`, `sync_thread.rs:49`) — no per-verse sharding, so 1000 peers on one Verse
means 1000-way fan-out serialized through a single core. P7 (synchronous `std::fs::read` at
`sync_thread.rs:377`, inside an async fn on that single thread) can stall that one thread on any slow
blob read, back-pressuring the whole chain.

**The externally-grounded ones (P8, P9, P11).** P8 is the single most actionable external number:
congestion-control choice is a 30x throughput lever (BBR ~40% of link vs CUBIC ~1-1.5%, iroh#4286).
FractalEngine must *verify and explicitly configure BBR* in the iroh transport — do not assume
defaults. P9 is a hard ceiling: any browser-hosted FractalEngine peer pays 100% relay cost forever
because the sandbox forbids raw UDP; WebTransport reaching "Baseline" in ~March 2026 is the
*precondition* for a non-relay browser mode, but iroh has not shipped it. P11 is mild and bridgeable:
content-addressed tiles need a small Morton-to-hash manifest fetched once, and the coarse-to-fine LOD
walk hides that latency because you were fetching the root anyway.

---

## 4. Fundamental limitations — what no implementation effort removes

These are boundaries, not bugs. For each: why it is fundamental, and the least-bad known pattern.

**4.1 Revocation cannot be time-bounded under partition.** UCAN — the natural fit for hexon's
policy-pattern + `did:key` identity — states it directly: revocation is "the last line of defense,"
revocations are eventually-consistent gossip block lists, immutable/irreversible, with "*no temporal
guarantees*" (ucan.xyz/revocation, HIGH — `stages/stage4...` s4.2). An offline or adversarially-
withholding holder keeps effective access until they sync the revocation. This sharpens the April
finding "peers retain what they've synced." *Least-bad pattern:* proactive short-lived capabilities
with routine re-delegation, never reliance on revocation to bound access.

**4.2 ACL-in-CRDT concurrent-removal is unsolved in production.** Two canonical failure modes recur:
dual-admin mutual revocation (no principled tie-break), and the revoked actor's concurrent write
surviving because causal order differs across replicas ("back-dating"). The state of the art —
Ink & Switch's Keyhive and p2panda's "strong removal" — is *pre-alpha, unaudited research*; Keyhive's
own repo carries "DO NOT use in production" (HIGH — `stages/stage4...` s4.1). *Least-bad
pattern:* model role grants/revokes as signed ops in the hexon op-log DAG (with `previous` +
`dependencies` causal links) and apply a strong-removal resolver at materialization — a revoked
author's concurrent ops are invalidated. Hexon would be *implementing* this subtle machinery, which
took Ink & Switch and p2panda years.

**4.3 Erasure is incompatible with content-addressing.** "Enforcing data erasure across the entire
IPFS network is not feasible" (HIGH — `stages/stage4...` s5.3); denylists are advisory off-gateway and
attackers re-chunk to change the CID. *Least-bad pattern:* crypto-shredding — put erasable/PII data
in an *encrypted* hexon payload; erasure = destroy the key, ciphertext persists but becomes
undecryptable (several data authorities accept this as functional erasure). Secondary: tombstone +
honor-locally, and confine PII to a controlled cluster (relay/registry), never open gossip. **Do not
promise GDPR-hard erasure for open-gossip hexons.**

**4.4 Availability economics — someone must seed.** "Free" seeding does not produce permanence:
neither Filecoin nor Storj guarantees it; only Arweave targets permanence, via a prepaid endowment
(HIGH — `stages/stage4...` s5.4). The free-rider tragedy applies directly — a rational peer evicts
*others'* blobs from its LRU first, so availability is systematically under-provisioned. *Least-bad
pattern:* erasure-code hexons across a Verse's members (Storj-style, ~2.75x expansion, any 29-of-80
reconstruct) so no single seed is load-bearing, plus the relay/registry container as an always-on
paid seeder for the cold-storage common case.

**4.5 Sybil resistance requires a trust root.** No approach solves {no central authority, privacy,
low friction, strong resistance} simultaneously in 2026 (HIGH — `stages/stage4...` s5.1). *Least-bad
pattern:* web-of-trust / invite-gating — a new peer needs a vouch/ticket from an existing member,
which the ticket-invite bootstrap already provides. A fully-open public commons is intrinsically
spam-vulnerable; the premise must pick a trust root, which "private by default" largely already does.

**4.6 Moderation — a commons cannot be obligated to host anything.** Even ATProto's composable
moderation keeps a *non-optional node-level baseline*: Bluesky hardcodes infrastructure-layer
moderation and handles illegal content (CSAM) below the labeling system entirely (HIGH —
`stages/stage4...` s5.2). *Least-bad pattern:* every hexon peer/relay needs (a) an unconditional local
denylist and (b) discretion to refuse to replicate. This is legal survival, not a policy nicety.

**4.7 CAP/PACELC — a churned twin cannot be both fresh and available.** IIoT twins are hard
real-time (2s cycle, 178ms avg end-to-end; a lost packet starves the controller — HIGH,
`stages/stage4...` s3.1). Gossip + NAT + CRDT-merge jitter is hundreds of ms to seconds under good
conditions and *unbounded under churn* — categorically incompatible with a sub-2s hard deadline.
*Least-bad pattern:* hexon's honest niche is *observational* digital shadows (dashboards, monitoring)
with staleness measured in seconds-to-minutes, **not** closed-loop control. High-rate control stays
on a LAN/broker; hexon federates the shadow, not the loop. This staleness tolerance must be
documented, not implied.

**4.8 Browser peers are structurally relay-dependent.** "Browser sandboxes don't support sending UDP
packets" — relay-only by construction (HIGH, s3/P9 above). *Least-bad pattern:* treat browser peers
as always-relayed first-class citizens; track WebTransport-based iroh browser mode as the eventual
partial-direct escape, but do not architect as if it exists yet.

---

## 5. Mitigation roadmap — sequenced against FractalEngine's actual primitives

Sequenced so earlier work does not get redone by later work. Each item names the concrete file or
track it lands in.

**(a) Unblock-now fixes — small, local, high leverage, no new subsystems.**
1. **Convert the replication bridge to `try_send` + bounded retry queue + drop-metric.** Make
   `replicate_row_with_petal` (`fe-database/src/lib.rs:155`) and the `main.rs:113-120` bridge match
   the `try_send`+log-and-drop pattern the two sibling bridges already use. Result: a stalled sync
   thread degrades to *replication lag* (observable via a drop counter), not *database freeze*. Do
   this **before** wiring real iroh-docs, or the mock stops masking the freeze and it ships to users.
2. **Verify + explicitly set BBR** in the iroh transport config (P8). One config line potentially
   worth a 30x single-stream throughput difference.
3. **Cap/ring-buffer the hot-cache `node_log`** (`fe-entity-store/src/lib.rs`): keep last-K in the
   in-memory snapshot, treat the durable SurrealDB op_log/VERSION as the source of truth for full
   history. Fixes P1's O(N) clone with a small local change.
4. **Add the gossip receive loop** (`fe-sync/src/sync_thread.rs`): the `SyncEvent::NodeTransformed`
   variant and `drain_sync_events` consumer already exist; wire `topic.next()` into the command loop
   and apply inbound transforms to the Bevy scene (the existing TODO). Without this the real send
   path broadcasts into the void.

**(b) Format evolution — do before committing the delta-hexon format, to avoid a format break later.**
5. **Delta-hexon as HashSeq/manifest-of-blobs, not ZIP,** for large/streamable types (tilesets,
   delta ranges). ZIP's central-directory-at-EOF forces whole-download-before-manifest (P3, verified
   `package.rs:120-238`); iroh-blobs `HashSeq` + BLAKE3 verified range reads (bao) preserve the
   partial/range-read property that tile streaming needs (`stages/stage4...` s2.2). Keep ZIP for small
   scene/model hexons where whole-archive semantics are fine. Mirror 3D Tiles: **blob-per-subtree,
   range-request sub-tiles, small Morton-to-hash manifest.**
6. **Real per-op ed25519 signing.** Replace the 13 `sig: "00".repeat(64)` placeholders with signing
   against the local `SigningKey`. This is the load-bearing precondition for "sovereign authorship"
   and for the strong-removal auth DAG in (c) — build it *before* auth, since auth verifies these
   signatures.
7. **Op-log compaction / ring-buffer + checkpoint snapshots.** Guards against the "naive full-replay
   exhausts memory" failure even modern CRDT engines hit (`stages/stage3...` s4), and gives cold-start
   peers a snapshot to sync from instead of O(all-history).
8. **Unify the two signature schemes** (canonical-JSON in `fe-format` vs raw-bytes in `fe-hexon`,
   `stages/stage1...` s1.1) before the delta format reuses "existing machinery unchanged" — specify
   *which* scheme.

**(c) Auth — depends on (b)6 (per-op signing) existing first.**
9. **Policy-pattern `evaluate(subject, action, resource)` on the sync write path.** Gate
   `handle_write_row_entry` (`fe-sync/src/sync_thread.rs:345`) behind an evaluate() call backed by
   the *existing* `role` table / `role_manager` — closing the Phase-8.4 gap with a mechanism that
   already exists rather than a new subsystem. This also gives revocation something real to disable
   (deny at evaluate-time) even before cross-peer revocation broadcast exists.
10. **Causal-DAG membership with strong-removal — never LWW-on-auth.** Model grants/revokes as signed
    ops in the op-log DAG; apply strong-removal at materialization. **Auth state MUST NOT be plain
    last-write-wins** — a naive LWW over auth fields reinvents the Matrix state-reset CVE (an older
    "grant" wins on timestamp skew and silently restores a revoked user — HIGH, `stages/stage4...` s4.2).
    This is the single most important design warning in the whole round.

**(d) Topology honesty.**
11. **Relay/registry as deliberate seam, stated as such:** relay-as-paid-seeder/super-peer, registry
    as the single load-bearing discovery path (which structurally avoids IPFS's "ask everyone"
    Bitswap amplification — `stages/stage3...` s2 — *because* discovery is pushed to the app layer).
    Size relay bandwidth against the pessimistic 70% NAT-success end, not 90-95% (see s6).
12. **iroh 1.0 upgrade before Dec 31 2026** (0.35 relay EOL). The April "isolate iroh behind a trait
    boundary" call was correct and is now on a concrete clock (`stages/stage3...` s1.1). The
    `VerseReplicator` trait already provides this seam.

**(e) Governance.**
13. **Vouch/invite gating** (already provided by ticket-invite bootstrap) as the Sybil trust root.
14. **Tombstone + crypto-shred erasure**; unconditional local denylist + refuse-to-replicate
    discretion on every peer/relay.
15. **Documented non-promises:** observational-shadow-not-control-loop staleness; no time-bounded
    revocation; no GDPR-hard erasure on open gossip.

**Ordering rationale:** (a) is independent and ships first (it also stops the mock from masking a
freeze). (b)6 signing precedes (c) auth because auth verifies signatures. (b)5 format choice precedes
any delta-hexon commitment because changing the container later is a format break. (c)10's
never-LWW-on-auth constraint must be designed in from the first auth line, not retrofitted.

---

## 6. Overturned prior findings — what this round revises from p2p-mycelium (April 2026)

| Prior claim (April) | This round | Evidence |
|---|---|---|
| "iroh 0.35 is the last production-quality version; 0.99 is current dev" | **Overturned.** iroh shipped **stable 1.0 on 15 June 2026**; 0.35 relay support EOL **Dec 31 2026** -> deliberate upgrade now on a clock | `stages/stage3...` s1.1 (iroh.computer/blog/v1) |
| "iroh-docs will be stable enough to depend on" (MEDIUM) | **Refined.** iroh 1.0 commits to wire-protocol stability, but Willow-backend suitability for redb/SurrealKV is a *new unclosed risk* | `stages/stage3...` s4, s7 |
| "LWW is acceptable for fractalengine v1" (MEDIUM) | **Sharpened / partially overturned for auth.** LWW is fine for name/position, but **LWW on auth fields reinvents the Matrix state-reset CVE** — auth must be a causal DAG with strong-removal | `stages/stage4...` s4.2 |
| Deletion / right-to-erasure (LOW confidence) | **Raised to HIGH limit.** Content addressing is *fundamentally* incompatible with hard erasure; crypto-shredding is the least-bad, not tombstone-alone | `stages/stage4...` s5.3 |
| "IrohPetalReplicator wraps MockVerseReplicator" (memory note) | **Verified correct and generalized** — `IrohDocsReplicator` is *also* mock-backed; availability flag hardcoded `false` | `fe-sync/src/replicator.rs:235-237,286-304` |
| "Per-op ed25519 signature exists" (delta-format spec ground truth) | **Overturned.** 13 placeholder sites; zero real signing | grep verified |
| NAT traversal ~90-95% (implicit vendor optimism) | **Overturned as planning number.** Plan against **70% +/- 7.1%** (libp2p DCUtR, 4.4M attempts, HIGH); iroh's 90-95% is vendor-claimed/LOW | `stages/stage3...` s1.3 |

---

## 7. Confidence table + remaining unknowns

**Verified this round (HIGH unless noted):**

| Finding | Confidence | Evidence |
|---|---|---|
| CRDT replication fully mocked | HIGH | `fe-sync/src/replicator.rs:235-237,286-304` |
| No gossip receive loop | HIGH | `sync_thread.rs` — broadcast only |
| 13 placeholder op-log signatures | HIGH | grep (13 sites) |
| No RBAC on sync write path | HIGH | grep `fe-sync/src` -> 0 |
| EntityStore O(N) clone-on-write | HIGH | `fe-entity-store/src/lib.rs:136,198-207` |
| ZIP full-unzip before sig check | HIGH | `fe-hexon/src/package.rs:190-237` |
| Blocking two-hop replication bridge | HIGH (mech) / MEDIUM (magnitude) | `lib.rs:155`, `main.rs:113` |
| Single-core sync runtime | HIGH (structural) / MEDIUM (saturation pt) | `sync_thread.rs:49` |
| BBR vs CUBIC 30x lever | HIGH | iroh#4286 |
| NAT ~70% independently measured | HIGH | ACM IMC '26 / arXiv |
| Browser relay-only by construction | HIGH | docs.iroh.computer |
| ACL-in-CRDT pre-alpha | HIGH | Keyhive/p2panda |
| No time-bounded revocation | HIGH | ucan.xyz/revocation |
| LWW-on-auth = Matrix state-reset | HIGH | matrix.org Hydra |

**Still-open unknowns (carried forward, none closed this round):**

1. **Mobile battery / background-execution — data vacuum.** No published measurement of iOS/Android
   background limits killing long-running QUIC, nor battery-drain numbers. Weakest-evidenced area in
   the entire question (`stages/stage3...` s6, LOW).
2. **Willow/RBSR backend suitability (redb, SurrealKV).** The RBSR spec makes its log-many-rounds
   guarantee *conditional* on the backend supporting efficient range-summarization; nobody has tested
   redb or SurrealKV against this. New unknown, not closed (`stages/stage3...` s4).
3. **Gossip topic-count scaling (N topics x M peers).** Per-Verse/per-Petal topic design multiplies
   subscriptions; no published data on the connection-budget cost. New unknown (`stages/stage3...` s5).
4. **SurrealDB schema evolution under replication.** No precedent for CRDT-synced SurrealDB; codebase
   question, not addressed externally.
5. **>100-peer benchmarks for iroh-docs.** Best available churn data tops out at 100 nodes (PS-CRDTs:
   25% churn -> ~4% traffic increase). The gap between 100 (measured, mild) and 1000s (vendor-claimed)
   is real and unclosed (`stages/stage3...` s4).
6. **Per-GB relay bandwidth cost** for iroh's hosted tiers — pricing page exists but not quantified;
   benchmark or price directly rather than assume (`stages/stage3...` s3).
7. **`fe-hexon-registry` own auth middleware** — not verified this pass; recommended follow-up since
   it is the single load-bearing discovery path.

---

## 8. Verification appendix

### 8.1 Spot-check outcomes (6 highest-impact codebase claims, all direct-in-source)

| # | Claim | Result | Evidence |
|---|---|---|---|
| SC1 | `sig: "00".repeat(64)` placeholder op-log signatures | **VERIFIED (and worse)** — grep returns **13** code sites, not the 11 stage 1 stated; includes `role_manager.rs:111` "// placeholder signature" | `fe-auth/src/revocation.rs:18`; `fe-database/src/{space_manager,queries,role_manager,handlers/transform,handlers/entity_property}.rs` (13 total) |
| SC2 | Blocking `.send()` on DB->sync bridge | **VERIFIED (with nuance)** — code is `tx.send(...).ok()`; crossbeam bounded `send` blocks when full and returns `Err` only on disconnect, so blocking semantics hold; `.ok()` swallows disconnect. Second hop `main.rs:113-120` identical pattern | `fe-database/src/lib.rs:155-162`; `fractalengine/src/main.rs:113-120` |
| SC3 | `IrohDocsEngineHolder::is_available()` hardcoded false | **VERIFIED** — reads an `AtomicBool` nothing ever sets true; `write_row`/`subscribe`/`close` delegate unconditionally to `MockVerseReplicator inner` | `fe-sync/src/replicator.rs:235-237, 286-304` |
| SC4 | Full-unzip-into-memory in `HexonPackage::open`, assets read before sig check | **VERIFIED** — assets read into `HashMap` at 190-206; `verify_manifest` runs at 223 *after*; read loop **not** gated on `sig_valid` | `fe-hexon/src/package.rs:120-238` |
| SC5 | No RBAC on sync write path | **VERIFIED** — grep `role_manager|require_role|require_scope|RoleLevel|evaluate(` in `fe-sync/src` returns **zero matches**; `handle_write_row_entry` applies `write_row` with no per-table/record check | `fe-sync/src/sync_thread.rs:345-397` |
| SC6 | EntityStore O(N) clone-on-write | **VERIFIED** — `get()` = `guard.get(node_id).cloned()` (full snapshot incl. `node_log`); `append_log` calls `get()` then `upsert()`s whole snapshot back | `fe-entity-store/src/lib.rs:136, 198-207` |

Bonus checks: single-core sync runtime **VERIFIED** (`new_current_thread`, `sync_thread.rs:49`);
sync `std::fs::read` on async runtime **VERIFIED** (`sync_thread.rs:377`); gossip send-only
**VERIFIED** (`broadcast` at 538/678, `subscribe()` at 915 is inside a test).

**Tally: 6 of 6 primary spot-checks passed (plus 3 bonus). 0 failed.** One claim was *understated*
by the source (SC1: 13 sites vs 11 claimed — the reality is worse than stage 1 reported, which
strengthens rather than weakens the finding).

### 8.2 Cross-stage consistency

Stages agree on all overlapping claims. Cross-validated points where two stages touch the same claim:

- **Op-log signatures:** stage 1 s0 (11 sites) and its s6.4 both assert placeholder-only; verified 13
  sites — consistent in kind, undercounted in degree.
- **iroh version timeline:** stage 3 s1.1 (iroh 1.0 shipped June 15 2026, 0.35 relay EOL Dec 31 2026)
  directly and explicitly supersedes p2p-mycelium s1 (0.35-is-last-prod, 0.99-is-dev). Both stages
  flag this as a supersession rather than contradicting silently — clean.
- **Relay dependency:** stage 2 s3 (no self-hosted iroh relay; `fe-relay` is an always-on peer, not a
  relay-protocol hop) and stage 4 s1.2 (relay/registry are the honest seam) are consistent — stage 2
  finds the absence, stage 4 prescribes the deliberate use.
- **LWW-on-auth danger:** stage 4 s4.2 (never LWW on auth) refines p2p-mycelium s6 ("LWW acceptable
  for v1") — the April note scoped LWW to name/position; stage 4 carves auth out explicitly. Not a
  contradiction, a scoping refinement, noted in s6.
- **Mock replication:** stage 2 s1.2 and MEMORY.md ("IrohPetalReplicator wraps MockVerseReplicator")
  agree; stage 2 generalizes it to `IrohDocsReplicator` too — verified SC3.

**No verbatim contradictions between stages were found.** The one apparent tension —
p2p-mycelium's "0.35 is production" vs stage 3's "iroh 1.0 shipped" — is an intentional, flagged
supersession (a time-shift, not a disagreement), documented in s6.

### 8.3 Uncited-claim flags

Claims appearing in a stage summary whose body support is thinner than the summary implies:

- **Stage 3 s5 iroh-gossip "few thousand peers":** carried as MEDIUM in the summary but the body
  correctly labels it "purely a qualitative vendor claim ... no published benchmark." Summary and body
  agree; flagged only because the number is load-bearing for P10 and should not be quoted as measured.
- **Stage 2 s7 "100 peers -> DB-thread stall under write burst":** the magnitude is inferred, not
  measured (the body says so: "no benchmark harness exists for this path"). The mechanism is verified;
  the saturation point is an estimate. Correctly rated MEDIUM in the body's confidence table.
- **Stage 1 s2 schema-evolution failure-mode:** the body self-flags MEDIUM ("didn't find an explicit
  test exercising this"). Consistent — no over-claim.
- **Stage 3 s1.1 "200M endpoints":** body correctly discounts it as a vanity metric (endpoint
  creations, not concurrent peers). Not used as a load-bearing capacity number anywhere. Clean.

No claim was found in a summary that lacked any citation in the corresponding body. The stages are
disciplined about confidence labeling; the only systematic pattern is that vendor-claimed numbers
(iroh NAT %, gossip peer count, endpoint count) are consistently down-rated to LOW/MEDIUM in the
bodies, which is the correct posture.

---

## 9. Decisions (added 2026-07-11)

The architecture decisions ratified against this report — consistency tiers with the auth
carve-out, handshake-then-swarm transport, accelerator-only verse services, log-first WAL with
SurrealDB as the operational view, sequencing, and documented non-promises — are recorded in
`conductor/decisions/hexon-p2p-commons-20260711.md` (§D1–§D6). The conductor tracks under
"Wave: P2P Commons Hardening" in `conductor/tracks.md` carry the resulting work.
