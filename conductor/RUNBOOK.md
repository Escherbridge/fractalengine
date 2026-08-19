---
type: runbook
title: FractalEngine session runbook
updated: 2026-08-18
head: d01b098
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

`main` @ **`b820b67`**, **NOT pushed** — `origin/main` is at `b8b2e0a`, so there
are **three unpushed commits**: `29cadd9` (pre-existing repairs, §11), `b820b67`
(Wave 3 code), and this runbook update. Wave 3 is deliberately held local: the
standing push authorization covers *finished, gated* work, and a wave whose
phase verdict is FAIL (§10) is not that. **Push once §10 items 1-3 are closed
and re-reviewed** — do not push a FAIL verdict to `origin`.

All other dirty files in the tree belong to the concurrent session
(`fe-ui/**`, `fe-terrain/**`, `fractalengine/src/gpx_bridge.rs` — 56 files,
verified untouched by this wave and by `cargo fmt -p`).

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
| Two pre-existing test defects | `29cadd9` | see §11; not Workstream G work |
| **G Wave 3** — operation-plane integration | `b820b67` | **2846 workspace tests, 0 failed**; clippy `-D warnings` clean; fmt clean; oracle green. **Security verdict FAIL — see §10.** Committed because every finding is dormant; **NOT pushed** |

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
| **G Wave 3 leaves + integration** | **FAIL — 2 CRITICAL, 9+ HIGH, all dormant** | `/security-review` @ `b820b67`, separate context, prompted to refute; 4 hard constraints VERIFIED. Full verdict in §10 |

### In flight / not started

Nothing is mid-edit. Everything below is not started:

- **Errata G1, G2, G4, G5** — **DONE and PUSHED** @`ee1d125` (2026-08-16).
  381 tests, clippy `-D warnings` clean, fmt clean, oracle green,
  `/security-review` 0 findings. Wave 3 is unblocked.
- **Errata G3, G6** — go with SPEC-10, not Wave 3.
- **G Wave 3** — **CODE LANDED @`b820b67`, LOCAL ONLY, SECURITY VERDICT FAIL.**
  All six slices executed, full-workspace gate green (2846/0). Not pushed. The
  wave is *not* done: remediation of §10's must-close set is the head of the
  queue, following the same NEEDS_FIXES → remediated → re-audited path Waves 1
  and 2 took.
- **G Wave 3 remediation** — **THE HEAD OF THE QUEUE.** §10 carries the findings
  and a four-way disjoint partition ready for `/slice`.
- **G Wave 4** — serial integration. **Do not wire any caller to the canonical
  append, epoch, or WS surfaces until §10 items 1-3 are closed and re-reviewed.**
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
  `; echo "EXIT:$?"` — and when the pipe is *inside* the command, `${PIPESTATUS[0]}`,
  not `$?`. This fired again in Wave 3: the harness reported "exit code 0" for a
  run that had actually exited **101**. Redirect to a log file and grep the file
  instead of piping; you also get the whole error list rather than a tail.
- **`cargo test -p <crate>` resolves features differently from `--workspace`**
  and forces a cold `surrealdb-core` rebuild — two Wave 3 runs looked like hangs
  and were 10-minute rebuilds. Stay in `--workspace` mode and narrow with a test
  filter (`cargo test --workspace -- <name>`), which reuses the warm build.
- **`cargo test` stops at the first failing test binary.** Use `--no-fail-fast`
  at a wave barrier or you will triage one crate at a time and never see the rest.
- **`cargo test --workspace --lib -- --test-threads=1` appears to deadlock here.**
  Not diagnosed; avoid it, use filters instead.
- **A global mutex poisoned by one panicking test cascades.** Eight red tests in
  Wave 3 traced to a single missing `init_hlc(0)`: the panic happened *while
  holding* `HLC_STATE`, so every later test died on `PoisonError`. When many
  tests fail at one `.lock().unwrap()`, find the one that panicked *inside* the
  critical section — the rest are victims. Run a suspect alone to confirm.
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
  published through `5b4d12b` as of 2026-08-17. Prior waves were deliberately
  local-only. Push finished, gated work; do not ask again.

