---
type: track-spec
title: Relay record warehouse — canonical log over the network (D-CL35..D-CL45)
description: Lift the canonical-log network gate. Relays hold sealed segments as the record, every scene write dual-emits, /ws/canonical is mounted, and a per-verse warehouse mode is set by the verse admin.
tags: [feature, relay_record_warehouse_20261120, pending]
timestamp: 2026-11-20T00:00:00Z
status: planned
resource: ./metadata.json
parent: ../canonical_data_log_20260808/spec.md
decisions: ../../decisions/canonical-data-log-20260808.md
---

# Relay record warehouse

## Purpose

Implement owner decisions D-CL35..D-CL45 (decision register, section "Network gate
lifted, relays as point of record (owner, 2026-11-20)"). Those decisions are the
authority for this track. When this spec and the decision text disagree, the
decision text wins and this spec is corrected.

In one paragraph: every scene write still runs the legacy path, which stays
authoritative. Each write also produces a signed canonical operation in the local
verified log. In a verse whose admin has turned on warehouse mode, the local log is
packed into sealed segments. Those segments go to the verse's dedicated record
relays, and each relay returns a signed custody receipt. Once R relays hold a
segment, it counts as recorded, and a peer may prune behind a checkpoint. Any peer
can rebuild its projection from relay-held segments. `/ws/canonical` is mounted
with token-then-capability auth. Distributed timeseries partials leave the DB
thread, and pulled observation segments can be scanned locally for ad hoc BI.

## Background

The parent track (`canonical_data_log_20260808`) built the protocol, the
verifiers, and the storage half locally. It deliberately left every network path
unwired. The deferral audit (`campaign-plan-w4-20261013.md` §6) lists these
deferrals, now in scope: segment production; a durable `ProofEnvironment` /
`ParentWalk` / `PayloadAdmission`; the §10.2.4 wrap delivery transport; the
`/ws/canonical` mount; CANON-DUAL-EMIT; and local capability-chain issuance. The
§trait-composition-map in `fe-canonical-log/src/AGENTS.md` still shows three
traits with no production caller: the wire `CapabilityVerifier`,
`ProofEnvironment`, and `CanonicalCommitPipeline`.

Ledger rows this track closes: CDL-NET, CANON-WS, SYNC-SIGNED-PLANE (remainder;
the `__shards` slice belongs to `role_propagation_20261013`), CANON-DUAL-EMIT
(for wiring only; the cutover stays gated), and CDL-WRAP-DELIVERY.

## Decision → requirement map

| Decision | Requirement(s) |
| --- | --- |
| D-CL35 gate lifted in full | FR-1 relay seeding, FR-2 inbound canonical P2P, FR-6 `/ws/canonical` mount, FR-9 signed sync plane |
| D-CL36 relays are the point of record | FR-1, FR-3 rebuildable projection |
| D-CL37 dual-emit everything, no cutover | FR-4, NG-1 |
| D-CL38 pre-GA, resettable | FR-11, NG-2 |
| D-CL39 durability floor = R relays | FR-5 |
| D-CL40 distinct retryable "held" commit answer | FR-7 |
| D-CL41 token, then capability chain | FR-6 |
| D-CL42 on in our build | FR-10, NFR-3 |
| D-CL43 per-verse warehouse mode, admin knobs, blind by default | FR-8 |
| D-CL44 ts partial off DB thread; columnar BI scan | FR-12, FR-13 |
| D-CL45 one relay, one verse | FR-14 |

## Functional requirements

Each FR names the decision it serves. Priority: P0 = the track is not done
without it.

### FR-1 Relay seeding of sealed segments (D-CL35, D-CL36) — P0

A peer in a warehouse-mode verse packs its admitted canonical operations into
SPEC-6 sealed segments: a header segment, payload shards, HashSeq nodes, and a
manifest. Every seal goes through `segment::artifact::seal_artifact` (the single
seal entry point). The peer offers each segment to the verse's designated record
relays. A relay accepts a seed only from a requester holding a currently valid
SPEC-3 `seed` capability for that verse (SPEC-6 §6 rule 2). It serves a fetch only
to a requester holding `fetch`. The transport is opaque, BLAKE3/HashSeq-addressed
bytes over the existing iroh-blobs `BlobTransport`. fe-sync never parses a
manifest.

Acceptance:
- AC-1.1 A segment sealed on peer A is fetched from relay R after A is offline. It
  replays on peer B to a projection root byte-identical to A's (harness
  scenario `relay_record_warehouse`).
