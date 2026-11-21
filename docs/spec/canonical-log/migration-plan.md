# Canonical log migration plan v1

**Status:** Owner-approved 2026-08-09. Implementation (Workstream G) is unlocked; network rollout, relay seeding, and inbound P2P were owner-gated until 2026-11-20 (lifted, D-CL35; approved, not yet built, track `relay_record_warehouse_20261120`; the `canonical_authoritative_local` cutover stays owner-gated).

This document defines the staged migration from FractalEngine's current
local-first SurrealDB write path to the Canonical Fractal Data Log. It is
SPEC-8. It preserves the current desktop editor while the canonical path is
built and validated locally. It does not define an envelope, certificate,
encryption suite, key distribution scheme, branch-control grammar, network
transport, or relay behavior.

This plan depends on [operation-envelope.md](operation-envelope.md),
[author-key-lifecycle.md](author-key-lifecycle.md),
[capabilities-and-revocation.md](capabilities-and-revocation.md),
[log-first-materialization.md](log-first-materialization.md),
[branches-checkpoints-retention.md](branches-checkpoints-retention.md),
[segment-shard-relay.md](segment-shard-relay.md), and
[commit-preview-wire.md](commit-preview-wire.md).

## 1. Conformance vocabulary and migration boundary

1. The key words **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are normative.
2. A **legacy mutation** is the current local editor mutation whose visible
   state is written through the existing SurrealDB path. It remains the only
   authoritative editor path until the owner approves a later local cutover.
3. A **migration candidate set** is the complete, deterministic set of exact
   canonical envelopes and, where applicable, payload artifacts derived from
   one user intent. Its members are scope-local and carry the registered causal
   relationships required by the governing specifications. The set is not
   authoritative merely because it was encoded, stored for shadow validation,
   or replayed.
4. **Dual emit** means that one future `commit_operation` ingress derives both
   the legacy mutation and one correlated migration candidate set from the
   same original intent before either path observes mutable SurrealDB state. It
   does not mean a new transport, row-replication extension, or independently
   authored user actions. Its scope is exactly the commands in
   `fe_runtime::scene_write::SCENE_WRITE_CENSUS` (D-CL37); a command outside
   the census is not a dual-emit scene write.
5. **Shadow materialization** is a non-authoritative projection reconstructed
   from migration candidates. It may compare its output with the live legacy
   projection, but it MUST NOT mutate, repair, or overwrite that projection.
6. A **cutover comparator** is the versioned, deterministic export mapping
   used to compare a legacy projection with a shadow projection. It records
   every mapped field, exact quantization rule, and intentional exclusion. An
   omitted, unsupported, unquantizable, or lossy-by-accident field is a
   validation failure unless the owner has approved its explicit exclusion
   before the run begins.
7. This plan creates no network authority of its own. D-CL35 and D-CL37
   separately approve seeding admitted candidate members to the verse's record
   relays (§2 rule 5). Nothing here authorizes extending `WriteRowEntry`, or
   sending shadow evidence or unadmitted candidate bytes to another process.
8. **Workstream G** means implementation of canonical envelope encoding,
   verified append, materialization, capabilities, branch/retention controls,
   commit wire behavior, segments, or relay seeding. It excludes the
   legacy-only `seam_ready` ingress hardening described in §3.
9. An **admitted candidate member** is a candidate member that passed SPEC-4
   §2 admission and was durably appended to the local verified log. It is a
   canonical record, not shadow evidence. A **record relay** is a relay on the
   verse's `record_relays` list, as defined in
   [relay-custody-durability.md](relay-custody-durability.md).

## 2. Migration invariants

1. The current local-only desktop editor MUST remain usable with canonical-log
   flags disabled. Flag-disabled behavior MUST retain the existing local write
   result and error semantics except for independently approved hardening work.
2. Every canonical-log mode MUST be locally configured. A flag MUST state its
   mode in diagnostics and MUST NOT be remotely activated by a peer, relay,
   WebSocket client, or replicated row. The project's own build defaults to
   `dual_emit_shadow` (D-CL37, D-CL42); every other build defaults to
   `legacy_only`. `legacy_only` MUST stay selectable in every build as a local
   kill switch.