### Owner calls, 2026-08-17 — how Wave 3 runs

Settled at the Wave 3 boundary. Do not re-litigate these.

- **Wave 3 executes as a `/slice` fan-out**, not serially. Chosen because the
  six-slice partition is already grilled, ratified, and carries disjoint
  `owned_files` — the condition `/slice` exists for. Serial was rejected because
  Wave 3 spans 6 slices across 5 crates and would outlive one context, forcing a
  second handoff mid-wave. **Agents do not run cargo**; every slice brief already
  says so, and concurrent builds deadlock on the shared build lock. The
  orchestrator runs one serial gate at the wave barrier.
- **Wave 3 gates on the FULL WORKSPACE**, not per-package. D-CL23's per-package
  scoping was valid for Waves 1-2 only because *nothing depends on
  `fe-canonical-log`*. Wave 3 lands in `fe-database` and `fe-api`, and **7 crates
  depend on those** (`fe-renderer`, `fe-sync`, `fe-test-harness`, `fe-ui`,
  `fe-webview`, `fractalengine`, `fractalengine-relay`, plus the workspace root).
  Per-package would defer real dependent breakage to Wave 4. Run
  `cargo test --workspace` at **`-j2`** — `-j4` hits `os error 1455` here.
- **D-CL21 already approved `chacha20poly1305` and `x25519-dalek`.**
  `workstream-g-plan.json`'s `W3-crypto-aead-keywrap` brief predates D-CL21 and
  still says they are "pending owner approval" with an instruction to hold the
  slice. **That instruction is stale — the plan JSON is wrong, the decision
  register is right.** Both deps are already declared in
  `fe-canonical-log/Cargo.toml` by the Wave 1 foundation. Do not hold the slice
  and do not re-ask.

---

## 5. Assumptions

`<assumption> · default taken · to reverse`

1. **SPEC-9's availability floor is undecided** · default: none chosen; the spec
   must name one · to reverse: expensive if built the wrong way — peers alone
   cannot underwrite availability, so the choice between accountable seeders, a
   registry floor, and a declared best-effort promise shapes the whole protocol.
   **Confirm this before writing SPEC-9.** Deliberately not asked at the Wave 3
   boundary: it gates step 7, not step 6, and asking early would have spent the
   round on work the next session will not reach.
2. **The concurrent session is still active and still owns `fe-ui/**`,
   `fe-terrain/**`, `fractalengine/src/gpx_bridge.rs`** · default: treat them as
   untouchable; Wave 3 does not overlap them, so no slice needs their files · to
   reverse: cheap if they've finished (check `git status`), but editing their
   files mid-flight would cause a real conflict, so verify before assuming.
   **Note the interaction with the workspace gate:** `fe-ui` depends on
   `fe-database`, so `cargo test --workspace` WILL compile their in-progress
   work. Compile errors from `fe-ui/**` are theirs, not Wave 3's.
3. **Their in-progress `terrain_proposal` rehydration test may still be red** ·
   default: the workspace gate may show one external failure that is not this
   workstream's · to reverse: cheap — attribute by file path before investigating.
   This is the known-noise line for the Wave 3 gate.
4. **The 15 documented Wave 3 contracts in the module `AGENTS.md` files are
   complete and accurate** · default: trust them as the Wave 3 obligation list ·
   to reverse: moderate — re-run the enforcement census to regenerate.
5. **`workstream-g-plan.json`'s Wave 3 briefs are otherwise current** · default:
   use them verbatim except for the two known staleness points — the D-CL21
   dependency-approval instruction (see §4) and the errata G1/G2/G4/G5 shape
   changes the briefs predate · to reverse: cheap per slice, but a slice that
   follows a stale brief writes code against the pre-errata API and the wave
   barrier is where you find out.
