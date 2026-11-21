---
type: track-spec
title: Relay record warehouse — canonical log over the network (D-CL35..D-CL49)
description: Lift the canonical-log network gate. Relays hold sealed segments as the record, every scene write dual-emits, /ws/canonical is mounted, a per-verse warehouse mode is set by the verse admin, and the campaign is reachable from the fe CLI and the P2P simulator.
tags: [feature, relay_record_warehouse_20261120, pending]
timestamp: 2026-11-20T00:00:00Z
status: planned
resource: ./metadata.json
parent: ../canonical_data_log_20260808/spec.md
decisions: ../../decisions/canonical-data-log-20260808.md
---

# Relay record warehouse

## Purpose

This track implements owner decisions D-CL35..D-CL49, recorded in the decision
register. D-CL35..D-CL45 were taken on 2026-11-20 under the heading "Network gate
lifted, relays as point of record"; D-CL46..D-CL49 are the follow-up answers. Those
decisions are the authority for this track. Where this spec and the decision text
disagree, the decision text wins and this spec gets corrected.

What changes, in short:

- **Every scene write is dual-emitted.** The legacy path still runs and stays
  authoritative. The same write also produces a signed canonical operation in the
  local verified log.
- **Relays become the point of record.** In a verse whose admin has turned on
  warehouse mode, the local log is packed into sealed segments. Those segments go
  to the verse's dedicated record relays, and each relay returns a signed custody
  receipt.
- **A durability floor gates pruning.** Once R relays hold a segment, it counts as
  recorded, and a peer may then prune behind a checkpoint.
- **Projections become rebuildable.** Any peer can rebuild its projection from the
  segments the relays hold.
- **`/ws/canonical` is mounted.** It authenticates with the API token first, then
  a capability chain.
- **Timeseries partials move off the DB thread.**
- **Local BI.** Pulled observation segments can be scanned locally for ad hoc BI.
- **Operator and QA reach.** The warehouse settings are exposed in the `fe` CLI,
  and the campaign gets a scripted-simulator scenario.

## Background

The parent track (`canonical_data_log_20260808`) built the protocol, the verifiers
and the storage half, all locally, and left every network path unwired on purpose.
Its deferral audit (`campaign-plan-w4-20261013.md` §6) lists what is now in scope:

- segment production;
- durable `ProofEnvironment`, `ParentWalk` and `PayloadAdmission` implementations;
- the §10.2.4 wrap delivery transport;
- the `/ws/canonical` mount;
- CANON-DUAL-EMIT;
- local capability-chain issuance.

In `fe-canonical-log/src/AGENTS.md`, the §trait-composition-map still shows three
traits with no production caller: the wire `CapabilityVerifier`,
`ProofEnvironment`, and `CanonicalCommitPipeline`.

Ledger rows this track closes:

- CDL-NET;
- CANON-WS;
- SYNC-SIGNED-PLANE, minus the `__shards` slice, which belongs to
  `role_propagation_20261013`;
- CANON-DUAL-EMIT, for wiring only (the cutover stays gated);
- CDL-WRAP-DELIVERY.

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
| D-CL46 issuance rooted in the verse role table (Manager+) | FR-6, FR-1 (issuance; was assumption A-7) |
| D-CL47 D-CL20 lifted for glTF import, node duplicate, node rename | FR-4 (was assumption A-1) |
| D-CL48 one-relay-one-verse binds record relays only | FR-14, NG-7 (was assumption A-5) |
| D-CL49 warehouse settings in the `fe` CLI; QA coverage of the campaign | FR-15, FR-16 |

## Functional requirements

Each requirement names the decision it serves. Priority P0 means the track is not
done without it.

### FR-1 Relay seeding of sealed segments (D-CL35, D-CL36) — P0

A peer in a warehouse-mode verse packs its admitted canonical operations into
SPEC-6 sealed segments: a header segment, payload shards, HashSeq nodes and a
manifest. Every seal goes through `segment::artifact::seal_artifact`. The peer
offers each segment to the verse's designated record relays.

- **Seed authority.** A relay accepts a seed only from a requester that holds a
  currently valid SPEC-3 `seed` capability for the verse (SPEC-6 §6 rule 2).
