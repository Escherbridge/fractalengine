---
type: runbook
title: FractalEngine session runbook
updated: 2026-08-16
head: ee1d125
---

# RUNBOOK — Canonical Fractal Data Log (Workstream G)

Written 2026-08-16. Supersedes nothing; this is the first runbook in this repo.
Read this first, then descend only where it points. `conductor/product.md`,
`tech-stack.md`, `workflow.md` and `tracks.md` are the foundation under it.

**Active track:** `conductor/tracks/canonical_data_log_20260808/`
**Decision register:** `conductor/decisions/canonical-data-log-20260808.md` (D-CL1..D-CL29)
**Wave partition:** `conductor/tracks/canonical_data_log_20260808/workstream-g-plan.json`

---

## 1. Goal

Turn FractalEngine's data layer into a signed, immutable **canonical operation
log** as the source of truth, with SurrealDB demoted to a rebuildable local
materialized view, and — as of 2026-08-16 — extend it into a **two-plane
peer-to-peer replicated time-series store** so the product can serve digital-twin
telemetry as well as scene editing.

Standing constraints, all still in force:

- **No network enablement.** `fe-sync`'s `IrohDocsEngineHolder::is_available()`
  must keep returning `false`. No relay replicas, no inbound P2P. Network
  rollout, relay seeding, and inbound P2P are the only things still requiring
  owner approval.
- **The legacy local editor must keep working** until SPEC-8 dual-emit
  equivalence is validated. The canonical path lands *alongside* it behind
  `FE_MIGRATION_MODE`, never replacing it mid-flight.
- **A concurrent agent session shares this working tree** and owns
  `fe-ui/**`, `fe-terrain/**`, and `fractalengine/src/gpx_bridge.rs`. Do not
  edit those.

---

## 2. State