6. **Ultracode is off** · default: use the standard Workflow opt-in rule; do not
   launch multi-agent fan-outs without the user asking · to reverse: trivial.
   **The Wave 3 `/slice` fan-out IS user-authorized** (2026-08-17, §4) — that
   authorization covers Wave 3 and does not extend to later waves.

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

**Step 6 is DONE** @`b820b67` (local only) — all six slices landed, full-workspace
gate green, but its `/security-review` returned **FAIL**. Remediation (§10) is
now the head of the queue; step 7 follows it. What step 6 said, for reference:

6. **Wave 3** — operation-plane integration. Run `/slice` against the six
   ratified slices in `workstream-g-plan.json` → `waves[wave==3].slices`, each of
   which already carries a full brief, `owned_files`, `model`, `spec_refs`, and
   `acceptance`:

   | Slice | Model | Owns |
   |---|---|---|
   | `W3-crypto-aead-keywrap` | opus | `fe-canonical-log/src/crypto/**` (new dir) |
   | `W3-db-canon-log` | opus | `fe-database` canonical persistence |
   | `W3-dual-emit` (SPEC-8) | sonnet | dual-emit flag surface, shadow ledger, comparator |
   | `W3-identity-x25519` | sonnet | `fe-identity` device key + rotation |
   | `W3-api-canonical-ws` | sonnet | `fe-api` `/ws/canonical`, compiled but **not mounted** |
   | `W3-sync-guard` | haiku | `fe-sync` test: every migration mode keeps iroh unavailable |

   Two obligations beyond the briefs, because the briefs predate them:
   **(a)** the 15 documented Wave 3 contracts in the module `AGENTS.md` files;
   **(b)** errata G1/G2/G4/G5 — in particular `CausalMaterializer::reduce` may
   read *only* `meta` and `envelope_bytes` (SPEC-4 §4 rule 8), the quarantine
   store must budget per `QuarantineReasonClass`, and `SegmentManifestBody::new`
   now takes 7 arguments.
   Two named Wave 3 obligations are easy to miss because they are gates with no
   caller yet: `wire::cursor::verify_frontier_commitment` MUST be called on
   every peer-supplied cursor (`wire/AGENTS.md` §Wave 3 obligation), and
   `capability/AGENTS.md` §5.3 lists three more.
   Wave 3 touches auth and crypto paths — `/security-review` is its phase
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

---

## 10. Wave 3 security verdict — **FAIL** @ `b820b67`

`/security-review`, separate context, prompted to refute rather than confirm.
**2 CRITICAL, 9+ HIGH, 14 MEDIUM.** Code is committed anyway and **not pushed**,
because every finding is **dormant**: `/ws/canonical` is unmounted and no
production caller reaches the canonical append, epoch, or key-wrap paths.

**All four hard constraints VERIFIED**, with the evidence that settles each:

| # | Constraint | Evidence |
|---|---|---|
| 1 | No network enablement | `IrohDocsEngineHolder.available` is a private `AtomicBool` with **no `store` call anywhere** — `is_available()` is structurally incapable of returning true. No network primitive in any new code. |
| 2 | `/ws/canonical` unmounted | Only `build_router_with_canonical_log` call sites are the two under `#[cfg(test)]` in `router.rs`. No binary references it. |
| 3 | Legacy editor path unchanged | `op_log.rs` untouched; the one `crud.rs` hunk is `init_hlc(0)` inside `#[cfg(test)] mod cascade_batch_update_tests`. |
| 4 | No shadow-side projection mutation | `migration/rebuild.rs` holds no `Db`; `shadow_store.rs` writes only the const ledger table. |

**Clean on traced evidence** — do not re-litigate these: the AEAD core (four-lane
domain separation, `assert_production_suite` unconditional on all four entry
points, keys zeroized and constant-time compared); SurrealQL injection (every
query a `&'static str` with bound placeholders); exactly-once append (UNIQUE
indexes at `schema.rs:443` plus content-addressing); SPEC-4 §4 rule 8 `reduce`
input purity (one call site workspace-wide); WS error disclosure.