- AC-1.2 A seed attempt without a valid `seed` capability is refused, and the
  relay stores nothing. A fetch without `fetch` is refused with the same response
  for "absent" and "not authorized" (no existence oracle).
- AC-1.3 The relay re-hashes every received artifact (SPEC-6 receipt rules) and
  never stores an artifact whose bytes do not hash to its id.

### FR-2 Inbound canonical P2P admission (D-CL35) — P0

Segments fetched from a relay or a peer are untrusted input. In order:
`segment::artifact::admit_sealed`, then the receipt re-hash and cross-check, then
the checkpoint proof over a durable `ProofEnvironment` (with durable `ParentWalk`
and `PayloadAdmission`), and then admission of each operation through
`admit_and_append_durably` with the production verifier
(`canon_log::verifier::durable_candidate_verifier`). No second admission path
exists. The raw append stays `pub(crate)`.

Acceptance:
- AC-2.1 A segment carrying a forged signature, a mismatched `op_id`, an
  unreferenced payload, or a sealed artifact under a retired epoch key is
  refused. Nothing is appended, and the refusal is counted.
- AC-2.2 A segment whose parents are not yet held lands in bounded quarantine
  (SPEC-5 per-reason budgets). It is admitted once the parents arrive, and is
  never materialized before that.

### FR-3 Every SurrealDB is a rebuildable projection (D-CL36) — P0

The projection a peer or a keyholder relay materializes from admitted canonical
operations can be dropped and rebuilt from relay-held segments alone, with the
same projection root. A blind relay (the default, FR-8) cannot open payloads. Its
rebuildable projection is its artifact catalog plus header-level metadata it can
verify without a scope key. Its legacy-row replica keeps replicating as it does
today.

Acceptance:
- AC-3.1 Drop a peer's canonical projection tables, re-fetch the segments from
  relays, and replay: the projection root equals the root before the drop.
- AC-3.2 Drop a blind relay's artifact catalog, re-scan its blob store: the
  catalog is identical.

### FR-4 Dual-emit every scene write (D-CL37) — P0

Every command in `fe_runtime::scene_write::SCENE_WRITE_CENSUS` passes through one
dual-emit tap in the DB dispatch. The tap derives the canonical candidate set from
the intent before the legacy attempt (SPEC-8 §4.1), runs the legacy handler
exactly once, and then admits the candidate set into the local verified log. The
legacy result is returned unchanged and stays authoritative. The tap is an
exhaustive match over the census variants, so a new scene-write variant does not
compile until its author picks a mapping or an explicit `Unsupported { reason }`.
Every unsupported write is counted by kind.

Acceptance:
- AC-4.1 For each census variant, one write produces exactly one admitted
  candidate set, or one counted `Unsupported` entry. There are zero silent
  bypasses (the coverage report states numerator and denominator).
- AC-4.2 The legacy reply and the legacy rows are byte-identical with the tap on
  and with `FE_MIGRATION_MODE=legacy_only`. This extends the existing
  `migration_equivalence_harness` drift guard.
- AC-4.3 A candidate that fails admission never changes the legacy outcome. The
  failure is recorded in the shadow ledger as a new row.
- AC-4.4 `mapping_inventory` lists every census variant. The six missing today
  (see Contradictions) are added.

### FR-5 Durability floor = R relays (D-CL39) — P0

A relay that stores a segment returns a signed custody receipt. The receipt
carries verse id, manifest id, relay principal, held-at time, and signature, under
a new provisional domain registered in the CDL-PROV register. A segment counts as
recorded once receipts exist from R distinct qualifying relays. R is the verse's
`ts_replication_factor`. A relay qualifies when its receipt signature verifies,
its principal holds a currently valid `seed` capability for the verse, it is on
the verse's designated record-relay list (FR-8), and its receipt names this verse
(FR-14). The checkpoint compaction gate (`checkpoint::compaction_decision`) takes
the durability evidence as a required parameter, never an `Option`, so a peer
cannot prune behind a checkpoint unless the floor is met for every segment the
checkpoint covers.