`origin/main` is at **`307c2df`** (the runbook's own first commit). Local `main`
is **2 commits ahead**: `ee1d125` (the errata) and this update. The owner
directed pushing on 2026-08-16, so **`git push` is a live pending action, not a
decision to re-litigate.**

### Verified and committed

| Phase | Commit | Evidence |
|---|---|---|
| HARD-1..6 substrate hardening | `916a3ae` | 2035 workspace tests, clippy clean |
| Nonce errata (vector regenerated) | `2849f72` | oracle green, 24-byte nonce asserted |
| SPEC-1..8 owner-approved, D-CL21..25 | `21ee8ba` | — |
| **G Wave 1** — fe-canonical-log foundation | `c2cea84` | 90 + 6 + 36 tests, clippy `-D warnings` clean, fmt clean, oracle green, `cargo check --workspace` green |
| **G Wave 2** — 7 leaf slices + remediation | `5675fcb` | 373 tests, clippy clean, fmt clean, workspace check green |
| D-CL26..29 direction ratification | `4b63c53` | — |
| **Errata G1/G2/G4/G5** | `ee1d125` | 381 tests (was 373), clippy `-D warnings` clean, fmt clean, oracle green; per-package gates per D-CL23 — nothing depends on `fe-canonical-log` |

`fe-canonical-log`, `fe-sdk`, `docs/spec/`, and `conductor/` are **fully
committed and clean**. All dirty files in the tree belong to the concurrent
session.

### Review ledger

| Phase | Verdict | By |
|---|---|---|
| Original canonical-log proposal | GO design / NO-GO implementation | Fable review, all defect citations independently verified |
| Workstream G wave partition | NEEDS_AMENDMENT → amended | 2 adversarial critics, 15 critical/high defects |
| G Wave 1 foundation | NEEDS_FIXES → remediated → clean | adversarial audit, 11 findings, re-audited |
| G Wave 2 leaves | NEEDS_FIXES → remediated → clean | adversarial audit, 3 high / 5 med / 5 low |
| G Wave 2 remediation | enforcement census: 66 checks, 51 production callers, 15 documented Wave 3 contracts, **0 dormant** | re-audit |
| Time-series direction | **GO** + 1 structural correction + 6 gates | 7-scout evidence review, every claim cited to spec/code |
| Errata G1/G2/G4/G5 | **PASS — 0 findings** | `/security-review` @ `ee1d125`; see §9 for what was cleared and the one hardening item deferred to Wave 3 |

### In flight / not started

Nothing is mid-edit. Everything below is not started:

- **Errata G1, G2, G4, G5** — **DONE 2026-08-16 @`ee1d125`**, local-only (not
  pushed). 381 tests, clippy `-D warnings` clean, fmt clean, oracle green,
  `/security-review` 0 findings. Wave 3 is unblocked.
- **Errata G3, G6** — go with SPEC-10, not Wave 3.
- **G Wave 3** — operation-plane integration. Partition already grilled and
  ratified; scope unchanged by the 2026-08-16 expansion.
- **G Wave 4** — serial integration.
- **SPEC-9** — peer N-of-M durability protocol (new, greenfield).
- **SPEC-10** — observation plane / columnar hexon format (new).

---

## 3. Key context

Material a fresh read of code and git log will **not** surface.

**The recurring defect in this codebase is the built-but-never-wired gate.**
Seven instances so far: three in the 2026-07-31 security audit (hexon scope
check, `authz.rs`, sync permissive policy), one in Wave 1
(`assert_production_suite` with zero callers), three in Wave 2 (session-generation
revocation on 1 of 4 paths, seal-time crypto on 1 of 4 lanes, delta scope filter
with no caller). **The fix that works is making the check unavoidable by
construction** — required (never `Option`) parameters, `#[non_exhaustive]` types
with fallible constructors, `pub(crate)` on raw paths — never a call site a
future author can forget. **The verification that works is a mechanical
enforcement census**: classify every `assert_*`/`verify_*`/`check_*`/`require_*`/
`may_*`/`decide_*` function as PRODUCTION_CALLER / TEST_ONLY / NO_CALLER /
DOCUMENTED_WAVE3_CONTRACT. Run that census at the end of every wave.

**Two audits caught requirements that agents silently dropped.** Author
equivocation (SPEC-1 §3.4) was absent from the entire first implementation plan
and is now D-CL25. Operation scope was missing from `VerifiedEnvelopeMeta`,
which would have made four downstream MUSTs unimplementable. Assume the next
wave drops something too; the adversarial pass is not optional.

**Why the codec is hand-rolled, not `ciborium`.** `ciborium`'s serde output is
not the RFC 8949 §4.2.1 profile — it does not guarantee sorted map keys and
accepts non-minimal arguments, indefinite lengths, floats, tags, and non-NFC
text, all of which SPEC-1 requires us to *reject*. Using it would need a full
canonicalization layer anyway, giving two places for byte drift. Since
`op_id = BLAKE3(complete_envelope)`, byte drift is a consensus fork.
**Do not "simplify" this into a library dependency.**

**`decode_and_admit` is the mandatory ingress for peer bytes.** It pins the
received bytes against their own re-encoding, checks the DID-to-key binding,
verifies the signature, runs structural rules, rejects the fixture-only suite,
and derives `op_id` from the **received** bytes. Plain
`decode_canonical` + `verify_envelope` is for locally-constructed envelopes
only. The re-encode assertion currently never fires — that is exactly why it
must stay.

**The golden vectors are frozen.** If a vector test fails, the Rust code is
wrong, never the fixture. One agent was explicitly instructed on this because
"fix the fixture to match the buggy codec" is the worst possible outcome.

**A spec defect was found by a critic, not by tests.** `operation-envelope.md`
§3.5 mandated a 24-byte nonce while the only payload-bearing vector encoded 12,
and the `.mjs` oracle passed because it asserted `Buffer.isBuffer` without
checking length. The vector was regenerated using the committed `.mjs` encoder
so bytes could not diverge from the validator. Tautological assertions are a
real failure mode here; three more were found and replaced in Wave 1.

**The 2026-08-16 direction review's load-bearing findings** (each verified
against spec text and shipped code by a dedicated scout):

