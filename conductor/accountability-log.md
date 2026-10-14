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

### Tick 4 (W1 close / W2 launch) — 2026-10-14 01:49 MDT · VERDICT: ON_TRACK (1 HIGH, 2 MED, 1 LOW)
Report: `.omc/accountability/tick-20261014-0149.md`. Verified sound: AC-16 (W1 S9 covered plan steps 1–6);
both mandatory S8b findings fixed AND regression-tested (`sync_thread.rs:2188` + test `:3767`;
`bundle.rs:336/:409` + `tileset_bundles_test.rs:318`); W2 script faithful (order, S1b, R7, gate_green halt).

| id | sev | directive (short) | disposition |
|---|---|---|---|
| AC-19 | HIGH | No gate compiles feature-gated code (fe-terrain `fetch`, fe-hexon `remote`/`conformance`); W2 S1's new `HexonManifest.integrity` breaks fetch-gated literals (`builder.rs:223,:326`) while gates stay green | **ACCEPT** → P5 plan §10 C10 (urgent docs agent): every remaining slice + S8 + final review also run the feature-matrix check/clippy; first post-amendment slice repairs the literals; feature-matrix TESTS added to the final-sweep manifest (conformance tests must appear by name). Per-slice `checks_run` verified at landing. CI `--all-features` clippy step: decide at the final sweep |
| AC-20 | MED | My S5 "extra" allowed lossy `try_send` for ALL SyncEvents (state-bearing ones would strand in-flight slots); slots live host-side in `terrain_bridge.rs`, not fe-sync; S7a TOFU pin source | **ACCEPT** → P5 plan S5 amendment explicitly SUPERSEDES my workflow-prompt extra (lossless for state-bearing events, host-side timeout into backoff, >256 burst test); S7a pin uses the `root_for`-chosen entry's `publisher_did` |
| AC-21 | MED | W1 residuals without disposition: (a) streamed cache / iroh store growth; (b) per-source tile budget; (c) non-atomic install; (d) set_petal_terrain_handler no authorize_edit; (e) blob-ALPN filter unscheduled | **ACCEPT** → (a) LRU disk budget → P5 S5; iroh-store GC → ledger row IROH-STORE-GC with trigger; (b)+(c) → P5 S6; (d)+(e) → role-propagation deliverables with named tests |
| AC-22 | LOW | Bookkeeping: harness 16 scenarios, PAS-P1V honesty, track-board counts, RUNBOOK §2 State, review rows ticks 2–4, root AGENTS crate lines | **ACCEPT** → docs lane (sonnet) |
| watch | — | W2 S1 editing `archive.rs`/`archive_index.rs` (plan assigns them to S2) | check S1's report at landing |

**Automated review of W2 S2 `cbbd806`, MEDIUM, CONFIRMED (narrower than reported):** `verify_header`
(`fe-format/src/partial.rs:85-124`) returns `uri`/`layout`/`archive_size` from the unauthenticated doc;
sealed Container chunk_count IS checked, but uri substitution, Monolithic-downgrade and unauthenticated
size remain. **Routed into W2 S8 as a mandatory fix + tests** (P5 plan S8 section) — S3 is building.

### W2 close — 2026-10-14
- **W2 PAS-P5 DONE** (`4865a54..e7762d9`, 17 commits, 11 agents, ~5.4 h). Every slice `gate_green`
  incl. the AC-19 feature matrix (S1b surfaced + fixed pre-existing feature-gated clippy failures).
- **S8 `d21a957` FIXED**: mandatory VerifiedHeader binding verified in code (`doc.uri` must equal the
  signed `hexon_id@version` → `Substituted`; layout derived from signed coverage; `archive_size` removed)
  + HIGH pin provenance after restart + HIGH no store-side publisher continuity + 5 MED + 2 L.
- **Final review `e7762d9` FIXED**: HIGH late streamed meta reply auto-installed + 3 MED + 2 L.
- **Open residuals (to critic tick 5 for disposition):** (1) production P2P download path does not go
  through FetchStrategy/P2pPartialFetch (only the header check is shared) — A7 "same conformance suite"
  acceptance gap; (2) `HexonOp::Install` (local file) runs on the Bevy main thread, now heavier with seal
  verification; (3) streamed path verifies on the main thread with `RequireSealed{pin:None}` (display-only);
  (4) narrow first-contact TOFU race (store-side continuity backs it); (5) relay `FE_DB_PATH` alone does not
  move the blob dir; (6) orchestrator: `install_tileset` temp+rename has no `sync_all` before rename;
  (7) S7b: no bearer-token auth for hosted registry fetch.
