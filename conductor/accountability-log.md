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

### Tick 3 (W1 launch) — 2026-10-13 20:12 MDT · VERDICT: ON_TRACK (4 MED, 1 LOW)
Report: `.omc/accountability/tick-20261013-2012.md`. W1 script faithful to the P3P4 plan (slices, §10
amendments, tiers, AC-13). W0d residuals 1–4 acceptable (recorded); residual 5 (reader read-only by
convention) needs a guarded newtype before SPEC10 A3.

| id | sev | directive (short) | disposition |
|---|---|---|---|
| AC-14 | MED | Every push cancels the in-progress CI run (`cancel-in-progress`); W0b/W0c never completed a CI run → hold ALL pushes until run 38498925963 concludes; set a red-CI policy; consider paths-ignore for conductor/** | **ACCEPT** → push freeze until that run concludes. Red-CI policy: YAML-only fixes push after actionlint; cargo-needing fixes wait for the W1→P5 cargo-free window (logged as an AC-5 override, main may sit red until then). paths-ignore `conductor/**`, `.omc/**` added with the next push |
| AC-15 | MED | Relay `main()` never launched before push (relay "7 green" = helper unit tests) | **ACCEPT** → relay smoke at the W1→P5 boundary (build fe-relay at W1 tip, temp CWD, `FE_SYNC_RELAY=disabled`, temp `FE_P2P_DIR`, expect shared-handle hydrate + API listening + 0 ERROR, kill exact PID); W0d review-ledger row states it honestly |
| AC-16 | MED | Script's S9 edits ledger rows the plan says it must not; omits spec.md strike, root AGENTS lines, D-77 drift note, evidence.md §6; ledger commit-collision risk | **ACCEPT with OVERRIDE of the plan text**: S9 may edit the 4 ledger rows (ledger accuracy needs it) because every docs lane commits before S9 starts; from now on docs lanes commit with `git commit -m … -- <paths>` (never stage-then-bare-commit while W1 runs); S9 coverage of plan steps 1–6 checked at W1 close |
| AC-17 | MED | Script halts only on agent death, never on a red gate; per-slice clippy misses crates slices touch | **ACCEPT** → each W1 slice result is checked as it lands (journal waiter); red gate → TaskStop + relaunch remaining slices from HEAD. P5/W4/SPEC10 scripts get a required `gate_green` field + halt, and workspace-wide clippy per slice |
| AC-18 | LOW | AC-12 still open (trigger never fired); review-ledger rows W0b/W0c/W0d/ticks 2–3; stale ledger/RUNBOOK lines; no durable final-sweep manifest; DB-READER-RO row; durability-env doc line | **ACCEPT** → haiku docs lane now (commits with `-- <paths>`); S8b + final-review must-cover appended to the manifest at W1 close |

**Automated commit security review (S3 `e5d8928`), MEDIUM→HIGH, CONFIRMED by reading the code:**
`TilesetProviderMap::record` (`fe-sync/src/sync_thread.rs` ~2180-2184) lets an unauthenticated (relayed)
ad overwrite an authenticated provider's `root`/`advertised_bytes`; `root_for` (~2204) feeds that root to
downloads → content poisoning. Disposition: **routed into W1 S8b as a MANDATORY fix + regression test**
(`8442e68` writes it into the plan's S8b section, which S8b reads) rather than editing fe-sync while S4's
cargo build runs (house rule). Verify at W1 close; if S8b misses it, fix before W1 is declared done.

**AC-14 closed — CI run 38498925963 on `53a4c4c`: SUCCESS (first completed run of today's work).**
Lint (warn-only) all steps green incl. cargo-deny advisories+bans+sources+licenses and fuzz
build/run/verdict; Linux build+test+package incl. "Generate iroh 0.35 store fixture" +
"Store-migration test (0.35 fixture required)" (`legacy_v035_fixture_migrates_and_reads_back ... ok`,
3 passed, grep-guarded against a skip); Windows build+package; **rolling `latest` published 4 runnable
archives** (fractalengine-{windows-x86_64.zip, linux-x86_64.tar.gz}, fe-relay-{windows-x86_64.zip,
linux-x86_64.tar.gz}) + SHA256SUMS. Push freeze lifted; next push = campaign final sweep (main now
carries W1 code commits interleaved with docs, so docs can't be pushed alone). paths-ignore
(`conductor/**`, `.omc/**`) rides with that push.

**Automated review of S4 `3d89b60`, MEDIUM, CONFIRMED:** peer-supplied bundle index's `tileset_id` is
never bound to the requested id (`bundle.rs` ~446 / `terrain_bridge.rs` ~324-336) → a provider can
overwrite a different installed tileset. **Routed into W1 S8b as mandatory finding #2** (plan S8b
section) — S5 is building, so no source edits now. Verify at W1 close.

### W1 close — 2026-10-14 ~01:40 MDT
- **W1 PAS-P3+P4 DONE** (`53a4c4c..069f8ea`, 11 agents, ~5.5 h): S1 `f25f906`, S2 `84cd342`, S3 `e5d8928`,
  S4 `3d89b60`, S5 `9725dd2`, S6 `b9a85f3`, S7 `0692e14`, S8 `a8a50c1` — every slice gate checked as it
  landed (AC-17): workspace check + clippy + fmt green; deferrals reviewed and justified.
- **S8b `4a61b19` FIXED**: both mandatory findings verified in code (`sync_thread.rs` `root_for` →
  `RootChoice`; `bundle.rs:336` `expected_tileset_id` binding) + HIGH peer-chosen `cache_dir` /
  `elevation_api_key_env` via replicated petal terrain + 7 MED (relay-URL SSRF, ad-storm DoS, partial
  streamed cache auto-install, resume-ledger tampering, streamed zip bomb, oversized blobs into the iroh
  store, Windows device stems in ids) + 1 L. Blob-ALPN request filter: decision recorded, not built.
- **Final review `069f8ea` FIXED** 3 HIGH (decode-failed bundle re-requested at frame rate; unbounded
  streamed fan-out + blocking main-thread send; installed downloads never cleaned up) + 5 MED + 2 L.
- **AC-15 CLOSED — relay smoke PASS** (W1 tip): alive 40 s, DB in its CWD, "Hydrated analytics
  EntityStore through the DB thread's shared handle", API listening :8765, S4 seeding published the
  installed PNW tileset (8 bundles, 7.69 MB), 0 ERROR lines; exact PID tree killed.
- **Residuals routed into W2 (PAS-P5) slice S5 (fe-sync):** (1) spawned sync-thread tasks use a
  BLOCKING crossbeam send on bounded(256) — >256 events between frames stalls the whole sync thread;
  (2) `chunk_requested` / `meta_requested` slots have no timeout (a never-answered request pins an
  in-flight slot). Other W1 residuals accepted as recorded in the journal (live peer terrain re-fetch
  deliberately off; double hash on startup = efficiency; GPU-pressure band edge; overlapping-source pin).
- W1 must-cover appended to `conductor/final-sweep-manifest.md` (`5c4745a`, 58 slice tests + S8b + final).
