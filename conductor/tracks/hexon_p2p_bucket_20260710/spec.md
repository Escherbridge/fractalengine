---
type: Track Spec
title: Hexon P2P Bucket — 3D Visual IPFS
tags: [spike, spec-only, hexon_p2p_bucket_20260710]
timestamp: 2026-07-10T00:00:00Z
updated: 2026-07-11T00:00:00Z
resource: ./metadata.json
decisions: ../../decisions/hexon-p2p-commons-20260711.md
---

# Specification: Hexon P2P Bucket

**Track ID:** `hexon_p2p_bucket_20260710`
**Type:** Spec / design (no implementation this round)
**Status:** spec_only
**Goal alignment:** 3D P2P analytics engine on the hexon format — this spec is
the *content layer* half of that goal (see Relationship to Hexon Deltas below).

## Overview

Today a Petal's nodes anchor exactly one thing: a GLTF/GLB model, ingested
through a GLB-specific pipeline. This spec sketches generalizing that anchor
to **any content** — a single file of any MIME type, or a directory of files
— rendered in-scene via a placeholder when there's no native 3D
representation, downloadable through a REST endpoint, and distributed
peer-to-peer as a content-addressed, sovereign-authored **bucket** layered on
the existing hexon format. The result: a petal becomes a 3D-navigable,
peer-replicated bucket of arbitrary content — a "visual IPFS" — with the
same integration flexibility IPFS offers (any content type, content
addressing, pull-based replication), but anchored to 3D scene positions
instead of a flat namespace.

## Relationship to `hexon_delta_format_20260710`

The two specs are complementary halves of the same P2P hexon vision:

- **Deltas (`hexon_delta_format_20260710`) = the operations layer.** Replayable,
  ordered, signed *mutations* — "what changed" over time (op-log entries).
- **Bucket (this spec) = the content layer.** Content-addressed, signed
  *blobs* — "what exists" at a point in time (arbitrary files/directories a
  node anchors).

A node's `asset_id` mutation (moving a node, attaching a new asset) is a
delta; the asset bytes themselves live in the bucket. Deltas reference
bucket content by hash, the same way `OpLogEntry.payload` already references
`asset_id`/`content_hash` today — this spec does not change that
relationship, it generalizes what "asset" is allowed to be and how the bytes
get to a peer that doesn't have them yet.

## What already exists (verified 2026-07-10 — ground truth for this spec)

| Primitive | Status | Where |
|---|---|---|
| Content-addressed blob storage (blake3) | **Exists** | `fe-runtime/src/blob_store.rs` (`BlobHash`, `hash_to_hex`/`hash_from_hex`, `get_blob_path`), `fe-renderer/src/addressing.rs::content_address()` |
| Local HTTP asset delivery by hash | **Exists** | `fe-api/src/assets.rs::get_asset` — `GET /api/v1/assets/:content_hash`, immutable-cache headers, 404 on miss |
| `asset` table with generic `content_type: String` + `content_hash` | **Exists** | `fe-database/src/schema.rs` (`table "asset"`) — schema is already MIME-agnostic; the *ingestion pipeline* is not |
| Node → single asset link (`node.asset_id: Option<String>`) | **Exists, single-asset only** | `fe-database/src/schema.rs` (`table "node"`) — no directory/manifest concept yet |
| GLB-only ingestion + placeholder fallback | **Exists, GLB-only** | `fe-renderer/src/ingester.rs` (magic-byte GLB validation), `fe-renderer/src/loader.rs` (`asset_server.load("assets/placeholder.glb")` on missing/loading asset) — the placeholder *mechanism* exists, but only as a GLB-shaped fallback, not a generic "unknown content type → placeholder" contract |
| Sovereign authorship (ed25519 signing) | **Exists** | `fe-format/src/signature.rs::{sign_manifest, verify_manifest, verifying_key_to_did}` — already used for hexon manifests (Phase 6.5) |
| Signed hexon manifests + registry + local install | **Exists** | `fe-format` (Phase 6.5), `fe-hexon`/`fe-hexon-registry` (Phase 8) |
| P2P blob transport over iroh | **Stub only, not implemented** | `fe-network/src/iroh_blobs.rs` is 13 lines — `register_asset`/`fetch_asset` are the original Wave-1 scaffold signatures, never filled in. **This is the single biggest gap this spec identifies**: none of the "transported over iroh" vision exists yet at the blob layer, though iroh *is* wired up elsewhere (`fe-sync` gossip/replication, iroh-docs) |
| `GET /nodes/{id}/asset`, `GET /assets/by-id/{asset_id}` endpoints | **Landed 2026-07-11**, commit `3a97fc1` | `fe-api` — three GET endpoints total (`/api/v1/assets/{content_hash}`, `/api/v1/assets/by-id/{asset_id}`, `/api/v1/nodes/{node_id}/asset`), Viewer+ RBAC via `resolve_node_scope`, real `content_type`/`name`/`size` from the DB. See `fe-api/AGENTS.md` §assets for the full contract (including the noted "asset has no owner column of its own" caveat) — not duplicated here. **These currently read the DB/blob store directly from the async API-gateway thread**, not through the `DbCommand` channel (see Follow-up below). |

