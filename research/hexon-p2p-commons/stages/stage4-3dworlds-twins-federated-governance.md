---
type: research-findings
stage: 4
date: 2026-07-11
title: "Distributed 3D Worlds, Digital Twins, and Federated Commons Governance — External Prior Art"
model: opus
confidence_scale: HIGH / MEDIUM / LOW per claim
---

# Stage 4 — What Comparable Systems and the Literature Actually Say

Scope: external research for the hexon P2P commons premise. Companion to the codebase stages
(1–2) and the P2P-distribution/CRDT stage (3). Raw fetches captured under
`research/hexon-p2p-commons/raw/`. This stage answers three questions with prior art:
(a) streaming 3D worlds without central servers, (b) digital-twin consistency requirements,
(c) permissioning/governing a federated commons — where prior attempts succeeded and failed.

Guardrails from `objective.md` are honored: no P2P/CRDT maximalism, no "everyone online" bias,
and the relay/registry are treated as honest infrastructure rather than a cheat.

---

## 1. Distributed 3D Worlds — Prior Art and Postmortems

### 1.1 The consistent pattern: everyone reaches for a server seam

Across every comparable system, **fully-serverless never survived contact with scale or law.**
The credible "open metaverse" projects all converge on *federation of per-world servers with a
documented protocol* — not a pure peer mesh.

- **Croquet / Multisynq** replicates *computation, not data*: identical deterministic logic runs on
  every client, so network traffic can be *exactly zero* while thousands of entities move in lockstep
  (HIGH — grokipedia.com/page/Croquet_Project; npmjs.com/package/@croquet/croquet). The catch is a
  **reflector**: a stateless central sequencer that orders and timestamps input messages. You can
  eliminate *state* replication but you still need *someone* to establish a total message order, and
  the whole model demands **100% deterministic** world logic (any `Date.now`/`Math.random`/float
  divergence breaks bit-identity). Multisynq's business *is* running reflector infrastructure
  (HIGH). **Lesson for hexon:** deterministic lockstep buys enormous bandwidth savings but requires
  (i) a total-order source — a centralization seam — and (ii) fully deterministic logic. Our
  CRDT/op-log model deliberately trades the sequencer away for eventual consistency; we therefore
  *cannot* obtain Croquet's zero-traffic lockstep. This is a real, named cost of the premise, not a
  gap to close.

- **Third Room** (Matrix-based 3D) — **paused; team laid off** (HIGH —
  mastodon.matrix.org/@thirdroom/110787604385806102). The stated cause was **economics**, verbatim:
  "funding the team to work full-time on Third Room was increasingly challenging — the macroeconomic
  environment did not lend itself to moonshot R&D projects, and Matrix as a whole continued to suffer
  from insufficient financial support." Technically it was sophisticated (bitECS + Three.js/WebGL2,
  SharedArrayBuffer lock-free scene graph, Rapier WASM physics, WebRTC DataChannels signaled over
  Matrix MSC3401). Tellingly, its *own roadmap* reached for an **SFU (selective forwarding unit)** —
  a server relay — because pure WebRTC mesh does not scale past small groups (n² connections)
  (HIGH — github.com/matrix-org/thirdroom README). **Lesson:** the killer was funding, not a
  technical wall; and even the "decentralised on Matrix" design conceded a server relay to scale.

- **Mozilla Hubs** — **shut down 31 May 2024**, handed to the Hubs Foundation, as part of Mozilla's
  org-wide restructuring — again **economics/organizational**, not technical (HIGH —
  support.mozilla.org/en-US/kb/end-support-mozilla-hubs). What let the commons *survive the sponsor's
  death* was the pre-existing **open-source + self-host (Community Edition) + a data-export tool**
  (glTF scenes, avatars, media). This is the single strongest external validation of the local-first
  premise: survivability under sponsor death depends on users *already holding their bytes* plus an
  export path (HIGH).

