---
type: Track Spec
title: Hexon Deltas — Replayable Op-Log Hexons over P2P
tags: [spike, spec-only, hexon_delta_format_20260710]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Specification: Hexon Deltas

**Track ID:** `hexon_delta_format_20260710`
**Type:** Spec / design (no implementation this round)
**Status:** Draft
**Goal alignment:** 3D P2P analytics engine on the hexon format, with an extension storage/query API and Rhai/WASM scripting.

## Overview

The hexon format (`docs/hexon-format-spec.md`, Phase 6.5) currently packages
*static* assets — GLB models, skyboxes, terrain tilesets, GPX collections —
each with a signed manifest and content-addressed entries. This spec sketches
the next evolution: a **delta hexon** — an append-only, replayable log of
*operations* rather than a snapshot of *state*. The delta hexon is the unit
the P2P layer actually pulls: compressed, content-addressed, sovereign-authored
(each entry ed25519-signed by its originating node).

This is a **spec-only** track. No implementation this round — it exists to
capture the vision precisely, grounded in what already exists, so a future
implementation track can start from an accurate baseline instead of
re-discovering the primitives.

## What already exists (verified 2026-07-10 — ground truth for this spec)

- **Op-log with HLC + lamport clocks + signatures:**
  `fe-database/src/types.rs::OpLogEntry { lamport_clock: u64, hlc_timestamp: String,
  node_id: NodeId, op_type: OpType, payload: serde_json::Value, sig: String }`.
  `sig` is a hex-encoded ed25519 signature (`fe-database/src/op_log.rs`). This
  is already exactly "an append-only operation log" per node — the delta
  hexon's raw material already exists, it just isn't packaged or distributed
  as a hexon today.
- **Content addressing:** `blake3::hash(bytes)` in
  `fe-renderer/src/addressing.rs::content_address()`. The same primitive
  `fe-hexon`/`fe-format` use for asset entries (per MEMORY.md: "blake3 1" key
  dep). A delta hexon would reuse this unchanged — hash the serialized delta
  payload, not a GLB.
- **iroh 0.35 P2P transport:** `fe-sync` (replication, gossip topics per
  Verse/Petal — `p2p_mycelium_completion_20260701` Phases 3-4) and
  `fe-network/src/iroh_blobs.rs`/`iroh_docs.rs`/`gossip.rs` already move
  signed payloads and content-addressed blobs between peers. A delta hexon
  is the natural *unit* for `iroh-blobs` to distribute instead of (or
  alongside) whole-asset blobs.
- **HexonArchive packaging + publisher signing:** `fe-format/src/{manifest,
  entries,license,signature,archive}.rs` — `HexonManifest` already carries
  `publisher_did`, `version`, `signature`, `hexon_type`, `tags`. A
  `hexon_type: "delta"` variant is additive to this schema, not a rewrite.

## What is new (this spec's proposal)

### Delta-hexon manifest type

