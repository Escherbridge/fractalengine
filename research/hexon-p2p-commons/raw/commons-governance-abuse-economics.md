# RAW: Commons Governance, Abuse Resistance, Availability Economics

Fetch date: 2026-07-11
Intent: Sybil resistance, moderation, illegal-content liability, GDPR-vs-content-addressing,
storage incentive results.

## Sybil resistance without central identity
Sources: https://en.wikipedia.org/wiki/Proof_of_personhood ,
https://arxiv.org/pdf/2008.05300 (Who Watches the Watchmen, PoP review), BrightID web-of-trust

- Sybil resistance = preventing one entity from controlling many identities. Core requirement:
  **proof of personhood (PoP)** = verifiable "one human = one account" WITHOUT central KYC.
- Decentralized approaches: web-of-trust / vouching / voting (BrightID), biometric PoP (Worldcoin-
  style), social-graph analysis. Review (arXiv 2008.05300) verdict: subjective/social approaches are
  "promising" but at "early stage" with "significant challenges in Sybil-resistance, decentralization,
  self-sovereignty, and privacy." NO approach solves all four simultaneously.
- HARD LIMIT: there is **no cheap, privacy-preserving, fully-decentralized Sybil resistance** in 2026.
  Every option trades away one of {no central authority, privacy, low friction, strong resistance}.
  => For hexon: a fully-open public commons is intrinsically Sybil-vulnerable. Realistic mitigation
  is **web-of-trust / invitation gating** (a new peer needs a vouch/ticket from an existing member,
  which the ticket-invite bootstrap already provides) — trading openness for resistance. This aligns
  with p2p-mycelium's "private by default, ticket-invite" decision. Public unbounded membership +
  Sybil resistance are in fundamental tension; pick a trust root (vouch chain) or accept spam.

## Federated moderation: ATProto composable moderation vs Mastodon defederation
Sources: https://docs.bsky.app/blog/blueskys-moderation-architecture ,
https://bsky.social/about/blog/03-12-2024-stackable-moderation

- Mastodon model: moderation tied to the server; abuse handled by **account suspension +
  DEFEDERATION** (a server cuts off another server). Coarse, all-or-nothing, community-splitting.
- ATProto "composable moderation": independent **Labeler** services emit **labels** on posts/accounts;
  clients + AppViews subscribe and choose how to act. Labels compose across apps; user-selectable.