3. A migration candidate MUST be derived from the original intent, including
   the original scope and authorization context. It MUST NOT be reverse-built
   from an already-mutated SurrealDB row, database-generated timestamp, old
   property value, or display float.
4. The canonical operation path MUST use the SPEC-1 canonical integer values
   and intent-only payload rules. Legacy display values may be explicitly
   quantized at the ingress boundary; they MUST NOT become signed floats.
5. Shadow evidence, including unadmitted candidate bytes, correlation
   records, validation outcomes, comparator exports, and shadow projection
   roots, MUST remain local to the migration environment and obey the
   applicable capability and privacy rules. Admitted candidate members are
   canonical records (§1 rule 9): they MAY be sealed into segments and seeded
   to the verse's record relays under SPEC-6 and
   [relay-custody-durability.md](relay-custody-durability.md), and MAY be
   served on the SPEC-7 canonical wire under its own authorization. Neither
   class MAY be exposed through the legacy public asset, tile, REST API, or
   WebSocket surfaces, and neither MAY ride legacy row replication.
6. A shadow mismatch, missing candidate, failed append, failed materialization,
   ambiguous outcome, or unapproved comparator exclusion MUST fail validation.
   It MUST NOT be hidden by updating legacy rows, synthesizing a replacement
   candidate, or skipping an operation during replay.
7. No stage in this plan may claim a checkpoint, durable cursor, retention
   compaction, bootstrap, branch merge, or network availability until the
   governing specifications and owner gates permit that claim.

## 3. Required staged modes

The implementation may choose its configuration syntax, but it MUST expose
the following mutually exclusive behavior. A later mode is never selected
automatically from observed parity.

| Mode | Legacy mutation | Canonical candidate | User-visible authority | Network behavior |
| --- | --- | --- | --- | --- |
| `legacy_only` | Execute current path. | Do not create one. | Legacy SurrealDB projection. | None. |
| `seam_ready` | Execute current path through the single ingress. | Do not append, materialize, or publish one. | Legacy SurrealDB projection. | None. |
| `dual_emit_shadow` | Execute current path once. | Derive before the legacy attempt; after a successful legacy attempt, admit the set all-or-nothing (SPEC-4 §10.4) into the local verified log and retain correlation evidence. | Legacy SurrealDB projection. | Admitted members only, to record relays (§2 rule 5). |
| `shadow_rebuild` | Execute current path once. | As `dual_emit_shadow`, and replay the admitted set into a separate projection. | Legacy SurrealDB projection. | Admitted members only, to record relays (§2 rule 5). |
| `canonical_authoritative_local` | Available only after the cutover gates in §7. | Use the approved verified-log and materializer path. | Approved canonical local projection. | None under this plan. |

1. The default mode is the one §2 rule 2 names. `legacy_only` is the local
   kill switch: selecting it creates no candidate, admits nothing, seeds
   nothing, and leaves the legacy reply and rows byte-identical to the
   dual-emit path's.
2. `seam_ready` is the only permitted first code change. It establishes the
   future ingress boundary without changing the authoritative data model.
3. `dual_emit_shadow` and `shadow_rebuild` MUST be unavailable until the
   owner has approved the governing SPEC documents and the implementation can
   satisfy their applicable admission rules. A syntactically convenient
   placeholder signature, plaintext payload, or local-only authorization
   bypass does not qualify. D-CL37 records that approval for
   `dual_emit_shadow` over `SCENE_WRITE_CENSUS`.
4. `canonical_authoritative_local` is a distinct, deliberate release step. It
   MUST NOT be enabled by a successful comparison, a process restart, a
   database migration, or a peer message. D-CL37 does not approve it.
5. The only network behavior a migration mode permits is seeding admitted
   candidate members to the verse's record relays (§2 rule 5, D-CL35,
   D-CL37), and only while the verse's `warehouse_mode` setting (D-CL43,
   default off) is on and its `record_relays` list is non-empty. That
   per-verse, Manager+-set setting gates seeding for the verse only; it never
   selects or changes the local migration mode, which §2 rule 2 keeps local.
   No mode sends shadow evidence to any process. Legacy row
   replication stays the authoritative replication path until a cutover is
   approved.