- **Resonite / FrooxEngine** is the most instructive live-world contrast because it *rejected* both
  pure-P2P and LWW. It is **host-authoritative**: a client's edit goes to the session **host**, who
  forwards it; conflicts use **version-based optimistic concurrency** — "if the host receives updates
  from more than one user for the same data with the same version number, the first is kept and later
  updates are **discarded**," and losers get a **rollback** to the authoritative value (HIGH —
  wiki.resonite.com Data_model_synchronization). Explicit tradeoff: "optimistic concurrency works best
  when most writes are **not contended**." **Lesson:** a team that has actually shipped real-time
  collaborative world-building chose a *single ordering point per session* and *rollback* over CRDT
  convergence, precisely because a live session needs one authoritative value *now*. Hexon's LWW-CRDT
  model converges eventually but *cannot* roll a peer back or present a single authoritative instant.

- **Substrata** (Glare Technologies) — native C++ **client/server**, one world of **>12,000 UGC
  objects at ~200 fps**, run-your-own-server federation with a promised published protocol + mesh
  format spec (MEDIUM — substrata.info/about_substrata; github.com/glaretechnologies/substrata).
  Same shape: per-world server as availability + ordering anchor, federation *between* servers.

- **Decentraland** — markets itself as decentralized, but the runtime backbone is the **Catalyst
  node**: a bundle (Content Server + Lambdas + comms) where "all content stored in a Content Server
  is **synchronized with the rest of the DAO Catalysts**" in a fully-meshed set (MEDIUM —
  docs.decentraland.org/contributor/architecture/catalyst). In practice this is a **small federation
  of DAO-run servers**, not a peer commons — the "decentralization" is governance (a DAO chooses
  Catalyst operators) layered over what is operationally a modest server cluster. *Negative-space
  note:* the official docs contain **no critique of their own centralization**; the honest reading is
  that Decentraland is federated-server, not peer-to-peer.