- **Fetch authority.** A relay serves a fetch only to a requester that holds
  `fetch`.
- **Who issues capabilities.** Capabilities are issued from the verse role table,
  Manager+ (D-CL46).
- **Transport.** Bytes are opaque, BLAKE3/HashSeq-addressed, and travel over the
  existing iroh-blobs `BlobTransport`. fe-sync never parses a manifest.

Acceptance:
- AC-1.1 Seal a segment on peer A, then take A offline. Peer B fetches the segment
  from relay R and replays it. B's projection root is byte-identical to A's. This
  is checked in two places: the harness scenario `relay_record_warehouse` and the
  simulator scenario `relay-record`.
- AC-1.2 A seed without a valid `seed` capability is refused, and the relay stores
  nothing. A fetch without `fetch` is refused, and "absent" and "not authorized"
  produce the same response.
- AC-1.3 The relay re-hashes every artifact it receives and never stores one whose
  bytes don't hash to its id.

### FR-2 Inbound canonical P2P admission (D-CL35) — P0

Segments fetched from relays or peers are untrusted input. Each one passes these
steps in order:

1. `segment::artifact::admit_sealed`.
2. Receipt re-hash and cross-check.
3. The checkpoint proof over a durable `ProofEnvironment`, with durable
   `ParentWalk` and `PayloadAdmission`.
4. Per-operation `admit_and_append_durably`, using the production verifier.

There is no second admission path, and the raw append stays `pub(crate)`.

Acceptance:
- AC-2.1 Each of these is refused, nothing is appended, and the refusal is
  counted:
  - a forged signature;
  - a mismatched `op_id`;
  - an unreferenced payload;
  - an artifact sealed under a retired epoch key.
- AC-2.2 A segment whose parents are missing goes into bounded quarantine (the
  per-reason budgets). It is admitted once the parents arrive and is never
  materialized before then.

### FR-3 Every SurrealDB is a rebuildable projection (D-CL36) — P0

A peer's canonical projection, or a keyholder relay's, can be dropped and rebuilt
from relay-held segments alone, and the rebuild reaches the same projection root.

A blind relay (the default) holds no scope key. What it can rebuild is narrower:
its artifact catalog, plus header-level metadata it can verify without a key. Its
legacy-row replica keeps replicating as it does today.

Acceptance:
- AC-3.1 Dropping the projection, re-fetching and replaying gives the same root.
- AC-3.2 Dropping a blind relay's catalog and re-scanning gives an identical
  catalog.

### FR-4 Dual-emit every scene write, no silent gap (D-CL37, D-CL47) — P0

Every command in `fe_runtime::scene_write::SCENE_WRITE_CENSUS` (23 variants) passes
through one dual-emit tap in the DB dispatch. For each write, the tap:

1. derives the canonical candidate set from the intent before the legacy attempt
   (SPEC-8 §4.1);
2. runs the legacy handler exactly once;
3. admits the candidate set into the local verified log;
4. returns the legacy result unchanged.

The tap is an exhaustive match over the census, with no wildcard arm. D-CL47 lifts
D-CL20 for `ImportGltf`, `DuplicateNode` and `RenameNode`. Those three get atomic
multi-row contracts (a SPEC-4 §10 / SPEC-8 §4.2.5 erratum) and mappings.

The six scene writes that the mapping inventory omitted today (C-4) are mapped in
the same waves: `CreateObject`, `ImportPetalSnapshot`, `PromoteCheckedStamp`,
`CommitNodeMutation`, `CommitCheckedNodeMutation` and `CommitStampOverrides`. The
target is zero `Unsupported` arms. If an `Unsupported` arm remains, it is counted,
named in the coverage report, and blocks track closure. It is never silent.

The three hierarchy-creation kinds (`CreateVerse`, `CreateFractal`, `CreatePetal`)
stay under D-CL20. They are not scene writes.

Acceptance:
- AC-4.1 For every census variant, one write produces exactly one admitted
  candidate set. The coverage report states its numerator and denominator, and
  closure requires `Unsupported = 0`.
