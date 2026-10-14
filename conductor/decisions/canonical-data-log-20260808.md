---
type: decision
title: Canonical Data Log decision register
timestamp: 2026-08-08T00:00:00Z
status: ratified
---

# Canonical Data Log decision register

## Owner-ratified decisions

| ID | Decision |
|---|---|
| D-CL1 | Uniform scope-key encryption; public data publishes its key; erasure is crypto-shredding. |
| D-CL2 | One per-verse DAG and branch registry; verse-wide headers with petal-scoped sparse payload replication. |
| D-CL3 | Custom canonical-binary Ed25519 capability certificate chains with UCAN-inspired delegation semantics. |
| D-CL4 | Manager+ checkpoints are replay-verifiable claims, not exclusive authorities. |
| D-CL5 | Canonical scalar values are i64 nano-base-units; plugin and WIT APIs expose exact typed values. |
| D-CL10 | Routine expiry uses short capability TTLs; hard revocation uses scope epochs and replicated revocation operations. |
| D-CL11 | Key rotation is an in-log self-certifying operation; suspect history needs Manager+ disavow semantics. |
| D-CL12 | Tracking heads converge by deterministic CRDT rules; detached reintegration is an explicit multi-parent merge operation. |
| D-CL13 | Preview traffic is a distinct WS type with no `op_id` and no commit-pipeline route. |
| D-CL14 | HARD-1 uses log-first-strict behavior; a failed log write fails the transform. |
| D-CL15 | WS lag triggers fresh snapshots for subscribed petals. |
| D-CL16 | HARD-6 desktop analytics wiring is in scope. |
| D-CL17 | V1 payload AEAD is XChaCha20-Poly1305 with a fresh 192-bit CSPRNG nonce for every encryption under a scope key. Scope keys are delivered to authorized member devices through X25519 HPKE-style key wraps and rotate on every scope-epoch bump. D-CL7's primitive set is extended by XChaCha20-Poly1305 and X25519; `suite_id = 65535` is test-only. |
| D-CL18 | A regenerated key is a new principal. An Owner-countersigned in-log continuity-grant may link old and new principals for attribution/display only; it grants no retroactive authority. A disavow is rescindable only by authority strictly higher than its issuer, and an Owner-issued disavow is final. |
| D-CL19 | A checkpoint binds BLAKE3 of the lexicographically sorted frontier `op_id` list, its segment manifest, and materializer version. Multi-head tracking frontiers may checkpoint and GC when the normal replay/retention proof holds. Verse-scoped branch create, pause, retarget, and detach operations require Manager+ capability. |
| D-CL20 | Legacy multi-row contracts for GLTF import, node duplicate/rename, and Verse/Fractal/Petal creation remain intentionally deferred to SPEC-4/SPEC-8. HARD-1 supplies no invented canonical meaning for them, and canonical cutover remains blocked until their replay-safe materializers are specified. |

## Default-adopted decisions

| ID | Decision |
|---|---|
| D-CL6 | Deterministic CBOR using RFC 8949 deterministic encoding; definite lengths and no floats in signed bytes. |
| D-CL7 | Suite v1 uses BLAKE3 content addressing and Ed25519 signatures; version selects algorithm suites. |
| D-CL8 | `op_id` is BLAKE3 of the complete canonical envelope including its deterministic signature. |
| D-CL9 | Wire HLC is `{ wall_ms: u64, counter: u32 }`, with author-public-key tiebreak. |

Any conflict with this register requires a new `PENDING-RATIFICATION` entry;
agents must not silently diverge. Workstream G still requires project-owner
approval of the complete normative specification set.

## Ratified amendments and supersession

1. D-CL3 requires a distinct signature domain for every signed artifact type:
   `fe-oplog-v1`, `fe-capability-cert-v1`, `fe-checkpoint-v1`, and each future
   artifact's registered domain. A generic cross-artifact signature domain is
   forbidden.
