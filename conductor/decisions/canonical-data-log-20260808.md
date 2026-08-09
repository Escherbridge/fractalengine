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

## Pending ratification

None. D-CL1 through D-CL20 are canonical as of 2026-08-08. Product retention
durations and implementation/cutover gates remain work items, not unrecorded
protocol decisions.