- **The system already anticipated the observation plane.** SPEC-3 ships
  `ObjectClass::{Shard, Tile, Asset}` with their own permission rows, and SPEC-5
  checkpoints already carry `snapshot_manifest_id` for bulk scope-affine
  artifacts. Only SPEC-6's *delivery* lane is missing (needs a fifth
  `LaneClass`).
- **Floats are structurally banned from signed bytes** — spec, `cbor.rs`, and
  tests all enforce it. Therefore hexon interiors **must stay opaque sealed
  blobs**: Parquet-with-floats is fine inside, and only exterior metadata lives
  in the CBOR signing domain. This also resolves the pre-existing fork between
  the two hexon formats (`.fecrate` in fe-hexon, `.hexon` in fe-format, with
  different signing schemes).
- **SPEC-7 previews are content-agnostic opaque bytes**, and §7.1.4's ban on
  previews becoming durable input *mandates* the dual live/durable path rather
  than merely permitting it. Live telemetry over previews needs **zero spec
  change** — just a registered `preview_kind` plus sender-side batching to
  respect the per-(principal, scope) rate bucket.
- **A canonical header measures ~546 bytes** against the actual golden vector.
  Per-reading signed operations would cost ~43 GB/day of headers **per peer**
  plus ~1 h/day of signature verification at 1 000 sensors @ 1 Hz. Batching
  ~10⁴ readings per hexon collapses both by ~4 orders of magnitude. This — not
  peer storage distribution — was the real limit.
- **Mobile header pruning behind a checkpoint is ambiguous-but-intended.**
  Conformance test 14 names "a mobile peer that releases … header data" as a
  tested path, gated by losing bootstrap-advertising rights. The mobile floor is
  structurally a replay tail since the last verified checkpoint; genesis-to-now
  is the archive's duty alone. G5 makes this affirmative.
- **`fe-terrain/src/iot/` is a false friend** — it is GPX route playback and
  route-deviation math, not sensor telemetry. The real sensor store is the
  `iot_reading` table (`fe-database/src/schema.rs`) plus
  `POST /api/v1/petals/{id}/iot/readings`, which deliberately bypasses the
  DB-thread command queue so IoT batches don't queue behind the render loop.
  A tested GeoParquet 1.0 writer already exists in `fe-query` but is scoped to
  node snapshots. The observation plane graduates these, it doesn't invent them.
- **Zero spec text or code exists** for replication factor, placement/holder
  index, under-replication detection, repair, or range→holder resolution. The
  only durability primitive is the single-holder GC lease.

**Environment gotchas that cost real time this session:**

- **Piping cargo through `tail`/`grep` masks the exit code.** Always append
  `; echo "EXIT:$?"`. A background run reported "exit 0" while actually failing.
- `os error 1455` (paging file too small) on `cargo test --workspace -j4` →
  retry at `-j2`. Full test execution needs free RAM; `cargo check --workspace
  --tests` works under memory pressure.
- Never run a bare `cargo fmt` — it would reformat the concurrent session's
  dirty files. Use `cargo fmt -p <crate>`.
- Agents writing code blind produce mechanical breakage (a moved sender, a
  missing `BTreeSet` import, `items after a test module` failing clippy). Budget
  a repair pass; don't treat it as a signal the work is bad.

---

## 4. Decisions

Full register with rationale: `conductor/decisions/canonical-data-log-20260808.md`
(D-CL1..D-CL29). The ones that shape the next session's work:

- **D-CL2** — one operation DAG per **verse**, not per petal (owner override of
  the per-petal recommendation), mitigated by sparse payload replication:
  payload-free headers replicate verse-wide, payload segments fetch per
  petal-scope capability, segments pack petal-affine.