- **Solipsis / VAST** (academic P2P-NVE lineage) — fully serverless worlds over an **n-dimensional
  Voronoi overlay (Raynet)** with **area-of-interest** links to neighbours by *virtual proximity*
  (HIGH — inria.hal.science/inria-00337057v1; en.wikipedia.org/wiki/Solipsis). Last stable release
  **1.09, Feb 2009**; now an "inactive MMO." **This is a NEGATIVE-SPACE finding worth stating
  plainly:** the academic P2P virtual-world tradition produced excellent *interest-management theory*
  (AOI, Voronoi neighbour sets, emissive/receptive fields) but **no lasting deployed commons**. There
  is *no published postmortem* — they died of neglect and non-adoption, not a documented technical
  ceiling. The reusable inheritance is the **interest-management math** (only sync state within a
  peer's AOI), which hexon should adopt for gossip scoping; the cautionary inheritance is that
  technical elegance did not produce durability.

### 1.2 Takeaway for the premise

Every credible system keeps a seam: a reflector (Croquet), an SFU (Third Room), a session host
(Resonite), a per-world server (Substrata/Hubs), or a DAO-run Catalyst set (Decentraland). The
FractalEngine **relay + hexon-registry containers are that seam, and the honest move is to use them
deliberately** — as sequencer/backstop-seeder/federation anchor — rather than pretending zero
infrastructure. The premise's real differentiator is *survivability* (local-first bytes + export),
which is exactly what saved Hubs' community, not *serverlessness* per se.

---

## 2. 3D Content Streaming at Scale vs. Content-Addressed Fetch

### 2.1 What real 3D streaming relies on

**Cesium 3D Tiles** (OGC community standard 2019; **1.1 ratified 2023**) is the reference design
(HIGH — github.com/CesiumGS/3d-tiles ImplicitTiling; docs.ogc.org/cs/22-025r4/22-025r4.html). Its
load-bearing properties:

- **Hierarchical LOD, streamed on demand**: the client loads *only* tiles visible at the current
  camera pose, coarse-to-fine as you zoom.
- **Implicit tiling**: a quad/octree indexed by **Morton code**, giving **random access to any tile
  or a *range* of tiles** and k-NN/range queries — "only the root bounding volume is stored," so
  `tileset.json` stays tiny. Availability is packed into fixed-size **subtree** buffers "to bound the
  size of each availability buffer for optimal network transfer and caching."
- **Two implicit assumptions:** (i) the client can **compute the next tile's URL from spatial
  position** (Morton index → URL) without a round trip, and (ii) **HTTP range requests** let it pull
  a sub-range of a large glTF buffer without downloading the whole thing. Plus CDN cacheability of
  near-immutable tiles.

### 2.2 The tension with content addressing (and why it is bridgeable)

Content addressing (BLAKE3 hash per blob) gives verifiable, deduplicated, immutable blobs — but **the
hash is not computable from spatial position.** You must fetch a **manifest** (index→hash map) before
requesting a tile. That is *one extra cold-start hop* versus computed HTTP URLs (MEDIUM–HIGH,
analysis grounded in the 3D Tiles spec above and iroh-blobs primitives from
`research/p2p-mycelium/findings.md §2`).

Two properties rescue the fit:

1. **Manifest = the `tileset.json` analog.** Keep a small per-tileset map from Morton/tile index →
   BLAKE3 hash (naturally a HashSeq). Fetch the root manifest once, then descend — the coarse-to-fine
   LOD walk *hides* the one-hop manifest latency because you were going to fetch the root first anyway.
2. **BLAKE3 verified range streaming (bao).** iroh-blobs' BLAKE3 tree supports **requesting and
   verifying a byte *range*** of a blob without the whole thing (HIGH — this is a documented BLAKE3/bao
   property; iroh-blobs exposes it, consistent with p2p-mycelium §2). This *preserves the exact
   range-request property* that tile streaming needs, over content-addressed transport.

**The real design fork** is *hash-per-tile* vs *range-into-blob*:
- *Hash-per-tile*: each tile is its own content-addressed blob → perfect dedup, fetch from any peer,
  but a manifest per tileset and *many small blobs* (per-blob overhead, gossip chatter).
- *Range-into-blob*: one blob per **subtree**, range-request sub-tiles → matches 3D Tiles' subtree
  design and far fewer blobs, but coarser dedup (the whole subtree shares one hash).

**Recommendation (MEDIUM):** mirror 3D Tiles — **blob-per-subtree with BLAKE3 range reads**, indexed
by a small Morton→hash manifest. Only the cold-start manifest hop is inherently extra vs HTTP tiling,
and LOD descent amortizes it. This is a genuine *fit*, not a compromise — the one property to protect
in the hexon blob layer is **verified partial/range reads** (do not force whole-blob fetches for
large tiled assets).

---

## 3. Digital-Twin Consistency Requirements

### 3.1 Concrete numbers: twins have hard, fast deadlines

The IIoT real-time case study (Sensors journal) gives usable figures (HIGH —
pmc.ncbi.nlm.nih.gov/articles/PMC8704305/):

- Platform **cycle/refresh time: 2 seconds** ("one packet every 2 s").
- Best-config end-to-end delay (TSCH/TASA): **average 178 ms, median 148 ms**; "all packets arrive
  with a delay that does not exceed 2 s."
- Requirement class: **hard real-time, periodic/isochronous** — "if a network message gets lost the
  controller will be deprived of fresh input data." *Freshness is a correctness property*, tracked by
  a "freshness indicator."

**Blunt implication (HIGH):** a churned P2P commons **cannot serve hard real-time control loops.**
Gossip fan-out + NAT hole-punching + CRDT merge jitter is hundreds of ms to seconds under good
conditions and *unbounded under churn* — categorically incompatible with a sub-2s hard deadline where
a lost packet starves a controller. Hexon's honest niche is **soft-real-time, eventually-consistent
*observational* twins** (dashboards, monitoring, digital *shadows*), **not closed-loop control.**
High-rate control must stay on a LAN/broker; hexon federates the *shadow*, not the *control loop*.
This staleness tolerance (seconds-to-minutes) must be documented, not implied.

### 3.2 Why twin platforms are centralized — and the one bridge to federation

Every production twin platform centralizes on a **single source of truth**: Azure Digital Twins puts
the twin "in the middle of all services to ingest and aggregate," modeled in **DTDL** as a queryable
**twin graph**; AWS IoT TwinMaker binds asset properties to live telemetry with AWS IoT as the source
of truth (HIGH — medium.com/globant/modeling-digital-twins-8b758dc4b4d6 and vendor docs). **Negative-
space finding (HIGH): there is no production P2P/federated digital-twin platform.** Vendors treat
centralization as a *feature* — unified governance and one authoritative, navigable graph — which is
the antithesis of a commons. A federated twin commons is genuinely novel territory.

**The one bridge is event sourcing.** An event-sourced twin = an **append-only immutable event stream
+ a derived materialized view**. This maps *directly* onto hexon's stated "append-only op-log,
replayable, content-addressed" delta file (MEDIUM–HIGH, analysis). The payoff: given the same event
log, every peer materializes the *same* twin state (a deterministic fold over events) — this is the
one place the commons earns *strong* convergence for free, **provided op-log order is agreed**
(HLC timestamps + causal ordering). Commutative/idempotent event appends converge trivially;
non-commutative ops still need CRDT discipline. **The premise's twin story is strongest when a twin
is framed as a replayable event log, weakest when framed as a mutable shared graph.**

---

## 4. Federated / Decentralized Permissioning — The Two Hardest Limits

This is where the literature is most cautionary and most directly load-bearing on the premise.

### 4.1 The ACL-in-CRDT problem is genuinely unsolved in production

Ink & Switch's **Keyhive** states the difficulty exactly: access control "must travel with the data
itself and work without a central guard," with "no single source of truth about who can do what at
any given time" (HIGH — inkandswitch.com/keyhive/notebook/). Two canonical failure modes recur across
every source:

1. **Dual-admin mutual revocation** — two admins concurrently revoke each other; with no central
   authority there is no principled tie-break.
2. **The revoked actor's concurrent write ("back-dating")** — a user is revoked but concurrently makes
   an edit; because causal order differs across replicas, the edit can *survive on some peers*. "The
   system must prevent back-dating — where a revoked user's operations survive because they were
   causally ordered differently across replicas" (HIGH).

Keyhive's answer (no consensus; accept causal consistency; **blank removed nodes' key-tree paths
*after* all concurrent ops are merged**) and **p2panda's** independently-arrived **"strong removal"
resolver** are the two concrete designs (HIGH — p2panda.org/2025/07/28/access-control.html). p2panda
models each grant/revoke as a **cryptographically-signed group control op** carrying `previous` +
`dependencies` (a causal DAG), finds concurrency "bubbles" via depth-first search, and applies:
*removal/demotion invalidates that member's concurrent actions; mutual removals invalidate both;
dependents transitively invalidated.* p2panda concedes the ruleset "isn't universal."