2. Keyed-BLAKE3 topic blinding is the required private discovery construction.
3. The maximum capability lifetime is 24 hours. A reconnecting member obtains
   a freshly issued or renewed chain only after current authorization and epoch
   revalidation; an expired chain is never resumed.
4. The deletion of unsigned live-transform gossip supersedes the D-73 through
   D-78 live-transform feature. No send or receive path may be restored until a
   separately approved signed durable path exists.

## Workstream G unlock and implementation-phase decisions (2026-08-09)

The project owner approved the complete SPEC-1..8 set, unlocking Workstream G.
Owner approval now applies only to network rollout, relay seeding, and inbound
P2P. The following were ratified at unlock.

| ID | Decision |
| --- | --- |
| D-CL21 | `chacha20poly1305` (~0.10) and `x25519-dalek` (~2) are approved as normal, non-optional dependencies of `fe-canonical-log`. Both are MIT/Apache-2.0 and on the `deny.toml` allow list. Verified absent from `Cargo.lock` beforehand: iroh vendors chacha20/curve25519-dalek transitively but exposes no importable AEAD or X25519 API. |
| D-CL22 | ERRATA to `operation-envelope.md` §3.5: the nonce rule stays a single **unconditional 24 bytes**. The payload-bearing golden vector, which encoded 12 bytes, was regenerated rather than making the rule suite-conditional — nonce, signature, complete envelope, `op_id`, `payload_aad_envelope`, and `payload_aad_preimage` were all re-derived using the committed `.mjs` encoder so the bytes cannot diverge from the validator. The oracle now asserts the length. |
| D-CL23 | Workstream G proceeds on `main`, not in an isolated worktree. Wave gates are therefore scoped per package (`cargo check/test -p <crate>`) rather than `--workspace`, because a concurrent session holds uncommitted work in `fe-ui/**` and `fe-terrain/**`. |
| D-CL24 | Five deferrals ratified: (1) D-CL5's **WIT `s64` exposure** waits for a plugin-facing canonical operation API — the Rust `fe-sdk` newtypes land now, but wiring the WIT would ripple into `fe-terrain`, which a concurrent session owns; (2) SPEC-2 §4.6's **rotation-fork resolver** is deferred — forks are detected and retained with both successors inactive and no resolver; (3) **provisional CBOR key numbers** may ship for the four specs whose maps lack normative key tables, consolidated into one ratification index, with no cross-implementation interop claimed; (4) reserved **policy numbers** (preview rate cap, quarantine bounds, GC lease durations, retention) stay caller-parameterized with no hardcoded defaults; (5) all **pre-canonical `op_log` rows are inherently unsigned and untrusted** — no signature backfill and no dual-read, because those rows carry a placeholder `"00" x 64` signature and were never actually signed. `update_node_url_handler` remains a documented no-op-log carve-out. |
| D-CL25 | Author equivocation (SPEC-1 §3.4) is a first-class primitive, not an emergent property. `EquivocationKey { author_public_key, wall_ms, counter }` lives in the envelope module; two distinct `op_id`s sharing one key must quarantine **both** candidates and materialize neither. This was silently absent from the first implementation plan and was recovered by adversarial review. |

## Time-series direction decisions (2026-08-16)

Ratified after a seven-scout evidence review of the "P2P replicated time-series
graph DB" direction. Every claim below was verified against spec text and
shipped code; citations are in the review record.