Acceptance:
- AC-5.1 With R=2 and one qualifying receipt, prune refuses with a named
  `DurabilityFloorNotMet`. After the second receipt it releases local bytes
  behind the checkpoint.
- AC-5.2 These do not count toward R: a duplicate receipt from the same relay; a
  receipt from a relay without `seed` capability; a relay not on the designated
  list; a receipt naming another verse; a receipt with a bad signature (table-driven
  test).
- AC-5.3 Pruning never removes a tombstone still needed for no-resurrection
  (`retention::tombstone::assert_no_resurrection` still runs).

### FR-6 `/ws/canonical` mounted with token, then capability chain (D-CL35, D-CL41) — P0

`build_router_with_canonical_log` gets its production caller in both binaries,
with a production `CanonicalLogState`. The five commit-side authorities come from
durable implementations:
- a DB-thread-backed `CanonicalCommitPipeline` (appends stay on the single
  writer);
- a durable `BranchRegistry` and `ScopeSnapshotSource`;
- `ChainCapabilityVerifier::for_requester`;
- `DurableAuthorizationView`.

The HTTP upgrade requires the existing verse-scoped API token
(`fe-api/src/auth.rs`). Its principal becomes the requester. The `authorize`
frame's SPEC-3 chain must name that principal as leaf audience. `wire::session::VerifiedAuthorization` gains a real
`leaf_certificate_id`, which retires the `chain_id` stand-in. A scope-epoch bump
reaches the verification cache through a real event source.

Acceptance:
- AC-6.1 An upgrade with no token, an expired token, or a revoked token is refused
  before the WebSocket upgrade.
- AC-6.2 A valid token plus a chain whose leaf audience is another principal gets
  `scope_not_authorized`. A token for verse X used against verse Y gets the same
  bytes as a never-registered scope (the existence-oracle test stays green).
- AC-6.3 A token revoked mid-session invalidates the session generation within
  one revalidation tick (5 s). Commit, replay, snapshot, and preview all stop.

### FR-7 Distinct retryable "held" commit answer (D-CL40) — P0

SPEC-7 gains a `held` commit-ack state with its own provisional wire value. Every
SPEC-4 quarantine disposition maps to `held`: `missing_parent`, `unknown_schema`,
`unknown_kind`, `opaque_payload`, and `authorization_evidence_unavailable`. Those
dispositions currently map to `rejected` (Contradictions C-1). Like
`accepted_pending_materialization`, `held` carries no cursor, branch, scope, or
projection identity. Resubmitting the identical bytes is the retry. This
discharges the AC-39 pre-mount precondition.

Acceptance:
- AC-7.1 A `commit_submit` whose parent is not yet held gets `held`. After the
  parent arrives, resubmitting the same bytes gets `committed` or
  `accepted_pending_materialization`.
- AC-7.2 A `held` ack encodes no keys 5-8. A peer cannot learn whether a private
  parent exists from the category it gets.
- AC-7.3 `PipelineResult` and `CommitAckState` matches are exhaustive with no
  wildcard arm.

### FR-8 Per-verse warehouse settings (D-CL43) — P0

The verse settings surface gains a Warehouse section, next to Timeseries Fabric.
Only a principal with Manager+ at verse scope may set it (`rbac::require_manage_role`).
Persistence follows the `SetVerseTimeseriesSettings` pattern: verse-row columns,
a read-back echo, and manifest re-emission so the settings replicate. The knobs:

| Knob | Values | Default |
| --- | --- | --- |
| `warehouse_mode` | off / on | off (assumption A-2) |
| `relay_key_custody` | blind / keyholder | blind (D-CL43) |
| `record_relays` | list of relay principal DIDs | empty |
| `peer_prune_after_floor` | off / on | off |

R itself is not a knob here. D-CL39 fixes it as the existing
`ts_replication_factor`, which the surface shows read-only beside the relay list,
along with a warning when `record_relays` is shorter than R. A keyholder relay
receives the scope key only through SPEC-3 §10.2 wrap delivery to its enrolled
device key (FR-9b). Switching keyholder → blind bumps the scope epoch, so the old
key opens nothing new.

