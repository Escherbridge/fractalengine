---
type: Track Spec
title: P2P Unblock-Now — Bridge Backpressure, BBR, Node-Log Cap, Sync-Thread Blocking Read
tags: [bug, perf, p2p_unblock_now_20260711]
timestamp: 2026-07-11T00:00:00Z
resource: ./metadata.json
decisions: ../../decisions/hexon-p2p-commons-20260711.md
---

# Specification: P2P Unblock-Now Fixes

**Track ID:** `p2p_unblock_now_20260711`
**Type:** Bug/perf fixes (small, local, no new subsystems)
**Status:** Spec ready — implementation not yet started as of 2026-07-14
**Priority:** P0 — decisions §D5-3: these ship **first**, independent of all format/auth work

## Overview

The hexon-p2p-commons research round (report §5a, all claims spot-check-verified §8.1)
identified four small, high-leverage fixes. Two bite even a single user; two turn into
user-visible freezes the moment real replication replaces the mock. None depends on the
delta-format, auth, or topology decisions — they are pure unblocking work.

## Functional Requirements

### FR-1: Replication bridge → `try_send` + drop metric (P5)

**Problem:** The DB→sync replication bridge chains two `bounded(256)` crossbeam hops with
*blocking* sends: `fe-database/src/lib.rs:155-162` (`replicate_row_with_petal`) and
`fractalengine/src/main.rs:113-120`. Crossbeam's bounded `send` blocks when full (the
trailing `.ok()` only swallows the disconnect error). A stalled sync thread therefore
freezes every DB write mid-handler. Two sibling bridges in the same files already use the
correct `try_send` + log-and-drop pattern. Today the mock's instant `HashMap::insert`
masks this; it must be fixed **before** `p2p_mycelium_completion_20260701` wires real
iroh-docs.

**Acceptance criteria:**
- Both hops use `try_send`; on `Full`, the event is dropped (or pushed to a small bounded
  retry queue) and a drop counter increments (`tracing::warn!` + metric), matching the
  sibling-bridge pattern.
- A saturated/stalled sync thread degrades to observable replication lag, never a blocked
  DB thread — provable with a test that fills the channel and asserts the DB handler
  returns.

### FR-2: Cap the hot-cache `node_log` (P1)

**Problem:** `EntityStore::get` clones the full `EntitySnapshot` including an unbounded
append-only `node_log` Vec; `append_log` clones, mutates, and reinserts the whole snapshot
(`fe-entity-store/src/lib.rs:136, 198-207`). Every update is O(N) in prior log length with
no compaction — directly hostile to IoT-frequency twin updates, the exact §D1-T0/T1
workload.

**Acceptance criteria:**
- In-memory `node_log` becomes a ring buffer / last-K window (K configurable, default
  small); the durable SurrealDB op_log remains the full-history source (per §D4 this later
  becomes the WAL — this fix must not conflict with that: cap the *cache*, don't touch the
  durable path).
- An update to a node with a long history no longer clones the full history; verified by a
  test appending >K entries and asserting bounded snapshot size.

### FR-3: Sync-thread blocking file read → `spawn_blocking` (P7)

**Problem:** A synchronous `std::fs::read` runs inside an async fn on the sync thread's
single-threaded runtime (`fe-sync/src/sync_thread.rs:377`), stalling all queued
gossip/replica/blob work for the duration of any slow disk read.

**Acceptance criteria:**
- The read moves to `tokio::task::spawn_blocking` (or async fs), keeping the
  current-thread runtime responsive; no other behavior change.

### FR-4: Verify + explicitly configure BBR congestion control (P8)

**Problem:** iroh-blobs throughput differs ~30x between BBR (~40% of link) and CUBIC
(~1-1.5%) per iroh#4286 (report §3 P8). FractalEngine's iroh 0.35 endpoint construction in
`fe-sync` does not explicitly select a congestion controller, so defaults may leak CUBIC.

**Acceptance criteria:**
- Endpoint construction explicitly configures BBR (or, if 0.35's API can't, the finding +
  version constraint is documented in `fe-sync/AGENTS.md` and rolled into
  `iroh_1_0_upgrade_20260711`'s scope).
- One line of startup logging records the active congestion controller.

## Out of Scope

- The gossip **receive loop** — that is `p2p_mycelium_completion_20260701` Phase 3 scope
  (its metadata now says so explicitly); duplicated here would collide.
- Real iroh-docs wiring, per-op signing, policy gating — separate tracks per decisions
  §D5 sequencing.

## Verification (single sweep at the end, per workspace test policy)

- `cargo test -p fe-database -p fe-entity-store -p fe-sync` (isolated
  `CARGO_TARGET_DIR` per workspace convention) plus the new FR-1/FR-2 tests.
- Manual: burst-write scenario (10k node updates) no longer stalls the DB thread.
