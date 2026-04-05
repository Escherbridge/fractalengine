# Research Objective: P2P Embedded DB + Asset Sharing Architecture for fractalengine

## Context

fractalengine is a local-first 3D collaborative world editor with Verse/Fractal/Petal/Node hierarchy, currently storing GLB assets as base64-encoded strings inside SurrealDB rows. The target architecture uses iroh-blobs for content-addressed asset storage and iroh-docs for CRDT-based database replication across peers.

**Dependencies already in tree:** iroh-blobs 0.35, iroh-gossip 0.35, iroh-docs 0.35, libp2p 0.56, ed25519-dalek 2.2, surrealdb 3.0 (kv-surrealkv), blake3 1.

**Target conductor track:** `p2p_mycelium_20260405`

## Core Research Questions

1. **DB replication strategy:** Should SurrealDB state be synced via file-level replication (ship the entire `data/fractalengine.db` as an iroh-blobs entry) or via an op-log CRDT bridge where each write is emitted as an iroh-docs entry and replayed on peers? What conflict-resolution guarantees does each approach provide, and how do production systems handle the seam?
2. **Asset fetch + caching:** What are production-proven patterns for lazy peer-fetching of content-addressed blobs, cache eviction under disk pressure, and graceful handling of missing/unreachable content? How does Bevy's `AssetServer` cooperate with an async P2P fetch layer?
3. **Identity/trust:** How do real systems compose a single ed25519 keypair across identity (DID), iroh node ID, DB signing, and verse membership signatures without introducing key-reuse attacks or correlation risk?
4. **Offline-first semantics:** What divergence-handling strategies exist for peers that write while partitioned, and what are the user-facing contracts (last-write-wins, causal order, manual merge)?
5. **Comparable systems:** What architectural choices did Anytype, Logseq sync, Jazz.tools, Automerge+iroh, Tana, and Pillar make? Where did they succeed and where did they regret?
6. **Scaling curves:** What performance cliffs exist at 10/100/1000 peer scale for gossip fan-out, iroh-docs replication lag, blob discovery latency, and DB merge throughput?
7. **Bootstrap/discovery:** How do peers find each other without a central server, and how do they bootstrap trust into a new verse?
8. **Migration path:** How do we evolve from the current base64-in-DB approach to a hash-reference + blob-store approach without breaking existing `data/fractalengine.db` files or forcing re-import of GLBs?

## Success Criteria

- [ ] All 8 personas deployed with distinct search strategies
- [ ] Minimum 8-10 parallel searches per persona executed
- [ ] Contradictions and disagreements captured, not smoothed over
- [ ] Evidence hierarchy applied (primary > secondary > synthetic > speculative)
- [ ] Cross-domain analogies explored (git, IPFS, Dat, Hypercore, Scuttlebutt, etc.)
- [ ] Temporal range covered (Freenet → BitTorrent → IPFS → iroh era)
- [ ] Concrete migration plan producible from findings

## Evidence Standards

- Primary sources: iroh/n0-computer docs, SurrealDB docs, actual source code of comparable projects
- Secondary: conference talks, postmortems, engineering blogs
- All claims must cite source URLs or specific repo commits
- Confidence ratings required on every finding
- Contradictions must be documented verbatim, not resolved

## Perspectives to Consider

- Local-first software manifesto (Kleppmann et al.)
- Content-addressed storage trade-offs (IPFS/BitTorrent lineage)
- CRDT vs OT for collaborative state
- Sybil resistance without central authority
- Asset churn in creative tools (users delete, re-import, version)
- Mobile / constrained device participation

## Potential Biases to Guard Against

1. **"New shiny tech" bias** — iroh is relatively young; favoring it uncritically over battle-tested alternatives (libp2p raw, Hypercore, Matrix)
2. **CRDT maximalism** — assuming CRDTs solve all sync problems; they don't (they solve convergence, not intent)
3. **"Everyone will be online" bias** — under-modeling long partitions and cold storage
4. **Central-server nostalgia** — some problems really are easier with one well-known peer; treating pure P2P as ideologically required