- CRUCIAL NUANCE (from Bluesky's own architecture doc): composability is NOT total. There is a
  **mandatory baseline**: "In the Bluesky app, we HARDCODE our in-house moderation to provide a
  strong foundation... users cannot opt out of the infrastructure layer entirely."
- **Illegal content (CSAM) is handled at the INFRASTRUCTURE layer, NOT via labelers** — it "bypasses
  the decentralized labeling system entirely." PDSs/Relays/AppViews retain "ultimate discretion over
  what content they carry" and "service providers should actively detect and remove content that
  cannot be hosted in the jurisdictions in which they operate."
- => LESSON: even the most decentralization-forward moderation design keeps a **non-optional,
  centralized-at-the-node baseline** for illegal content + infra abuse. Composability is for TASTE
  and policy on legal-but-unwanted content; ILLEGAL content is a per-operator hard requirement.
  For hexon: every peer/relay that hosts others' hexons needs (a) a local denylist it enforces
  unconditionally and (b) discretion to refuse to replicate. "Fully self-permissioned" cannot mean
  "obligated to host anything" — each node must retain a veto for legal survival.

## Illegal content liability + GDPR vs content-addressing
Sources: https://arxiv.org/html/2506.04307v1 ("Anonymity Abuse in IPFS"),
https://www.sciencedirect.com/science/article/abs/pii/S0167739X19323003 ("Delegated content erasure
in IPFS"), https://voussoir.net/writing/ipfs_misconceptions

CONCRETE NUMBERS (IPFS anonymity-abuse study, 24h monitoring of 3 pinning services):
- CIDs advertised in 24h: **Pinata 1,124,780 | Filebase 718,578 | Fleek 339,684**.
- **5 CIDs** matched the Bad Bits Denylist in the window; one advertised by ALL THREE services.
  Retrieved 3 = BoA phishing JS, Korean webmail phishing page, a suspicious image.
- Gateway cache persistence: **2 of 5 gateways evicted after ~16h; 3 retained longer**. Content stays
  available while ANY node (uploader/pinner/cache) holds it.
- Denylist limits: the **Bad Bits Denylist is enforced only on Protocol Labs' public gateways; it is
  ADVISORY for all other nodes.** Attackers **circumvent it by changing chunk size → different CID**
  for identical bytes (content addressing makes blocking whack-a-mole: re-chunk → new hash).
- KYC bypass: Pinata/Fleek accepted a temp email; Filebase after 4 tries; 4EVERLAND needed only a
  crypto wallet, no email. => uploader anonymity is trivial.

GDPR / right-to-erasure (Delegated content erasure paper + misconceptions):
- "Enforcing data erasure across the entire IPFS network is **not feasible** due to its decentralized
  nature." Once a peer has a blob, deletion cannot be guaranteed network-wide.
- The ONLY place deletion is enforceable is a **cluster you control** ("in an IPFS CLUSTER,
  contrarily to a regular IPFS network, it is possible to ensure deletion from all peers"). I.e.
  erasure is only real within an operator's own administrative boundary.
- Pinning services can honor DMCA on THEIR storage but "this does not translate into deletion from
  the IPFS network."
- => HARD LIMIT: **content addressing is fundamentally incompatible with a hard right-to-erasure
  guarantee** in the open network. Confirms p2p-mycelium §"asset deletion" LOW-confidence finding.
  Mitigation options (all partial): (1) tombstone + honor locally (best-effort, cooperating nodes
  only); (2) **encrypt-at-rest with per-object keys, erase by DESTROYING THE KEY** ("crypto-shredding")
  — the bytes persist but become undecryptable, which several DPAs accept as functional erasure;
  (3) confine erasable/PII data to a controlled cluster (relay/registry), never to the open gossip.
  Crypto-shredding is the strongest hexon-compatible answer: put PII in an encrypted hexon payload,
  and erasure = revoke/destroy the key. Confidence MEDIUM that regulators accept crypto-shredding.

## Availability economics: who seeds when nobody must
Sources: makeuseof/securities.io Filecoin-vs-Arweave-vs-Storj comparisons,
https://arxiv.org/pdf/1901.03375 , general free-rider / commons literature.

- Incentive models: Filecoin pays FIL to miners per PROVEN storage; Arweave pays AR "farmers" from a
  one-time-fee **endowment**; Storj pays STORJ per byte + retrieval.
- Permanence results: **Neither Filecoin nor Storj guarantees permanence** — Filecoin data persists
  only while **storage contracts are renewed**; if a deal lapses, data may vanish. **Arweave** targets
  permanence via a crypto-economic **endowment** (pay once, store "forever" funded by declining
  storage cost). => permanence requires EITHER perpetual payment (Filecoin) OR a prepaid endowment
  (Arweave). "Free" seeding does not produce permanence.
- Storj reliability via **erasure coding**: split each file into **80+ pieces, any 29 reconstruct**
  (≈2.75x expansion), distributed globally → retrieval "as fast as centralized cloud" and survives
  many node departures. This is the concrete answer to churn: erasure coding >> naive replication for
  the same durability at lower storage overhead.
- FREE-RIDER lesson (classic commons tragedy applied to storage/bandwidth): a pure "seed if you feel
  like it" commons under-provisions availability — the rational peer consumes and evicts others' blobs
  from its LRU cache first. Empirically, decentralized storage that WORKS attaches a **payment or
  endowment** to seeding. => hexon's "nobody MUST seed" premise is exactly the free-rider setup.
  Mitigations grounded in the economics: (1) **erasure-code hexons across the members of a Verse** so
  no single seed is load-bearing and modest redundancy survives churn; (2) an OPTIONAL **paid pinning
  tier** (the relay/registry container as a paid always-on seeder) — honest infrastructure for the
  "cold storage / everyone offline" common case; (3) **tit-for-tat / reciprocity** (a peer that seeds
  gets priority fetch) to make free-riding locally costly. None make a zero-infrastructure commons
  durable; the relay-as-seeder is the realistic backstop, matching the existing relay container.

## Cross-cutting negative-space findings
- No production digital-twin platform is P2P/federated (all centralize on a "single source of truth").
- No lasting deployed academic P2P virtual world (Solipsis/VAST died of neglect, no postmortem).
- No production-audited decentralized-ACL library exists (Keyhive pre-alpha, unaudited, July 2026).
- No decentralized system achieves hard right-to-erasure or hard real-time control over open P2P.
- Every credible "open metaverse" (Hubs, Substrata, ATProto, Third Room's own SFU roadmap) retains a
  server/relay/baseline-moderation seam. The consistent pattern is FEDERATION-of-servers, not pure mesh.
