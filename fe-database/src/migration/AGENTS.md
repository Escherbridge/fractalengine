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

**Nothing in this codebase is ever `Mapped`.** `register_mapped` refuses all
six D-CL20 kinds unconditionally (§4.2.5: inventing a per-kind canonical
schema for one of them before its approved contract exists is forbidden, not
merely undesirable), and no non-D-CL20 kind has a reviewed schema yet either.
`MappingInventory::seeded()` therefore always returns every kind as
`UnmappedDeferred` — this is the correct current state, not a placeholder to
be "filled in" casually; each entry needs its own reviewed SPEC-4/SPEC-8
mapping first.

## §bypass-counter

`record_bypass()`/`bypass_count()` follow the same process-global `AtomicU64`
pattern as `REPLICATION_DROPS` in `fe-database/src/lib.rs` (see
`fe-database/src/AGENTS.md` §replication-backpressure): a single counter,
`Ordering::Relaxed`, read via an accessor function. It feeds the §6.1
"Ingress coverage" measurement (`unmapped_bypass_count`). `boundary.rs` calls
`record_bypass()` exactly when `CandidateDerivation::derive` fails — it does
**not** call it on shadow-ledger bound exhaustion, which is a distinct,
orthogonal failure mode (a *mapped* kind can still hit a full ledger).

## §candidate-derivation

`CandidateDerivation::derive(&self, intent: &dyn IntentSnapshot)` (`candidate.rs`)
takes no database handle and no access to a mutated row. This is not a
convention callers are trusted to honor — it is the type signature. There is
no argument through which an implementor could reach live SurrealDB state, so
§2.3's ban on reverse-building a candidate from an already-mutated row is
enforced structurally, the same way `CausalMaterializer::reduce` in
`fe-canonical-log` enforces its own two-input purity contract.

`MigrationCandidateMember::envelope_bytes` is opaque `Vec<u8>` — this module
never encodes or decodes it. Canonical envelope encoding is another
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
call site (`handlers/crud.rs`, `op_log.rs`) — that wiring, along with the
D-CL20 bypass-counter one-liners at each of the six deferred handlers, is
serial integration-wave work that lands after every Wave 3 slice merges.
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

## §no-shadow-exposure

Per §2.5, shadow evidence (candidate bytes, ledger rows, comparator output,
projection roots) MUST NOT be exposed through the current public asset, tile,
API, or WebSocket surfaces. Nothing in `fe-database/src/migration/` is
imported by `fe-api`, `fe-ui`, or any WebSocket/replication code path — this
module has no dependents yet, and adding one that surfaces shadow data
through any of those layers before the owner approves a cutover would
violate this invariant.
