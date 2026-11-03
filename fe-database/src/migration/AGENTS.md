# fe-database/src/migration — module notes

SPEC-8 (`docs/spec/canonical-log/migration-plan.md`) staged-migration scaffolding.
Every module here wraps — never modifies — the existing legacy log-first seam
(`op_log.rs`, `handlers/crud.rs`). Code carries terse one-line doc comments;
the "why" lives here.

## §flag-isolation

`MigrationMode`/`MigrationFlags` (`mode.rs`) read exactly one thing: the local
process environment variable `FE_MIGRATION_MODE`. This was chosen over a cargo
feature or a database row for three reasons SPEC-8 §2.2 requires jointly:

- **Runtime, not compile-time.** A cargo feature bakes the mode into the
  binary; an operator couldn't flip a build back to `legacy_only` without a
  rebuild. An env var is the cheapest mechanism that is still entirely local
  and requires no code change to change.
- **Never remotely activated.** §2.2 forbids a flag being settable by "a
  peer, relay, WebSocket client, or replicated row." A DB row is reachable by
  anything that can write to that table — including, eventually, a
  replicated write. A process-local env var has no such path by
  construction: nothing outside this process can set it.
- **No silent fallback.** `from_env()` returns a typed `MigrationFlagsError`
  for an unrecognized token and for `canonical_authoritative_local`
  specifically (owner-gated, §3.4) rather than silently defaulting. Only
  *absence* of the variable defaults, to `LegacyOnly`.

`diagnostic_summary()` exists so any process can log its resolved mode at
startup per §2.2's "MUST state its mode in diagnostics."

## §mapping-inventory / §inventory-scope

Addressed invitation issuance, admission and Manager-proof finalization are registered
as deferred mutation kinds. Preparation and readiness inspect current authority without
publishing state and remain outside this inventory. Local proof activation and the
checked role writer do not establish a canonical mapping or paired convergence.

`MappingInventory` (`mapping_inventory.rs`) is seeded from a **hand-written**
list of every currently-dispatched `DbCommand` (`fe-runtime/src/messages.rs`)
mutation variant, split into the six permanently-deferred D-CL20 kinds
(`D_CL20_MUTATION_KINDS`) and everything else
(`OTHER_DISPATCHED_MUTATION_KINDS`). It is hand-written, not derived from
`DbCommand` via a macro or reflection, because there is no reliable way to
distinguish a mutating variant from a read-only one (`ResolveNodeScope`,
`ListApiTokens`, `RawQuery`, `CountNodeDescendants`, …) from the enum shape
alone — the distinction is semantic and documented per-variant. Read-only /
query variants (`Ping`, `Shutdown`, `LoadHierarchy`, every `Resolve*`,
`List*`, `Get*`, `RawQuery`, `CountNodeDescendants`) are excluded on purpose:
they never reach a future `commit_operation` boundary, so counting them would
inflate `unmapped_bypass_count` with operations that were never candidates
for coverage in the first place.

When a new mutating `DbCommand` variant is added to `fe-runtime`, add its name
to `OTHER_DISPATCHED_MUTATION_KINDS` (or to `D_CL20_MUTATION_KINDS` if it is
one of the deferred multi-row kinds) in the same change — there is
intentionally no automated check that would catch a missed one yet.