## 4. The future `commit_operation` ingress

### 4.1 Boundary contract

1. Every editor/API mutation that changes canonical-domain state MUST
   eventually enter one future `commit_operation` boundary. Direct handler
   writes, ad hoc operation-log calls, and independent sync sends are not
   allowed bypasses once the seam is introduced.
2. The boundary receives one immutable request containing the user intent,
   principal, effective authorization context, target scope, and caller
   correlation identifier. It derives all legacy commands and at most one
   migration candidate set before performing either side effect.
3. The boundary returns a correlation record that identifies the one intent,
   legacy outcome, candidate-set outcome, and validation disposition. It MUST
   NOT infer correlation by arrival order, a database row ID, a wall-clock
   value, or a best-effort broadcast.
4. If a mapping requires multiple scope-local candidate members, the boundary
   MUST derive their complete membership, scopes, deterministic ordering, and
   required parent references before either side effect. An incomplete set is
   uncovered; it MUST NOT be represented as a partially successful intent.
5. The boundary MUST preserve one user action to one legacy mutation attempt.
   It MUST NOT retry the legacy mutation merely because shadow capture,
   candidate validation, or shadow materialization had a transient failure.
6. The boundary is a future dual-emit seam, not a second source of authority.
   Before `canonical_authoritative_local`, it MUST NOT feed a shadow result
   back into the live editor, API result, WebSocket delta, analytics surface,
   or legacy SurrealDB projection.

### 4.2 Intent and compatibility mapping

1. Before dual emit begins, each supported legacy mutation kind MUST have a
   reviewed mapping to one deterministic, complete candidate set or an explicit
   unsupported status. Every non-structural member names a registered canonical
   intent schema; a structural member conforms to its registered SPEC-1
   structural grammar. Creation, hierarchy changes, property changes,
   transforms, tombstones, and lifecycle changes require separate mappings.
2. A mapping MUST state the source fields, integer quantization, scope
   derivation, candidate-set membership and parent requirements,
   author/capability requirements, expected materialized effect, and comparator
   fields. It MUST name the schema hash and materializer version used by the
   validation run.
3. Current legacy behavior that embeds a previous row value, such as an old
   transform, MUST be removed from the candidate intent rather than copied
   into it. Migration does not relax the SPEC-1 intent-only rule.
4. A mutation kind without a complete mapping MUST remain `legacy_only` for
   that kind. It reduces capture coverage and therefore prevents authoritative
   cutover; it MUST NOT be represented as a successful no-op candidate.
5. The deferred D-CL20 kinds are `CreateVerse`, `CreateFractal`, and
   `CreatePetal` only. They are hierarchy creation, not scene writes, and stay
   outside the dual-emit scope until each gains an approved contract; that
   blocks cutover. D-CL47 lifted D-CL20 for `ImportGltf`, `DuplicateNode`, and
   `RenameNode`: their atomic contracts are SPEC-4 §10.1-§10.3, and each MUST
   be mapped. Every `SCENE_WRITE_CENSUS` variant MUST have a mapping. A
   remaining `Unsupported` mapping MUST be counted and named in the coverage
   report, never silent, and blocks track closure. The mapping inventory MUST
   report `unmapped_bypass_count = 0` for every census variant.
6. Every row identifier a candidate names, such as a created node's id, MUST
   be fixed at the boundary before either side effect and carried in the
   candidate through the SPEC-10 §4.5 `legacy_identifier` mapping. The legacy
   attempt MUST use the same identifier. A candidate MUST NOT learn an
   identifier from the legacy result; if an identifier cannot be fixed first,
   the intent is `candidate_unavailable` under §4.3 rule 1.

### 4.3 Failure and rollback behavior during shadow modes

1. If candidate construction or local admission preconditions fail before the
   legacy attempt, `dual_emit_shadow` MAY still return the legacy outcome in
   order to preserve the current editor. It MUST persist a
   `candidate_unavailable` validation outcome and count the operation as
   uncovered.
2. If the legacy mutation fails, no candidate set may be described as
   equivalent to a successful legacy mutation. If any candidate bytes were
   already made durable, the run is tainted, the mismatch is retained as
   evidence, and the migration operator MUST stop the shadow run before
   resuming comparisons.
