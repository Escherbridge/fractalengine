# Source: CRDT-in-volatile-networks literature (PS-CRDTs, gossip anti-entropy)

Fetch date: 2026-07-11
Query intent: CRDT convergence time vs peer count / churn — closing p2p-mycelium unknown re: >100 peer behavior

## PS-CRDTs paper (ScienceDirect, "CRDTs in highly volatile environments")
- Test scenario: 100-node network, varying % of nodes that temporarily disconnect; disconnection causes a node to miss 5-20 updates before rejoining.
- Quantified finding: a 25% churn rate produces approximately a 4% increase in total network traffic across the CRDTs studied (i.e., churn cost is sub-linear/modest in their model — but their model is much smaller than fractalengine's target scale).
- General literature admission (multiple secondary sources agree): convergence TIME as a function of peer count is "usually overlooked" in CRDT literature; most papers measure correctness/traffic, not wall-clock convergence latency at scale.
- Standard mechanism: anti-entropy via gossip — each node periodically picks a random peer subset and exchanges deltas; convergence rate is "probabilistically bounded" by gossip fanout and topology, not given as a closed-form guarantee anywhere found.

## Relevance / gap assessment
- This is the closest published data to "CRDT behavior under churn" but it tops out at 100 nodes — still short of the >100-peer / >100k-entry unknown flagged in p2p-mycelium §8. The unknown is NOT closed; it is now bounded (100-node behavior is mild, but no data exists between 100 and "thousands," which is where iroh-gossip's own vendor claim of "a few thousand peers" would need to be tested against real CRDT payloads, not just gossip message delivery).

## Confidence: MEDIUM (peer-reviewed model, but small-scale relative to fractalengine's aspirational "federated commons" scale; extrapolation beyond 100 nodes is NOT supported by this source)