- AC-4.2 The legacy reply and legacy rows are byte-identical with the tap on and
  with `FE_MIGRATION_MODE=legacy_only` (the `migration_equivalence_harness` drift
  guard).
- AC-4.3 A candidate that fails admission never changes the legacy outcome. The
  failure is recorded as a new shadow-ledger row.
- AC-4.4 `mapping_inventory` lists every census variant. The bypass counters for
  the three lifted kinds are removed. `dcl20_bypass_counter_test` now pins only
  the three hierarchy-creation kinds.
- AC-4.5 For the multi-row kinds (glTF import, duplicate, cascade tombstone,
  editor batch, snapshot import), either all members are admitted or none are.
  Each kind has a test that fails one member and asserts that zero members were
  appended.

### FR-5 Durability floor = R relays (D-CL39) — P0

A relay that stores a segment returns a signed custody receipt:
`{verse_id, manifest_id, relay_principal, held_at_ms}`, under the provisional
domain `fe-custody-receipt-v1\0`.

A segment is recorded once R distinct qualifying relays hold it. R is the verse's
`ts_replication_factor`. A relay qualifies when all of these hold:

- its signature verifies;
- it holds a valid `seed` capability;
- it is on the verse's `record_relays` list;
- its receipt names this verse.

`checkpoint::compaction_decision` takes the durability evidence as a required
parameter. A caller cannot leave it out.

Acceptance:
- AC-5.1 With R=2 and one qualifying receipt, prune refuses with
  `DurabilityFloorNotMet`. After a second qualifying receipt, prune releases.
- AC-5.2 None of these count toward R:
  - a duplicate receipt from the same relay;
  - a receipt from a relay with no `seed` capability;
  - a receipt from a relay that isn't on the list;
  - a receipt naming another verse;
  - a receipt with a bad signature.

  These are checked as one table-driven test.
- AC-5.3 A tombstone needed for no-resurrection always survives pruning.

### FR-6 `/ws/canonical` mounted, token then capability chain (D-CL35, D-CL41, D-CL46) — P0

`build_router_with_canonical_log` gets its production caller in both binaries,
and the canonical argument is no longer optional. Its parts:

- a DB-thread-backed `CanonicalCommitPipeline`;
- a durable `BranchRegistry` and `ScopeSnapshotSource`;
- `ChainCapabilityVerifier::for_requester`;
- `DurableAuthorizationView`.

The upgrade requires the existing verse-scoped API token. The token's principal
becomes the requester, and the chain's leaf audience must equal that principal.
Chains are issued from the verse role table, Manager+ (D-CL46).

`VerifiedAuthorization` gains a real `leaf_certificate_id`. A scope-epoch bump now
reaches the verification cache through a real event source.

Acceptance:
- AC-6.1 A missing, expired or revoked token is refused before the upgrade.
- AC-6.2 A chain whose leaf audience is a different principal gets
  `scope_not_authorized`. A token for a foreign verse gets bytes identical to the
  never-registered case, so the existence-oracle test stays green.
- AC-6.3 Revoking a token mid-session stops commit, replay, snapshot and preview
  within one 5 s tick.

### FR-7 Distinct retryable "held" commit answer (D-CL40) — P0

SPEC-7 gains a `held` commit-ack state. Its wire value is provisional (`4`). The
SPEC-4 quarantine dispositions now map to `held` instead of `rejected`:

- `missing_parent`
- `unknown_schema`
- `unknown_kind`
- `opaque_payload`
- `authorization_evidence_unavailable`

A `held` ack carries no cursor, branch, scope or projection identity. The retry is
resubmitting the identical bytes.

Acceptance:
- AC-7.1 A missing parent produces `held`. After the parent arrives, resubmitting
  the same bytes produces `committed` or `accepted_pending_materialization`.
- AC-7.2 A `held` ack encodes no keys 5-8.
- AC-7.3 The matches over the ack state are exhaustive, with no wildcard arm.

### FR-8 Per-verse warehouse settings (D-CL43) — P0