**HARD LIMIT #1 (HIGH):** the *state of the art* for decentralized ACLs is **pre-alpha, unaudited
research.** Keyhive's own repo (beelay-core, keyhive_core, keyhive_wasm) carries "⚠️ **DO NOT use in
production** ⚠️" with no security audit as of 2025. **There is no commodity, production-proven
"convergent capabilities" library in July 2026.** A federated commons that needs *revocable roles* is
building on research, not infrastructure. The directly transplantable pattern for hexon is clear —
**model role grants/revokes as signed ops in the hexon op-log DAG and apply a strong-removal resolver
at materialization** (invalidate a revoked author's concurrent ops) — but hexon would be *implementing*
this, and it is subtle enough that Ink & Switch and p2panda each spent years on it.

### 4.2 Revocation cannot be time-bounded, and auth must not be plain LWW

**UCAN** — the natural fit for hexon's "policy-pattern auth" + `did:key` identity — is capability-based,
delegable, and **verifiable offline without contacting the issuer** (HIGH — ucan.xyz/specification/).
But its revocation caveats are the second hard limit (HIGH — ucan.xyz/revocation/), verbatim:
"Revocation SHOULD be considered the **last line of defense**"; revocations are **eventually-consistent
gossip block lists**, **immutable and irreversible** (a mistaken revoke → reissue a new delegation, you
cannot un-revoke); and there are **"no temporal guarantees"** — out-of-order delivery is typical and
"malicious actors may strategically **delay** revealing capabilities."

**HARD LIMIT #2 (HIGH):** in an offline-first system you **cannot bound the time to revoke.** An
offline (or adversarially-withholding) holder retains effective access until they sync the revocation.
This confirms and sharpens the p2p-mycelium finding "peers retain what they've already synced." The
only real mitigations are **proactive**: short-lived capabilities + routine re-delegation, *not*
reliance on revocation.

**The Matrix cautionary tale ties both limits together.** Matrix's **State Resolution v2** — the
largest deployed system for converging auth/membership state across a federation — produced **two
high-severity CVEs in 2025** (CVE-2025-49090 + one unallocated) and a named remediation, **Project
Hydra** (HIGH — matrix.org/blog/2025/07/security-predisclosure/; matrix.org/blog/2025/08/project-hydra;
synapse issue #15987 "Another case of state reset"). The attack is a **state reset**: a participating
server can "corrupt the chatroom's state by **resetting it to a prior value (e.g. reverting access
control or room membership to an earlier configuration**)" — i.e. **silently restore a removed admin
or revoked user by forcing older state to win.** The fix required a room-version upgrade (v12), and
"state reset" is a *recurring class*, not a one-off, after 6+ months of a dedicated project.

**This is the single most important design warning for hexon (HIGH):** a **naïve LWW over auth fields
reintroduces the Matrix state-reset bug** — an older "grant" can win on timestamp skew and restore a
revoked user. **Auth state MUST NOT be plain last-write-wins.** It must be a causal DAG of signed ops
with a strong-removal resolver and *monotonic, never-un-revoke* revocation. Getting decentralized
auth-state convergence right is a decade-long, CVE-generating problem even for a well-resourced,
widely-deployed protocol.

---

## 5. Commons Governance and Abuse

### 5.1 Sybil resistance: pick a trust root or accept spam

Sybil resistance requires **proof of personhood** — "one human = one account" without central KYC —
and the review literature is candid that **no approach solves {no central authority, privacy, low
friction, strong resistance} simultaneously** (HIGH — en.wikipedia.org/wiki/Proof_of_personhood;
arXiv 2008.05300). Web-of-trust/vouching (BrightID) is "promising" but "early stage" with
"significant challenges." **There is no cheap, privacy-preserving, fully-decentralized Sybil
resistance in 2026.** For hexon this means a *fully-open public* commons is intrinsically
Sybil-vulnerable; the realistic mitigation is **web-of-trust / invitation gating** — a new peer needs
a vouch/ticket from an existing member, which the existing **ticket-invite bootstrap already provides**
(from p2p-mycelium §6). Public unbounded membership and Sybil resistance are in fundamental tension;
the premise must pick a trust root (vouch chain), which it largely already has by being "private by
default."

### 5.2 Moderation: composability is for taste; illegal content stays per-operator

**ATProto composable moderation** (Labelers emit labels; clients/AppViews subscribe and choose) is a
genuine advance over **Mastodon's coarse defederation** (HIGH — bsky.social/about/blog/03-12-2024-
stackable-moderation). **But composability is not total.** Bluesky's own architecture doc is explicit:
there is a **mandatory baseline** — "we **hardcode** our in-house moderation… users cannot opt out of
the infrastructure layer entirely" — and **illegal content (CSAM) is handled at the *infrastructure*
layer, bypassing the labeling system entirely**; PDSs/Relays/AppViews retain "ultimate discretion over
what content they carry" (HIGH — docs.bsky.app/blog/blueskys-moderation-architecture). **Lesson:** even
the most decentralization-forward moderation design keeps a **non-optional, node-level baseline** for
illegal content. **"Fully self-permissioned" cannot mean "obligated to host anything."** Every hexon
peer/relay needs (a) an unconditional local denylist and (b) discretion to refuse to replicate — this
is a *legal survival* requirement, not a policy nicety.