Acceptance:
- AC-8.1 A Viewer, an Editor, a petal Manager, and a fractal Manager are refused.
  A verse Manager's change is persisted, echoed from the read-back, and seen by a
  second peer through the manifest.
- AC-8.2 A blind relay never receives a scope-key wrap (asserted over the wrap
  table).
- AC-8.3 After keyholder → blind, a payload sealed at the new epoch cannot be
  opened with the relay's old key.

### FR-9 Signed sync plane (D-CL35, SYNC-SIGNED-PLANE) — P0

- `FE_SYNC_GOSSIP_SIGNING` defaults to `require`. `FE_SYNC_RELAY_ADVERT_SIGNED`
  defaults to `require`.
- Custody receipts and segment announcements ride signed gossip frames (a new
  `GossipFrameKind`).
- Canonical operations travel signed and are admitted only through FR-2.
- Legacy op-log rows stay unsigned (D-CL24(5)).

FR-9b, wrap delivery (CDL-WRAP-DELIVERY): scope-key wraps are issued with
`crypto::key_wrap::issue_scope_key_wrap` to enrolled devices of authorized
principals. They are delivered as recipient-sealed replicated rows on the legacy
row plane, following the R2 enrolment precedent. The local device enrols
automatically when a warehouse-mode verse replica opens, because a consumer now
exists.

Acceptance:
- AC-9.1 With no environment set, an unsigned gossip frame and an unsigned
  `__relays` row are refused.
- AC-9.2 A forged relayed frame, attributed to a principal that did not sign it,
  is refused (harness).
- AC-9.3 A wrap addressed to device D opens only with D's key. A wrap from an
  issuer without `decrypt`-delegation authority is refused on receipt.

### FR-10 On in our build (D-CL42) — P0

With no environment variables set, both binaries mount `/ws/canonical`, run the
dual-emit tap, seed and ingest segments for warehouse-mode verses, and enforce
signed gossip. No cargo feature or off-by-default flag hides any of the four
paths. `FE_MIGRATION_MODE=legacy_only` stays as a local operator kill switch,
because SPEC-8 §2.2 requires a local process flag. It is not the default.

Acceptance:
- AC-10.1 The startup diagnostic line of each binary states the canonical
  surfaces it runs, and a seam test asserts the defaults.
- AC-10.2 Each of the four paths has a seam test at its highest public seam, plus
  a recorded security-review verdict (see plan §Reviews).

### FR-11 Pre-GA, resettable relay records (D-CL38) — P1

Each relay stamps held records with a `warehouse_format_epoch`. When that epoch
does not match the binary's, the relay wipes its held records and asks peers to
re-seed. The relay never tries to migrate. The behavior is documented as pre-GA
until the `CDL-PROV` batch ratification.

Acceptance: AC-11.1 Starting a relay over a store with an older format epoch
wipes the records, logs one line, and the floor recovers as peers re-seed.

### FR-12 Timeseries partial off the DB thread (D-CL44) — P1

A dedicated reader worker executes `execute_ts_partial` on a reader connection
(a clone of the shared handle, per fe-database §shared-handle), fed by its own
bounded channel. It reads the deferred-admission state through a shared snapshot,
not DB-thread state. The DB-thread `ExecuteTsPartial` arm is removed. The
deadline skip and the F23 failed-partial honesty rule are kept.

Acceptance: AC-12.1 While the DB thread is busy with a long write, a partial
returns within its deadline. A failed partial still reports the host as missing.

### FR-13 Columnar scan for ad hoc local BI (D-CL44) — P1

A DataFusion table provider in `fe-query` scans observation hexons (SPEC-10 Parquet
bulk artifacts) pulled from segments and opened with locally held keys. It is
exposed through one read-only fe-api route behind Viewer+ and verse scope. No SQL
from the wire reaches SurrealDB.

Acceptance: AC-13.1 A window aggregate over pulled hexons equals the same
aggregate over the hot plane for the same readings (one table-driven comparison).