- **D-CL5** — canonical scalars are `i64` nano-base-units (nanometres,
  nanodegrees, parts-per-billion). Rust `fe-sdk` newtypes landed; the WIT `s64`
  half is deferred because it would ripple into `fe-terrain`.
- **D-CL17** — XChaCha20-Poly1305 payload AEAD + X25519 HPKE-style scope-key
  wrap, rotating on epoch bump.
- **D-CL19** — checkpoints commit to BLAKE3 over the lexicographically sorted
  head `op_id` frontier, so multi-head frontiers checkpoint and GC freely.
- **D-CL21** — `chacha20poly1305` and `x25519-dalek` approved as normal deps.
- **D-CL23** — work proceeds on `main`, not an isolated worktree, so wave gates
  are scoped per package (`cargo check/test -p <crate>`) rather than
  `--workspace`.
- **D-CL25** — author equivocation is a first-class primitive
  (`EquivocationKey`); two distinct `op_id`s sharing one key quarantine **both**
  and materialize **neither**.
- **D-CL26** — two-plane architecture ratified (operation plane + observation
  plane; telemetry as columnar hexons committed by reference).
- **D-CL27** — peer N-of-M replication in scope now (**owner override** of
  registry-floor-first). Enters as SPEC-9 design work, not a code wave.
- **D-CL28** — six gates G1..G6 ratified.
- **D-CL29** — the new work folds into Workstream G rather than sibling tracks
  (**owner override**), roughly tripling remaining scope. Wave 3's already-grilled
  scope is preserved intact.
- **Push posture** — as of 2026-08-16 the owner directed pushing; `main` is
  published through 4b63c53. Prior waves were deliberately local-only.

---

## 5. Assumptions

`<assumption> · default taken · to reverse`

1. **The concurrent session is still active and still owns `fe-ui/**`,
   `fe-terrain/**`, `fractalengine/src/gpx_bridge.rs`** · default: treat them as
   untouchable and scope all gates per-package · to reverse: cheap if they've
   finished (check `git status`), but editing their files mid-flight would cause
   a real conflict, so verify before assuming they're done.
2. **Their in-progress `terrain_proposal` rehydration test may still be red** ·
   default: a full `cargo test --workspace` may show one external failure that is
   not this workstream's · to reverse: cheap — attribute by file path before
   investigating.
3. **SPEC-9's availability floor is undecided** · default: none chosen; the spec
   must name one · to reverse: expensive if built the wrong way — peers alone
   cannot underwrite availability, so the choice between accountable seeders, a
   registry floor, and a declared best-effort promise shapes the whole protocol.
   **Confirm this before writing SPEC-9.**
4. **The 15 documented Wave 3 contracts in the module `AGENTS.md` files are
   complete and accurate** · default: trust them as the Wave 3 obligation list ·
   to reverse: moderate — re-run the enforcement census to regenerate.
5. **Ultracode is off** · default: use the standard Workflow opt-in rule; do not
   launch multi-agent fan-outs without the user asking · to reverse: trivial.

---

## 6. Relevant files

Open these first.

- `conductor/tracks/canonical_data_log_20260808/metadata.json` — machine source
  of truth for every task's state, including the six errata.
- `conductor/tracks/canonical_data_log_20260808/workstream-g-plan.json` — the
  ratified wave partition with per-slice briefs, file boundaries, and models.
- `conductor/decisions/canonical-data-log-20260808.md` — D-CL1..29 with rationale.
- `fe-canonical-log/src/AGENTS.md` — crate invariants, module ownership, the
  ingress contract, errata E1/E2/E3, provisional wire numbering.
- `fe-canonical-log/src/compose.rs:23` — the single 15-variant `QuarantineReason`.
  **G1 adds an `UnknownKind` variant here.**