### 5.3 Content addressing vs. erasure and illegal-content liability

The IPFS anonymity-abuse study is the empirical warning (HIGH — arXiv 2506.04307v1): in **24 hours**,
pinning services advertised **1.12M (Pinata) / 719k (Filebase) / 340k (Fleek)** CIDs; **5 matched the
Bad Bits Denylist** (one on all three), including a Bank-of-America phishing script and a phishing
page. Crucially, **the Bad Bits Denylist is enforced only on Protocol Labs' own gateways and is
*advisory* for every other node**, and attackers **re-chunk to change the CID** for identical bytes —
content addressing makes blocking *whack-a-mole*. KYC was trivially bypassed (temp emails; one service
needed only a wallet).

On erasure, the literature is unambiguous (HIGH — sciencedirect S0167739X19323003 "Delegated content
erasure in IPFS"; voussoir.net/writing/ipfs_misconceptions): "**enforcing data erasure across the
entire IPFS network is not feasible** due to its decentralized nature"; deletion is only enforceable
"in an IPFS **cluster** you control." **HARD LIMIT (HIGH): content addressing is fundamentally
incompatible with a hard right-to-erasure guarantee on the open network** — confirming p2p-mycelium's
LOW-confidence deletion finding and raising its confidence. The best hexon-compatible mitigation is
**crypto-shredding** (MEDIUM): put PII/erasable data in an **encrypted** hexon payload and make erasure
= **destroy the key** — the ciphertext bytes persist but become undecryptable, which several data
authorities accept as functional erasure. Secondary mitigations: tombstone-and-honor-locally
(cooperating nodes only) and confining PII to a controlled cluster (relay/registry), never to open
gossip. **Do not promise GDPR-hard erasure for open-gossip hexons.**

### 5.4 Availability economics: "nobody must seed" is the free-rider setup

Empirically, decentralized storage that *works* attaches a payment or endowment to seeding (HIGH —
Filecoin/Arweave/Storj comparisons). **Neither Filecoin nor Storj guarantees permanence** — Filecoin
data persists only while **storage contracts are renewed**; only **Arweave** targets permanence, via a
prepaid **endowment**. **"Free" seeding does not produce permanence.** Storj's reliability comes from
**erasure coding — split each file into 80+ pieces, any 29 reconstruct** (~2.75× expansion),
surviving many node departures with retrieval "as fast as centralized cloud." The classic **free-rider
tragedy** applies directly to a "seed if you feel like it" commons: the rational peer consumes and
evicts *others'* blobs from its LRU cache first, so availability is systematically under-provisioned.

Mitigations grounded in the economics (MEDIUM): (1) **erasure-code hexons across the members of a
Verse** so no single seed is load-bearing and modest redundancy survives churn (strictly better
durability-per-byte than naïve replication); (2) an optional **paid pinning tier** — the
**relay/registry container as an always-on seeder** — honest infrastructure for the "everyone offline /
cold storage" *common case* the objective flags; (3) **tit-for-tat reciprocity** (seeders get fetch
priority) to make local free-riding costly. **None makes a zero-infrastructure commons durable; the
relay-as-seeder is the realistic backstop** — and it already exists in the architecture.