| ID | Decision |
| --- | --- |
| D-CL26 | **Two-plane architecture.** The *operation plane* is the canonical log as built: signed intent, scene structure, authority. The *observation plane* carries telemetry as columnar hexons committed **by reference** — a small canonical-CBOR payload holding `{hexon_artifact_id, time_range, stats_hash, count}` — with live display on the SPEC-7 preview channel and durable history on batch commits. Verified additive: floats can never enter signed bytes (SPEC-1 §2/§4, enforced in `cbor.rs`), so the hexon interior stays an opaque sealed blob whose format is free (Parquet with floats is fine) while only exterior metadata lives in the CBOR signing domain. SPEC-3's `ObjectClass::{Shard,Tile,Asset}` and SPEC-5's `snapshot_manifest_id` slot already anticipate this shape. Rationale: per-reading signed operations are ~4 orders of magnitude too heavy — 1 000 sensors at 1 Hz produces ~43 GB/day of ~546-byte headers **per peer** plus ~1 h/day of signature verification; batching ~10⁴ readings per hexon collapses both. |
| D-CL27 | **Peer N-of-M replication is in scope now** (owner override of the recommended registry-floor-first sequencing). The distributed store will define replication factor, a placement/holder index, under-replication detection, and repair across peers. Verified greenfield: zero spec text or code exists for any of it; the only durability primitive today is the single-holder GC lease. Consequences accepted and recorded: (a) this is protocol design, not an implementation wave, so it enters as a specification phase (SPEC-9) rather than a code wave; (b) an availability/placement index discloses which principal holds which scope, colliding with the SPEC-3 blinded-discovery model, so a leakage analysis is a hard gate on SPEC-9; (c) peers still cannot underwrite availability alone, so SPEC-9 must state an explicit floor — whether accountable seeders, registries, or a declared best-effort promise. |
| D-CL28 | **Six pre-implementation gates ratified.** G1: a distinct `UnknownKind` quarantine reason plus per-reason budget partitioning — today the quarantine is a single reason-blind pool that declines new candidates on overflow and evicts oldest-first, so a high-volume new operation kind would starve an un-upgraded peer's own legitimate missing-parent backlog; the generic `CandidateVerifier` has no implementation yet, so this is cheap exactly once. G2: SPEC-4 erratum fixing the determinism bright line — `reduce()` may write only a deterministic *reference* to an external artifact, never read into one to compute state — plus an expressible "referenced artifact unavailable" outcome, which the infallible `reduce()` signature currently cannot represent. G3: a fifth `LaneClass` for independently addressable bulk artifacts, activating SPEC-3's dormant object classes. G4: manifest statistics tiering — coarse in the clear (HLC range, petal set, count), fine-grained sealed under the scope key, because column statistics are an information leak. G5: an affirmative mobile header-pruning rule (today only inferable from conformance test 14). G6: observation-plane source identity, so blended analytics can honestly satisfy SPEC-4 §7.3 reproducibility. |
| D-CL29 | **The observation plane and the durability protocol fold into Workstream G** (owner override of the recommended separate-tracks split), rather than becoming sibling tracks. Consequence recorded: G's remaining scope roughly triples, and the wave partition ratified on 2026-08-09 no longer covers it, so the program re-partitions into implementation waves and specification phases that run concurrently. Wave 3's already-grilled operation-plane scope is preserved intact and is not renegotiated. |

## SPEC-10 observation-plane decisions (2026-10-13)

The owner pre-approved SPEC-10 (`docs/spec/canonical-log/observation-plane.md`)
with the draft's recommended defaults (RUNBOOK §3 R7), so Phase B is no longer
gated on review. It still runs in campaign order, after slices A2–A6 and its
own code dependencies (`campaign-plan-spec10-20261013.md` §1.3). Each row
records the default; the alternatives and reasoning are in SPEC-10 §11.2. A
change from post-review lands as a dated SPEC-10 Errata entry and an amendment
to the row.

