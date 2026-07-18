---
type: Track Spec
title: Crate Consolidation Round 2 — user-directed merges with audit counter-evidence embedded
description: Re-open three merge candidates (fe-plugin-test→fe-plugin, fe-hexon-registry→fe-hexon, fe-query→fe-api) under the 2026-07-18 user directive that crate count is itself a cost; each merge carries the 2026-07-17 audit's KEEP rationale and an explicit accept/defer gate
tags: [chore, crate_consolidation_r2_20260718, pending]
timestamp: 2026-07-18T00:00:00Z
resource: ./metadata.json
---

# Specification: Crate Consolidation Round 2

**Track ID:** `crate_consolidation_r2_20260718`
**Priority:** P1 PLATFORM
**Crates:** workspace root, `fe-plugin`, `fe-plugin-test`, `fe-hexon`,
`fe-hexon-registry`, `fe-format`, `fe-query`, `fe-api` (+ every consumer Cargo.toml)

## Driving decision (user, 2026-07-18)

Verbatim: *"we should be able to do things like merge plugin-test and plugin;
hexon + hexon-registry + format; query + api — its just a lot of crates, some
of them are sparse, seems like an anti pattern."*

The 2026-07-17 audit ([decision record](../../decisions/crate-consolidation-20260717.md))
weighed merges on closure hygiene and resolved these pairs to KEEP. **The user
has re-weighted the objective: crate count is a cost in itself.** This track
re-opens exactly the three named candidates — the audit's counter-evidence is
embedded below per merge so the executor (and the user at each gate) decides
with eyes open, without re-litigating from scratch. KEEP verdicts *not* named
by the user (fe-entity-store, fe-sdk, fe-identity/fe-policy, fractalengine-relay)
stand untouched. **fe-network is out of scope** — stage-2 retirement was
RESOLVED-KEEP (D-71, ratified 2026-07-17: P2P is the differentiator).

## Functional Requirements

Each FR is one merge candidate: mechanical steps, what breaks, audit
counter-evidence, and an **accept/defer gate** (user or explicit default at
plan-time; a DEFER is a first-class outcome recorded in the decision register).

- **FR-1 — fe-plugin-test → fe-plugin (gate G-1).**
  *Mechanical:* move `fe-plugin-test/src` under `fe-plugin/src/test_utils/`
  behind a `test-utils` cargo feature; re-point the consumers' dev-deps to
  `fe-plugin` with `features = ["test-utils"]`; delete the crate; regenerate
  the lock.
  *What breaks:* crates.io publishing shape for OSS plugin authors — the audit
  (F8) kept it precisely as the conventional published `*-test-utils` crate;
  post-merge, third-party plugin authors enabling `test-utils` pull **wasmtime
  29 + rhai + bevy** into their test closure just to run MockHostEnv/fixtures.
  Feature unification can also activate test-utils code in non-test builds of
  workspace consumers. *Counter-counter:* internal-only usage today; if no
  external author exists before OSS launch the convention cost is theoretical.
  *Gate:* accept if the OSS-onboarding cost is judged acceptable or deferrable
  (a later re-split is mechanical); otherwise defer with rationale.

- **FR-2 — fe-hexon-registry → fe-hexon, fe-format routed (gate G-2).**
  *Mechanical:* registry service becomes `fe-hexon/src/registry_service/` +
  a feature-gated `[[bin]]` (`registry`, feature `registry-service` pulling
  axum); `docker/Dockerfile.hexon-registry` re-pointed at
  `cargo build -p fe-hexon --bin registry --features registry-service`;
  `compose.dev.yml` unchanged externally.
  *What breaks:* the audit (F7) kept the registry for two structural reasons —
  (a) its **zero internal runtime deps** (parses manifest.json as raw
  `serde_json::Value` by design) let the Docker image build **without the
  engine closure**; post-merge the image compiles fe-hexon's full dep tree
  (fe-format, fe-database edges, policy) unless the feature graph is carved
  very carefully; (b) the roadmap's **hexon-foundry extraction** intended to
  lift the crate wholesale — post-merge, extraction means surgery instead of
  a `git mv`.
  *fe-format:* the `+ format?` part of the user's ask is **NOT decided here**.
  fe-format/fe-hexon restructuring is owned by `hexon_unification_20260716`
  (audit F3 already routes the fe-format/fe-runtime edge fix there). This
  track adds the format-merge question as a gate **inside that track's scope**
  (cross-reference task in the plan) and coordinates sequencing — whichever
  track executes second inherits the other's landed shape. No parallel
  fe-format workstream here.
  *Gate:* accept if the Dockerfile builds the registry bin with a measured,
  acceptable image-build cost and the foundry extraction path is documented
  as "re-split at extraction time"; otherwise defer.

- **FR-3 — fe-query → fe-api (gate G-3, evidence-first).**
  *Not audited as a pair* — evidence precedes the merge design. **First task:
  establish fe-query's real consumer set** via grep of all workspace
  Cargo.tomls + use-site check. Preliminary grep (2026-07-18):
  `fe-api/Cargo.toml` (features `["datafusion"]`) **and
  `fe-database/Cargo.toml`** both dep fe-query — if that holds, the merge
  as-stated **inverts layering** (fe-database would depend on fe-api, and the
  audit's F5 note records the fe-query → fe-entity-store feature edge that
  already shapes this cycle). Options to record at the gate: (a) merge and
  move fe-database's needed pieces down/out, (b) merge only the egress/builder
  surface fe-api uses, (c) defer with the consumer map as the artifact.
  *What breaks if consumers beyond fe-api exist:* axum + the API closure drag
  into every fe-query consumer (fe-database → everything above it).
  *Gate:* accept only if the consumer map shows fe-api as sole consumer (then
  the merge is mechanical: fe-query becomes `fe-api/src/query/`, features
  `parquet`/`datafusion` hoisted onto fe-api); otherwise record the finding
  and defer.

- **FR-4 — Workspace + release bookkeeping.** For every ACCEPTED merge:
  workspace `members` updated, `Cargo.lock` regenerated, crates.io metadata
  (names/descriptions/features) updated for the OSS release shape,
  `oss_release_20260717` checklist cross-referenced (placeholder-sig register
  addresses if any move), directory-level AGENTS.md rationale recorded per the
  fe-auth precedent, and the decision register gains one entry per gate
  outcome (accept AND defer both recorded).

## Acceptance criteria

- Each of G-1/G-2/G-3 has a recorded outcome (ACCEPT+executed or
  DEFER+rationale) in this folder + the decision register; no gate silently
  skipped.
- Executed merges: workspace builds, single end-of-track sweep green
  (test/clippy/fmt), docker registry image builds if G-2 accepted, zero
  references to deleted crate names.
- FR-3 consumer map recorded as an artifact regardless of outcome.
- `hexon_unification_20260716` updated with the format-merge gate
  (cross-reference visible from both tracks).

## Out of scope

- fe-network retirement (D-71 RESOLVED-KEEP — do not touch).
- fe-entity-store / fe-sdk / fe-identity / fe-policy / fractalengine-relay
  merges (audit KEEPs stand; user did not name them).
- fe-format ↔ fe-hexon restructuring execution (owned by
  `hexon_unification_20260716`; this track only plants the gate there).
- Any behavior change — merges are structure-only, code moves verbatim.