3. If a candidate-set shadow-store append succeeds but its shadow materializer
   fails, the legacy outcome remains authoritative and the set is pending only
   in the shadow store. A shadow-store append is never verified-log admission,
   a segment seal, or a canonical availability claim. The boundary MUST record
   `materialization_pending` or `materialization_failed`; it MUST NOT report a
   canonical commit to a user.
4. If a comparison fails, the operator MUST disable the shadow mode before
   continuing ordinary editor work, preserve the candidate and comparator
   evidence, and return to `legacy_only` or `seam_ready`. No deletion, rewrite,
   compensating SurrealDB edit, or synthetic canonical operation is rollback.
5. If the process crashes with an unknown dual-emit outcome, recovery MUST
   classify the correlated intent by durable evidence. It MUST NOT assume a
   candidate exists because a legacy row exists, or vice versa. An unresolved
   correlation record blocks promotion until investigated.
6. A rollback from `canonical_authoritative_local` is not a license to write
   new divergent legacy state. It MUST first stop new writes, retain all
   verified canonical bytes, identify the last approved compatible projection
   boundary, and require an owner-approved recovery decision. The ordinary
   shadow-mode fallback rules do not apply after authority has changed.
7. In `dual_emit_shadow`, admission follows a successful legacy attempt. A
   failed legacy attempt admits no member. A candidate set that fails
   admission, wholly or in one member, appends no member (SPEC-4 §10.4),
   leaves the legacy reply and rows unchanged, and is recorded as a new
   shadow-ledger row with its disposition. It is never retried by repeating
   the legacy attempt (§4.1 rule 5).

## 5. Shadow rebuild and comparison procedure

### 5.1 Run preparation

1. A validation run MUST pin the SPEC versions, schema hashes, materializer
   identity/version, comparator version, supported mutation mapping set, and
   local flag mode before accepting its first intent.
2. Until the owner approves an import/bootstrap design, a shadow run MAY claim
   parity only for an owner-authorized test Verse created for that run. Its
   first captured canonical artifact MUST be the declared SPEC-1
   `branch_genesis` fixture. The run MUST NOT claim parity for legacy history
   that predates capture, and it MUST NOT reverse-build that history from
   current SurrealDB rows.
3. A run MUST record a bounded immutable correlation ledger with at least:
   run identifier; intent digest; caller correlation identifier; legacy result;
   run-local candidate-set correlation identifier and member `op_id` values
   when present; candidate byte hashes; branch/scope selection;
   materializer identity/version; comparator version; and final disposition.
   The run-local identifier is diagnostic-only: it has no canonical, branch,
   cursor, or content-address meaning. The member `op_id` list is the
   canonical evidence.
4. A run MUST retain, in an isolated access-controlled local store, every
   exact verification input required for its candidate set: envelope and
   payload artifacts where the applicable rules permit them, capability-chain
   artifact bytes and IDs, schema/interpreter identity, and the authorization
   view root/version. A missing input makes the result unresolved rather than
   eligible for replay or comparison.
5. The ledger MUST distinguish `legacy_succeeded_without_candidate`,
   `candidate_rejected`, `append_pending`, `materialization_pending`,
   `compared_equal`, `compared_different`, and `outcome_unknown`. It MUST NOT
   collapse those states into a boolean success metric.
6. The ledger and associated retained artifacts MUST have predeclared local
   byte, entry, and age bounds. On bound exhaustion, the operator MUST stop the
   shadow run or mark subsequent intents uncovered. It MUST NOT silently evict,
   rewrite, or merge evidence; an archival or disposition action requires
   owner approval.
7. A run MUST use only local test or owner-authorized dogfood data. Apart
   from §2 rule 5 seeding of admitted members, it MUST neither contact a peer
   nor make candidate inventory or shadow evidence discoverable to a peer,
   relay, or untrusted local client.

### 5.2 Rebuild rules

1. The shadow projection MUST be rebuilt from an empty projection, or from a
   verified compatible checkpoint only when checkpoint use is authorized by
   the applicable owner gates. It MUST NOT seed itself from copied legacy rows.
