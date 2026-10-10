---
type: runbook
title: FractalEngine session runbook
updated: 2026-10-10
head: d4a4744
---

# RUNBOOK — Prod-ready mission COMPLETE (2026-10-10)

Read this first, then descend only where it points. Foundation under it:
`conductor/product.md`, `tech-stack.md`, `workflow.md`, `tracks.md`.

> **Supersedes** the 2026-08-20 runbook (Canonical Data Log / Workstream G —
> recover via `git log -- conductor/RUNBOOK.md`). Its standing constraint
> "no network enablement" is now FALSE: the factory mission turned the real
> P2P stack on. The canonical-data-log workstream itself remains a separate,
> unfinished initiative (track `canonical_data_log_20260808`, GO-protocol /
> NO-GO-implementation per its 2026-08-08 review).

**Primary record:** `conductor/tracks/mission_prodready_continuation_20261008/`
— `plan.md` (per-feature evidence, milestone review verdicts, retro, deferred
register) and `spec.md` (decision log DEC-C1…C22). Factory-era record:
`C:\Users\atooz\.factory\missions\77c4c579-4f99-403f-8bcf-3b1ae3f0c718\`
(mission.md, validation-contract.md A1–A33, library/decisions.md D1–D4).

## 1. Goal (was)

Finish factory mission mis_6c57917e — "FractalEngine prod-ready: real P2P
replication + sharded timeseries fabric + verified BI egress" — from where the
droid paused (usage-402, mid-F9). **Done 2026-10-09**: F9–F19 delivered,
acceptance A20–A33 all validated.

## 2. State — COMPLETE, verified, PUSHED

- **Pushed (corrected 2026-10-10).** The 30 mission commits (14 factory + 16
  continuation) reached `origin/main` at `1e9be3a` on 2026-10-09 17:42 -0600 —
  reflog `update by push` from this clone, confirmed by `git ls-remote`. The
  first roll of this runbook (`cc080b9`) wrongly said "NOTHING PUSHED"; who
  pushed is unrecorded, so the A33 go-ahead gate was bypassed for those 30.
  The 2026-10-10 session pushed `cc080b9`, the lint fix, and this correction
  on the user's explicit go-ahead.
- **CI on `1e9be3a`** (run 38005764519): Linux build+test ✓, Windows build ✓,
  **Lint ✗** → rolling `latest` prerelease skipped. Cause: CI's floating
  stable is rustc 1.99.0 (local default 1.94.0) — 17× `clippy::double_must_use`
  in fe-canonical-log, emitted by `#[async_trait]` 0.1.89's generated code.
  Lint was already red on the pre-mission 2026-10-06 push. **Fix:** Cargo.lock
  bump async-trait 0.1.89 → 0.1.92 in `d4a4744` (upstream #303; adds syn 3.0.7, MSRV 1.71),
  no source change. That unmasked ONE more error CI never reached (clippy
  halts at the first failing crate): `clippy::collapsible_match` at
  `fe-sync/src/sharding.rs:367`, fixed as a match guard (behavior-identical;
  covered by `sync_thread.rs` inbound-seam test at :3441). Local proof on
  rustc 1.99.0: `clippy --workspace --all-targets --keep-going -- -D
  warnings` EXIT 0 across all 24 members; `fmt --all --check` clean.
  Tests on local 1.94.0 (also proves syn 3's MSRV fits): `cargo test -p
  fe-sync -p fe-canonical-log -p fe-database` 964 passed / 0 failed / 15
  ignored (22 binaries). The full-workspace test run is left to CI's Linux
  job on this push — check it (§8 step 1).
- **Verified, not just written** — final sweep on the closing tree:
  `cargo test --workspace` 3,245 passed / 0 failed / 21 ignored (96 binaries);
  `cargo clippy --workspace --all-targets -- -D warnings` clean (local
  rustc 1.94.0 — CI's 1.99.0 disagreed, see above); `cargo fmt
  --all --check` clean; harness bin 15/15 real-loopback-iroh scenarios; relay
  release build OK; e2e scripts checked in WITH logs: `scripts/
  bi-egress-verify.ps1` 49/49, `hexon-install-verify.ps1` 15/15,
  `hexon-serve-verify.ps1` 37/37.
- **Review ledger** (every milestone has a recorded verdict in plan.md):
  M3 PASS-WITH-FIXES → fixed → validated; M4 FAIL → fixes → re-review
  STILL-FAILING → remediation + grammar audit → validated; M5 FAIL → fixes →
  validated-with-notes; M6 PASS-WITH-FIXES → fixed → validated (acceptance
  re-runs green). M7 = the F19 sweep above.
- **User-gated residuals** (cannot verify headlessly): in-app terrain render +
  scale-bar widget for the installed Zurich/Fuji hexons (numeric scale fields
  machine-verified to ~0.03%); the Hexon Manager UI install button (its
  underlying seam is verified); a general in-app smoke of this build.

## 3. Key context a fresh read would miss

- **Every milestone review found real issues green CI had certified** — the
  retro in plan.md lists them. The four reviewer-named lessons are now house
  doctrine: (1) a scoped-table list is a denylist — denormalized copies
  (node_log) live one table over; (2) shared cores + separate guards = drift —
  authz chokepoints protect only what routes through them (the REST twins);
  (3) a writer of a shared JSON column must satisfy the STRICTEST reader
  (bind-terrain vs fe-terrain's serde parser); (4) serialize builds AND
  source/Cargo.toml edits — a mid-build manifest edit stacked 236 GB of
  artifacts and zeroed the disk.
- **Build ops on this machine** (DEC-C12, plan.md): disk check before every
  gate; prune `target/debug/incremental` + stale `deps/*.pdb` when < 10 GB
  free (PDBs once held 215 GB); workspace sweeps run
  `CARGO_PROFILE_DEV_DEBUG=0` (several-fold smaller, `-j4`-safe — with
  debuginfo, rustc OOMs above `-j2`); `RUST_MIN_STACK=134217728` for anything
  touching surrealdb.
- **C: is a 1.9 TB drive at ~90% full from NON-project data** — flagged to the
  user; project caches were a symptom.
- **fe-hermes / fe-pi siblings are pruned git-worktree stubs** of this repo
  (not standalone clones); `git -C` against them errors harmlessly.
- The e2e scripts are the standing re-runnable verification — they boot a real
  relay, mint tokens offline (`fe-identity --example mint_api_token` sharing
  the relay's seed env — there is deliberately NO HTTP mint route), seed via
  REST, and attach real DuckDB (`tools/duckdb/duckdb.exe` v1.5.6, gitignored).

## 4. Decisions

All in `conductor/tracks/mission_prodready_continuation_20261008/spec.md`
(DEC-C1…C22), each with rationale. Highest-leverage: DEC-C7 (sim control =
fe-runtime seam + relay-only feature), DEC-C14 (29 tools = documented
superset of the 20-name vocabulary), DEC-C16 (GeoParquet crs=null + fe:crs),
DEC-C19/C20 (guard hardening layers), DEC-C22 (strictest-reader rule).
Factory-era locked user decisions: D1–D4 in the mission library.

## 5. Assumptions

- The user's 2026-10-10 answer "remove" for `../gis-tile-etl` means "drop its
  push from the plan" · default taken: push step removed, repo and `33eff03`
  untouched · to reverse: re-add the step (nothing was deleted).
- In-app verification is the user's own next action, not a new agent task ·
  default: recorded as user-gated · to reverse: none.
- The old Workstream-G runbook content is recoverable from git and needs no
  copy kept · to reverse: `git show ca338bc:conductor/RUNBOOK.md`.

## 6. Relevant files

- `conductor/tracks/mission_prodready_continuation_20261008/plan.md` — the
  whole story: evidence log, review verdicts, retro, deferred register.
- `conductor/tracks/mission_prodready_continuation_20261008/spec.md` — DEC log.
- `fe-api/src/query_guard.rs` — the hardened egress guard (source
  substitution + egress re-check); read its AGENTS.md section before touching.
- `fe-api/src/mcp/mod.rs` — the ToolSpec/ScopeRule dispatcher (29 tools).
- `fe-sim/src/session.rs` + `fe-api/src/sim.rs` — sim control surface.
- `scripts/*-verify.ps1` + `.log` — re-runnable acceptance evidence.
- `docs/bi-egress.md`, `docs/simulation-lab.md` — operator docs.

## 7. Environment

- Branch `main`, clean, in sync with origin after the 2026-10-10 push —
  **still do not push without the user's go-ahead**. No stashes, no running
  processes, no open worktrees from the mission sessions.
- Toolchain `1.99.0` is installed locally (build `b940084d7`, identical to
  CI's stable as of 2026-10-10): `cargo +1.99.0 clippy --workspace
  --all-targets --keep-going -- -D warnings` reproduces the CI Lint job
  (Linux-only `#[cfg]` code excepted). `--keep-going` matters — without it
  clippy halts at the first failing crate and hides its dependents.
- `tools/duckdb/duckdb.exe` (gitignored) used by the egress e2e.
- Sibling repo `../gis-tile-etl`: commit `33eff03` added intl regions; built
  hexons live in ITS gitignored `dist/` (rebuild commands in
  `sample-hexons/*/README.md`). It has **no git remote**; pushing it was
  dropped from the plan 2026-10-10 (see §5).
- Secrets: relay e2e seeds live only inside script runs; the keystore slots
  are `node_keypair` and `share_signer` (OS keystore on GUI; `FE_SECRET_*`
  env on relay — operator must export or keys are ephemeral, warned loudly).

## 8. Continuation plan (next session)

1. ~~Confirm the push decision~~ — moot, done 2026-10-10 (§2). **Confirm the
   Lint job AND the Linux test job are green** on the pushed tip:
   `gh run list --branch main --limit 1`.
   If it is red again, reproduce with `cargo +1.99.0 clippy` (§7).
2. **User runs the in-app verification** (not agent work): launch the GUI,
   install or view the Zurich/Fuji hexons, confirm terrain + scale bar, smoke
   the editor. File any findings as new track items.
3. If taking new work, pick from the deferred register (plan.md retro, each
   with its trigger) — top candidate: **SetPetalTerrain reply correlation**
   (fe-runtime/fe-database/fe-ui), which removes the
   `TERRAIN_MUTATION_UNAVAILABLE` product gap and unlocks live tileset
   binding; or return to the board's P0 slates (`conductor/tracks.md`:
   Spatial Builder Program, UI shell) or the canonical-data-log initiative.
4. Housekeeping: the Lint job runs on floating `@stable`, so each rustc
   release can turn latent warnings red again (memory:
   ci-stable-toolchain-drift — third occurrence). Pinning the workflow's
   toolchain is an unratified option; trigger: the next drift-only red.

## 9. Open questions

- What did "remove" mean for `../gis-tile-etl` — only "don't push it", or
  delete the repo/commit? Nothing was deleted. (Trigger: user's reply.)
- Does `verse.namespace_id` readable via `/query` act as a read capability
  for the iroh replica? (Flagged by the M4 re-review; trigger: next fe-sync
  security pass.)