### FR-14 One relay, one verse (D-CL45) — P0

A record relay is configured with exactly one verse (`FE_RELAY_VERSE=<id>`). It
refuses to start with zero verses or with more than one. It opens only that
verse's replica, and its custody ledger refuses any receipt or seed naming
another verse.

Acceptance: AC-14.1 Starting with two verses exits non-zero with a named reason. A
seed for another verse is refused.

## Non-functional requirements

- NFR-1 Security: every enforcement is unavoidable by construction (required
  parameters, `pub(crate)` raw paths, `#[non_exhaustive]` results). The
  enforcement census is re-run, with zero dormant gates as the gate. Security
  review is mandatory at four points: token/chain auth, capability issuance,
  key custody and wrap delivery, and inbound segment admission.
- NFR-2 Performance: the dual-emit tap adds no DB round-trip to the legacy reply
  path beyond the candidate append. Segment packing and seeding run off the DB
  thread's command loop. A seed burst cannot starve Serve admissions
  (`command_lane` priority is unchanged).
- NFR-3 Process: no clippy. The gate is `cargo check -p <crate>`. Tests run only
  through named batches (`conductor/test-batches.md`). No `cargo test --workspace`.
- NFR-4 Privacy: custody receipts and the record-relay list form a placement
  index. A slice documents what they reveal against SPEC-3 blinded discovery
  before any code ships (the SPEC-9 hard gate, scoped to relay custody).

## User stories

- As a verse admin, I want to turn on warehouse mode and name my verse's record
  relays, so that the verse's history survives every peer going offline.
  Given a verse Manager, when they enable warehouse mode with two relays and R=2,
  then each new segment reports "recorded" after both relays' receipts arrive.
- As a peer joining late, I want to rebuild the verse from relays, so that I do
  not depend on the original author being online.
  Given segments held by R relays and the author offline, when I fetch and
  replay, then my projection root matches the author's.
- As an SDK client, I want to commit over `/ws/canonical` with my API token and
  capability chain, so that I can write canonical operations directly.
  Given a valid token and a matching chain, when my commit lacks a parent, then I
  get `held` and can retry the same bytes later.

## Technical considerations

- `fe-canonical-log` gains no dependency on fe-runtime, fe-identity, fe-policy,
  fe-database, or surrealdb. `lib.rs` and `Cargo.toml` stay frozen. New files go
  under existing modules (`segment/`, `retention/`), declared from those
  modules' `mod.rs`, so no new top-level module (and no recorded exception) is
  needed.
- Appends stay on the DB thread (single writer). fe-api submits candidates over
  the command channel and never appends on `db_reader`.
- Scope keys live only in `SecretStore` custody, never in SurrealDB.
- All new wire numbers (the held state, custody receipt, receipt domain, gossip
  frame kind, wrap row) are provisional and go into the single CDL-PROV register.

## Explicit non-goals

- NG-1 Cutover to `canonical_authoritative_local`. Legacy replication stays
  authoritative, and `blocks_promotion` stays true.
- NG-2 Ratifying or freezing any wire number (CDL-PROV stays open).
- NG-3 Product retention durations and quarantine budgets (owner policy, D-CL24).
- NG-4 Atomic contracts for the six D-CL20 kinds (assumption A-1).
- NG-5 CDL-ROTATION-ASSEMBLY and the CDL-FORK resolver.
- NG-6 General SPEC-9 peer N-of-M placement beyond relay custody.
- NG-7 Changing desktop relay mode, which serves owned verses as a transport only
  (assumption A-5).

## Assumptions (owner answer not yet given; default and reversal cost)