2. Rebuild uses only the retained exact candidate bytes and artifacts admitted
   under the run's pinned rules. It MUST never use live SurrealDB values,
   event ordering, local receipt order, or UI buffers as replay inputs.
3. The same admitted candidate closure MUST be replayed at least three times
   from independently initialized empty shadow projections. The canonical
   projection export and root MUST be byte-identical across all three runs.
4. A missing parent, unknown schema, opaque payload, invalid candidate,
   authorization failure, or materializer failure makes the affected source
   selection unresolved. The run MUST not compare a partial projection as if
   it were complete.
5. D-CL19 permits branch-mode validation only when a run uses the exact
   sorted-frontier commitment and replays Manager+-authorized verse-scoped
   create, pause, retarget, and detach operations. A run still MUST NOT claim a
   local canonical selection, checkpoint, cursor, compaction result, or any
   Workstream G behavior before the owner approves the complete SPEC set.

### 5.3 Projection comparison

1. The comparator MUST compare deterministic canonical exports, not raw
   SurrealDB storage layout, physical row order, generated record IDs, cache
   entries, local timestamps, or presentation floats.
2. For every candidate-covered intent, comparison MUST account for the target
   entity's existence/tombstone state, hierarchy membership, canonical
   transform integers, schema-defined properties, and every mapped lifecycle
   effect. A tombstone mismatch is never an ignorable deletion artifact.
3. A comparator MAY normalize an explicitly documented legacy representation
   only through its mapping's exact canonical quantization or a lossless
   representation conversion. It MUST emit the source and normalized value
   for every such normalization. Tolerance-based float equality is not
   permitted for canonical state; an unquantizable or out-of-range value fails
   validation.
4. The comparator MUST produce a machine-readable difference record containing
   the run, correlation, scope, candidate ID, legacy export hash, shadow export
   hash, field path, and classification. It MUST redact or encrypt payload
   values according to the run's authorization policy.
5. A comparison of current live state is insufficient. The run MUST also
   compare historical prefixes and restart/replay results, so a compensating
   sequence cannot conceal an earlier divergent reduction.

## 6. Measurable validation and cutover evidence

### 6.1 Required measurements

For each pinned validation run, operators MUST report the following values by
mutation kind and scope class:

| Measurement | Required value for promotion |
| --- | --- |
| Capture completeness | `legacy_success_with_candidate / legacy_success` is 100% for every supported mutation kind. Unsupported kinds are 0% coverage and block promotion. |
| Ingress coverage | `unmapped_bypass_count` is 0 for every enabled editor and API mutation surface: every `SCENE_WRITE_CENSUS` variant, including GLTF import, duplicate, and rename, plus Verse/Fractal/Petal creation once their D-CL20 contracts exist. |
| Candidate integrity | 100% of retained candidates decode/re-encode canonically, verify their derived IDs and signatures, and match the retained payload reference. |
| Admission and replay completeness | 100% of candidates expected for the selected history are admitted or have an explicit non-success disposition; no required candidate is missing, opaque, pending, or quarantined at comparison time. |
| Projection parity | 0 unapproved field differences across every captured prefix, final selection, and three independent empty-state replays. |
| Determinism | The three shadow rebuild exports and roots are byte-identical for every selected history. |
| Crash recovery | 100% of planned interruption points resolve to one recorded correlation disposition with no duplicate legacy mutation, duplicate canonical effect, or unclassified outcome. |
| Authorization and privacy | 0 instances of an unauthorized scope, payload, candidate inventory, comparison value, or preview becoming visible through the migration path. |
| Local-editor service | The flag-disabled and shadow paths meet the owner-approved local interaction latency and error-rate budget measured against the recorded `legacy_only` baseline. |
| Storage accounting | Every candidate, payload artifact, comparator export, and diagnostic is classified by local retention policy; 0 unbounded or unaccounted stores are permitted. |
| Network confinement | The only canonical network events a migration mode causes are record-relay seeding of admitted members (§2 rule 5). 0 shadow-evidence or unadmitted-candidate bytes leave the process. Legacy API/WS listeners and legacy row replication transmit 0 candidate, payload, capability, shadow, or comparator data. |

