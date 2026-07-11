---
type: Track Plan
title: P2P Unblock-Now — Implementation Plan
tags: [bug, perf, p2p_unblock_now_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
---

# Plan: P2P Unblock-Now Fixes

Small independent fixes; one phase. Apply all, then a single test sweep at the end
(workspace test policy). Use an isolated `CARGO_TARGET_DIR` (e.g. `c:/tmp/fe-sweep-target`)
and never run cargo concurrently with the user's Antigravity/Codex auto-tests.

## Phase 1: All four fixes

- [ ] Task 1.1 — FR-1a: `fe-database/src/lib.rs:155-162` `replicate_row_with_petal` →
      `try_send` + `tracing::warn!` drop counter (mirror the sibling bridge in the same file).
- [ ] Task 1.2 — FR-1b: `fractalengine/src/main.rs:113-120` bridge hop → same pattern.
- [ ] Task 1.3 — FR-1 test: fill the bounded channel, assert DB handler returns without
      blocking and drop counter increments.
- [ ] Task 1.4 — FR-2: ring-buffer/last-K `node_log` in `fe-entity-store/src/lib.rs`
      (`get`/`append_log`/`upsert` paths); config for K; test with >K appends asserting
      bounded snapshot size and O(K) clone.
- [ ] Task 1.5 — FR-3: `fe-sync/src/sync_thread.rs:377` `std::fs::read` →
      `tokio::task::spawn_blocking`.
- [ ] Task 1.6 — FR-4: explicit BBR selection at iroh endpoint construction in `fe-sync`
      (or document 0.35 API limitation in `fe-sync/AGENTS.md` and defer to
      `iroh_1_0_upgrade_20260711`); log active congestion controller at startup.
- [ ] Task 1.7 — Single sweep: `cargo test -p fe-database -p fe-entity-store -p fe-sync`
      + clippy on touched crates.
- [ ] Task 1.8 — Update `fe-sync/AGENTS.md` / `fe-database/AGENTS.md` sections if behavior
      notes change (directory-level docs convention); retro + archive per track workflow.