| # | Question | Default taken | Reversal cost |
| --- | --- | --- | --- |
| A-1 | "Dual-emit everything" vs the D-CL20 prohibition: three scene writes (`ImportGltf`, `DuplicateNode`, `RenameNode`) are D-CL20 kinds | They stay counted `Unsupported(DCl20)` and are reported as a coverage gap | Moderate: one atomic-contract spec erratum plus one mapping slice per kind |
| A-2 | Warehouse mode default for new and existing verses | Off. Dual-emit into the local log runs for every verse; relay seeding runs only in warehouse verses | Cheap: a column default |
| A-3 | R in `mirror` mode (column default 1, unused for placement there) | R = `ts_replication_factor` regardless of mode, so a mirror verse's floor is 1 relay | Cheap |
| A-4 | Which writes are "scene writes" | `SCENE_WRITE_CENSUS` (23 variants). Governance writes (roles, tokens, invites, settings) are not emitted | Moderate: more mappings |
| A-5 | Does D-CL45 bind desktop relay mode? | No. It binds record (warehouse) relays only | Moderate |
| A-6 | Wrap delivery transport | Recipient-sealed replicated rows on the legacy row plane (R2 precedent) | Moderate |
| A-7 | Root of capability issuance | The interim Manager authority (role-table projection, `canon_log` §interim-manager-authority) | High, once SPEC-2 authority history lands |
| A-8 | Keep the `legacy_only` env override under D-CL42 | Yes, as a local kill switch; it is not the default | Cheap |
| A-9 | `held` carries a retry hint? | No number (D-CL24 policy); identical resubmission is the retry | Cheap |

## Contradictions found in existing code and docs

- C-1 `fe-canonical-log/src/compose/storage.rs:360-374` maps the quarantine
  dispositions `MissingParent`, `UnknownSchema`, `UnknownKind`,
  `AuthorizationEvidenceUnavailable`, and `OpaquePayload` to
  `PipelineResult::Rejected`. Lines 369-371 also label
  `AuthorizationEvidenceUnavailable` as `MissingParent`. `wire/commit.rs:107-120`
  and `:384-396` have no held state (D-CL40).
- C-2 `fe-database/src/migration/mode.rs:15-17` defaults to `LegacyOnly` (D-CL42),
  and `boundary.rs:92-104` dual-emit only captures shadow evidence. It does not
  admit or ship.
- C-3 `fe-database/src/migration/AGENTS.md` §no-shadow-exposure (SPEC-8 §2.5)
  forbids exposing candidate evidence. Shipping admitted operations to relays
  (D-CL37) needs a SPEC-8 erratum.
- C-4 `mapping_inventory.rs:49-108` omits six census scene writes from
  `scene_write.rs:56-80`: `CreateObject`, `ImportPetalSnapshot`,
  `PromoteCheckedStamp`, `CommitNodeMutation`, `CommitCheckedNodeMutation`, and
  `CommitStampOverrides`. `mapping_inventory.rs:31-37` puts three census writes
  under D-CL20.
- C-5 `docs/spec/canonical-log/segment-shard-relay.md:299-309` says a relay is a
  "transport and ciphertext seeder only" that "MUST NOT decrypt". This conflicts
  with keyholder relays (D-CL43) and with a relay holding a payload projection
  (D-CL36). It needs a SPEC-6 erratum.
- C-6 `fe-runtime/src/timeseries.rs:61-64` says R "is a request, never a hard
  requirement", clamped to reachable peers, with a default of 1 (`:75`). D-CL39
  makes R a hard floor for pruning.
- C-7 The operator relay hosts every stored verse
  (`fractalengine-relay/src/main.rs:671` `load_hosted_verse_ids`; fe-sync
  §peer-relay-server "Hosted set per binary"; Operator cap 4096). This conflicts
  with D-CL45.
- C-8 `fe-api/src/canonical_ws/router.rs:15-17` and §deliberately-unmounted: the
  route has no production caller. `wire/session.rs:230` `VerifiedAuthorization`
  has no `leaf_certificate_id`.
- C-9 fe-sync §gossip-signing and §relay-advertise default to `compat` (D-CL42).
- C-10 `fe-database/src/lib.rs:2307-2315` runs `execute_ts_partial` on the DB
  thread with DB-thread `admissions` state (D-CL44).
- C-11 Parent `metadata.json` `SPEC-9-durability` puts a hard gate (placement
  privacy analysis) before durability. Custody receipts are a placement index
  (NFR-4).
- C-12 `fe-database/src/canon_log/AGENTS.md:11` "Nothing in the running engine
  calls this module" stops being true. The docs slice must rewrite it.

## Open questions

The assumptions table above is the open-question list. None of them blocks wave 0
or wave 1.
