---
type: Track Spec
title: Hexon Deltas — Replayable Op-Log Hexons over P2P
tags: [spike, spec-only, hexon_delta_format_20260710]
timestamp: 2026-07-10T00:00:00Z
updated: 2026-07-11T00:00:00Z
resource: ./metadata.json
decisions: ../../decisions/hexon-p2p-commons-20260711.md
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

- **Op-log with HLC + lamport clocks:**
  `fe-database/src/types.rs::OpLogEntry { lamport_clock: u64, hlc_timestamp: String,
  node_id: NodeId, op_type: OpType, payload: serde_json::Value, sig: String }`.
  This is already "an append-only operation log" per node — the delta
  hexon's raw material exists structurally, it just isn't packaged or distributed
  as a hexon today.
  **CORRECTED 2026-07-11:** the `sig` field is a schema placeholder, **not** a real
  signature — all **13** construction sites across the workspace hardcode
  `sig: "00".repeat(64)` (verified by the hexon-p2p-commons round, report §8.1 SC1;
  e.g. `fe-database/src/role_manager.rs:111` carries a literal "placeholder signature"
  comment). Sovereign authorship must be **built** (decisions §D5-1), not repackaged;
  it is a prerequisite of this track, not an existing primitive.
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
| Per-op ed25519 signature | **Missing** — `OpLogEntry.sig` is `"00".repeat(64)` at all 13 sites; must be built first (decisions §D5-1) |
| Content addressing (blake3) | Exists (`fe-renderer/src/addressing.rs`) |
| Signed hexon manifests + registry/P2P distribution | Exists (Phase 6.5, Phase 8) |
| iroh P2P transport, gossip topics | Exists (`fe-sync`, `p2p_mycelium_completion_20260701`) |
| Delta-hexon manifest type (`hexon_type: "delta"`) | **New** |
| Replay/materialization function | **New** |
| Time-travel checkpoint tagging in a delta hexon | **New** (SurrealDB-side `VERSION` query exists live; offline equivalent doesn't) |
| Entry hash-chaining (sequence tamper detection) | **New** |
| Delta payload compression | **New** |

## Amendments (2026-07-11) — ratified decisions from the hexon-p2p-commons round

Grounded in `conductor/decisions/hexon-p2p-commons-20260711.md` (§D-refs) and
`research/hexon-p2p-commons/report.md`. These amendments are binding on the
implementation track that follows this spec.

### A1. Container: HashSeq/manifest-of-blobs for streamable types — not ZIP

The current hexon container (ZIP) places its central directory at EOF, so a
hexon must be **fully downloaded and fully unzipped into memory before even the
manifest is readable**, and assets are read before the signature check
(`fe-hexon/src/package.rs:120-238`, report §3 P3). This is disqualifying for
the two hexon types that grow or stream: **tilesets** and **delta ranges**.

- Streamable hexon types (delta, tileset) use an **iroh-blobs HashSeq /
  manifest-of-hashes** container: per-blob fetch + BLAKE3 verified range reads,
  mirroring 3D Tiles (blob-per-subtree, small Morton→hash manifest). Report §5b-5.
- ZIP is **retained** for small whole-archive hexons (scene/model) where
  whole-download semantics are acceptable.
- Decide this **before** implementing the delta format — changing container
  later is a format break.

### A2. Log-first write path (decisions §D4) — supersedes the out-of-scope note below

The original spec scoped out "changing the live op_log write path." Decision
§D4 reverses that: the **signed op-log becomes the source of truth (WAL)**;
SurrealDB remains the realtime operational representation as a rebuildable
materialized view; **fe-query routes by intent** (live/operational → SurrealDB;
history/time-travel/audit → op-log). Today's DB-first, best-effort op-log
writes (non-fatal on failure — replay history silently under-represents edits,
report stage 1) cannot anchor replay or the serverless operating mode. The
inversion is part of this track's implementation scope, sequenced after
§D5-1 signing.

### A3. Unify the two manifest-signing schemes

Two incompatible schemes coexist: `fe-format` signs canonical-JSON, `fe-hexon`
signs raw manifest bytes (report stage 1 §1.1). This spec's "reuse existing
machinery unchanged" claim is ambiguous until one scheme is chosen. The
implementation track must pick **one** (default recommendation: canonical-JSON,
since delta entries are JSON-shaped and canonicalization is already implemented
in `fe-format`) and migrate the other.

### A4. Consistency + distribution context

Delta distribution operates under the §D1 consistency contract (T2 eventual for
federation; T1 verse-live for gossip-carried hot deltas) and the §D2
handshake-then-swarm transport (deltas swarm within the authorized member set;
relay seeds). Compression (zstd, below) plus content addressing makes the delta
unit exactly what §D4's serverless workers consume.

## Out of Scope (this spec)

- Implementation of any of the above — tracked separately once this spec is
  reviewed.
- Conflict resolution / CRDT merge semantics for concurrently-authored
  deltas from different peers (a real and hard problem — deserves its own
  spec once this foundational shape is agreed). **Note (2026-07-11):** for the
  *auth* subset of ops this is no longer open — decisions §D1 mandates causal-DAG
  strong-removal, never LWW; see `auth_policy_pattern_20260710`.
- ~~Changing the live `op_log` write path — this is purely an
  export/distribution format layered on top of it.~~ **Superseded by A2
  (decisions §D4): the write path inversion is in scope for the implementation
  track.**

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