A `HexonManifest` with `hexon_type: "delta"` whose `entries.json` describes
an **ordered sequence of `OpLogEntry`-shaped records** (or a superset — see
below) instead of asset files. Each entry keeps its own `sig` (the
originating node's per-op signature) *in addition to* the outer manifest
signature (the packager's — which may be a different node re-publishing a
range it received via replication). This is the "sovereign authorship"
requirement: the manifest signature proves who *packaged* the delta; each
entry's own `sig` proves who *authored* that specific mutation, and the two
must be distinguishable so a relay repackaging deltas from many peers cannot
launder authorship.

### Op replay / materialization

A `replay(delta_hexon, base_state) -> state` function — conceptually the
inverse of how `op_log` entries are already applied when written (each
mutation already goes through `write_op_log()` before/alongside the SurrealDB
write per `AGENTS.md`/`workflow.md` conventions). Replay should be an
*offline* version of that same application logic, so a peer with only a base
snapshot hexon + a sequence of delta hexons can materialize current state
without a live connection to the origin node.

### Time-travel checkpoints

SurrealDB's existing `VERSION` clause / time-travel query capability
(referenced in `PLAYBOOK.md` Track 3 scope: "Time-travel: implement
query_petal_at_time(petal_id, timestamp)") is the live-DB analog of what a
delta hexon should support offline: given a target `hlc_timestamp` or
`lamport_clock`, replay only entries up to that point. A "checkpoint" is
simply a delta hexon whose last entry is tagged as a snapshot boundary
(compaction point), so replay doesn't have to walk the entire history from
genesis every time.

### Signature chain (sovereign authorship)

Each `OpLogEntry.sig` already exists per-entry. The delta hexon's contribution
is the **chain**: entry N's signature covers (payload, lamport_clock,
prev_entry_hash) so tampering with ordering or dropping an entry is
detectable — not just "this payload wasn't tampered with" (which `sig`
already proves) but "this sequence wasn't tampered with." This is new;
today's `op_log` entries are independently signed but not hash-chained.

### Compression

Delta payloads are small, repetitive JSON (`OpType` + typed payload). A
straightforward compression pass (e.g. zstd, matching `fe-hexon`'s existing
blob-encryption-adjacent dependency posture — `chacha20poly1305` is already a
key dep for paid hexons, zstd would be a new, uncontroversial addition)
before content-addressing keeps delta hexons small enough that "any peer/relay
can host hexons for all verses" (existing Hexon Registry goal) remains true
for high-churn petals too.

### Content-addressed P2P distribution

Reuse `fe-hexon`'s existing registry/distribution machinery (Phase 8,
already shipped) unchanged — a delta hexon is just a `HexonManifest` with
`hexon_type: "delta"`, so `fe-hexon-registry`, DHT+iroh distribution, and the
existing publisher-DID identity model all apply without modification. The
only new code is (a) the delta-specific entries.json shape, (b) the
replay/materialization function, (c) the signature chain.

## What exists vs what's new — summary table

| Primitive | Status |
|---|---|
| Per-op HLC + lamport clock | Exists (`fe-database/src/op_log.rs`) |
| Per-op ed25519 signature | Exists (`OpLogEntry.sig`) |
| Content addressing (blake3) | Exists (`fe-renderer/src/addressing.rs`) |
| Signed hexon manifests + registry/P2P distribution | Exists (Phase 6.5, Phase 8) |
| iroh P2P transport, gossip topics | Exists (`fe-sync`, `p2p_mycelium_completion_20260701`) |
| Delta-hexon manifest type (`hexon_type: "delta"`) | **New** |
| Replay/materialization function | **New** |
| Time-travel checkpoint tagging in a delta hexon | **New** (SurrealDB-side `VERSION` query exists live; offline equivalent doesn't) |
| Entry hash-chaining (sequence tamper detection) | **New** |
| Delta payload compression | **New** |

## Out of Scope (this spec)

- Implementation of any of the above — tracked separately once this spec is
  reviewed.
- Conflict resolution / CRDT merge semantics for concurrently-authored
  deltas from different peers (a real and hard problem — deserves its own
  spec once this foundational shape is agreed).
- Changing the live `op_log` write path — this is purely an
  export/distribution format layered on top of it.

## Open Questions

1. Does every `OpLogEntry` become a delta-hexon entry, or only a
   filtered subset (e.g. exclude high-frequency transform updates, include
   only structural changes)? Affects delta hexon size dramatically for
   digital-twin petals with continuous IoT/transform churn.
2. Who packages delta hexons — the originating node only, or can any
   replicating peer compact a range into a checkpoint (requiring the
   packager-vs-author signature distinction above)?
3. Does replay need to be deterministic across peers (i.e. is `OpType`
   application order-independent for a given lamport range, or are there
   ops whose effect depends on interleaving with concurrent ops from other
   nodes)? This interacts with the out-of-scope CRDT question above but is
   worth flagging now.
