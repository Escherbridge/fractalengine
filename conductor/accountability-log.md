---
type: log
title: Accountability critic log
created: 2026-10-13
---

# Accountability critic log

User directive 2026-10-13: a critic monitor periodically checks the long-horizon session and steers
for accountability — overriding unneeded deferrals and ordering review agents. Its directives bind
the orchestrator unless overridden **with a logged reason** here.

- Brief: `.omc/accountability/critic-brief.md` (standing, read every tick)
- Cadence: at every workflow boundary (launch/finish) and before any push. An hourly session cron
  was attempted 2026-10-13 and denied by the permission classifier — the user can allow CronCreate
  to add it.
- Dispositions: `ACCEPT → <action/commit>` or `OVERRIDE → <reason>`

## Ticks

<!-- newest last; one section per tick: time, verdict, directives table, dispositions -->

### Tick 1 — 2026-10-13 17:54 MDT · VERDICT: DRIFTING (2 HIGH, 3 MED, 3 LOW)
Report: `.omc/accountability/tick-20261013-1754.md`. Sound: purge (Duck unreachable, 128/130 cited
SHAs resolve; the 2 others are foreign), W0 (review FIXED, gate green, legit deferrals), plan
deferral audits (except SPEC-10 Phase B).

| id | sev | directive (short) | disposition |
|---|---|---|---|
| AC-1 | HIGH | Role-prop plan body still encodes superseded rules; Deferred queue evicts at 4096/verse (flood can evict legit grants) | **ACCEPT** → opus docs agent rewriting plan/spec body + durable flood-proof retry source + tests |
| AC-2 | HIGH | Don't push code to main after a compile-only gate (`bf28971` untested, fe-database outside CI test scope; main push publishes `latest`) | **ACCEPT (option A)** → scoped test gate for exactly the pushed code (W0 `test_sweep_must_cover` + W0b-touched crates) before pushing W0+W0b; then **no main pushes from W1 until the final sweep** (docs-only excepted) |
| AC-3 | MED | SPEC-10 body still gates Phase B; header's "Phase B immediately after A1" wrong (B2/B3 need P3P4/P5/W4 code); W4/P5 lack R7; A1-early reorder unlogged | **ACCEPT** → sonnet docs agent reconciling 3 plan files; reorder logged in RUNBOOK §4 |
| AC-4 | MED | Fuzz demoted to warning — a fuzz-found crash must still block publish | **ACCEPT** → applied after W0b lands (W0b's workflow can't be steered mid-run) |
| AC-5 | MED | Finished W0 sat uncommitted + main red ~8 h behind a blocking question | **ACCEPT** → standing rule in RUNBOOK §3 working rules |
| AC-6 | LOW | Review-ledger rows missing, stale ledger/RUNBOOK lines, 3 decisions without rows, GitHub old-object residual | **ACCEPT** → haiku ledger pass + orchestrator RUNBOOK edits |
| AC-7 | LOW | `asset_backfill.rs:106` path join safe only while asset rows don't replicate; SPEC10 A3 makes them replicate | **ACCEPT** → added to A3 owned files + security review (guard + 2 tests) |
| AC-8 | LOW | Prune `target/debug/incremental` (244 dirs) after W0b, before W1 | **ACCEPT** → at the W0b→W1 boundary |

### Tick 2 (pre-push) — 2026-10-13 18:57 MDT · VERDICT: ON_TRACK (1 HIGH, 2 MED, 2 LOW)
Report: `.omc/accountability/tick-20261013-1857.md`. Tick-1 directives verified applied (AC-1..AC-7;
AC-8 prune scheduled after the gate). `actionlint` clean; a Python port of the KTX2 guard over the
user's real blob store (164 files) refuses nothing that loads today.

| id | sev | directive (short) | disposition |
|---|---|---|---|
| AC-9 | HIGH | Packaged binary never launched; R7 asks for a download users can actually run; the push prunes the old bare binaries | **ACCEPT** → after the gate: debug build → package_release.py → extract to scratchpad → launch from another CWD with `FE_SYNC_RELAY=disabled` → check data/ beside exe, asset root, no panic → kill exact PID. Pre-checked safety: asset GC only deletes hashes of rows it deletes from ITS OWN DB (`asset_gc.rs:21-22`), so a fresh DB cannot touch the user's blob store |
| AC-10 | MED | `fe-api --test mcp_integration` (MCP upload_asset shares the changed validate_glb) missing from the gate | **ACCEPT** → added to the gate |
| AC-11 | MED | QUICKSTART-gui.txt still says crafted models can OOM-crash; must state the KTX2 refusal | **ACCEPT** → text replaced (KTX2 refused peer+local, convert to PNG/JPEG; unpack somewhere writable) |
| AC-12 | LOW | Review-ledger rows W0b/W0c/AC-1; false "CI green on 119dbd0 / tests pass" claims in ledger :227-228; OSS-1.3 → DONE; stale lines | **ACCEPT** → docs pass after the push + first CI run |
| AC-13 | LOW | Nothing tests the local-import KTX2 refusal (`crud.rs:376-379`) | **ACCEPT** → folded into W1 slice S6 (fe-database) brief |

**Orchestrator finding (OF-1, MED, found while pre-checking AC-9):** the blob store is per-USER
(`dirs::data_local_dir()/fractalengine/blobs`) while the DB is per-INSTALL (`data/` beside the binary,
or the CWD for `cargo run`). Asset GC consults only its own DB's references, so install A collecting
an unreferenced asset removes the blob even when install B's DB still names that hash. Pre-existing
(different CWDs already split DBs), but packaging makes multiple installs normal. → logged as ledger
row BLOB-STORE-SCOPE; fix options: co-locate the blob store with the DB (`data/blobs`, one-time
migration) or make GC never remove shared-store blobs. Scheduled for the campaign (not a push blocker:
no consumers yet; a single install is unaffected).

**AC-9 result (2026-10-13 19:26):** packaged smoke PASS — `fractalengine-windows-x86_64.zip` (86.5 MB,
debug exe) extracted + launched from a different CWD with `FE_SYNC_RELAY=disabled`: alive after 45 s,
`data/fractalengine.db` beside the exe, nothing written to the launch CWD, no panic.log,
`AssetPlugin file_path = <extract>\assets`; exact PID tree killed. AC-10: `mcp_integration` 30/30.

**Orchestrator finding (OF-2, HIGH, pre-existing since `5199352`):** the smoke log shows `ERROR Could not
open local API read connection; analytics API will remain unavailable … (os error 33)` —
`fractalengine/src/main.rs:360` opens a SECOND SurrealKV handle on `data/fractalengine.db` in the same
process after the DB thread holds it, so on Windows the desktop app's BI/analytics API is unavailable
whenever the DB thread opens first (e2e BI scripts test the headless relay, so it went unseen). Not a
regression of this push → fix next (W0d: share the DB thread's `Surreal<Db>` handle) before W1.

**W0d / OF-2 closed (2026-10-13):** `558197c` — one SurrealKV open per process (the reader shares the
DB thread's handle; DB runtime multi-thread so the router serves while the loop blocks in recv). Also
fixed a second pre-existing defect: the old reader's router lived on a throwaway runtime → dead after
hydration. Review FIXED (1M 3L); push gate green (fe-database 311, fe-api 289, fractalengine 62, relay 7).
Packaged re-smoke: "Hydrated EntityStore through the DB thread's shared SurrealKV handle node_count=3",
"API server listening on 127.0.0.1:8765", zero ERROR lines.