| ID | Decision | Status |
| --- | --- | --- |
| D-CL30 | **SPEC-10 as a whole.** The observation plane commits telemetry and device payloads as signed batches **by reference** (D-CL26); `iot_reading` and `device_payload` stay the queryable hot projection and become rebuildable from batches through the SPEC-8 stages (`legacy_only` default → `dual_emit_shadow` → `shadow_rebuild` → owner-gated `canonical_authoritative_local`); one ingest door serves scalars, device payloads, and decoded readings (R5). A sealer signs only observations its own ingest door accepted, with membership recorded at the door and never selected by a table scan or a replicated column. Rejected: one operation per reading, canonical-only storage, observations outside the log. | PRE-APPROVED 2026-10-13 (owner pre-approved the draft's defaults; post-review pending) |
| D-CL31 | **Dedicated `operation_kind = 5` (`observation_batch`).** Header rules: at least one parent, an encrypted non-empty payload, petal scope with no resource. Schema hash `BLAKE3("fe-schema-observation-batch-v1\0")`. An un-upgraded peer quarantines it under the G1 `unknown_kind` budget. Its reduction writes one reference row from the envelope and its own admitted payload and never reads the hexon. Rejected: kind 1 plus a registered schema (it would spend the `unknown_schema` budget shared with ordinary schema skew), an application-range kind. | PRE-APPROVED 2026-10-13 (owner pre-approved the draft's defaults; post-review pending) |
| D-CL32 | **Hexon container and Parquet profile.** A stored-zip `.hexon` (`HexonType::ObservationBatch`; members `manifest.json`, empty `entries.json`, `observations/readings.parquet`, `observations/payload_refs.parquet`; DOS-epoch timestamps) sealed as `BulkArtifact`/`shard`. segmentby = anchor node, orderby = ingestion HLC, required row-group statistics, **uncompressed** pages, a pinned writer (`created_by = fe-observation-hexon-v1`), byte-identical output for any input order, and a separate sealed statistics artifact for selective reads. Rejected: a bare Parquet blob, a new CBOR wrapper, zstd pages (the tree compiles no Parquet codec, and codec upgrades would break byte determinism), and entity = `(node_id, metric)`. | PRE-APPROVED 2026-10-13 (owner pre-approved the draft's defaults; post-review pending) |
| D-CL33 | **`preview_kind` registry.** `0` invalid, `1` = `telemetry_sample_v1` (canonical CBOR `{anchor, metric, value_nano: i64, units, recorded_at_ms}`, no floats), `2..=1023` reserved for specification registration, `1024` and above application/experimental. A registry-aware receiver drops unregistered and unhandled kinds; structural SPEC-7 decoding is unchanged. Live display travels as a preview and durable history as batches (SPEC-7 §7.1 rule 4). Codec only: no transport while CANON-WS is closed. Rejected: no registry, string-named kinds, per-verse negotiation. | PRE-APPROVED 2026-10-13 (owner pre-approved the draft's defaults; post-review pending) |
| D-CL34 | **Provisional wire numbers join the CDL-PROV ratification index** with no interop claim, under D-CL24(3): lane class 5 and AAD domain `fe-segment-bulk-artifact-v1`; the bulk-artifact inner-body keys; kind-5 payload keys 0–10 (key 10 `stats_stored_length` added to the planned 0–9 so every artifact reference carries its stored length, SPEC-6 §2.1 rule 4); the `ObservationSource`/`DecoderIdentity` keys and `source_class` values 1–5; the `telemetry_sample_v1` keys; and the domains `fe-schema-observation-batch-v1`, `fe-observation-source-id-v1`, and `fe-legacy-identifier-v1`. The legacy-identifier mapping must become the same function as SPEC-8's candidate scope derivation once that is ratified. Rejected: hold Phase B until ratification. | PRE-APPROVED 2026-10-13 (owner pre-approved the draft's defaults; post-review pending) |

## Pending ratification

No PENDING-RATIFICATION entries. D-CL1 through D-CL29 are canonical as of
2026-08-16. D-CL30 through D-CL34 are pre-approved as of 2026-10-13 and await
owner post-review; until it happens they bind implementation exactly as written.
Product retention durations, replication-factor numbers, and the
network-rollout gate remain work items, not unrecorded protocol decisions.