The verse settings gain a Warehouse section, placed next to Timeseries Fabric.
Only Manager+ at verse scope can set it (`rbac::require_manage_role`). It follows
the `SetVerseTimeseriesSettings` pattern: verse-row columns, a read-back echo, and
re-emission of the manifest.

| Knob | Values | Default |
| --- | --- | --- |
| `warehouse_mode` | off / on | off (assumption A-2) |
| `relay_key_custody` | blind / keyholder | blind |
| `record_relays` | relay principal DIDs | empty |
| `peer_prune_after_floor` | off / on | off |

R is shown read-only, because it is `ts_replication_factor` (D-CL39). A warning
appears when fewer relays are listed than R requires.

A keyholder relay receives the scope key only through §10.2 wrap delivery.
Switching from keyholder to blind bumps the epoch.

Acceptance:
- AC-8.1 Viewer, Editor, petal Manager and fractal Manager are all refused. A verse
  Manager's change persists, echoes the read-back value, and replicates. This holds
  on every door: GUI, REST and `fe` CLI.
- AC-8.2 A blind relay never receives a wrap.
- AC-8.3 After switching keyholder → blind, a payload sealed at the new epoch can't
  be opened with the relay's old key.

### FR-9 Signed sync plane, plus wrap delivery (D-CL35) — P0

- `FE_SYNC_GOSSIP_SIGNING` and `FE_SYNC_RELAY_ADVERT_SIGNED` default to `require`.
- Custody receipts and segment announcements ride a signed `GossipFrameKind`.
- Canonical operations are admitted only through FR-2.
- Legacy op-log rows stay unsigned (D-CL24(5)).

**FR-9b** Scope-key wraps go to the enrolled devices of authorized principals, as
recipient-sealed replicated rows (assumption A-6). A peer auto-enrols its device
when a warehouse-mode verse replica opens.

Acceptance:
- AC-9.1 With no environment set, unsigned gossip and unsigned `__relays` rows are
  refused.
- AC-9.2 A forged relayed frame is refused (harness).
- AC-9.3 A wrap opens only for its own device, and an unauthorized issuer is
  refused.

### FR-10 On in our build (D-CL42) — P0

With no environment variables set, both binaries:

- mount `/ws/canonical`;
- run the tap;
- seed and ingest segments for warehouse verses;
- enforce signed gossip.

`FE_MIGRATION_MODE=legacy_only` stays available as a local kill switch, required by
SPEC-8 §2.2 (assumption A-8). It is not the default.

Acceptance:
- AC-10.1 Each binary's startup diagnostic states its canonical surfaces, and a
  seam test asserts these defaults.
- AC-10.2 Each of the four paths has a seam test and a recorded security-review
  verdict.

### FR-11 Pre-GA, resettable relay records (D-CL38) — P1

A relay stamps its held records with `warehouse_format_epoch`. If the stamp doesn't
match the running binary, the relay wipes the records and requests a re-seed. It
never migrates them.

Acceptance:
- AC-11.1 On a mismatch, the relay wipes, logs one line, and the floor recovers as
  peers re-seed.

### FR-12 Timeseries partial off the DB thread (D-CL44) — P1

A dedicated reader worker runs `execute_ts_partial` on a reader-connection clone,
fed by its own bounded channel. It reads a shared snapshot of the deferred
admissions. The DB-thread `ExecuteTsPartial` arm is removed. The deadline skip and
the F23 failed-flag behaviour are kept.

Acceptance:
- AC-12.1 While the DB thread is busy with a long write, a partial still returns
  within its deadline. A failed partial still reports its host as missing.

### FR-13 Columnar scan for ad hoc local BI (D-CL44) — P1

A DataFusion provider scans the observation hexons (SPEC-10) that were pulled from
segments and opened with keys held locally. It is reachable through one read-only
REST route (Viewer+, scoped to the verse). No SQL from the wire ever reaches
SurrealDB.

Acceptance:
- AC-13.1 A window aggregate over the pulled hexons equals the same aggregate over
  the hot plane.

### FR-14 One record relay, one verse (D-CL45, D-CL48) — P0

A record relay is configured with exactly one verse, via `FE_RELAY_VERSE=<id>`. It
refuses to start with zero verses or with more than one, and its custody ledger
refuses anything that names another verse. Desktop relay mode, which serves owned
verses as a transport, is unaffected (D-CL48).

