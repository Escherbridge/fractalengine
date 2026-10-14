---
type: runbook
title: FractalEngine session runbook
updated: 2026-10-13
head: 850466d
---

# FractalEngine — Runbook

**Last updated:** 2026-10-13 · **Branch:** `main` · **Last commit:** see `git log -1`
(written at `a6e1a4a` + this runbook commit) · Previous runbooks: `git log -- conductor/RUNBOOK.md`.

Foundation under this file: `conductor/product.md`, `tech-stack.md`, `workflow.md`,
`tracks.md`, and the work queue `conductor/doneness-ledger.md`.

## 0. Session-open protocol — AGGRESSIVE GRILL LOOP (do this BEFORE any work)

User directive 2026-10-12: every session opens by re-aligning with the user through a
multi-round `AskUserQuestion` loop. Do not start building until the loop ends.

**0a. Retrieve context first (silently, cheap, before the first question):**
1. `git status --short --branch`, `git log --oneline -10`, `git reflog show refs/remotes/origin/main -n 5`
   — did ANOTHER actor push since `4765bc5`/`a6e1a4a`? (It happened twice: §1 incidents.)
2. `gh run list --branch main --limit 3` — CI verdict on the last pushed tip (§2 names the run to check).
3. Read §2 State, §3 Decisions, §4 Assumptions of this file; skim `conductor/doneness-ledger.md`
   class counts (`grep -c NEEDS_RATIFICATION conductor/doneness-ledger.md` etc.).
4. Memory: `completeness-campaign-20261010`, `long-horizon-autonomy-directive-20261010`,
   `verify-workflow-stopped-before-relaunch` (they hold the working rules).
5. `df -h /c`, and `Get-Process cargo,rustc,clippy-driver,fe-relay,duckdb` — nothing should be running.
   Report drift between this file and reality in the first message, before asking anything.

**0b. Grill rules:**
- Minimum 3 rounds unless the user picks "Done" earlier; max 4 questions per round;
  **multiSelect by default**; every question explicitly invites freeform ("Other") input —
  the user prefers to type context; the recommended option goes first with its *flip*
  (what changes downstream) in the description.
- Record each round's answers into §3 Decisions immediately (rolling) — never at the end only.
- Every round ends with a "keep grilling? / Done" question whose options name the next topics.
- Unasked items become ranked lines in §4 Assumptions.

**0c. Topics to cover (in this order; drop what is already settled in §3):**
1. **Delta since last session** — anything the user did (in-app checks, pushes, decisions made
   elsewhere, new constraints, budget/time). Ask what they verified in-app and what broke.
2. **Direction** — confirm the §7 order (large phases first) or reorder; anything to add/drop.
3. **Working rules & outputs** — confirm §3 "Working rules" + "Outputs"; ask what to change.
4. **Open decisions** — walk the NEEDS_RATIFICATION rows of the ledger in batches of ≤4
   (multiSelect "which can you decide now", then one question per checked item with concrete
   options + freeform). Priority: D-69 project licence (v1 gate), CDL-R2 device-enrolment
   registry format, the 0.35 fixture commit, the 20 SCEA Duck copies in
   `fractalengine/assets/imported/` (untrack + ignore vs keep), pending-track gating decisions, the
   `outstanding_decisions_20260715` register (55 items — ask whether to batch-ratify).
5. **Context the agent lacks — desired retrieval (ask explicitly, freeform-first):**
   - Who/what is the other actor that pushed `d198bd2` and `a064144` (a hermes-agent session?
     the user?) — should it be stopped or locked down?
   - In-app verification results for W4 UI work (Measure/grid, visibility groups, settings.ron,
     terrain live-update) and the Zurich/Fuji terrain + scale bar.
   - Self-hosted relay plan (D-23): where it will run, domain/TLS, who operates it.
   - v1 OSS release: target date, project licence (D-69), what "ready" means beyond §3's gate.
   - External commitments, consumers, or demos that change priority.
   - Machine constraints (disk, RAM, LM Studio usage) for long builds.
6. **Budget/cadence** — how long the next campaign may run, report cadence (end-only), model tiers.

## 1. Retro — everything completed 2026-10-09 → 2026-10-12