---

## 6. Synthesis — Where the Premise Is Strong, Where It Must Concede

**Strong / validated by prior art:**
- *Survivability* (local-first bytes + export path) is what saved the Hubs community after sponsor
  death — the premise's genuine edge is durability, not serverlessness (HIGH).
- *Event-sourced twins* map cleanly onto the append-only hexon op-log and earn strong convergence for
  free when order is agreed (MEDIUM–HIGH).
- *Content-addressed tile streaming* is a real fit given BLAKE3 verified range reads + a small
  Morton→hash manifest; protect partial/range reads in the blob layer (MEDIUM).
- *Interest management* (Solipsis/VAST AOI theory) is directly reusable for gossip scoping (HIGH).

**Must concede / design against explicitly:**
- No hard real-time control over churned P2P → hexon twins are *observational shadows*, staleness
  seconds-to-minutes (HIGH).
- No time-bounded revocation, and **auth must be a signed causal DAG with strong-removal + monotonic
  revocation, never plain LWW** — or it *reinvents the Matrix state-reset CVE* (HIGH).
- No commodity decentralized-ACL library exists (Keyhive pre-alpha, unaudited) — hexon *implements*
  this subtle machinery itself (HIGH).
- No hard right-to-erasure on open gossip → crypto-shredding + controlled-cluster PII, and no
  GDPR-hard promises (HIGH limit; MEDIUM mitigation).