1. The owner-approved run charter MUST declare the exact selected histories,
   mutation corpus, interruption points, device profiles, baseline window, and
   local service budget before a formal run starts. These values make the last
   two measurements reproducible without silently choosing a product-retention
   or network policy.
2. The corpus MUST cover every currently supported mapped mutation kind and
   include hierarchy creation, transform changes, concurrent-intent ordering,
   tombstones, restart/replay, denied writes, malformed candidates, and every
   enabled D-CL19 branch mode. A feature not present in the corpus cannot
   satisfy the cutover claim.
3. A validation report MUST retain the counts and the complete set of
   difference, exception, and unresolved-outcome records. A percentage without
   its denominator is insufficient evidence.

### 6.2 Required conformance and operational cases

A future implementation MUST provide deterministic tests with at least these
names and outcomes:

1. **`legacy_only_preserves_local_editor_path`** — disabling all canonical
   modes creates no candidate and preserves the local editor result.
2. **`commit_operation_derives_legacy_and_candidate_set_from_one_intent`** —
   the future ingress produces one correlated legacy attempt and one complete
   candidate-set request without reading a mutated SurrealDB row or old-value
   payload. The fixture includes multiple scope-local members with their
   required causal links.
3. **`shadow_candidate_failure_never_repairs_legacy_state`** — candidate
   construction, append, and materialization failures are observable evidence
   and cannot cause a compensating legacy write or a canonical success reply.
4. **`legacy_failure_taints_preappended_shadow_candidate`** — a candidate that
   survives a failed legacy mutation is retained as evidence, blocks the run,
   and is never compared as equivalent success.
5. **`shadow_rebuild_is_empty_state_and_arrival_independent`** — three clean
   replays of identical admitted bytes produce identical exports and roots
   without reading live SurrealDB state.
6. **`cutover_comparator_rejects_lossy_float_or_old_value_mapping`** — float
   tolerance, a stale materialized value, or an undocumented field exclusion
   fails validation rather than masking a divergence.
7. **`comparison_covers_prefixes_tombstones_and_restart_recovery`** — each
   selected history's prefixes, tombstones, restart, and replay have exact
   disposition and parity evidence.
8. **`migration_flags_are_local_default_off_and_isolated_from_networking`** —
   the mode is local and starts at the §2 rule 2 default; `legacy_only`
   remains selectable and creates, admits, and seeds nothing; no mode sends
   shadow evidence or unadmitted candidate bytes to any process, and none
   sends candidate data over a legacy API or WebSocket listener. The name
   predates the D-CL37 erratum and is kept for test selection.
9. **`unresolved_candidate_or_branch_contract_blocks_promotion`** — a missing
    candidate, materialization pending state, unresolved branch selection, or
    missing D-CL20 materializer contract for Verse/Fractal/Petal creation, or
    a counted `Unsupported` census mapping cannot advance
    `canonical_authoritative_local`.
10. **`authoritative_rollback_stops_writes_before_legacy_fallback`** — once
    authority changes, a failure stops writes and preserves canonical evidence;
    it cannot silently resume divergent legacy authoring.

## 7. Owner approval gates

The gates below are prerequisites, not approvals granted by this document.
The project owner records each approval with the reviewed evidence.

1. **SPEC gate — before Workstream G:** The owner MUST approve SPEC-1 through
   SPEC-8 as a consistent set, including SPEC-1 byte-exact vectors and their
   test. Before that approval, work is limited to the specification and the
   independently authorized hardening substrate, including `seam_ready` with
   legacy-only behavior; no Workstream G implementation, dual emit,
   materializer, or local authoritative mode may begin.
2. **Ratified protocol contracts — D-CL17 through D-CL19:** V1 encryption/key
   lifecycle, exceptional identity correction, and sorted-frontier branch
   controls are specified, but their implementation remains blocked by the
   SPEC gate and Workstream G authorization. This plan does not enable them.
3. **Legacy multi-row gate — D-CL20, D-CL47:** Verse/Fractal/Petal creation
   remains deliberately deferred and needs an approved atomic intent schema,
   candidate-set, materializer, replay, and failure contract in SPEC-4/SPEC-8
   before canonical cutover. GLTF import, node duplicate, and node rename
   left this gate under D-CL47; their contracts are SPEC-4 §10.1-§10.3.