## Functional Requirements (design sketch — no implementation this round)

### FR-1: Generic asset model

**Description:** Extend the node/asset model beyond GLB. An asset is either
(a) a single file of any MIME type, addressed by `content_hash` exactly as
today, or (b) a **directory asset** — a set of files under one logical asset
id, described by a manifest (FR-2). A node's `asset_id` continues to point
at one asset record; that record's `content_type` distinguishes `single` vs
`directory` (or a new `kind` field does, if overloading `content_type` gets
awkward once real MIME strings are stored there).

**Sketch of acceptance criteria for the eventual implementation:**
- Existing GLB behavior is unchanged (a GLB asset is a `single` asset with
  `content_type: "model/gltf-binary"`).
- A non-GLB single-file asset (image, PDF, arbitrary blob) round-trips
  through ingestion → storage → `GET /nodes/{id}/asset` without being routed
  through `GltfIngester`.
- No 3D representation for a given `content_type` → the placeholder
  rendering contract (FR-3) applies instead of a hard failure.

### FR-2: Directory asset manifest

**Description:** A directory asset needs its own small manifest (distinct
from — but structurally similar to — the hexon `entries.json` from Phase
6.5) listing the files in the directory, each with its own `content_hash`,
relative path, and MIME type. The directory's own `asset_id`/`content_hash`
addresses the *manifest*, not a concatenation of the files.

**Sketch of acceptance criteria:**
- A directory manifest is a small JSON document, itself content-addressed
  like any other blob (turtles all the way down — no special-cased storage
  path for manifests vs. files).
- `GET /nodes/{id}/asset` for a directory asset returns the manifest by
  default (or a `?format=archive` variant that zips the whole tree — a
  decision for the implementation track, not this spec).
- Reuses `fe-format`'s existing `entries.json`/`AssetEntry` shape where
  reasonable rather than inventing a second, incompatible manifest format
  (see Open Questions).

### FR-3: Placeholder rendering contract

**Description:** Generalize `loader.rs`'s existing "missing/loading GLB →
`placeholder.glb`" fallback into a contract: any node whose asset
`content_type` has no native 3D representation renders as a placeholder
primitive in-scene (not necessarily the same `placeholder.glb` — a
directory asset, a PDF, and a missing-blob GLB probably want visually
distinct placeholders, e.g. shape + icon by content-type family).

**Sketch of acceptance criteria:**
- A small, extensible `content_type -> placeholder` mapping (default
  fallback for unrecognized types), not a hardcoded single placeholder
  asset.
- The placeholder is clickable/selectable exactly like a real model (reuses
  the existing selection system, `NodeManager`, raycasting — no new
  interaction model).

### FR-4: Download / upload endpoints

**Description:** Round out the existing read-only `GET
/api/v1/assets/:content_hash` (hash-addressed, already shipped) and the
in-progress `GET /nodes/{id}/asset` (node-addressed, concurrent work) with
an upload path: a capability/RBAC-gated `POST` that ingests a file or
directory, computes content addressing, and creates the `asset`
record(s)/manifest.

