---
type: Track Plan
title: Per-key clock plan
track: per_key_clock_20261113
tags: [security, p2p, rbac, fe-policy, fe-database, fe-sync]
created: 2026-11-13
status: planned
resource: ./metadata.json
spec: ./spec.md
findings: [QA-0156, QA-0165, QA-0166, QA-0126]
---

# Plan: per-key clock

Design: [spec.md](./spec.md). Slices run in dependency order; CLOCK-01 (this track) is done.

## Tasks

- [ ] CLOCK-02 fe-policy: closed grant windows on RoleStatement, frontier-aware continuing, as_of over windows, held_at (owns fe-policy/src/role_grants.rs, fe-policy/src/lib.rs, fe-policy/AGENTS.md; depends CLOCK-01)
- [ ] CLOCK-03 fe-database field_clock module: wire, pure merge fold, local stores, legacy-floor migration, submodule stubs (owns fe-database/src/field_clock/**; depends CLOCK-01)
- [ ] CLOCK-04 fe-database role_grants: persist grant windows, frontier on local revoke/lower, held_at_write helpers (owns fe-database/src/role_grants.rs, fe-database/tests/role_propagation_local_test.rs; depends CLOCK-02, CLOCK-03)
- [ ] CLOCK-05 Inbound clocked verse/fractal/petal rows: per-key judge and merge, write-time admission, legacy rows fill only unclocked keys (owns fe-database/src/field_clock/judge.rs, fe-database/src/handlers/clocked_row.rs, fe-database/src/handlers/replicated_row.rs, fe-database/src/handlers/mod.rs; depends CLOCK-03, CLOCK-04)
- [ ] CLOCK-06 Node publisher stamps per-key own versions into complete snapshots (_fe_clock beside _fe_scene) (owns fe-database/src/handlers/scene_publication/publisher.rs, fe-database/src/handlers/scene_publication/wire.rs, fe-database/src/handlers/scene_publication/mod.rs, fe-database/src/handlers/scene_publication/AGENTS.md, fe-database/src/op_log.rs; depends CLOCK-03)
- [ ] CLOCK-07 Node inbound per-key merge in apply_complete and write-time-only re-judge (QA-0165, QA-0156) (owns fe-database/src/handlers/scene_publication/inbound.rs, fe-database/src/handlers/scene_publication/AGENTS.md, fe-database/src/op_log.rs, fe-database/src/handlers/replicated_row.rs; depends CLOCK-04, CLOCK-05, CLOCK-06)
- [ ] CLOCK-08 Outbound clocked hierarchy rows: publish_clocked_row seam; create/rename/description/catalog paths publish through it (owns fe-database/src/field_clock/publish.rs, fe-database/src/handlers/crud.rs, fe-database/src/handlers/local_catalog.rs; depends CLOCK-03)
- [ ] CLOCK-09 Rewire remaining verse/petal emitters (settings, default access, terrain, render distance, groups, elevated) to the clocked seam (owns fe-database/src/handlers/verse_settings.rs, fe-database/src/handlers/petal_terrain.rs, fe-database/src/handlers/petal_render_distance.rs, fe-database/src/handlers/visibility_groups.rs, fe-database/src/handlers/elevated_query.rs, fe-database/tests/visibility_groups_test.rs; depends CLOCK-08)
- [ ] CLOCK-10 DB-local re-judge of clocked rows on role change (risen, fallen, owner change, local statements) (owns fe-database/src/field_clock/rejudge.rs, fe-database/src/handlers/replicated_row.rs, fe-database/src/handlers/clocked_row.rs; depends CLOCK-05, CLOCK-07)
- [ ] CLOCK-11 fe-sync: clocked rows over real transport (QA-0166 restore), parser tolerance, docs for unchanged-rewrite and rejudge (owns fe-sync/src/replicator.rs, fe-sync/src/AGENTS.md; depends CLOCK-08)
- [ ] CLOCK-12 Simulator p2p-core: revoke keeps authorized past edits, concurrent per-key writes, multi-author restore, documented transform winner (owns tools/qa/sim/scenarios/p2p-core.json, tools/qa/sim/AGENTS.md, tools/qa/sim/driver.py; depends CLOCK-01)
- [ ] CLOCK-13 Docs and findings close-out: fe-database AGENTS §write-time-role rewrite, field_clock AGENTS reconcile, QA rows, track plan ticks (owns fe-database/src/field_clock/AGENTS.md, fe-database/src/merge.rs, qa/findings.jsonl, conductor/tracks/per_key_clock_20261113/plan.md, conductor/tracks/per_key_clock_20261113/metadata.json; depends CLOCK-02, CLOCK-04, CLOCK-05, CLOCK-06, CLOCK-07, CLOCK-08, CLOCK-09, CLOCK-10, CLOCK-11, CLOCK-12)

## Verification

Tests run only when the user names the batches; nothing here runs them automatically.

- R7: `fe-policy` plus `role_propagation` (grant windows, `held_at`, local role statements).
- R10: `fe-database` lib including the new filters `field_clock::`, `handlers::replicated_row::`, `op_log::`.
- R8: `fe-sync` (clocked rows over real transport, QA-0166 restore).
- Simulator: `python tools/qa/sim/run.py p2p-core`, only on the user's go.
- Per-slice gate is `cargo check -p <touched crates>` at wave barriers; no clippy.