**A. Push reconciliation + CI (2026-10-10).** The 30 prod-ready-mission commits had already been
pushed (`d198bd2`) by another actor although the prior runbook said "nothing pushed" →
runbook corrected (`a5f012b`). CI Lint had been red since August: rustc 1.99's
`clippy::double_must_use` fired 17× inside `#[async_trait]` expansion (fixed by bumping
async-trait 0.1.89→0.1.92, upstream #303) and `--keep-going` exposed one hidden
`collapsible_match` (`0682517`) → first fully-green main since 2026-08-20.

**B. Security/correctness, 4 workflows (2026-10-10; pushed in `a064144` by another actor, CI green):**
`94be5b6` SetPetalTerrain reply correlation (REST/MCP terrain mutation live; HIGH fix: an API
write to petal B no longer hijacks the desktop's live petal A) · `ba0b8d2` `/query/elevated`
fail-closed + **Viewer `/query` could read any record by id** (`role:x.*`, `$v.to_record()`) —
closed · `04c91f1` one uniform denial for missing vs foreign resources (`rest::covered_scope`)
+ stamp cross-tenant read via `?path_id=` closed · `e2c6796` TilesetMeta Default · `87f72d3`
CI clippy `--keep-going`. Reviewers found 4 live holes beyond their briefs.

**C. Completeness campaign, 5 workflows (2026-10-10/11):**

| Commit | Workflow | Delivered |
|---|---|---|
| `97663e4` | W1 | Board truth: 41 tracks verified vs code/git; `doneness-ledger.md` (181 items) |
| `562f551` | W2 | 14 data/API deferrals: GetPetalTerrain + GUI RawQuery correlation, elevated writes via DB thread (snapshot → atomic op-log commit → diff → replicate), atomic tombstones, id indexes, asset owner + GC + quota, zip-bomb caps |
| `49e2de0` | W3 | 23 items: fe-policy TokenScope/Ownership/RoleFloor + fe-api routed through it, stamp translate lock, REST/MCP surface tests, cargo-deny + bounded fuzz in CI, canonical-log hardening |
| `7a74214` | W4 | Measure tool (M) + metric grid, visibility groups P2–P6 (pickers, H keys, persistence, registry, REST/MCP), settings.ron persistence, editor/terrain residue |
| `ff8b1e4` | W5 | iroh 0.35 → 1.3 (docs 0.101, gossip 0.101, blobs 0.103.1, noq BBRv3, MSRV 1.91), store migration, relay-health watcher. **Flag day** |
| `df35a7f` | fix | Reverted W4 responder filtering (creator copies are load-bearing) + non-blocking query echo |
| `4765bc5` | docs | Runbook close-out |

Final sweep (tests deferred until code complete, per user rule): `cargo test --workspace`
**3452 passed / 0 failed** (106 binaries; was 3245); harness real-loopback iroh **15/15 ×2**;
e2e bi-egress 49/49, hexon-install 15/15, hexon-serve 40/40. Pushed `a064144..4765bc5`
on 2026-10-12 under the new push-on-green rule.

**Incidents (and the rule each produced):**
1. External pushes bypassed the go-ahead gate twice → user now authorizes agent push-on-green;
   the other actor is still unidentified (§0c.5).
2. Two W4 workflows ran concurrently ~2 h (orchestrator misjudged a "stopped" workflow from a file
   mtime) → reconcile found no duplicate code; rule: always `TaskStop` the prior id first.
3. The campaign introduced a regression (W4 responder filtering → silently incomplete
   distributed queries); the final sweep caught it; the first debug fix (provision roles in the
   harness) would have **hidden** it → reverted instead; pinned by a test.
4. The original W4 ran tests against the no-tests rule → user stopped it.

**What worked:** reviewers that fix their own findings (real holes found in every workflow);
compile+clippy gates between workflows + ONE final sweep; capture-then-grep logs; pruning
`target/` between workflows (~110 GB reclaimed). **What didn't:** process-state verification;
trusting "pure efficiency" claims; push control; a board that silently drifts stale.

## 2. State

- **2026-10-13 session open:** origin/main = `850466d` — two `ci(fuzz)` commits (`f002e8e`,
  `850466d`) landed after this runbook was written (prior Claude session). **CI fully green on
  `850466d`** (run 38277696834: Lint incl. cargo-deny + bounded fuzz, Linux build+test, Windows,
  `latest` prerelease) — §7 step 2 is moot. Ledger is stale (dated 10-10; "commit pending" rows
  are committed; 7 NEEDS_RATIFICATION rows were decided 10-12) → W0 bookkeeping refreshes it.
- **origin/main = `4765bc5` + `a6e1a4a` `3e87279` `93b1bcd`** (all pushed 2026-10-12 under
  push-on-green; the last three are docs/assets only): untrack `.claude/settings.local.json`
  (its `.gitignore` line landed in `93b1bcd`), this runbook, and the sample-GLB swap —
  `duck.glb` (SCEA, non-OSI) → CC0 Khronos "Cameras" `cameras.glb` (2 embedded cameras, still
  the `deactivate_foreign_cameras` regression asset); `info_panel.glb` verified CC-BY-4.0.
- **Open licence residual:** `fractalengine/assets/imported/` tracks 20 byte-identical copies of
  the SCEA Duck (runtime import cache). Untracking them deletes the files on other clones at
  their next pull and may break nodes whose `asset_path` points there — USER decision (§0c.4).
- CI for `4765bc5` was being watched at writing time — **verify in §0a**. It is the FIRST real run
  of the cargo-deny and bounded-fuzz lint steps (fuzz was only `cargo check`ed locally; sanitizer
  builds need Linux nightly). If Lint is red, fix that first.
- **Verified:** the final sweep above. **Not verified:** in-app GUI behavior of W4 work;
  relay-assisted connection through a self-hosted iroh-relay 1.3; the fuzz CI step; the 0.35 store
  migration on a REAL user data dir (only the committed generator + the local fixture).
- ~~Untracked on purpose: the 0.35 fixture … commit decision open~~ — RESOLVED 2026-10-13 (R2):
  CI generates it (`119dbd0`); the test fails, not skips, under `FE_REQUIRE_STORE_FIXTURE=1`.

### Review ledger

| Phase | Checkpoint | Findings | Verdict |
|---|---|---|---|
| CI fix `0682517` | independent review of async-trait diff | 1 misread corrected | SOUND |
| `94be5b6` terrain correlation | adversarial review-and-fix | 1H 3M 2L | FIXED |
| `ba0b8d2` elevated + query guard | security review-and-fix | 1H (record-id leak) 2M 1L | FIXED |
| `04c91f1` existence oracle | review-and-fix | 1H (stamp cross-tenant) 2M | FIXED |
| `e2c6796` TilesetMeta | review-and-fix | 1L advisory | SOUND |
| W2 `562f551` | review-and-fix | 1H (zip-bomb reserve) 1M (GC vs stamp blob) 4L | FIXED |
| W3 `49e2de0` | review-and-fix | 1M (stamp lock bypass via move_waypoint) 1L | FIXED |
| W4 `7a74214` | 2 reviews + reconcile | 1H (hidden ribbons pickable) 2M 4L | FIXED |
| W5 `ff8b1e4` | review-and-fix | 1H (watcher masked stack-failure health) 1M (doc handle leak) 3L | FIXED |
| Final sweep | full tests + harness + e2e | harness 14/15 → root-caused, `df35a7f` | GREEN after fix |
| 2026-10-13 purge `877e801` | critic tick 1 audit | Duck unreachable from every ref; 128/130 cited SHAs resolve (2 foreign) | SOUND |
| W0 `119dbd0` `bf28971` | adversarial review-and-fix | 1M (zero-match test filter passed vacuously) 2L | FIXED |
| Planning swarm (5 plans) | cross-check critic edits | 29–37 plan edits (collisions, ordering, security slices) | AMENDED |
| Critic tick 1 | accountability critic | 2H 3M 3L (R7 not in plan bodies; push gate) | DRIFTING → all ACCEPTED |

## 3. Decisions

**From the 2026-10-13 grill (user, rolling):**
- R1 · Delta: nothing new on the user's side (no in-app checks, pushes or constraints).
- R1 · Git identity: this clone's `.git/config` had `user.name = hermes-agent`, which made every
  commit (Claude's included) indistinguishable. Now `Jade Zaher` (matches global + GitHub);
  Claude is credited via the `Co-Authored-By` trailer. Pushes `d198bd2`/`a064144` appear in no
  Claude transcript — not investigated further (user chose the identity fix).
- R1 · Direction: keep §7 as written (large phases → role propagation → 2026-10-12 decisions).
- R2 · D-69 was already RESOLVED 2026-07-17 (Apache-2.0 single) — the old "licence gate" line was stale.
- R2 · SCEA Duck copies (`fractalengine/assets/imported/`, 20 files): untrack + `.gitignore`
  (local files kept) **and** purge from git history (filter-repo + force-push; root `duck.glb`
  history too) **and** retire the `imported/` disk fallback in `crud.rs` (backfill any
  referenced GLB into the blob store first).
- R2 · 0.35 store fixture: generate it in Linux CI with the pinned generator
  (`fe-sync/tests/fixtures/p2p_store_v035_gen`); the migration test FAILS instead of skipping
  when `FE_REQUIRE_STORE_FIXTURE=1`. Fixture stays out of git.
- R2 · CDL-R2 enrolment artifact: **self-signed, replicated** — the principal's ed25519 key signs
  `{principal, device X25519 key, enrolled_at}`; only that principal may write it; replicates as
  an ordinary row; revocation = signed tombstone. Unblocks a production `DeviceEnrolmentView`
  in CDL Wave 4.
- R3 · Decision register (`outstanding_decisions_20260715`): CLOSE 14 superseded (D-1, D-2,
  D-10, D-14, D-21, D-36, D-50, D-55, D-61, D-68 per the 10-10 audit; D-11, D-22, D-23 per the
  10-12 grill; D-62 by the iroh 1.3 port). RATIFY the 28 remaining [DEFAULTED] checklist items
  as written (D-3, D-4, D-6, D-9, D-13, D-24, D-25, D-28, D-33, D-34, D-37, D-39, D-41, D-44,
  D-45, D-49, D-52, D-54, D-56..D-60, D-63..D-67) — a 2026-10-12 decision wins on conflict
  (D-54: f64 positions + ENU WKB frame). The no-default [USER] entries: agent drafts a
  recommendation each AND walks them in grill rounds 4+.
- R3 · History purge timing: NOW, before the campaign — bundle backup outside the repo →
  `git filter-repo` strip the Duck blob(s) from every path → remap old→new SHA citations in
  tracked docs + memory via filter-repo's commit-map → force-push `main` + `latest` → watch CI.
  Repo is already PUBLIC (`Escherbridge/fractalengine`, 0 forks); 235/256 commits rewrite.
- R4 · ACCEPTED as recommended: D-7 (node_log = local audit ring, K=1024; fe-sync never reads it),
  D-8 (BIM stats via fe-query SQL only), D-31 (anything addressable is reportable via Copy API /
  Report; bbox later), D-12 (open engine + format + reference tooling here; closed foundry /
  registry / marketplace separate, repo created when first scheduled), D-15/16/18/19 (deferred to
  the foundry), D-30 (agents may seed findings.md, tagged `[agent-seeded]`), D-35
  (sign-first-then-promote OK), D-38 (snap to terrain when present, Y=0 otherwise → MSA-P4 doable).
- R4 · OVERRIDES (user, verbatim intent): **D-5** "surrealdb stores a recordId for a file stored in
  blob storage on iroh; extensions for IoT devices can bring their format and store whatever data
  they want that way; allow for multiple records (a list of records)". **D-17** generic assets must
  work alongside `hexon_p2p_bucket` (NOT foundry-only). **D-20** path asset stays. **D-32** "FR-4 is
  needed". Exact shapes confirmed in R5.
- R5 · **Generic asset + IoT payload model (D-5/D-17):** the existing `asset` table (MIME
  `content_type` + iroh blob `content_hash` + `owner_did`, `fe-database/src/schema.rs:228`) is the
  generic asset in core (open). A NEW device-payload record (node, petal, format, recorded_at,
  **list of asset ids**) sits ALONGSIDE scalar `iot_reading` (which stays SQL/BI-queryable).
  `hexon_p2p_bucket` builds on the same asset model (one model across IoT, path assets, buckets).
- R5 · **Device extensions:** BOTH a declared format descriptor (format_id + MIME + optional
  schema; bytes stored opaque) AND sandboxed decoder plugins (Rhai/Wasm → scalar readings /
  properties). Decoders activate the dormant plugin host (PLUG-HOST) → mandatory sandbox security
  review. Sequencing default: descriptor first, decoders after the host is wired.
- R5 · **D-20:** path-asset W5 op-store (PetalHexon op-store + multi-asset x-fe-directory) is IN v1,
  reconciled with the generic asset model. **D-32:** inspector FR-3 (petal/fractal/verse
  inspection) AND FR-4 in full (role list + invites + role presets + audit history), after role
  propagation.
- R6 · ACCEPTED: D-40 archive light_box · D-42 Tauri shell spike closed NO-GO (Tauri stays the
  webview backend only) · D-43 splat LOD fix not revived · D-47 onboarding wizard → user's UX
  track, seedling_onboarding archived (its CRUD gap → D-48) · D-27 SSO admin UI in egui · **D-48**
  verse/fractal/petal rename + delete over REST + MCP behind fe-policy (Manager+), on the existing
  `RenameEntity`/`DeleteEntity` DbCommands · D-51 keep the current release matrix (no macOS).
- R6 · **D-26 NOT accepted** (1 h + refresh unticked, no override) → stays open.
- R6 · Relay host (D-23): undecided — the relay-assisted acceptance test runs against a LOCAL
  iroh-relay 1.3 container; `FE_SYNC_RELAY` stays a settings placeholder until a host exists.
- R6 · Loop ended by the user ("Done: start building") after 6 rounds.
- R7 (mid-campaign, after the planning swarm) · **Role statements from a not-yet-Manager+
  author are HELD BACK as Deferred** until the author has authority (user chose this over
  stored-but-inactive) → the implementation MUST re-try deferred rows whenever an author's
  authority changes so the outcome stays delivery-order independent. **Grant authority: any
  Manager, within their own subtree** (petal Manager → that petal and below; fractal Manager →
  that fractal; verse Manager/Owner → whole verse) — overrides the critic's verse-only rule.
- R7 · Confirmed defaults: petal creators NO LONGER become petal Owners (escalation fix, migrate
  existing implicit rows); generic asset + device payloads + format descriptors are built in THIS
  campaign (CDL-SPEC10 A3–A6; supersedes my §4 "second campaign" assumption); seeders never
  re-bundle sealed tilesets; CDL-W4 defaults (unavailable capability chain/epoch key → quarantine,
  erratum E-W4-1; new wire numbers provisional in the one register; equivalence evidence = dev run
  until an owner run charter exists).
- R7 · **SPEC-10: PRE-APPROVED** — Phase B (observation module, columnar codec, shadow
  sealer) builds straight after the draft using the draft's recommended defaults; user reviews after.
- R7 · CI: **Lint must warn, not fail the run** (don't let cargo-deny/clippy drift block the
  build or the `latest` publish), and the rolling release must publish a **download users can
  actually run** (packaged per platform with assets).

**From the 2026-10-12 grill (user):**
- **Next work order:** large in-progress phases FIRST — PAS-P3+P4 (transport + chunk handlers +
  resumable downloads; residency ledger / lazy tiles / exit-ring eviction), PAS-P5 (partial
  hexon fetch), CDL Wave 4 integration, CDL SPEC-10 observation plane. Then role propagation,
  the decision backlog, pending tracks (all four streams are in scope).
- **Role propagation:** *(SUPERSEDED in part by R7 2026-10-13 — grant authority is any Manager
  within their own subtree, not "Manager+ at the verse scope"; not-yet-authorized statements are
  held as durable Deferred and re-evaluated on every authority change; implemented in
  `conductor/tracks/role_propagation_20261013/plan.md` §2.2/§2.4, decisions D-RP-1/D-RP-2 in §7)*
  replicate the `role` table as ordinary replicated rows, admitted only when
  the author is Manager+ at the verse scope (owner bootstraps from `verse.created_by`). Chosen
  over signed role grants (simpler, no new signature format). After it lands: fix the
  distributed-query settlement race, then consider re-enabling responder filtering.
- **Token client identity:** add a `client` claim at mint; asset `owner_did`/quota key on it;
  `sub` stays the issuing node DID.
- **iroh:** hard flag day — "no consumers of the tool; iroh 1.0 is part of v1". No 0.35 window.
- **D-23 relay posture:** the self-hosted iroh-relay 1.3 container is the production default
  (FE_SYNC_RELAY / settings); n0 relays only as dev fallback.
- **Licences:** allow all 7 outlier licences in deny.toml and turn `cargo deny check licenses`
  on in CI.
- **D-11:** dimension callouts persist as addressable nodes (`node_kind=dimension_callout`).
- **Hiding a GPX track hides its stamps** (cascade on `owning_path_id`); a per-stamp override wins.
- **Per-petal render_distance:** a `petal.render_distance` column (replicates); session override on top.
- **Query contracts (no consumers yet, breaking is cheap):** WKB petal-local frame = east,north,up
  `(x, -z, y)`; f64 positions end to end in egress; fresh tilesets stamp `crs=EPSG:4326`;
  elevated: DB-side TIMEOUT on every statement + 32-statement cap.
- **v1 ship gate:** public OSS release ready.
- **Push rule:** the agent may push when the gate is green (no per-batch ask) and must watch CI.
- **Do-now (2026-10-12):** pushed `4765bc5`; untracked `.claude/settings.local.json`; replace
  licence-unverified sample GLBs (keep an embedded-camera regression asset).

**Working rules (user, standing):** small sequential workflows (builds never overlap; `TaskStop`
the previous workflow id before relaunch); reviewers fix their own findings (no read-only
reviewers); **no tests until the code is complete** — compile + clippy between workflows, one full
test + e2e + harness sweep at the end; prune `target/debug/incremental` + PDBs at every safe
point; network access granted; **report at the end** (not mid-run).
- *(2026-10-13, critic AC-5)* Before any blocking question: commit finished work and launch the next
  independent workflow first, or take the stated default as a logged assumption — never let done
  work sit uncommitted (or main sit red) behind a question.
- *(2026-10-13, critic AC-2)* **Push gate:** code reaches `main` only after a test gate covering
  exactly the code being pushed (main publishes `latest`). W0+W0b get a scoped gate (their
  reviewers' `test_sweep_must_cover` + the crates W0b touched); **from W1 on, no main pushes until
  the campaign final sweep** (docs-only commits excepted). Accountability critic ticks at every
  workflow boundary and before any push (`conductor/accountability-log.md`).

**Outputs (user, standing):** reports are published as **artifacts** — status + decisions on top,
teaching depth inside (file:line links, quoted load-bearing lines, why/what-to-watch, Sharpen),
diagrams where they help.

**Carried from earlier:** DEC-C1…C22 (`conductor/tracks/mission_prodready_continuation_20261008/spec.md`);
canonical-data-log network rollout / relay seeding / inbound P2P / `/ws/canonical` mount stay
NO-GO (local Workstream G implementation is GO); `../gis-tile-etl` kept, never pushed.

## 4. Assumptions (highest reversal cost first — confirm the top line before building on it)

**Added 2026-10-13 (unasked in the grill):**
- Campaign shape: W0 housekeeping first (history purge + SHA remap, fixture CI generation,
  `imported/` fallback retirement with blob backfill, ledger/register bookkeeping), then §7 steps
  3-6 as ratified, ending in a final sweep + push. The 2026-10-13 scope additions (generic asset +
  device-payload records + format descriptors, decoder plugins/PLUG-HOST, D-20 W5 op-store, D-32
  inspector FR-3/FR-4, D-48 CRUD, MSA-P4 snap) form a SECOND campaign after that sweep, so the
  first sweep is not deferred behind months of code · to reverse: merge the campaigns (cheap now).
- Decoder plugins come after descriptors and after the plugin host is wired; sandbox security
  review mandatory · to reverse: reorder (cheap).
- v1 date: none given → "ready" = the v1 gate (public OSS release readiness) green; no external
  consumers or demos · to reverse: tell me a date (re-plan).
- User-owned, unscheduled: D-29 UX-QA pass, D-46 Download check, D-53/RCI-2.7 `v*` tag +
  publishing, every in-app verification · to reverse: schedule them.
- Campaign length unbounded (long horizon); report once at the end as an artifact; model tiers
  per standing rules (haiku bookkeeping, sonnet standard, opus hard/review) · to reverse: time box.
- The 0.35 fixture "stays untracked" assumption below is superseded by R2 (CI generation).
- *(partly superseded by R7)* the "second campaign" line above no longer covers the generic asset /
  device payload / format descriptor work — R7 builds it in THIS campaign (CDL-SPEC10 A3–A6).
- Reorder logged (critic AC-3): SPEC-10 **A1 (spec draft, docs only)** was pulled ahead of
  PAS/W4 because it needs no cargo; A2–A6 and Phase B keep the §7 order (after CDL-W4), and A2
  re-grounds A1's file:line citations after W4 lands · to reverse: none needed (docs).

**Carried from 2026-10-12:**
- ~~Role-table replication semantics: LWW by HLC per (verse, peer) row …~~ SUPERSEDED by the
  per-grantor statement model + R7 (role_propagation plan §0/§7). Replaced by:
- **D-RP-1 (AC-1 agent, 2026-10-13):** deferred role statements are durable per plane — DB: a
  persisted `role_statement_pending` table keyed (scope, peer, grantor) with a per-grantor budget
  (1024/verse, refuse-new) and NO shared cap (no cross-author eviction → flood-proof); sync: a
  per-author replica re-read (`Query::author(id).key_prefix`) when an author's authority rises ·
  to reverse: redesign before RP-2 (moderate).
- **KTX2 guard scope (W0c, unasked):** the interim RUSTSEC-2026-0344 guard refuses KTX2/basisu
  textures for EVERY GLB, including the user's own local imports — the blob store replicates and
  `BlobAssetReader` cannot tell a blob's origin, so fail-closed is the only sound choice without
  provenance tracking · to reverse: add blob provenance (local-origin flag) and exempt local blobs,
  or transcode KTX2→PNG at import (moderate) · lifts entirely when bevy requires ruzstd ≥ 0.9.1.
- **D-RP-2 (AC-1 agent, security):** owner pins cut the subtree authority walk (a verse Manager the
  owner pinned to viewer in petal P cannot appoint a Manager in P), and Managers only RAISE (a
  Manager's `none` is a no-op — no shadow-demotion of a higher Manager) · to reverse: RP-1 is pure,
  cheap before RP-2.
- Order after the large phases = role propagation → implement the 2026-10-12 decisions → ratify
  backlog → pending tracks · default: as stated · to reverse: reorder §7 (cheap).
- Legacy API tokens without a `client` claim are treated as node-level ownership · to reverse:
  force re-mint (cheap now — no consumers).
- WKB east-north-up applies only to petal-local outputs; `coords=latlon` output unchanged · to
  reverse: one switch in fe-query (cheap).
- "Report at the end" = one artifact per workflow/campaign end, no mid-run chatter except
  blockers · to reverse: cadence change (cheap).
- The 0.35 store fixture stays untracked (the user did not pick "commit") · to reverse: `git add` 7.1 MB.
- Docs-only commits may be pushed without re-running the full sweep · to reverse: gate them too.

## 5. Environment & gotchas

- **Build doctrine (house rule):** never edit source/Cargo.toml/Cargo.lock while cargo runs (a
  mid-build manifest edit once stacked 236 GB and zeroed the disk); one builder at a time; sweeps
  use `CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728 -j4`; prune `target/debug/incremental`
  + `*.pdb` between workflows (`target/` reached 150 GB). `du` on `target/` takes minutes.
- **House doctrine from reviews:** a scoped-table list is a denylist (denormalized copies live one
  table over); shared cores + separate guards = drift; a writer of a shared JSON column must
  satisfy the STRICTEST reader; "pure efficiency" changes that remove redundancy need a
  correctness check (creator copies were load-bearing).
- **Toolchains:** local default 1.94.0, CI floating stable (1.99.0 at writing); 1.96/1.97.1/1.98/
  1.99.0 installed; MSRV now **1.91**. Reproduce CI Lint with `cargo +1.99.0 clippy --workspace
  --all-targets --keep-going -- -D warnings`.
- **iroh 1.x store migration ops:** the first 1.x start on an existing `FE_P2P_DIR` copies it to
  `<dir>.v035.bak-<unix_ts>`, upgrades `docs.redb` + `blobs/blobs.db`, writes
  `.fe-store-format=iroh1`; failure → loud offline/mock; rollback = restore backup + run a 0.35
  build (`fe-sync/src/AGENTS.md` §store-migration). Hermetic runs: `FE_SYNC_RELAY=disabled`.
- **Standing verification:** `cargo run -p fractalengine-test-harness` (15 real-loopback scenarios;
  `iroh_docs "Failed to close connection"` warnings are loopback noise) and
  `scripts/{bi-egress,hexon-install,hexon-serve}-verify.ps1` (boot a real relay, mint tokens
  offline via `fe-identity --example mint_api_token`, attach `tools/duckdb/duckdb.exe`, gitignored).
- **Secrets:** relay keys via `FE_SECRET_*` env (operator must export or keys are ephemeral);
  GUI keys in the OS keystore; relay e2e seeds live only inside script runs. Never in files.
- **Other actors:** something else pushes from this clone (§0c.5). `fe-hermes`/`fe-pi` siblings
  are pruned worktree stubs. A concurrent hermes-agent session has shared this tree before.
- **C: drive** 1.9 TB, ~85% full mostly from non-project data; ~290 GB free at writing.

## 6. Key files

- `conductor/doneness-ledger.md` — per-track doneness + classified work queue (source for §7).
- `conductor/tracks/p2p_asset_streaming_20260718/plan.md` + `evidence.md` — PAS-P3..P5 (next).
- `conductor/tracks/canonical_data_log_20260808/` — CDL Wave 4 / SPEC-10 (local only).
- `fe-sync/src/AGENTS.md` §distributed-query ("Creator copies are load-bearing"),
  §store-migration, §relay-health — read before touching sync.
- `fe-database/src/handlers/replicated_row.rs` — A3 inbound admission (role propagation lands here).
- `fe-api/AGENTS.md` §existence-oracle, §elevated-query, §query-guard — authz contracts.
- `fe-sync/src/distributed_query.rs` — test `a_non_host_authors_creator_copy_serves_rows_its_denying_host_refused` pins the revert.

## 7. Continuation plan

1. **Run §0** (context retrieval → aggressive grill loop). Record answers into §3/§4 as you go.
2. **CI check:** if the last pushed run is red (likely suspects: the new fuzz step or cargo-deny),
   fix it first; `cargo +1.99.0 clippy` reproduces Lint.
3. **Large-phases campaign** (sequential workflows, compile+clippy gates, tests at the end):
   PAS-P3+P4 → PAS-P5 → CDL Wave 4 → CDL SPEC-10. Each workflow: plan from the ledger rows +
   track plan.md, executors, a reviewer that fixes, a small docs/README pass, local commit.
   `TaskStop` the previous workflow before each launch; prune caches between them.
4. **Role propagation** (replicate the role table with Manager+ admission) → fix the
   distributed-query settlement race → only then reconsider responder filtering.
   Security review mandatory (auth path).
5. **Implement the 2026-10-12 decisions:** `client` token claim; dimension-callout nodes; GPX
   hide cascades to stamps; `petal.render_distance`; query contracts (WKB ENU, f64, EPSG:4326,
   elevated timeout + 32 cap); self-hosted relay default + the relay-assisted acceptance test
   against a self-hosted iroh-relay 1.3; deny.toml licences + CI `licenses` on.
6. **Final sweep** (full tests + harness + e2e) → push on green → watch CI → report as an artifact.
7. Then: ratify the remaining backlog via grill loops; pending tracks (hexon_unification,
   map_scale_authority, road_builder_ux, crate_consolidation_r2) each need their own go;
   v1 gate = public OSS release readiness (`oss_release_20260717`).

## 8. Session log

### 2026-10-13 — grill loop → purge → W0 + planning swarm (rolling)
- Done: 6-round grill (§3 R1–R6, §4); git identity → `Jade Zaher` (+ Claude co-author trailer);
  deleted `../fe-hermes`, `../fe-pi`, `../fe-pibridge` + branches `impl/tauri-{hermes,pi,pibridge}`
  (user); **SCEA Duck purged from all history** (blob `217170d2`, filter-repo, 260 commits
  rewritten, 905 citations remapped in 160 docs + memory) and force-pushed atomically with leases
  (`main`, 3 remote branches, tag `latest`) → origin/main `877e801`.
- Backup: `../backups/2026-10-13-pre-duck-purge/` (all-refs bundle, imported/ copies, 0.35
  fixture, refs before/after, filter-repo log). Old→new map: `.git/filter-repo/commit-map`.
- Accountability critic: brief `.omc/accountability/critic-brief.md`, log
  `conductor/accountability-log.md`; runs at workflow boundaries (hourly cron denied by the
  permission classifier).
- Finding: CI's Linux test step is scoped to `fe-ui fe-terrain fractalengine` (D-66) — fe-sync's
  own tests never ran in CI; W0 adds a fixture-generation + migration-test step.
- In flight: CI watch `bdxqawa1s` (run on `877e801`); W0 workflow `wf_aa8dded9-e92` (task
  `wnzsvkwjm`); planning swarm `wf_7f0fd597-d05` (task `wtenzbfpx`) writing
  `campaign-plan-*-20261013.md` under the PAS/CDL tracks + new track `role_propagation_20261013`.
- W0 DONE (review FIXED 1M 2L; gate check+clippy+fmt green): `119dbd0` fixture CI, `bf28971`
  imported/ fallback retired, `0848237` bookkeeping + 5 campaign plans. Not pushed yet.
- CI on `877e801`: Lint RED — cargo-deny advisories (NEW RustSec: quinn-proto ×5 via libp2p-quic,
  ammonia ×2 via surrealdb-core, ruzstd via bevy_image); build/test jobs green; publish SKIPPED.
- In flight: W0b `wf_e727510a-f59` (lint → warnings not failure, advisory bumps, runnable release
  packages); critic tick 1 (background agent); SPEC-10 A1 spec draft (background agent, docs only).
- Critic tick 1 (DRIFTING, 8 directives, all ACCEPTED → accountability-log). Docs lanes landed:
  `0a702f1` SPEC-10 draft (PRE-APPROVED) + G3/G6 errata, `2c93499` A1 cross-slice items routed
  into W4/SPEC-10 plans. W0b DONE `6a53a2b` (lint warn-only, runnable packages, advisories;
  ruzstd ignored — reachable via KTX2 in peer GLBs).
- In flight: W0c `wf_34174034-e00` (fuzz-crash still blocks publish, licences allow-list + CI,
  KTX2 GLB intake guard, setup-python); AC-1 role-prop plan rewrite (docs agent).
- W0c DONE `da3e6a1` (review FIXED 2H 1M 3L). Scoped push gate GREEN (fe-sync migration 3/3 with
  fixture required; fe-database 221 lib + all integration; fe-runtime + fractalengine; fe-api upload,
  asset_ownership 6/6, mcp_integration 30/30; logs `.omc/research/push-gate-20261013/`). Critic tick 2
  ON_TRACK; packaged smoke PASS (AC-9). **Pushed `877e801..7441ab5`** 2026-10-13 ~19:30 MDT; CI watch
  in flight. AC-8 prune done (523 incremental dirs).
- OF-2 found by the smoke run (desktop analytics API never had its reader on Windows — double SurrealKV
  open) → **W0d `wf_e724d2d0-2de`** in flight (shared handle + scoped test gate) → push → then W1.
- Next: W0d push → AC-12 docs pass → critic tick 3 → **W1 PAS-P3+P4** (S1..S9 + S8b, sequential,
  per-slice local commits; AC-13 test folded into S6). No main pushes from W1 until the final sweep.

### 2026-10-12 — handoff after the completeness campaign
- Done: retro (§1); 5-round grill loop → §3 decisions; pushed `a064144..4765bc5`; untracked
  `.claude/settings.local.json` (`a6e1a4a`); runbook rewritten.
- Reviewed: every phase in the §2 ledger has a verdict.
- Also done: sample GLBs licence-verified/replaced (`93b1bcd`); ledger OSS-4.3 updated.
- In flight: CI on the pushed tip (verify in §0a).
- Blocked: nothing; remaining decisions are the ledger's NEEDS_RATIFICATION rows.
- Next: §0 grill loop, then the large-phases campaign.

## 9. Recommended invocations

- **`Workflow` with the reusable `completeness-batch-workflow` script** (sequential batches, plan →
  executors → review-and-fix, compile-only gates) — trigger: "large phases, ratified, sequential,
  no tests until code complete".
- **`/security-review`** (or a security review-and-fix agent) on role propagation — trigger:
  "touches inbound admission / RBAC".
- **`Artifact`** for every end-of-campaign report — trigger: "user rule: reports as artifacts,
  status + decisions on top, teach depth, diagrams".
- **`/handoff rolling`** at each workflow boundary — trigger: "multi-hour campaign; keep §8 live".
