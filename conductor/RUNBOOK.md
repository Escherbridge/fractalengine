---
type: runbook
title: FractalEngine session runbook
updated: 2026-10-12
head: f986ee1
---

# FractalEngine — Runbook

**Last updated:** 2026-10-12 · **Branch:** `main` · **Last commit:** see `git log -1`
(written at `f986ee1` + this runbook commit) · Previous runbooks: `git log -- conductor/RUNBOOK.md`.

Foundation under this file: `conductor/product.md`, `tech-stack.md`, `workflow.md`,
`tracks.md`, and the work queue `conductor/doneness-ledger.md`.

## 0. Session-open protocol — AGGRESSIVE GRILL LOOP (do this BEFORE any work)

User directive 2026-10-12: every session opens by re-aligning with the user through a
multi-round `AskUserQuestion` loop. Do not start building until the loop ends.

**0a. Retrieve context first (silently, cheap, before the first question):**
1. `git status --short --branch`, `git log --oneline -10`, `git reflog show refs/remotes/origin/main -n 5`
   — did ANOTHER actor push since `9e09b12`/`f986ee1`? (It happened twice: §1 incidents.)
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
   - Who/what is the other actor that pushed `1e9be3a` and `e96c972` (a hermes-agent session?
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
pushed (`1e9be3a`) by another actor although the prior runbook said "nothing pushed" →
runbook corrected (`6bce04b`). CI Lint had been red since August: rustc 1.99's
`clippy::double_must_use` fired 17× inside `#[async_trait]` expansion (fixed by bumping
async-trait 0.1.89→0.1.92, upstream #303) and `--keep-going` exposed one hidden
`collapsible_match` (`d4a4744`) → first fully-green main since 2026-08-20.

**B. Security/correctness, 4 workflows (2026-10-10; pushed in `e96c972` by another actor, CI green):**
`9cbdea6` SetPetalTerrain reply correlation (REST/MCP terrain mutation live; HIGH fix: an API
write to petal B no longer hijacks the desktop's live petal A) · `9c4da00` `/query/elevated`
fail-closed + **Viewer `/query` could read any record by id** (`role:x.*`, `$v.to_record()`) —
closed · `6763302` one uniform denial for missing vs foreign resources (`rest::covered_scope`)
+ stamp cross-tenant read via `?path_id=` closed · `7760be1` TilesetMeta Default · `da922a6`
CI clippy `--keep-going`. Reviewers found 4 live holes beyond their briefs.

**C. Completeness campaign, 5 workflows (2026-10-10/11):**

| Commit | Workflow | Delivered |
|---|---|---|
| `9f7a882` | W1 | Board truth: 41 tracks verified vs code/git; `doneness-ledger.md` (181 items) |
| `7d40a4e` | W2 | 14 data/API deferrals: GetPetalTerrain + GUI RawQuery correlation, elevated writes via DB thread (snapshot → atomic op-log commit → diff → replicate), atomic tombstones, id indexes, asset owner + GC + quota, zip-bomb caps |
| `40590e9` | W3 | 23 items: fe-policy TokenScope/Ownership/RoleFloor + fe-api routed through it, stamp translate lock, REST/MCP surface tests, cargo-deny + bounded fuzz in CI, canonical-log hardening |
| `4b30d17` | W4 | Measure tool (M) + metric grid, visibility groups P2–P6 (pickers, H keys, persistence, registry, REST/MCP), settings.ron persistence, editor/terrain residue |
| `d44f008` | W5 | iroh 0.35 → 1.3 (docs 0.101, gossip 0.101, blobs 0.103.1, noq BBRv3, MSRV 1.91), store migration, relay-health watcher. **Flag day** |
| `a210961` | fix | Reverted W4 responder filtering (creator copies are load-bearing) + non-blocking query echo |
| `9e09b12` | docs | Runbook close-out |

Final sweep (tests deferred until code complete, per user rule): `cargo test --workspace`
**3452 passed / 0 failed** (106 binaries; was 3245); harness real-loopback iroh **15/15 ×2**;
e2e bi-egress 49/49, hexon-install 15/15, hexon-serve 40/40. Pushed `e96c972..9e09b12`
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

- **origin/main = `9e09b12` + `f986ee1` `26546ec` `e2a3349`** (all pushed 2026-10-12 under
  push-on-green; the last three are docs/assets only): untrack `.claude/settings.local.json`
  (its `.gitignore` line landed in `e2a3349`), this runbook, and the sample-GLB swap —
  `duck.glb` (SCEA, non-OSI) → CC0 Khronos "Cameras" `cameras.glb` (2 embedded cameras, still
  the `deactivate_foreign_cameras` regression asset); `info_panel.glb` verified CC-BY-4.0.
- **Open licence residual:** `fractalengine/assets/imported/` tracks 20 byte-identical copies of
  the SCEA Duck (runtime import cache). Untracking them deletes the files on other clones at
  their next pull and may break nodes whose `asset_path` points there — USER decision (§0c.4).
- CI for `9e09b12` was being watched at writing time — **verify in §0a**. It is the FIRST real run
  of the cargo-deny and bounded-fuzz lint steps (fuzz was only `cargo check`ed locally; sanitizer
  builds need Linux nightly). If Lint is red, fix that first.
- **Verified:** the final sweep above. **Not verified:** in-app GUI behavior of W4 work;
  relay-assisted connection through a self-hosted iroh-relay 1.3; the fuzz CI step; the 0.35 store
  migration on a REAL user data dir (only the committed generator + the local fixture).
- Untracked on purpose: `fe-sync/tests/fixtures/p2p_store_v035/` (7.1 MB; the migration test
  skips when absent — a skip-as-green smell; commit decision open).

### Review ledger

| Phase | Checkpoint | Findings | Verdict |
|---|---|---|---|
| CI fix `d4a4744` | independent review of async-trait diff | 1 misread corrected | SOUND |
| `9cbdea6` terrain correlation | adversarial review-and-fix | 1H 3M 2L | FIXED |
| `9c4da00` elevated + query guard | security review-and-fix | 1H (record-id leak) 2M 1L | FIXED |
| `6763302` existence oracle | review-and-fix | 1H (stamp cross-tenant) 2M | FIXED |
| `7760be1` TilesetMeta | review-and-fix | 1L advisory | SOUND |
| W2 `7d40a4e` | review-and-fix | 1H (zip-bomb reserve) 1M (GC vs stamp blob) 4L | FIXED |
| W3 `40590e9` | review-and-fix | 1M (stamp lock bypass via move_waypoint) 1L | FIXED |
| W4 `4b30d17` | 2 reviews + reconcile | 1H (hidden ribbons pickable) 2M 4L | FIXED |
| W5 `d44f008` | review-and-fix | 1H (watcher masked stack-failure health) 1M (doc handle leak) 3L | FIXED |
| Final sweep | full tests + harness + e2e | harness 14/15 → root-caused, `a210961` | GREEN after fix |

## 3. Decisions

**From the 2026-10-12 grill (user):**
- **Next work order:** large in-progress phases FIRST — PAS-P3+P4 (transport + chunk handlers +
  resumable downloads; residency ledger / lazy tiles / exit-ring eviction), PAS-P5 (partial
  hexon fetch), CDL Wave 4 integration, CDL SPEC-10 observation plane. Then role propagation,
  the decision backlog, pending tracks (all four streams are in scope).
- **Role propagation:** replicate the `role` table as ordinary replicated rows, admitted only when
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
- **Do-now (2026-10-12):** pushed `9e09b12`; untracked `.claude/settings.local.json`; replace
  licence-unverified sample GLBs (keep an embedded-camera regression asset).

**Working rules (user, standing):** small sequential workflows (builds never overlap; `TaskStop`
the previous workflow id before relaunch); reviewers fix their own findings (no read-only
reviewers); **no tests until the code is complete** — compile + clippy between workflows, one full
test + e2e + harness sweep at the end; prune `target/debug/incremental` + PDBs at every safe
point; network access granted; **report at the end** (not mid-run).

**Outputs (user, standing):** reports are published as **artifacts** — status + decisions on top,
teaching depth inside (file:line links, quoted load-bearing lines, why/what-to-watch, Sharpen),
diagrams where they help.

**Carried from earlier:** DEC-C1…C22 (`conductor/tracks/mission_prodready_continuation_20261008/spec.md`);
canonical-data-log network rollout / relay seeding / inbound P2P / `/ws/canonical` mount stay
NO-GO (local Workstream G implementation is GO); `../gis-tile-etl` kept, never pushed.

## 4. Assumptions (highest reversal cost first — confirm the top line before building on it)

- Role-table replication semantics: last-writer-wins by HLC per (verse, peer) row; revocation =
  tombstoned role row; the owner role is never replicated-in (derived from `created_by`) ·
  default taken: as stated · to reverse: redesign before anything depends on it (moderate).
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

### 2026-10-12 — handoff after the completeness campaign
- Done: retro (§1); 5-round grill loop → §3 decisions; pushed `e96c972..9e09b12`; untracked
  `.claude/settings.local.json` (`f986ee1`); runbook rewritten.
- Reviewed: every phase in the §2 ledger has a verdict.
- Also done: sample GLBs licence-verified/replaced (`e2a3349`); ledger OSS-4.3 updated.
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