### Must close before ANY Wave 4 caller is wired

Ordered by what a caller trips over first. **1-3 are blocking; do not wire a
caller to the canonical append, epoch, or WS surfaces until they are closed and
re-reviewed.**

1. **C1** `handler.rs` `dispatch_subscribe` — pushes the peer-supplied
   `body.scope` into the same `subscribed_scopes` set that `PinnedSession::covers`
   reads, with no check against `session.epoch_scope`, no capability
   re-verification, and no resolution of `authorization_binding_id`. Subscribe
   writes its own authorization. The `.map_err(|_| ScopeNotAuthorized)` looks
   like a check but `bind` only fails on ID collision. *Independently verified.*
2. **C2** `state.rs` `cached_verification` — validates the **stored** `CacheKey`
   by set membership without recomputing it against the live view version,
   epoch, or expiry, and fe-api has **no production gate-invalidation path**.
   The timer detects revocation; re-sending identical `authorize` bytes hits the
   cache and skips verification. *Independently verified.*
3. **H1** `append_store.rs` — the `VerifiedLogStore::append` impl reaches the log
   through `decode_canonical` alone, skipping signature verification. Two doors
   into one log; only `admission.rs:181` is guarded. **H2** `canonical_epoch.rs`
   `admit_epoch_bump` performs no Manager+ authority check and takes no view that
   could answer. H1 supplies the unsigned envelope H2 admits; together, one
   unsigned envelope advances an epoch and locks out every legitimate actor.
4. **H3/H4** `key_wrap.rs` — `open_scope_key` never verifies issuer authority
   (`issuer_capability` is signed over and then never read), and
   `issue_scope_key_wrap` never verifies the recipient **device** binding (it
   authorizes against the Ed25519 principal while sealing to a caller-supplied
   X25519 key; no device-enrolment registry exists). Both are spec MUSTs.
5. **H5/H6** `canon_log/rebuild.rs` — an unverified checkpoint suppresses the
   empty-state reset, and the computed root is never compared against the
   claimed one; `record_replay_verified` then fires for a replay that reduced
   nothing. `canon_log_materialization_test.rs:980` currently *encodes the
   forged-checkpoint acceptance as expected behaviour* — fix the test too.
6. **M1** `crypto/aead.rs` — `FreshNonce`'s one-shot guarantee is defeated by its
   own public `from_params`/`params` pair. Cheap, and it removes a false claim
   from two documents.

### Suggested remediation partition (four disjoint `owned_files` sets)