**Pre-existing drift, fixed (CDL-SPEC10 A3).** `InsertIotReadings` was a
dispatched mutation kind (`lib.rs`'s dispatch arm, `fe-runtime/src/messages.rs`)
since M2/F7, but was never added here — exactly the drift this rule exists to
prevent. This slice adds it, alongside its own two new mutating variants
(`RegisterDeviceFormat`, `InsertDevicePayloads` — `ListDeviceFormats` and
`ListDevicePayloads` are read-only and correctly excluded).

**Nothing in the live product is ever `Mapped`.** `register_mapped` refuses all
six D-CL20 kinds unconditionally (§4.2.5: inventing a per-kind canonical
schema for one of them before its approved contract exists is forbidden, not
merely undesirable). `MappingInventory::seeded()` always returns every kind as
`UnmappedDeferred` — this is the correct current state, not a placeholder to
be "filled in" casually; each entry needs its own reviewed SPEC-4/SPEC-8
mapping first. The one reviewed mapping that exists (`UpdateNodeTransform`,
§transform-mapping) is registered `Mapped` only by the equivalence harness's
own run charter, on its own inventory value (§equivalence-harness); `seeded()`
is unchanged.

**Observation family (CDL-SPEC10 B3).** `register_observation_family()` maps
`InsertIotReadings` and `InsertDevicePayloads` (`OBSERVATION_FAMILY_MUTATION_KINDS`) onto
the approved SPEC-10 kind-5 schema hash and `observation-batch-reference@1`. Like the
`UpdateNodeTransform` charter it is called on a run charter's own inventory; `seeded()` is
unchanged and `RegisterDeviceFormat` stays unmapped (SPEC-10 §8 rule 7). The comparator's
`QuantizedValue::f64_bit_pattern` is the observation family's exact-bit `value` rule
(SPEC-10 §8 rule 4): bytes, so §5.3.3's no-float rule still holds. Rationale:
`canon_log/AGENTS.md` §observation.

## §bypass-counter

`record_bypass()`/`bypass_count()` follow the same process-global `AtomicU64`
pattern as `REPLICATION_DROPS` in `fe-database/src/lib.rs` (see
`fe-database/src/AGENTS.md` §replication-backpressure): a single counter,
`Ordering::Relaxed`, read via an accessor function. It feeds the §6.1
"Ingress coverage" measurement (`unmapped_bypass_count`). `boundary.rs` calls
`record_bypass()` exactly when `CandidateDerivation::derive` fails — it does
**not** call it on shadow-ledger bound exhaustion, which is a distinct,
orthogonal failure mode (a *mapped* kind can still hit a full ledger).

**Per-kind counters (CDL-W4 slice W4-1).** `record_bypass_for(kind)` /
`bypass_count_for(kind)` add a fixed `[AtomicU64; 6]`, index-aligned with
`D_CL20_MUTATION_KINDS`, alongside the global counter above —
`record_bypass_for` increments both its own kind's slot *and* the global
`UNMAPPED_BYPASS_COUNT` in the same call, so `bypass_count()` and
`MeasurementReport` keep exactly their existing, broader meaning; nothing
reads the per-kind slots yet (§6.1 still reads the global accessor). Each of
the six D-CL20 handlers (`create_verse_handler`, `create_fractal_handler`,
`create_petal_handler`, `import_gltf_handler`, `rename_node_handler`,
`duplicate_node_handler`, `handlers/crud.rs`) calls `record_bypass_for("<its
exact D_CL20_MUTATION_KINDS string>")` as the FIRST statement in its body —
before any lookup, authorization-adjacent query, or mutation — so a bypass is
recorded even when the legacy op itself is later refused downstream (e.g. a
`RenameNode` on an already-tombstoned node still counts: the call precedes
the `tombstone = NONE` lookup). The same holds for `ImportGltf`: an import the
KTX2 guard (`da3e6a1`, `fe_runtime::ktx2_guard`) refuses still counts as an
attempt for bypass-counter purposes, because the counter line precedes the
guard. This is strictly additive: no `OpLogEntry` is
built, `commit_operation_dual_emit` is not called, and no canonical payload
is invented — `git diff` on `crud.rs` is exactly six `+` lines, zero `-`
lines. A `kind` outside `D_CL20_MUTATION_KINDS` only increments the global
counter (`debug_assert!` catches a caller typo in debug builds).

## §candidate-derivation

`CandidateDerivation::derive(&self, intent: &dyn IntentSnapshot)` (`candidate.rs`)
takes no database handle and no access to a mutated row. This is not a
convention callers are trusted to honor — it is the type signature. There is
no argument through which an implementor could reach live SurrealDB state, so
§2.3's ban on reverse-building a candidate from an already-mutated row is
enforced structurally, the same way `CausalMaterializer::reduce` in
`fe-canonical-log` enforces its own two-input purity contract.

`IntentSnapshot::intent_fields()` (CDL-W4 W4-9) is the one way a per-kind
derivation reads the intent's typed fields: it returns the snapshot value
itself as `&dyn Any` for the derivation to downcast (`TransformIntent` overrides
it; the default is `None`). It exposes the intent and nothing else, so the
structural guarantee above still holds.

`MigrationCandidateMember::envelope_bytes` is opaque `Vec<u8>` to the generic
boundary and ledger code — they never encode or decode it. A per-kind mapping
(§transform-mapping) builds and reads real SPEC-1 envelopes. Canonical envelope encoding is another
Workstream G slice's concern; this scaffolding only carries the bytes and
hashes them for ledger identity (`MigrationCandidateSet::candidate_byte_hashes`).

## §member-op-ids

§5.1.3 calls the candidate set's member `op_id` values "the canonical
evidence." This scaffolding does not implement the canonical envelope/append
layer that assigns real `op_id`s, so `boundary::build_ledger_entry` always
writes `member_op_ids_json = "[]"` and uses the BLAKE3 candidate-byte hashes
as the ledger's interim identity for retained candidate bytes
(`candidate_byte_hashes_json`). Populating real member `op_id`s is
integration-wave work once the canonical append layer exists — do not treat
the empty array as a bug.

## §shadow-ledger

`shadow_store.rs` writes against the `migration_shadow_ledger` table by its
contractual column names (`entry_id`, `run_id`, `intent_digest_hex`,
`mutation_kind`, `run_local_correlation_id`, `member_op_ids_json`,
`candidate_byte_hashes_json`, `disposition`, `created_at`) without depending
on any specific Rust struct from `schema.rs` — that table's schema is owned
by the parallel `W3-db-canon-log` slice. `append_ledger_entry` is
**append-only**: `LedgerBounds` (entry count / content bytes / run age) are
checked before every write, and exceeding one returns
`AppendOutcome::Exhausted` rather than evicting, rewriting, or merging an
existing row (§5.1.6). A "taint" or "materialization pending" state
transition is recorded by appending a **new** row, never by updating the
original one — the ledger's immutability is therefore structural: this module
exposes no update or delete operation at all.

## §testing

`boundary::commit_operation_dual_emit` depends on four traits
(`IntentSnapshot`, `CandidateDerivation`, `ShadowMaterializer`,
`ShadowLedgerWriter`) precisely so `tests/migration_conformance_test.rs` can
exercise dual-emit logic with local in-memory fakes instead of a real
canonical envelope encoder, a real materializer, or real shadow persistence.
The one piece that is *not* abstracted is the legacy leg itself
(`op_log::commit_operation`, which needs a real `Db`) — because that function
is explicitly unmodified, the conformance tests still spin up a throwaway
in-memory SurrealDB (`surrealdb::engine::local::Mem`, the same pattern as
`fe-database/src/lib.rs`'s private `migration_tests` module) purely to host
the legacy write path. "Local test doubles" means the migration-specific
abstractions are faked, not that the pre-existing op-log dependency on
SurrealDB is removed.

## §rebuild / §materializer-purity

`ShadowMaterializer::reduce(&self, member: &MigrationCandidateMember)`
(`rebuild.rs`) mirrors `CausalMaterializer::reduce`'s two-input purity
contract from `fe-canonical-log`: it may read only `member`. This is what
makes `replay_admitted_closure_three_times`'s three-way determinism check
meaningful rather than a coincidence — a materializer that consulted
anything else (a clock, a counter, live DB state) would make the three
replays diverge by construction, and the function has no way to give it
anything else to consult.

## §comparator

`comparator.rs`'s `QuantizedValue` enum has no float variant. This is
deliberate: §5.3.3 forbids tolerance-based float equality outright, so the
type system removes the possibility of writing a tolerance comparison rather
than relying on review to catch one. A legacy float that cannot be losslessly
quantized becomes `QuantizedValue::Unquantizable { raw_debug }`, and
`compare_projections` treats *any* pairing involving `Unquantizable` as a
difference unconditionally — even two `Unquantizable` values with identical
`raw_debug` text are reported as different, because textual similarity is not
canonical equality.

## §report / §always-blocked

`report::MeasurementReport::blocks_promotion` returns `true` today, and will
keep returning `true` until real progress closes two independent gaps:
`ingress_coverage` cannot pass while `MappingInventory::unmapped_count()` is
nonzero (permanently true for the six D-CL20 kinds until each gets its own
approved mapping), and `local_editor_service` cannot pass without an
owner-approved local interaction latency/error-rate budget, which no run
charter in this build declares. **This is correct, not a bug to work
around** — do not "fix" `blocks_promotion` to return `false` to unblock
local testing; that would defeat the entire point of §6.1's gate.

## §boundary

`boundary::commit_operation_dual_emit` is not wired into any real handler
call site (`handlers/crud.rs`, `op_log.rs`) — that wiring is serial
integration-wave work still pending CANON-DUAL-EMIT ratification (the
D-CL20 per-kind bypass-counter one-liners at each of the six deferred
handlers landed in CDL-W4 slice W4-1, see §bypass-counter above; they are
observation only and do not touch this boundary).
Ordering inside `commit_dual_emit_shadow` is load-bearing and matches SPEC-8
§4.1/§4.3 exactly: `derive()` runs before any legacy attempt; on success the
candidate is appended to the shadow ledger (durable) *before* the legacy
mutation runs, so a subsequent legacy failure can be recognized as tainting
already-durable evidence (`CorrelationDisposition::CandidateRejected`)
instead of a routine miss; the legacy mutation itself is attempted exactly
once, in every branch, with its `anyhow::Error` propagated unmodified on
failure so error text stays byte-identical between `LegacyOnly` and
`DualEmitShadow`. `ShadowRebuild`'s live commit path reuses the same
dual-emit logic as `DualEmitShadow` — the mode's distinguishing three-replay
verification (`rebuild.rs`) is a separate, out-of-band procedure, never
something this boundary call performs inline (§4.1.6: the boundary must
never feed a shadow result back into the live editor path).

## §transform-mapping (CDL-W4 W4-9)

`transform_mapping.rs` is the first reviewed SPEC-8 §4.2 mapping, for
`UpdateNodeTransform` (single-row, not D-CL20, quantizable). The §4.2 rule 2
mapping statement, item by item:

| §4.2 rule 2 item | This mapping |
| --- | --- |
| Source fields | `TransformIntent`: the caller's target `petal_scope` (the §4.1 rule 2 boundary request carries it; the `DbCommand` has no petal) plus `node_id`, `position`, `rotation`, `scale` exactly as the command carries them. |
| Integer quantization | `ExactTransformQuantizer`: `round(v * 1e9)` per axis after widening `f32` to `f64`; position in nanometres, rotation converted radians to degrees first then nanodegrees (Euler XYZ, as stored), scale in parts per billion (SPEC-1 §4). Non-finite or `i64`-overflowing values refuse derivation. The legacy export uses the SAME function on the stored `f64` columns, which hold the exact widened `f32`, so both sides quantize identical inputs (§5.3 rule 3). |
| Scope derivation | `Scope { verse: legacy_identifier("verse", verse_id), petal: legacy_identifier("petal", petal_id) }` through `fe_sdk::legacy_identifier` (SPEC-10 §4.5; never a local hash). The node anchor `legacy_identifier("node", node_id)` goes inside the encrypted payload only (§4.5 rule 3). |
| Candidate set / parents | One kind-1 member. Parents are the run's ADMITTED frontier (`record_admitted` advances it): two intents derived with no admission between them are causal siblings. That is how concurrent intents arise, and SPEC-4 deterministic causal order, not arrival order, decides which wins. |
| Author / capability | The run charter's author key and `append/op` chain at the verse's current epoch; the payload is sealed under the verse-wide scope key (`TransformCandidateDerivation::new` refuses a non-verse-wide key or an epoch mismatch). |
| Payload (`node-transform-v1`, provisional) | Canonical CBOR map `{0: node anchor bstr32, 1: position [i64;3], 2: rotation [i64;3], 3: scale [i64;3]}`. No previous value (§4.2 rule 3); any fifth key is refused. Schema hash `BLAKE3("fe-schema-node-transform-v1\0")` (the SPEC-10 §4.1 rule 3 convention W4-7's lifecycle schemas use). Registered on the production verifier by `register_node_transform_schema` (the W4-7 critic-C3 call). |
| Expected effect / comparator field | `node/<anchor hex>/transform = Text("position_nm=[..];rotation_ndeg=[..];scale_ppb=[..]")`, or `Tombstoned` on the legacy side for a tombstoned row. One field per node, because `ShadowMaterializer::reduce` returns one effect per member. |
| Materializer version | `node-transform-shadow` v1 (its `MaterializerVersion::registry_id()` is pinned in the evidence). |

- **Stamps are an explicit unsupported sub-case.** A `node_kind = 'stamp'`
  node is position-locked by a private check inside the live handler (D-A5).
  Rather than copy that check, `dual_emit_update_node_transform` refuses a
  stamp before any side effect (`StampOutsideMapping`) and the caller keeps the
  legacy path (§4.2 rule 1, "explicit unsupported status").
- **The legacy leg is a copy, pinned by a drift guard.** Handlers are not
  edited (the live path stays `LegacyOnly`), and the live handler builds its
  `OpLogEntry` and `UPDATE` inline, so `dual_emit_update_node_transform`
  repeats them verbatim after the live precondition (`require_node_exists`).
  The harness writes one transform through the live `DbCommand` and the same
  transform through this composition in `LegacyOnly`, and asserts equal node
  rows and op-log payloads. If `handlers/transform.rs` changes, that assertion
  fails. The DB thread's dispatch-level side effects (scene-change broadcast,
  `node_log`) are UI deltas, not projection, and are not repeated.
- **`TransformShadowMaterializer` purity.** It reads `member` plus two
  content-addressed sources the run pinned: ciphertexts (`RetainedCandidates`
  during the run, the durable `SurrealArtifactStore` after a restart) and scope
  keys (custody). Neither carries ordering or DB state, so the three replays
  are deterministic. It opens payloads with the same op, petal, verse key
  search and the same `aead::open` + SPEC-1 §5.2 AAD as the composed verifier.
  It does not re-run authorization: replay inputs are admitted candidates only.
- **Retention (§5.1 rule 4).** `RetainedCandidates` holds each derived set by
  intent digest and each ciphertext by hash, in process memory: bounded by the
  run, never served, never persisted outside it. After admission the
  ciphertext is also in the local-only artifact store (W4-7).

## §equivalence-harness (CDL-W4 W4-9)

`fe-database/tests/migration_equivalence_harness.rs` is the harness: a test
binary, not a product surface. Owner R7 decision 3 (2026-10-13): this
dev-evidence run on a harness-created test verse is sufficient for now; the
formal §6.1 validation run still waits on an owner-approved run charter.

What one run does, in order:

1. Creates the test verse, fractal, petal and nodes through the live
   `DbCommand` path (§5.1 rule 2: no legacy history claimed). The hierarchy
   creates are D-CL20 kinds, so they record per-kind bypasses.
2. Seeds canonical authority for that verse with in-test keys: a SPEC-1 kind-2
   `branch_genesis` (zero parents, no-payload form; the golden
   `branch-genesis-empty-payload` vector's shape, because the golden bytes name
   a fixture verse) as the run's FIRST canonical artifact, a Manager epoch bump
   to epoch 1, the author's `append/op` chain, and the verse key in durable
   custody.
3. Runs the corpus through `commit_operation_dual_emit` in `DualEmitShadow`:
   sequential transforms; a concurrent pair, admitted in reverse order; a
   malformed candidate (one signature byte flipped, so `InvalidEnvelope` and
   nothing appended); two denied writes that must leave no evidence (an
   `Anonymous` `TombstoneNode` the DB thread's policy denies, and a transform
   on a missing node refused by the live precondition before derivation); a
   mid-run DB-thread restart; post-restart transforms (one merges the two
   siblings); and a live tombstone.
4. Admits every candidate through `admit_and_append_durably` with THE
   production verifier, compares each intent at its own prefix (§5.3 rule 5)
   against `export_legacy_transforms`, and appends each final disposition as a
   NEW ledger row that carries the real member `op_id`.
5. Writes the `MeasurementReport` (numerators AND denominators), per-kind
   tallies, every comparison and normalization, and the replay roots to
   `target/migration-evidence/<run_id>.json` (`/target/` is gitignored).

What the run must show, and why none of it is a tautology:

- Every transform intent is `compared_equal` at its prefix, and the mapped kind
  has zero `legacy_succeeded_without_candidate`.
- Three byte-identical replays every time; the same root after the restart
  (from the durable artifact store and keystore, not process memory); the same
  root from a reversed input list. A replay that folds in arrival order
  produces a DIFFERENT root for the concurrent pair, and the test asserts it,
  so the ordering is shown to matter.
- The uncovered tombstone is reported, not masked: `TombstoneNode` is
  unmapped, so the final comparison shows exactly one `ValueMismatch`
  (`Tombstoned` against the shadow transform) on that node, counted as an
  unapproved difference.
- `blocks_promotion` stays **true** (§always-blocked): the D-CL20 kinds are
  unmapped and recorded bypasses, and `local_editor_service` has no budget.
  Do NOT "fix" this to make the report green.
- `a_lossy_transform_mapping_is_detected_as_a_difference` derives with a
  millimetre-rounding quantizer and asserts the comparator reports a
  `ValueMismatch`: the harness can fail.

`authorization_and_privacy`, `storage_accounting` and `network_prohibition`
are reported as structurally zero, not measured: the DB thread runs with no
replication sender and no sync thread, and nothing here is reachable from
fe-api.

## §no-shadow-exposure

Per §2.5, shadow evidence (candidate bytes, ledger rows, comparator output,
projection roots) MUST NOT be exposed through the current public asset, tile,
API, or WebSocket surfaces. Nothing in `fe-database/src/migration/` is
imported by `fe-api`, `fe-ui`, or any WebSocket/replication code path — this
module has no dependents outside fe-database's own tests yet (the W4-9
equivalence harness is a test binary), and adding one that surfaces shadow data
through any of those layers before the owner approves a cutover would
violate this invariant.