4. **Local cutover gate:** After the preceding applicable gates, every
   applicable normative conformance case from SPEC-1 through SPEC-7 and §6.2
   MUST pass for the enabled mode, including SPEC-4 reproducible analytics
   source identity. The owner MUST then review a complete §6 validation report
   and explicitly authorize a named local environment, candidate mapping set,
   materializer version, and rollback operator before
   `canonical_authoritative_local` is enabled.
5. **Network gate:** Network, Iroh, relay, peer replication, and seeding are
   not a migration stage. D-CL35 lifted this gate on 2026-11-20 for relay
   seeding, `/ws/canonical`, inbound canonical P2P, and the signed sync plane;
   their behavior is governed by SPEC-6, SPEC-7, and
   [relay-custody-durability.md](relay-custody-durability.md), not by this
   plan. A local cutover approval does not imply any further network
   approval.

## 8. Design notes

- **Preserve the editor before proving the log:** Shadow failures are evidence
  during migration, not permission to degrade a user into an unproven source
  of truth. That is why legacy remains authoritative until a deliberate local
  cutover, and why a post-cutover rollback is stricter.
- **Parity is a replay property:** Comparing one final database image misses
  order, tombstone, and crash bugs. Empty-state replay, historical prefixes,
  and exact canonical exports make the comparison meaningful for both the
  editor and the future analytics surface.
- **A seam prevents split authorship:** One ingress derived from one intent
  gives validation a stable correlation point. It does not make two stores
  atomic by assertion; failed and indeterminate outcomes remain explicit.
- **Local-first does not mean network-ready:** A successful local materializer
  validates deterministic state reduction. It says nothing about capability
  privacy, relay retention, peer availability, or transport safety, which
  remain separately gated.

### Errata

Owner decisions D-CL37, D-CL42, and D-CL47, 2026-11-20 (track
`relay_record_warehouse_20261120`).

- **RRW-1 (2026-11-20, D-CL37, D-CL42):** §1 rules 4, 7, and 9, §2 rules 2
  and 5, §3 (table and rules 1, 3, 4, 5), §4.3 rule 7, §5.1 rule 7, §6.1
  network row, §6.2 test 8, and §7 gate 5 changed. Dual emit now covers every
  `SCENE_WRITE_CENSUS` command, and its admitted members are canonical records
  that may be seeded to the verse's record relays, because D-CL36 makes those
  relays the durable record. Before this erratum §2 rule 5 kept all candidate
  bytes local, which made a relay record impossible. The line that still
  holds is between canonical records and shadow evidence: correlation rows,
  comparator exports, and unadmitted bytes stay local, and nothing reaches the
  legacy public surfaces or legacy row replication. The project's own build
  now defaults to `dual_emit_shadow` (D-CL42). `legacy_only` stays as a local
  kill switch so an operator can stop emission without a rebuild. Legacy row
  replication stays authoritative, and `canonical_authoritative_local` stays
  unapproved.
- **RRW-2 (2026-11-20, D-CL47):** §4.2 rule 5 and §7 gate 3 now defer only
  `CreateVerse`, `CreateFractal`, and `CreatePetal`. `ImportGltf`,
  `DuplicateNode`, and `RenameNode` are mapped under the SPEC-4 §10
  contracts, so the relay record has no gap for scene writes. §4.2 gained rule
  6, which fixes row identifiers before either side effect. A multi-row
  candidate that read its node or asset id back from the legacy result would
  be reverse-built from database outcome (§2 rule 3), and two replays could
  not agree on it. An `Unsupported` mapping is now counted and named, and it
  blocks closure, rather than being a permitted state.
- **RRW-1a (2026-11-20, critic review):** §3 rule 5 now gates seeding on the
  verse's `warehouse_mode` (D-CL43, default off) and a non-empty
  `record_relays` list. Without it, rule 5 let every dual-emit build seed by
  default, and the replicated per-verse setting read as if it could switch the
  local migration mode, which §2 rule 2 forbids for any replicated row.