| Slice | Owns | Findings |
|---|---|---|
| `W3R-api-authz` | `fe-api/src/canonical_ws/{handler,state}.rs` | C1, C2, H9, H10, M7, M14 |
| `W3R-db-canon` | `fe-database/src/canon_log/**`, `fe-database/tests/canon_log_materialization_test.rs` | H1, H2, H5, H6, H7, M3, M4, M5 |
| `W3R-crypto` | `fe-canonical-log/src/crypto/**` | H3, H4, M1, M6 |
| `W3R-leaf-seams` | `fe-canonical-log/src/{materialize/traits.rs,capability/**}` | `VerifiedEnvelopeMeta` constructibility (H1's other half), `AppendError` erratum, H8 |

Same rules as Wave 3: agents do not run cargo, the orchestrator runs one serial
`cargo test --workspace -j2` at the barrier, and `fe-ui/**`, `fe-terrain/**`,
`fractalengine/src/gpx_bridge.rs` stay untouchable.

### Carried items, not yet filed anywhere else

- **`AppendError` needs a `StorageUnavailable`/`Indeterminate` variant.** A
  storage fault is currently indistinguishable from "absent"; the impl refuses
  conservatively and records causes out-of-band. Worse than first filed:
  `append_store.rs:340` returns `IntegrityConflict` when bytes merely fail to
  *decode*, which per §3.3 permanently blacklists an op_id that may only be
  undecodable *here* (e.g. a future protocol version). `NotAnEnvelope` exists and
  is discarded. Needs a `fe-canonical-log` erratum.
- **Three provisional wire-number assignments** (`ScopeKeyWrapBody` keys 0-4,
  `ScopeKeyWrap` key 5, and reading §10.2.3's `canonical_complete_wrap` as
  signature-excluding) live only in `crypto/AGENTS.md`. Lift them into the
  crate-root "Provisional wire numbering" ratification surface.
- **Four traits marked "Wave 3" in the module AGENTS.md files have no owner and
  were routed to Wave 4**: `SealedArtifactStore`, `ArtifactSetMembership`,
  `BlindedTopicDerivation`, and the `fe-database` `QuarantineStore` override plus
  its GC driver. Wave 3's six slices had no home for them.
- **The deferred manifest-statistics cross-check** (§9) still has no home: Wave 3
  built no selective-fetch consumer.

## 10a. Wave 3R remediation — verdict **PASS (conditional)** @ `d01b098`

Ran 2026-08-18 as a four-slice `/slice` fan-out against the §10 partition, plus
a serial integration pass, then one independent fable-tier adversarial review
prompted to refute. **The §10 FAIL is closed.** Gate: **2893 passed / 0 failed /
20 ignored** (baseline 2846), clippy `-D warnings` clean, fmt clean, Cargo.lock
untouched — zero packages enter or leave the graph.

Commits: `fa0d9d9` (remediation), `d01b098` (adversarial fix round).

### What closed

| # | Finding | Now |
|---|---|---|
| 1 | **C1** subscribe wrote its own authorization | Five in-order gates before a scope enters the set `covers` reads: binding resolved in the connection's table, pinned session valid against the live view, `epoch_scope.contains`, real `CapabilityVerifier` over the handshake's exact chain bytes. Sole write site confirmed by grep. |
| 2 | **C2** cache defeated revocation | fe-api stores **no `CacheKey` and builds none**. Entries hold an `AdmittedDecision` that deliberately omits `authority_view_version`, so there is no stale value to supply; `RevalidationGate::admitted_now` reads expiry, epoch and version off the live view itself. Four invalidation paths; `sweep_revocations` runs every tick. |
| 3 | **H1/H2** one unsigned envelope could advance an epoch | Both append doors route through `signing::decode_and_admit`; `meta_of` re-verifies on **read**, so a hand-written row is unreadable. `admit_epoch_bump` requires Manager+ and its lost-update `UPDATE` became a compare-and-swap preserving the §5.1 rule 6 evidence row. |
| 4 | **H3/H4** possession was authority in key_wrap | `open_scope_key` reads the previously-signed-and-discarded `issuer_capability` against a required `IssuerAuthorityView`; `RecipientDeviceBinding` (private fields, one constructor) gives recipient/device key/scope/epoch one source that **cannot disagree**. |
| 5 | **H5/H6** forged checkpoints | `AdmittedCheckpoint` (private fields, one constructor); an admitted checkpoint must also **reproduce** the claimed projection root or the rebuild falls back to empty. `record_replay_verified` cannot fire on an accelerated pass. |
| 6 | **M1 + AppendError** | `FreshNonce` seal door closed; `AppendError` gains `NotAnEnvelope` / `StorageUnavailable` + `is_definite()`. Decode failure no longer returns `IntegrityConflict`, which §3.3 makes a permanent blacklist of an op_id this build may merely predate. |

### Found beyond the brief (the audit half earned its cost)

- Caveat rows were **fail-open while allowlist rows were fail-closed**: `unwrap_or(0)` let a request satisfy a `max_payload_bytes` caveat by declining to state its size.
- A request carried **two independent resource identifiers with only one scope-checked** — authorized against resource A while naming resource B.
- **Equivocation evidence was fail-open**: `None` admitted, so a storage fault admitted exactly when the substrate was sick.
- **Delta fan-out and preview send were unprotected paths**; `snapshot_ack` let a peer nominate its own trusted baseline.
- `wrap_scope_key` accepted **any** `SigningKey` while the body named a different issuer.

### The through-line, and the irony

The §10 review's real finding was *gates documented as structural that are only
conventional*. The remediation **shipped one new instance of it** and the
adversarial round caught it: `canon_log/AGENTS.md` claimed both append doors
verify a signature, justified by "a `VerifiedEnvelopeMeta` only exists
downstream of `admit_candidate`" — false, and contradicted by
`materialize/traits.rs` written in the same wave. `append_admitted` verifies
content-addressing only. The property holds via the **read** side, not the
claimed mechanism. Retracted in place, per the house style.

`crypto/AGENTS.md` now carries an **8-row Structural / at-the-seam / absent
table**, with the rule that a row moves left only when code moves with it. That
converts the named defect into a standing mechanism rather than a one-off fix.
Four false claims were retracted, including "the type system refuses it" and a
no-copy claim the module's own test helper disproved.

### Residual — open by decision, not by oversight

1. **`VerifiedEnvelopeMeta` remains forgeable.** All fields `pub`; sealing it
   would have broken `admission.rs:245`'s `#[cfg(test)]` struct literal in a
   concurrently-edited sibling. One line (`#[non_exhaustive]`) once that literal
   goes — `VerifiedCapability` was sealed exactly that way, zero call-site churn.
2. **No production `DeviceEnrolmentView` or `IssuerAuthorityView` implementor.**
   H3's seam is structural; **its answer is not yet trustworthy.** Wave 4.
3. **`AeadSuite` remains nonce-reuse-capable** — cannot close without breaking
   `open`. Documented, not claimed shut.
4. **`SurrealVerseDagView::load` fail-closed amplifier** — one unreadable row
   denies a whole verse. Correct direction; documented, behaviour unchanged.
5. **Hazardous `pub` surface retained** (`RevalidationGate::admit`/`is_admitted`,
   `wrap_scope_key`, `append_admitted`) — documented, unused in production.
   Re-introduction risk for a future author.
6. **`MockCapabilityVerifier` still ignores scope in its accept/reject logic.**
   It now *records* the full request and the C1 test asserts on it, but no
   end-to-end scope-sensitivity proof exists in fe-api (needs `async-trait` in
   its `[dev-dependencies]` to build a real double).
7. **CAS untested under real concurrency** — the `ConcurrentModification` path
   itself is unexercised; a true interleaving test would be flaky.

### Still true, re-verified from code by the adversarial pass

All four hard constraints hold. `fe-sync` is **not in the diff at all**;
`IrohDocsEngineHolder.available` still has **zero `.store(` calls**, so
`is_available()` remains structurally incapable of returning true.
`/ws/canonical` stays unmounted (only `#[cfg(test)]` call sites). `op_log.rs`
and the non-test half of `crud.rs` untouched. No shadow-side projection mutation.

**Wave 4 may now wire a caller to the canonical append, epoch and WS surfaces**,
subject to residual items 1-2 above being understood as still open.

## 11. Pre-existing defects repaired at the Wave 3 barrier — `29cadd9`

Not Workstream G work; recorded so they are not misattributed. Both were
invisible until now because D-CL23 scoped Waves 1-2 to per-package gates, so no
`cargo test --workspace` had run since `916a3ae`. **This is the concrete
vindication of the 2026-08-17 owner call to widen the Wave 3 gate** — under
per-package scoping both would have shipped silently into Wave 4.

- `handlers/crud.rs` — two `setup_mem_db` helpers; the `cascade_batch_update_tests`
  copy omitted `init_hlc(0)`, panicking inside the `HLC_STATE` critical section
  and poisoning it. One omission, eight red tests.
- `fractalengine/src/main.rs` — hydration query ordered by `created_at` without
  carrying it in the projection; SurrealDB 3.x rejects that at parse time.