- No Sybil resistance without a trust root → keep vouch/invite gating; open public membership is a
  spam magnet (HIGH).
- No durable availability without incentives → erasure-code across a Verse + relay-as-paid-seeder;
  "nobody must seed" is the free-rider trap (HIGH problem; MEDIUM mitigation).

**The through-line:** every comparable system keeps a deliberate seam (reflector, SFU, host,
per-world server, Catalyst, baseline moderation, paid seeder). FractalEngine's **relay + hexon-registry
containers are that seam.** The premise is most defensible when they are used *honestly* — as
sequencer, federation anchor, denylist-enforcing baseline, and backstop seeder — rather than
disavowed. "Fully distributed" should mean *no single point whose death kills the commons*, not *no
infrastructure at all.*

---

## Confidence Summary

| Claim | Confidence | Key source |
|---|---|---|
| Every comparable 3D world keeps a server/relay seam | HIGH | Croquet reflector, Third Room SFU, Resonite host, Substrata/Hubs/Decentraland |
| Third Room & Hubs died of economics, not tech | HIGH | mastodon.matrix.org/@thirdroom; support.mozilla.org |
| Local-first bytes + export saved Hubs' community | HIGH | Mozilla Hubs shutdown docs |
| Croquet needs a central reflector + deterministic logic | HIGH | grokipedia/npm Croquet |
| Resonite chose host-authority + version rollback over LWW | HIGH | wiki.resonite.com |
| Academic P2P worlds (Solipsis) left no deployed commons / no postmortem | HIGH (neg-space) | Wikipedia/Inria |
| BLAKE3 verified range reads preserve tile-streaming range property | HIGH | 3D Tiles spec + iroh-blobs |
| Content-addressed tiling needs a manifest hop (extra cold-start latency) | MEDIUM–HIGH | analysis on 3D Tiles spec |
| IIoT twins are hard real-time (2s cycle, 178ms avg) | HIGH | PMC8704305 |
| P2P commons cannot serve hard real-time control loops | HIGH | derived from above |
| No production P2P/federated digital-twin platform exists | HIGH (neg-space) | Azure/AWS twin docs |
| Event sourcing bridges twins to CRDT/gossip | MEDIUM–HIGH | analysis |
| ACL-in-CRDT unsolved in production; Keyhive pre-alpha/unaudited | HIGH | inkandswitch.com/keyhive |
| Strong-removal resolver is the transplantable auth pattern | HIGH | p2panda.org |
| Revocation cannot be time-bounded offline | HIGH | ucan.xyz/revocation |
| Naïve LWW on auth reinvents Matrix state-reset CVE | HIGH | matrix.org security-predisclosure / Hydra |
| No cheap decentralized Sybil resistance in 2026 | HIGH | proof-of-personhood review |
| Even ATProto keeps a mandatory node-level baseline for CSAM | HIGH | docs.bsky.app moderation |
| Content addressing incompatible with hard erasure; crypto-shredding is best fit | HIGH limit / MEDIUM mitigation | IPFS erasure papers |
| "Nobody must seed" is the free-rider trap; erasure-coding + paid seeder mitigate | HIGH problem / MEDIUM mitigation | Filecoin/Arweave/Storj |