Acceptance:
- AC-14.1 Starting with two verses exits non-zero with a named reason. A seed for a
  foreign verse is refused.

### FR-15 Warehouse settings in the `fe` CLI (D-CL49) — P1

New REST routes (REST only; no MCP tool is added, so the MCP tool count of 64 and
the `tools/fe-cli/catalog/mcp-tools.json` snapshot do not change):

- `GET /api/v1/verses/{v}/warehouse/settings` (Viewer+)
- `PUT /api/v1/verses/{v}/warehouse/settings` (Manager+ at verse scope)
- `GET /api/v1/verses/{v}/warehouse/status` (Viewer+): floor state per recent
  manifest, the receipt count against R, and the canonical projection root

Matching `fe` verbs are added as table-driven rows in `tools/fe-cli/src/catalog.rs`:

- `fe warehouse get|set|status`

The rules from `tools/fe-cli/AGENTS.md` apply: role checks stay on the server, the
uniform not-found maps to exit 5, and a role refusal maps to exit 4.

Acceptance:
- AC-15.1 The `cli_journey` gets a new leg:
  1. a viewer's `fe warehouse set` exits 4;
  2. the Manager's set succeeds;
  3. `fe warehouse get` returns the read-back;
  4. `fe warehouse status --json` carries the floor fields.

### FR-16 QA coverage of the campaign in the scripted simulator (D-CL49) — P1

The new simulator scenario `tools/qa/sim/scenarios/relay-record.json` runs these
steps:

1. Start one headless record relay bound to the verse.
2. The owner turns on warehouse mode with that relay and R=1.
3. The owner writes a mix of scene writes, including a glTF import, a duplicate and
   a rename.
4. Wait until `warehouse/status` reports the floor met.
5. Stop the author.
6. Bootstrap a fresh peer from the relay.
7. Compare the fresh peer's canonical projection root and node fingerprint with
   the values the author recorded before stopping.

Acceptance:
- AC-16.1 `python tools/qa/sim/run.py validate relay-record` passes.
- AC-16.2 A run named by the owner reports equal roots and fingerprints. The
  report also shows the floor-met time and the bootstrap time.

## Non-functional requirements

- **NFR-1 Security.** Every enforcement point is unavoidable by construction:
  required parameters, `pub(crate)` raw paths, and `#[non_exhaustive]` results. The
  enforcement census is re-run, and the gate is zero dormant checks. Security
  review is mandatory at five points:
  - token and chain auth;
  - capability issuance;
  - key custody and wrap delivery;
  - inbound segment admission;
  - the REST governance write.
- **NFR-2 Performance.** The tap adds only the candidate append to the legacy reply
  path. Packing and seeding run off the DB command loop. The `command_lane`
  priority for Serve admissions is unchanged.
- **NFR-3 Process.** No clippy; the gate is `cargo check -p`. Tests run only
  through named batches, never `cargo test --workspace`.
- **NFR-4 Privacy.** Custody receipts and the record-relay list together form a
  placement index. Before any code ships, a slice must analyse it against SPEC-3
  blinded discovery (the SPEC-9 hard gate, scoped to relays).

## User stories

- **Durable history (verse admin).** As a verse admin, I want to turn on warehouse
  mode and name my record relays, so the verse survives every peer going offline.
  - Given R=2 and two listed relays,
  - when both relays have returned receipts,
  - then the segment reads as recorded.
- **Bootstrap without the author (late peer).** As a late peer, I want to rebuild
  from relays without needing the author online.
  - Given segments held by R relays and the author offline,
  - when I fetch and replay,
  - then my projection root matches the author's.
- **Scripted control (operator).** As an operator, I want `fe warehouse status` to
  show the floor, so a script can wait for "recorded" before taking a peer down.
- **Retryable commits (SDK client).** As an SDK client, I want to get `held` and
  retry the same bytes, so that commits arriving out of parent order still land.

## Technical considerations