- `fe-canonical-log/src/retention/quarantine.rs` — the bounded pool.
  `admit_candidate` fails closed; `evict_expired_or_over_capacity` is
  reason-blind oldest-first. **G1 partitions budgets per reason here.**
- `fe-canonical-log/src/materialize/traits.rs:149` — `CausalMaterializer::reduce`,
  infallible. **G2 needs an expressible unavailable-artifact outcome here.**
- `fe-canonical-log/src/segment/manifest.rs:64` — `SegmentManifestBody`, which
  has no time range and no statistics. **G4 adds the tiered statistics block.**
  This is a wire format — cheap now, expensive after Wave 3.
- `docs/spec/canonical-log/branches-checkpoints-retention.md` §4 (quarantine
  bounds), §5.2 (storage-role table — **G5** makes header pruning affirmative).
- `docs/spec/canonical-log/log-first-materialization.md` §5 (admission error
  taxonomy), §6.1 (rebuild contract — the **G2** determinism bright line).
- `docs/spec/canonical-log/operation-envelope.md` §6.7 — the unknown-kind MUST
  that G1 addresses.
- `fe-canonical-log/src/kind.rs:85` — comment already flagging that unknown-kind
  quarantine is deferred to an admission layer that does not exist yet.
- `fe-database/src/op_log.rs:93` — `commit_operation`, the single log-first seam
  and where SPEC-8 dual-emit slots in.
- `fe-database/src/schema.rs` (`iot_reading`) + `fe-api/src/iot.rs` — the
  existing observation ingest that SPEC-10 graduates.
- `fe-query/src/columnar/geoparquet/` — the working Parquet writer SPEC-10 reuses.
- `docs/spec/canonical-log/operation-envelope-v1.test.mjs` — the conformance
  oracle. Run `node --test` on it after any codec change.

---

## 7. Continuation plan

**Steps 1-5 are DONE** (`ee1d125`, local-only). What landed, in case a later
reader needs the shapes without re-reading the diff:

- **G1** — `compose::QuarantineReason::UnknownKind` + `QuarantineReasonClass`
  (4 classes: the three independently-driven retry reasons plus one residual
  `Other`) + `retention::PerReasonBudgets`/`ReasonBudget` on `QuarantineBounds`.
  `admit_candidate` checks the class budget before the pool; eviction runs a
  per-class pass before the pool-wide one. `unknown_kind_promotion_ready` +
  `OperationKindAvailability` added. SPEC-5 §4 rules renumbered — former 4-7 are
  now 5, 6, 8, 9; two stale `§4 rule 4` citations were repointed.
- **G2** — `ProjectionMutation::ReferencedArtifactUnavailable { artifact_id }`.
  SPEC-4 §4 gained rules 8-9, §5 gained the `referenced_artifact_unavailable`
  and `unknown_kind` rows, §6 gained rule 7.
- **G4** — manifest wire keys **7** (`SegmentStatistics`) and **8**
  (`SealedStatisticsRef` per lane). `SegmentManifestBody::new` now takes 7
  arguments. SPEC-6 §3.3 gained rules 5-7.
- **G5** — SPEC-5 §5.2 rules 4-5, spec text only, no code in this crate.
- Nine new conformance-test names were added across SPEC-4/5/6; the Rust tests
  matching them are in place, so the §6/§8 lists and the code agree.

**Step 6 is now the head of the queue.**

6. **Wave 3** — operation-plane integration from the existing partition
   (fe-database `canon_log` + migration, fe-api canonical WS, crypto
   AEAD/keywrap, identity x25519, sync guard). It must honour the 15 documented
   Wave 3 contracts in the module `AGENTS.md` files **plus** errata G1/G2/G4/G5.
   Wave 3 touches auth and crypto paths — plan `/security-review` as its phase
   verdict, not `/code-review` alone.