**Sketch of acceptance criteria:**
- Upload goes through the same RBAC gate every other mutation does (see
  `auth_policy_pattern_20260710` — this is exactly the kind of entry point
  that should call into the policy engine rather than hand-roll a check).
  Until the policy engine exists, reuse `fe-database::rbac::require_write_role`
  the same way every other mutating handler does today.
- Upload size limits mirror the existing 256 MB `GltfIngester` cap,
  generalized to all content types (with the cap itself possibly becoming
  content-type-aware — a directory asset's manifest should probably have a
  smaller cap than a single large binary).

### FR-5: P2P pull-through (fetch-by-hash from peers on miss)

**Description:** When `GET /api/v1/assets/:content_hash` or `GET
/nodes/{id}/asset` misses locally, instead of a flat 404, attempt a
peer fetch by hash over iroh-blobs before failing — this is the actual "3D
visual IPFS" behavior: any peer that has ever seen a piece of content can
serve it to any other peer that references it, without a central host.

**Sketch of acceptance criteria:**
- `fe-network/src/iroh_blobs.rs`'s `register_asset`/`fetch_asset` stubs get
  a real implementation (the concrete, non-negotiable prerequisite — nothing
  else in this FR works without it).
- A local-miss triggers a bounded-timeout peer fetch (matching the existing
  "rate-limit all peer inputs" / no-block-render-loop rules in
  `general.md`'s Safety & Security section) before returning 404.
- Successfully pulled content is cached locally (written to the blob store)
  so subsequent requests — from this peer or others — don't re-fetch.
- Every fetched blob's hash is verified against its claimed content_hash
  before being trusted/served further (the same "verify before trusting"
  principle already enforced for gossip messages in `general.md`).

### FR-6: Quota / GC

**Description:** A bucket that grows by peer replication needs bounds.
Extend the existing LRU asset cache concept (referenced in `fractal_mesh`'s
original scope: "LRU asset cache (2GB default, 7-day eviction)") to the
generalized bucket: per-petal or per-node-owner storage quotas, and garbage
collection for content no longer referenced by any live node/delta.

**Sketch of acceptance criteria:**
- A configurable quota (default mirrors the existing 2GB/7-day LRU
  precedent) triggers eviction of least-recently-used, unreferenced blobs.
- GC never evicts a blob still referenced by a live `node.asset_id` or
  directory manifest — reference counting (or a simpler "still reachable
  from the DB" reachability scan) gates eviction.
- Eviction is logged (`tracing::info!`) per `general.md`'s Failure
  Transparency rule.

## Amendments (2026-07-11) — ratified transport decisions from the hexon-p2p-commons round

Grounded in `conductor/decisions/hexon-p2p-commons-20260711.md` §D2 (transport)
and the research report's measured numbers. Binding on the implementation track:

- **Topology: handshake-then-swarm (§D2).** The "visual IPFS" is *not* an open
  DHT: authorization happens **once** at the membership/capability boundary
  (ticket-invite / registry discovery); within the authorized set, FR-5's
  fetch-by-hash is an anonymous content-addressed swarm fetch. No per-asset
  pairwise negotiation — that would forfeit swarming and re-concentrate load on
  origin peers. This is also what structurally avoids IPFS's "ask everyone"
  Bitswap discovery amplification: discovery is the registry's job, not a
  flood's.
- **Relay-as-seeder is part of the design, not a fallback (§D3/§D4).** Plan
  against the pessimistic end of NAT reality — ~70% independently-measured
  hole-punch success, and **browser peers are relay-only by construction**
  (sandbox forbids UDP). The relay/registry container doubles as the always-on
  paid seeder for cold content; FR-6's quota/GC applies to it with a larger
  budget. Optional hardening for high-value verses: erasure-code bucket content
  across members (Storj-style ~2.75x expansion) so no single seed is
  load-bearing (report §4.4).
- **BBR congestion control is a requirement, not a tuning knob.** iroh-blobs
  throughput differs ~30x between BBR (~40% of link) and CUBIC (~1-1.5%)
  (iroh#4286, report §3 P8). FR-5's implementation must verify and explicitly
  configure BBR on the iroh endpoint.
- **Container alignment:** large/streamable bucket content follows the
  delta-track's A1 amendment — HashSeq/manifest-of-blobs with BLAKE3 verified
  range reads, not ZIP — so a directory asset's files are individually
  fetchable/verifiable (this also answers Open Question 3's direction:
  per-file addressability falls out of blob-per-file + manifest).
- **Erasure/PII (§D6):** bucket content that may need erasure is encrypted at
  ingest (extend the existing ChaCha20-Poly1305 paid-hexon mechanism);
  erasure = key destruction (crypto-shred) + local tombstones. No GDPR-hard
  erasure promise for openly-swarmed content.

## Out of Scope (this spec)

- Implementation of any of the above — a future track once reviewed.
- Conflict resolution for concurrently-uploaded directory assets (relates to
  the same open CRDT question flagged in `hexon_delta_format_20260710`).
- Paywall/encryption for bucket content — `fe-hexon`'s existing
  ChaCha20-Poly1305 paid-hexon mechanism (Phase 8) is the model to extend
  if/when this is needed, not something this spec redesigns.
- Search/discovery over bucket content (what's in a petal's bucket, browsing
  by content type) — a UI-layer concern for a later track.

## Dependencies / Related Tracks

- `hexon_delta_format_20260710` — the operations-layer counterpart; both
  specs should be read together.
- `crate_registry_20260508` (Phase 8, Hexon Registry) — the existing
  fe-hexon/fe-hexon-registry machinery this generalizes rather than
  replaces (signed manifests, publisher DID, registry install/uninstall all
  reused as-is).
- `auth_policy_pattern_20260710` — the upload endpoint (FR-4) is exactly the
  kind of new mutating entry point that should be built against the policy
  engine, not a new ad-hoc RBAC check.
- The in-progress `GET /nodes/{id}/asset` endpoint (fe-api, concurrent work
  as of this spec) — FR-1/FR-4 should compose with it, not duplicate it.

## Open Questions

1. Does the directory-asset manifest (FR-2) reuse `fe-format`'s
   `entries.json`/`AssetEntry` type directly, or is a lighter-weight,
   non-hexon-packaged manifest more appropriate for "a directory a node
   happens to point at" vs. "a full signed, versioned hexon package"? The
   former avoids inventing a second format; the latter avoids overloading a
   format designed for install/registry semantics onto ad-hoc directories.
2. Where does the pull-through fetch (FR-5) live — in `fe-api` (HTTP-request
   time, synchronous-feeling), or should a miss instead enqueue a background
   fetch and have the HTTP layer return a "fetching, retry" status? Affects
   whether `fe-api` needs a direct dependency on `fe-network`'s iroh-blobs
   client or goes through a channel to the network thread (per the existing
   three-thread topology — `fe-api` should not itself own iroh state,
   consistent with `AGENTS.md`'s thread-isolation rules).
3. Is `content_hash` alone sufficient as the bucket's addressing scheme, or
   does directory-asset support need the 3-level address system
   (NodeID/AttrID/ItemID) already defined for hexons (Phase 6.5) — i.e.
   should a file *within* a directory asset be individually addressable
   without going through the manifest first?

## Follow-up (post-close-out, 2026-07-11)

The `GET /nodes/{id}/asset` family of endpoints landed during the same
ultrapilot run this spec was written in (commit `3a97fc1`) — the exists-table
above has been updated accordingly. That landing surfaced one concrete gap
this spec's FR-1/FR-4 (generic asset model, download/upload endpoints)
should account for:

- **`DbCommand::GetNodeAsset`/`GetAssetMeta` channel variants are needed so
  asset endpoints work channel-only.** The endpoints as landed read the DB
  and blob store directly from `fe-api`'s async gateway thread rather than
  going through the `DbCommand`/`DbResult` channel pattern the rest of the
  three-thread topology uses. This works today (`fe-api` already has direct
  read access per the Phase 2 "Direct API DB Reads" precedent), but as this
  spec's FR-1 (generic asset model) and FR-4 (upload endpoint, which
  *mutates*) land, the mutating half in particular should go through typed
  `DbCommand` variants rather than a second direct-write path growing next
  to the existing channel-based mutation path. See `fe-api/AGENTS.md`
  §assets for the current endpoint contract this needs to compose with (not
  duplicated here — that file is outside `conductor/**`).