- `fe-canonical-log` takes no new dependency edges. Its `lib.rs` and `Cargo.toml`
  stay frozen. New files go under the existing `segment/` and `retention/`
  modules.
- Appends stay on the DB thread. fe-api never appends through `db_reader`.
- Scope keys live only in `SecretStore` custody.
- Every new wire number is provisional and goes into the single CDL-PROV register.
- No MCP tools are added anywhere in this track (FR-15).

## Explicit non-goals

- **NG-1** No cutover to `canonical_authoritative_local`; `blocks_promotion` stays
  true.
- **NG-2** No wire number is ratified or frozen.
- **NG-3** Retention durations and quarantine budgets are not set here.
- **NG-4** No canonical contracts for `CreateVerse`, `CreateFractal` or
  `CreatePetal`; they are still under D-CL20 and are not scene writes.
- **NG-5** CDL-ROTATION-ASSEMBLY and CDL-FORK are out of scope.
- **NG-6** General SPEC-9 peer N-of-M placement is out of scope.
- **NG-7** Desktop relay mode does not change (D-CL48).

## Assumptions still open

| # | Question | Default | Reversal cost |
| --- | --- | --- | --- |
| A-2 | Warehouse mode default | Off per verse; local dual-emit still runs for every verse | Cheap |
| A-3 | R in mirror mode | R = `ts_replication_factor` (mirror defaults to 1) | Cheap |
| A-4 | Which writes count as "scene writes" | The 23 variants of `SCENE_WRITE_CENSUS`; governance writes are not emitted | Moderate |
| A-6 | Wrap delivery transport | Recipient-sealed replicated rows (the R2 precedent) | Moderate |
| A-8 | Keep the `legacy_only` override | Yes, as a local kill switch | Cheap |
| A-9 | Retry hint on `held` | None | Cheap |

Resolved: A-1 was reversed by D-CL47 (map the three lifted kinds). A-5 was
confirmed by D-CL48. A-7 was confirmed by D-CL46.

## Contradictions found in existing code and docs (checked at `98470a73`)

- **C-1** `fe-canonical-log/src/compose/storage.rs:360-374` maps quarantine
  dispositions to `Rejected`. Lines 369-371 also label
  `AuthorizationEvidenceUnavailable` as `MissingParent`. `wire/commit.rs:107-120`
  and `:384-396` have no held state.
- **C-2** `fe-database/src/migration/mode.rs:15-17` defaults to `LegacyOnly`, and
  `boundary.rs:92-104` only shadows; nothing is admitted.
- **C-3** SPEC-8 §2.5 no-shadow-exposure (`migration/AGENTS.md`) conflicts with
  seeding admitted operations to relays.
- **C-4** `mapping_inventory.rs:49-108` omits six census scene writes. Lines 31-37,
  and the handlers at `handlers/crud.rs:320,1694,1816`, bypass-count `ImportGltf`,
  `RenameNode` and `DuplicateNode`, which D-CL47 now lifts.
- **C-5** `docs/spec/canonical-log/segment-shard-relay.md:299-309` says "seeder
  only … MUST NOT decrypt". This conflicts with keyholder relays and relay
  projections.
- **C-6** `fe-runtime/src/timeseries.rs:61-64` treats R as a request, not a
  requirement; `:75` defaults it to 1.
- **C-7** `fractalengine-relay/src/main.rs` `load_hosted_verse_ids` serves every
  stored verse; the Operator cap is 4096.
- **C-8** `fe-api/src/canonical_ws/router.rs:15-17` has no production caller.
  `wire/session.rs:230` has no `leaf_certificate_id`.
- **C-9** `gossip_auth.rs:26` and `relay_config.rs:88-92` default to `compat`.
- **C-10** `fe-database/src/lib.rs` (~2304-2335) runs `execute_ts_partial` on the
  DB thread.
- **C-11** The parent track's SPEC-9 hard gate (placement privacy) applies to
  custody receipts.
- **C-12** `fe-database/src/canon_log/AGENTS.md:11` says "Nothing in the running
  engine calls this module"; that stops being true.
- **C-13** `tools/qa/stage.ps1` builds no `fractalengine-relay` binary, so the
  simulator has no relay process to start.