- W2 must-cover appended to `conductor/final-sweep-manifest.md`.

### Tick 5 (W2 close / W3 pre-launch) — 2026-10-14 07:22 MDT · VERDICT: ON_TRACK (4 MED, 2 LOW)
Report: `.omc/accountability/tick-20261014-0722.md`. Key ruling: W2 residual (1) is NOT an A7 failure —
production seeders publish the same root the conformance suite covers (`partial_fetch_p2p.rs:316-319`
"one root for every seeder") and the download path shares the one header check; what is unmet is A3
(FetchStrategy as the single policy layer) — a plan-design consequence (fe-sync format-agnostic), not a
scope trim; `plan.md` wrongly ticked it.

| id | sev | directive (short) | disposition |
|---|---|---|---|
| AC-23 | MED | Residual (1) as honest bookkeeping: A3 caveat, A7 evidence named, PAS-P5 shas, ledger row PAS-P5-P2P-POLICY with triggers, fe-hexon AGENTS note | **ACCEPT** → docs agent (conductor) + W2b executor (fe-hexon AGENTS line) |
| AC-24 | MED | W2b fixups BEFORE W3: F1 streamed pin chain off-main-thread; F2 HexonOp::Install off main thread; F3 per-store install lock + sync_all; F4 FE_HEXON_REGISTRY_TOKEN; F5 relay blob dir from FE_DB_PATH; F6 registry nosniff; STREAMED-TERRAIN-PIN routed to RP; TOFU first-contact race / >64 MiB monolithic / trim race / member_keys accepted with reasons | **ACCEPT** → W2b `wf_cbb4d1ed-495` launched (sonnet exec → opus review-and-fix, workspace + C10 gates) |
| AC-25 | MED | W4-9/C5 `legacy_identifier` premise false (fe-database/fe-query don't depend on fe-sdk; fe-sdk is serde-only) → fe-sdk + blake3, hand-encoded CBOR, golden cross-check, owned Cargo files; SPEC10 B2 adds fe-sdk to fe-query + `--features parquet` gate | **ACCEPT** → docs agent (W4 C7, SPEC10 row) |
| AC-26 | MED | W4 plan gate/test drift: per-crate-only gates + campaign-end `cargo test` after slice 9 contradict AC-17 + the tests-at-end rule | **ACCEPT** → docs agent (W4 C6: workspace triple + gate_green halt; tests → manifest "W3 CDL-W4"); no C10-style feature gate needed for W4 crates (fe-identity no-default check only if edited) |
| AC-27 | LOW | W4 precision: W4-7 key-id index (SecretStore can't enumerate), restart tests must drop+join (one open per process), W4-1 KTX2-guarded fixture, W4-8 role-None review item, re-grounded citation table @8d8eaf8 | **ACCEPT** → docs agent (W4 C8, C9) |
| AC-28 | LOW | Bookkeeping + prune 7.5 GB PDBs | **ACCEPT** → docs agent; PDBs pruned (114 files) before W2b |

### W2b close — 2026-10-14
- **W2b DONE** `4306cb2` + review `474ab9b`, gate_green (workspace triple + C10 matrix). Reviewer FAILED the
  executor on 2 HIGH (streamed path still verified an archive on the main thread via `installed_publisher`;
  both F1 system tests were vacuous — the unsolicited reply was dropped before verification) + 2 MED
  (per-frame RequestTilesetMeta storm; registry.json lost updates worker↔main) + 1 L — all fixed.
- Accepted: streamed path records no TOFU pin of its own (covered by ledger STREAMED-TERRAIN-PIN, RP after
  RP-4); bare `FE_DB_PATH=db.kv` falls back to `data/blobs` (documented); race tests are probabilistic-red
  without the fix, deterministic-green with it.

### Tick 6 (W2b close / W3 launch) — 2026-10-14 08:23 MDT · VERDICT: ON_TRACK (1 MED, 3 LOW)
Report: `.omc/accountability/tick-20261014-0823.md`. Verified: W2b F1–F6 really in the code, rewritten F1
tests non-vacuous; `20bb91b` faithful except 4 defects; W3 script faithful (no relaunch needed).

| id | sev | directive (short) | disposition |
|---|---|---|---|
| AC-29 | MED | Final-sweep manifest lines truncated at a fixed width by the ORCHESTRATOR's extraction (`x[:220]`/`x[:260]`) — named tests silently dropped | **ACCEPT, orchestrator bug** → every truncated line restored from the journals (**73 lines**, not 14 — `tests_written` lines were cut too); no line left at a truncation length; future appends never truncate |
| AC-30 | LOW | W2b gaps: F4 test bypasses the seam; main-thread `remove`/`set_seeding` can block on the install lock held across a 1 GiB write+sync; `save_registry` lacks sync_all; F2 test name over-claims | **ACCEPT** → W3b fixup at the W3→SPEC10 boundary (brief B) |
| AC-31 | LOW | Plan amendments before W4-7/W4-9 read the plan (C10, C8(iii) correction, strikes, W4-9 owned files, golden test `legacy_identifier_matches_the_canonical_log_cbor_encoder` over id lengths 0/1/23/24/255/256; RP C10 + ledger `:243` terrain_bridge/petal_binding + streamed-TOFU item); manifest note to copy W4-9 evidence JSON before any target/ prune | **ACCEPT** → docs agent now (plans + ledger); manifest note added by the orchestrator |
| AC-32 | LOW | Orchestrator duties: review rows W2b + ticks 5/6; RUNBOOK §2 `:116` stale; at W3 close verify docs verdict lines, run both C6 greps, fe-identity `--no-default-features --tests` if touched | **ACCEPT** → rows + §2 fixed now; W3-close items on the checklist |
| watch | — | 681 processes (> the 646 lag mark) | dry-run reap of own strays at the next boundary |

### W3 close — 2026-10-14
- **W3 CDL-W4 DONE** (`38dd271..381264e`, 12 agents, ~3.8 h): W4-1..W4-9 all gate_green; security
  review `e886539` FIXED (MED: device-enrolment rows were the one row-plane table exempt from A3
  deny-by-default — membership floor added, overriding W4-8's interim "no floor"; LOWs: pipeline byte
  binding, keystore secret text in hex-decode errors); correctness review `00a2a08` FIXED (MED: W4-1
  counter tests raced on process-global counters; MED: KTX2-refused-import-counts rule untested → test
  added; 3 L). Docs `381264e` recorded both verdicts in G-WAVE-4 + the decisions record (verified).
- **AC-32 checks:** fe-identity untouched (no extra check needed); C6 NO_GO greps clean —
  `build_router_with_canonical_log` callers only inside `router.rs` `#[cfg(test)]` (0 in binaries/server),
  `commit_operation_dual_emit` called nowhere live; KTX2-attempt rule documented (`migration/AGENTS.md:90`).
- **Routed:** W4-2's replay-time payload-facts deferral → ledger CDL-REPLAY-PAYLOAD-FACTS (owner decision:
  crypto-shredded key semantics on replay; first consumer SPEC10 B3). Accepted residuals: revocation reach
  per verse (documented); quarantine outcomes mapped to best-fit Rejected wire categories pending owner
  ratification (W4-4); W4-9 network metrics structurally zero (NO_GO).
- W3 must-cover appended untruncated to the manifest. Next: W3b fixups (AC-30) → critic tick 7 → SPEC-10.

### Tick 7 (W3 close / SPEC-10 pre-launch) — 2026-10-14 12:19 MDT · VERDICT: ON_TRACK (1 HIGH, 5 MED, 2 LOW)
Report: `.omc/accountability/tick-20261014-1219.md`. Verified (code read): all five W3 review fixes real
(enrolment membership floor after the self-authorship branch; counter tests serialised; pipeline op_id
binding; keystore error redaction; KTX2-attempt test — one weak `contains("ktx2")` assertion → AC-35h).
Review ledger now complete; other false/stale "done" claims listed (AC-39).

| id | sev | directive (short) | disposition |
|---|---|---|---|
| AC-33 | HIGH | CDL-REPLAY-PAYLOAD-FACTS collides with SPEC-10 B1/B3: replay passes `None` facts (`canon_log/replay.rs:203`) but SPEC-10 §4.4 rule 3 forbids the only lawful fallback → B1 has no legal replay output; the owner question was queued AFTER its first consumer (orchestrator sequencing error) | **ACCEPT** → default adopted NOW as a §4 assumption (shredded/absent key → pending with a distinct reason, pre-shred row kept, never Excluded; owner post-review in the final report); new slice **B2b** before B3 (opus, security review-and-fix) |
| AC-34 | MED | A3's new tables vs the pre-manifest bootstrap window (`replicated_row.rs:329-338`) — device_format/asset admissions permanent and arrival-order-divergent | **ACCEPT** → per-table pre-manifest rule (device_format + asset fail closed) mirroring W3's enrolment floor, named test |
| AC-35 | MED | Phase A stale owned files + compile breakers (compose.rs split; receipt.rs/artifact_aad.rs unowned; `matches!` lane gate won't fail to compile; SourceClass home → fe-sdk; 7 struct-literal files; 4 app.rs routing tables; reuse `is_safe_path_id`) | **ACCEPT** → plan C17/C18 |
| AC-36 | MED | Phase B collisions (fe-query→fe-format default-features=false; in-memory `HexonArchive::import`, plaintext never on disk; B2 owns archive.rs packer; real `legacy_identifier` signature; spec authoritative; tests 10/12 owners; §4.2 rule 2 enforcement) | **ACCEPT** → plan C20/C21/C23 |
| AC-37 | MED | Plan gate text stale; harness → 17 scenarios | **ACCEPT** → plan C16/C24 |
| AC-38 | MED | DB-READER-RO (must land before SPEC10 A3) unscheduled | **ACCEPT** → new slice **A3b** between A3 and A4 |
| AC-39 | LOW | False/stale done-claims (ledger `cross_spec_integration` "green"; G-WAVE-4 greps "not run"; tracks.md; RUNBOOK §2); ledger rows CDL-WRAP-DELIVERY + LEGACY-TRANSFORM-TOMBSTONE; CANON-WS pre-mount precondition; four W3 judgement calls → owner queue | **ACCEPT** → docs agent (brief §3) |
| AC-40 | LOW | Process watch — dry-run reap of own strays not logged | **DONE**: dry-run at 678 processes → 0 session strays (only the live slice watcher when running; both smoke apps killed) |

**Orchestrator lesson (sequencing):** an owner question whose answer a scheduled slice needs must be
answered or defaulted-as-assumption BEFORE that slice — not queued for the end report (AC-5 applies).

### W3b close — 2026-10-14
- **W3b DONE** `bccb654` + review `78eab0e`, gate_green. AC-30 (a) bearer token through the real seam, (d)
  off-main-thread assertion — hold. Reviewer FIXED (b) MED: one thread per Remove/SetSeeding raced on the
  lock → a quick seeding on→off could land off→on (republished) → one long-lived FIFO mutation worker; LOW:
  publish only after a successful seeding write; (c) LOW: fixed `registry.json.tmp` could tear between GUI
  and relay sharing a store dir → unique temp path. Accepted: (d) is probabilistic-red by nature (documented).

### W4 CDL-SPEC10 launch — 2026-10-14
- Launched `wf_22ded023-74a` (task `wji7vro01`), base `26833b9`: A2, A3, A3b, A4, A5, A6, B1, B2, B2b, B3
  → Phase A security → Phase B security → correctness review → B4 docs. gate_green halt + full feature
  matrix per slice. Critic tick 7 was the pre-launch check for this phase (amendments `26833b9` landed
  first), so the next tick runs at W4 close.
- Orchestrator slip (twice): a watcher started with a shell `&` inside a foreground call is an untracked
  process no notification reaches — both killed by exact command-line match (PIDs 50852, 29288); watchers
  only via `run_in_background`.

**Automated review of W4 A3 `65f39ec`, 2× MEDIUM, CONFIRMED:** (1) `fetch_known_payloads`
(`iot_reading.rs` ~258-283) unscoped by petal → cross-tenant payload reference; (2) device-payload
idempotent retry (`device_payload.rs` ~196-205) returns another petal's `asset_ids` → IDOR. **Routed into
W4's Phase A security review as MANDATORY** (block at the top of the SPEC-10 plan) — A4 is building.
