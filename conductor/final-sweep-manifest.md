---
type: manifest
title: Campaign final-sweep manifest
created: 2026-10-13
---

# Campaign final-sweep manifest

Durable list of verification steps that the campaign's **final sweep** (RUNBOOK §7 step 6) must execute beyond `cargo test --workspace`.

This manifest captures everything that cannot be deferred, is not automated in CI, or requires live environment verification. It survives between sessions and the W1 closure rolls up into it.

## Core suite (mandatory, ~2 hours)

All of these must pass green before pushing the campaign to origin.

| Step | Command / Procedure | Evidence |
|---|---|---|
| **Unit + integration tests** | `cargo test --workspace` (3452 baseline) | Test summary with count + zero failures |
| **Harness scenario 16** | `cargo run -p fractalengine-test-harness` (real loopback iroh, all 15 scenarios ×2) | Terminal output: `15/15 ×2 scenarios pass` |
| **fe-sync store migration** | `FE_REQUIRE_STORE_FIXTURE=1 cargo test -p fe-sync --lib p2p_store_migration` | Test pass; `0.35 → 1.x store upgrade` confirmed |
| **fe-database ignored tests** | `cargo test -p fe-database -- --ignored` (15 deferred tests; judge each) | Judgment table: PASS / SKIP reason / FIX needed for each test |
| **BI egress e2e** | `scripts/bi-egress-verify.ps1` | Output pasted into `conductor/tracks/analytics_egress_20260714/evidence.md` §X with DuckDB rowcount + schema |
| **Hexon install e2e** | `scripts/hexon-install-verify.ps1` | Output pasted into `conductor/tracks/oss_release_20260717/evidence.md` §X |
| **Hexon serve e2e** | `scripts/hexon-serve-verify.ps1` | Output pasted into `conductor/tracks/oss_release_20260717/evidence.md` §X |
| **Relay health** | `scripts/relay-health-verify.ps1` (headless fe-relay 1.x) | Output pasted into `conductor/tracks/p2p_asset_streaming_20260718/evidence.md` §6 |
| **Packaged binary smoke** | Build → extract → launch from different CWD with `FE_SYNC_RELAY=disabled` (AC-9 procedure) | Window opens, data dir beside exe, no panic.log, exact PID tree killed |
| **Relay binary smoke** | Build fe-relay → temp CWD → launch with temp FE_P2P_DIR → `FE_SYNC_RELAY=disabled` (AC-15 procedure) | API server listening, zero ERROR lines, exact PID tree killed |

## W0 / W0b / W0c / W0d must-cover (verify from journals)

Extracted 2026-10-13 from the reviewers' `test_sweep_must_cover` / `test_gate_commands` in the
session journals (`~/.claude/projects/…/a871e6ff-…/subagents/workflows/<wf>/journal.jsonl`). All of
these already PASSED once in the pre-push scoped gates (logs `.omc/research/push-gate-20261013/`,
`.omc/research/w0d-*.log`); the final sweep re-runs them on the campaign tip.

**W0 (`119dbd0`, `bf28971`) — `wf_aa8dded9-e92`**
- `FE_REQUIRE_STORE_FIXTURE=1 cargo test -p fe-sync --lib p2p_store_migration` — output must show `legacy_v035_fixture_migrates_and_reads_back ... ok`, not a skip
- `cargo test -p fe-database asset_backfill` (both backfill tests) and `legacy_imported_dir_is_where_old_imports_were_written`
- the full workspace sweep (crud.rs `load_hierarchy_handler` resolver changed)

**W0b (`6a53a2b`) — `wf_e727510a-f59`** (reviewer listed none): `cargo test -p fe-runtime -p fractalengine`
(packaged_root / file_asset_root); packaged GUI smoke (row above).

**W0c (`da3e6a1`) — `wf_34174034-e00`**
- `cargo test -p fe-runtime --lib ktx2_guard` and `--lib bevy_blob_reader`
- `cargo test -p fe-api --lib upload`, `--test asset_ownership_test`, `--test mcp_integration`
- `cargo test -p fe-database --lib handlers::crud`

**W0d (`558197c`) — `wf_e724d2d0-2de`**
- `cargo test -p fe-database --test shared_handle_test` (2 tests; mutation-checked against a current-thread runtime)
- `cargo test -p fe-database -p fe-api -p fractalengine -p fractalengine-relay`
- relay binary smoke (row above, AC-15) — relay `main()` has never been launched

## W1 PAS-P3P4 (append at W1 close)

*Appended by the orchestrator at W1 close: must-cover lists from slices S1..S8 (sequential), the S8b
security review-and-fix and the final review-and-fix; S9 is docs only.*

*Placeholder — leave empty until W1 closure.*

---

## Signoff checklist

- [ ] All core suite steps executed and passing
- [ ] W0/W0b/W0c/W0d must-cover lists reconciled and verified
- [ ] W1 PAS-P3P4 must-cover list appended and verified
- [ ] Zero test failures across all scopes
- [ ] No regressions in e2e scripts
- [ ] Packaged binaries (GUI + relay) launch and connect cleanly
- [ ] Campaign final sweep complete → ready to push

