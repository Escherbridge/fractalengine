---
type: track-spec
title: Canonical Fractal Data Log
timestamp: 2026-08-08T00:00:00Z
status: active
resource: ./metadata.json
decisions: ../../decisions/canonical-data-log-20260808.md
---

# Canonical Fractal Data Log

## Purpose

Define the protocol and make only the safe local repairs required for a signed,
immutable per-verse operation DAG. SurrealDB becomes a rebuildable local
materialization and catalog; authenticated peers and relays later distribute
encrypted, BLAKE3-addressed segment shards.

## Status and boundary

Fable ratified **GO** for the protocol-design phase and **NO-GO** for
implementation or network rollout. Workstream G is blocked on owner approval
of every normative specification under `docs/spec/canonical-log/`.

No task in this track may enable iroh-docs, open relay replicas, enable inbound
P2P, or extend `WriteRowEntry` row-replication. The local editor must remain
fully functional.

## Ratified architecture

1. Every payload and segment is encrypted under its scope key; public data uses
   a published key rather than a plaintext pipeline.
2. A verse has one operation DAG and branch registry. Headers replicate
   verse-wide; petal-affine encrypted payload segments fetch only with scope
   capability and interest.
3. Capabilities use canonical binary Ed25519 certificate chains with delegation,
   attenuation, TTL, and scope epochs.
4. Checkpoints are Manager+-signed, independently replay-verifiable claims.
5. Canonical scalars are i64 nano-base-units. Float values are render-tier
   views only and cannot appear in signed bytes.
6. The encoding is deterministic CBOR; v1 uses BLAKE3 and Ed25519.
7. `op_id` is BLAKE3 of the complete signed canonical envelope bytes.
8. HLC wire state is `{ wall_ms: u64, counter: u32 }` with author-key tiebreak.
9. Tracking heads use deterministic CRDT convergence; detached reintegration
   uses an explicit multi-parent merge operation.
10. Preview messages are a distinct, rate-limited WS class and cannot enter the
    commit pipeline.

## Required outcomes

- Eight owner-approved normative specifications with named conformance tests.
- HARD-1 through HARD-5 complete; HARD-6 is in scope and complete unless
  explicitly deferred by the owner.
- One serial, integrated workspace test/lint/format sweep passes.
- iroh-docs stays unavailable and no network path is enabled.