7. **SPEC-9 and SPEC-10** can run concurrently with Wave 3 since they touch
   different surfaces. SPEC-9 must resolve assumption 5.3 (the availability
   floor) and carry a placement-index privacy-leakage analysis as a hard gate —
   an availability index discloses which principal holds which scope, which
   collides with SPEC-3's blinded discovery. SPEC-10 carries errata G3 (fifth
   `LaneClass`) and G6 (observation-plane source identity).

---

## 8. Open questions

- **SPEC-9's availability floor** — accountable seeders, a registry floor, or a
  declared best-effort promise? Trigger: before writing SPEC-9. Peers alone
  cannot underwrite availability; the choice shapes the whole protocol.
- **Reserved policy numbers** (quarantine bounds, GC lease durations, retention
  windows, preview rate cap, replication factor) remain deliberately
  unparameterized per D-CL24. Trigger: first deployment that needs runnable
  end-to-end defaults.
- **The two coexisting hexon formats** (`.fecrate` with raw-JSON signing,
  `.hexon` with canonical-JSON signing) are pre-existing debt that SPEC-10 will
  have to reconcile or explicitly leave separate. Trigger: SPEC-10 format design.

---

## 9. Security review verdict — errata G1/G2/G4/G5 @ `ee1d125`

`/security-review`, **0 findings**. Recorded here rather than left in a
transcript because the cleared items are exactly the questions a later reader
will re-ask about G4.

**Cleared, with the reasoning that made each clean:**

- **The new clear-text `petals` set is not a new disclosure.** The manifest body
  already carries raw petal IDs through key 5 (`PayloadTopicScope.petal_id`) and
  key 6 (`LaneKey::Payload`). Separately, SPEC-6 §2.2 rule 2 seals manifests and
  header segments under the *same* verse-wide header scope, so any reader who
  can decrypt a manifest can decrypt the headers it indexes and derive the same
  petal set from signed `scope` fields — there is no reader who gets manifests
  but not headers. SPEC-3 §7's raw-ID prohibition applies to the sealed **outer**
  map and blinded topic names (relay-visible surfaces), not to sealed plaintext
  bodies, and its risk table already records verse-wide cross-petal metadata
  exposure as an accepted D-CL2 consequence.
- **No decode asymmetry in the new paths.** `SegmentStatistics::from_cbor` uses
  the same exact-key-set `require_uint_keys` as every sibling; the
  `BTreeSet` dedup hazard is foreclosed by the explicit strictly-ascending check
  *before* insertion; `Hlc::from_cbor`'s `u32_at` returns `IntegerOutOfRange`
  rather than truncating; `assert_canonical_bytes` re-encodes the whole tree, so
  it does cover keys 7 and 8.
- **Validation is not bypassable.** All `SegmentManifestBody` fields are private,
  the only constructors are `new()` (validates) and `decode_canonical()` (routes
  through `new()` then pins bytes), there is no serde derive or mutable accessor,
  and `to_cbor()` re-validates.
- **G1/G2 weaken nothing.** `class()` maps every security-relevant reason
  (`Unauthorized`, `AuthorEquivocation`, `ArtifactIdMismatch`, `FailedDecryption`)
  to `Other`, none of which gains a promotion path; the new admission checks are
  additive and fail-closed, so they can only reject more. No `match` on
  `ProjectionMutation` exists outside `materialize/traits.rs`, so no wildcard arm
  can swallow `ReferencedArtifactUnavailable` and treat it as applied.

**One hardening item deferred to Wave 3, deliberately not filed as a defect:**
`validate()` does not cross-check `statistics` against the lanes and roots the
manifest actually carries, so a publisher could under-report a range and cause a
§3.3.7-conformant consumer to skip a segment it needs. It is implemented exactly
as §3.3.5 is written; the check is not locally decidable here because the crate
holds no header bodies at manifest-validation time; and there is no consumer of
`statistics()` yet. **Wave 3's selective-fetch consumer is where this becomes
real** — whoever builds segment skipping must validate the range against the
headers it actually receives rather than trusting the manifest's claim.
