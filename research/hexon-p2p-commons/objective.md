# Research Objective: Hexon as P2P Digital Twin Format — Performance, Limits, Mitigations

## Premise Under Examination

The hexon is a P2P digital twin format. FractalEngine acts as both **browser** and **client peer-server** for a fully distributed, resilient, self-permissioned and self-operated **federated 3D commons**. Every peer can author, host, replicate, and permission 3D world state (Verse > Fractal > Petal > Node) without central infrastructure.

## Core Research Questions

1. **Performance issues**: Where does this architecture hit performance walls — hexon serialization/verification, blob distribution, gossip fan-out, CRDT replication lag, 3D streaming under churn, embedded DB write amplification, render-thread coupling?
2. **Fundamental limitations**: What is *impossible or inherently degraded* in a fully distributed model — revocation, erasure, availability under churn, cold-start discovery, consistency vs. availability, Sybil resistance, moderation of a commons?
3. **Mitigations**: For each issue/limit, what are the concrete architectural workarounds — grounded in FractalEngine's actual primitives (hexon deltas/op-log, blake3 blob store, iroh 0.35, policy-pattern auth, hexon registry, relay containers) and in external state of the art?

## Relationship to Prior Research

- Builds on `research/p2p-mycelium/findings.md` (April 2026): iroh 0.35 pin rationale, iroh-docs LWW model, hash-reference decision, migration path, and its **known unknowns** (§8: throughput at scale, GC, private petals, mobile). This round must *extend and refine*, not repeat.
- Builds on `research/cross-track-alignment/report.md`: peer presence gaps, identity canonicalization (did:key), RBAC petal-scoping.
- Incorporates platform vision (2026-07-10): hexon delta file (append-only op log, replayable, content-addressed, sovereign-authored), policy-pattern auth (evaluate(subject, action, resource), deny-by-default), hexon registry + relay as federation seams.

## Stage Decomposition

| Stage | Focus | Model |
|---|---|---|
| 1 | Codebase: hexon format + data layer as digital-twin substrate | sonnet |
| 2 | Codebase: P2P/sync/replication/runtime data path bottlenecks | sonnet |
| 3 | External: P2P content distribution + CRDT performance SOTA | sonnet |
| 4 | External: distributed 3D worlds, digital twins, federated commons governance | opus |
| 5 | Verification + synthesis: fundamental limits, mitigation roadmap | opus |

## Evidence Standards

- Codebase claims cite `file:line` or module paths.
- External claims cite URLs; raw fetches captured under `raw/` per capture-then-filter convention.
- Confidence ratings (HIGH/MEDIUM/LOW) on every finding.
- Contradictions documented verbatim, not smoothed over.

## Biases to Guard Against

1. P2P maximalism — some problems really are easier with one well-known peer (the relay/registry already exist; use them honestly).
2. CRDT maximalism — convergence is not intent preservation.
3. Benchmark optimism — vendor benchmarks ≠ churned residential-NAT reality.
4. "Everyone online" bias — long partitions and cold storage are the common case for a commons.
