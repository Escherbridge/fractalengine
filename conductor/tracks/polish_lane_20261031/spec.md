---
type: Track Spec
title: Polish lane — finish stubs, complete features, close seams, usability of every feature
track: polish_lane_20261031
status: pending
created: 2026-10-31
timestamp: 2026-10-31T00:00:00Z
inputs:
  - ../../../.omc/research/polish-stubs-20261031.md
  - ../../../.omc/research/polish-ux-20261031.md
tags: [polish, ux, feedback, rbac-ux, stubs, seams, advocate-critic]
---

# Polish lane (2026-10-31)

## Why

The user asked for a dedicated lane that "fans out and works to polish this application, finishing off
stubs, completing features, closing remaining seams and improving the usability of every feature", with
every wave reviewed by two reviewers: an **advocate**, who argues for design patterns and applies them,
and a **critic**, who hunts defects and anti-patterns and fixes them. Lane A (read-only recon) produced
two reports, and this track turns them into a mechanical, wave-ordered execution plan:

- **R1 stubs & seams** (`.omc/research/polish-stubs-20261031.md`): 6 user-visible gaps (UVG), 13 seams,
  8 dead-code candidates, 7 intentional placeholders, 2 items that need a decision.
- **R2 UX QA** (`.omc/research/polish-ux-20261031.md`): 82 findings (S1 = 5, S2 = 16, S3 = 47, S4 = 14).
  The headline is that **every failed GUI database command, role denials included, ends at
  `bevy::log::error!`** (`fe-ui/src/verse_manager/db_results/query.rs:74-75`). A Viewer who clicks
  "Set petal map" sees "Active map ✓" while no terrain ever loads, and gets no explanation.

Lane B (concurrent, code) delivers the frameless window, custom title bar, close X, app icon, and the
deny-by-default screenshot surface. This lane does not build any of those. It reviews Lane B's title bar
against R2 T-01 in Wave 3 and uses the screenshot surface as optional visual evidence once it lands.

## Recon corrections (planner-verified at 6531d19, recorded so no slice re-litigates them)

| Report claim | Verified reality | Plan consequence |
|---|---|---|
| R1 #2: fe-webview Tauri IPC stubs are "the single largest user-visible gap" | No `tauri::Builder`, `generate_handler!` or `invoke_handler` exists anywhere in the workspace, and `fe-webview/src/AGENTS.md` §tauri-commands says "Nothing registers them yet". The host shell they scaffold was closed **NO-GO by D-42**. | They are unreachable scaffolding, not a UVG. W6-S2 makes them honest (OD-8: delete the six stubs and keep `resolve_asset`/`get_asset_base_url`). |
| R1 #1: fe-plugin `GetNode`/`QueryNodes` is "the entire read-half of the scripting story" | The plugin host is **library-only**: README says so (ledger OSS-TRUTH), and its only consumers are `extensions/iot-bridge` and `fe-plugin-test`. `fe-plugin` has no `fe-entity-store` dependency. | It is a real seam but not app-visible. W6-S1 wires request-reply through an injected reader trait (OD-9). |
| R1 #9: `queries.rs:13` and `role_manager.rs:276` are missing permission checks | Ledger `DB-TODO-QUERIES` (`:364`): the only caller is the seed path. `DB-TODO-SCOPEMEMBERS` (`:365`): the function has no callers. | These are stale-TODO cleanup (W5-S3), not authz work. |
| R1 `plugin.rs:904`: "fe-identity not yet a fe-ui dependency" | `fe-ui/Cargo.toml:23` already depends on `fe-identity`. | The TODO is stale. W3-S2 populates `node_identity`. |
| R2 DW-04: in-app share-link mint | D-64 was ratified-as-written 2026-10-13 as "no" (ledger `EGR-MINT`). | TTL validation is in scope (W4-S5). The mint button is OUT. |
| R2 M-03 fix "needs X-01's correlation" | `DbResult::PetalTerrainSetFailed` already exists for correlated sets (`fe-database/src/handlers/petal_terrain.rs:121-131`). The GUI simply sends `correlation_id: None`. | M-03 is a GUI-only fix: mint a `ui-petalmap-N` id with the existing `actions::query::next_ui_query_id`. No protocol change. |
| F7 "server-side RawQuery scope" | `DbCommand::RawQuery` has about 33 constructors across 20 files (several are Lane B's). User-authored SQL reaches it only from the GUI Query tab. API SQL already goes through `fe-api/src/query_guard.rs::prepare_scoped_sql`. | F7 adds a new `ScopedQuery` variant for the Query tab and reuses the scoped-FROM rewrite moved down into `fe-query` (OD-7). `RawQuery` is documented as carrying server-built SQL only. |
| OF-13 location "fe-ui/src/verse_manager" | `push_mapping_layer` (`fe-sync/src/sharding.rs:365`) and `contested_claimed_fractals` (`:449`) are in fe-sync. | W1-S5 owns `fe-sync/src/sharding.rs`. |

## Goal

Every feature a user can reach either works, says plainly why it can't, or is removed. Concretely:

1. **Feedback is never silent.** A failed command (including a role denial) produces visible,
   plain-language feedback at the place it was triggered. Optimistic UI rolls back when the server
   refuses.
2. **Roles are visible before the click.** Edit controls are disabled for roles that cannot use them, and
   hovering a disabled control says which role is needed and which role you have.
3. **No dead ends.** Every visible control has a working path: the polygon sculpt can be completed, the
   Volume report can't hang, Join confirms, and Claim Ownership is reachable.
4. **Destructive actions are two-step**, follow one shared widget, and are role-gated.
5. **Stubs are finished or made honest**, seams are closed, and dead code is deleted.
6. **Queued items land:** F7 (server-side scope for user SQL), OF-13a/b/c (petal-map hardening), and
   visible explanations for role denials.

## Scope IN

- R2 findings S1 through S4, as scheduled in plan.md (82 findings; each one is either assigned to a slice
  or listed under OUT with a reason).
- R1 UVG/SEAM/DEAD items, with the corrections above.
- F7, OF-13a/b/c (preferred shapes from `conductor/accountability-log.md:637-658`), plus OF-12b and the
  `reverdict_petal_mappings` author-indexing when they stay inside `fe-sync/src/sharding.rs`.
- Stale-TODO cleanup ledgered as `DB-TODO-QUERIES`, `DB-TODO-SCOPEMEMBERS`, `SYNC-STALE-TODOS` and
  `FEUI-TERRAIN-MODE-TODO`.
- Undo/redo, bounded to transform commits and path-point moves (OD-6).
- Wave 3 integration of Lane B surfaces once Lane B has landed: the notice renderer in
  `panels/mod.rs`, the role chip and app state in `plugin.rs`, theme contrast, and topbar gating. Wave 3
  also checks Lane B's title bar against T-01.

## Scope OUT (explicit)

| Item | Why it is out |
|---|---|
| Lane B deliverables (frameless window, title bar, close X, icon, screenshot route, MCP `screenshot`, F12) | Owned by Lane B. This lane only reviews T-01 conformance (W3-S5) and consumes the screenshot surface. |
| The user's in-app look for any wave | User-gated. Agents may capture screenshots as evidence, but that never replaces the user's verify. |
| `CANON-WS` mount, `CDL-NET` network rollout of canonical-log | NO_GO by owner decision (local GO, network NO-GO). |
| D-23 production relay host | Needs a user action (name a host/domain). |
| T28 / BLOB-HASH-CAPABILITY | A security design decision (membership filter vs namespace rotation) is required first. Route it to the role_propagation_20261013 design lane. |
| S-1 relay message signing | Needs its own recon/spec pass. |
| fe-sync peer blob fetch, Phase F (`fe-sync/src/sync_thread.rs:1476-1504`) | L-size P2P feature that belongs to p2p_asset_streaming_20260718 (OD-10). |
| DB-plane `confine_petal` parity, P2P-REF-CONFINE | Owned by role_propagation_20261013 (ledger `:275`). |
| EGR-MINT in-app share-link mint (DW-04 second half) | D-64 is "no" (ratified 2026-10-13). |
| Migrating all 62 `DbResult::Error` emitters to correlated replies | OD-2: this lane ships the humanized stop-gap plus typed failures for Join and petal-map. A full migration is its own track. |
| ServoBackend, Gaussian-splat ingestion, `fe-query` graphql feature, `placeholder.glb` fallback, fe-network gossip TODO | Intentional or documented by design (R1 INTENT). |
| Large ratified tracks (hexon_unification, map_scale_authority, road_builder_ux, role_propagation, crate_consolidation_r2) | They have their own tracks and plans. Polish does not absorb them. |
| Pivot option for Rotate (O-05 "Pivot (soon)") | OD-14: the hint is removed and the pivot stays at the selection origin. |
| Push to origin | Held by standing rule. Waves commit locally only. |

## Acceptance (track-level)

- A1. Every S1 finding (X-01, M-03, D-05, D-01, O-01) is closed, with a named flow test that would go red
  if the fix were reverted.
- A2. With a Viewer role, every edit control listed in X-02 is disabled with the hover reason "Editor
  role needed — you are Viewer in <verse>", and a forced denial produces an error notice that lasts at
  least 6 s and appears in the status-bar error history.
- A3. No `.send(..).ok()` remains in fe-ui on a `DbCommand` channel. A Grep for
  `\.send\(.*\)\s*\.ok\(\)` limited to `fe-ui/src` returns only non-DB channels, and each of those is
  justified in a comment.
- A4. F7: a `ScopedQuery` for petal A on a store holding nodes in A and B returns only A's rows, even
  for `WHERE petal_id = 'B'`. The Query tab uses `ScopedQuery`, and its scope chip is honest.
- A5. OF-13a/b/c each have a mutation-checked test in `fe-sync` (a lapse-rollback restores the right
  mapping, a revoked author's refused fractal stops blocking settlement, and an authorized move is never
  silently dropped).
- A6. Each wave has a recorded advocate verdict and critic verdict under `reviews/`, and a green re-gate
  commit. A wave without both verdicts is unreviewed, not done.
- A7. Full workspace gate green at track close: `cargo check --workspace --tests`, `cargo clippy
  --workspace --all-targets -- -D warnings`, and tests for every crate touched (`fe-terrain` always with
  `--features render`).
- A8. The ledger and the board are updated, and every R2 finding id appears either in a slice's
  "closes" list or in OUT.

## Assumptions (unasked, with defaults and reversal cost)

The user said to proceed on best guesses. Each open decision has a default in plan.md §0. Two
assumptions are not decisions:

- **Lane B's file set is frozen until its commit lands.** Its working diff (at 6531d19) also touches
  `fe-ui/src/panels/mod.rs`, `fe-ui/src/plugin.rs`, `fe-ui/src/theme.rs`, `fe-runtime/src/app.rs`,
  `fe-runtime/src/lib.rs`, the fe-api core files and `fractalengine/Cargo.toml`, which is more than the
  brief listed. Waves 1 and 2 avoid all of them, and Wave 3 starts only after Lane B's commit is in
  `git log`. Reversal cost is nil, because this only changes ordering.
- **R2 line numbers are hypotheses.** Each slice re-greps its anchors before editing, and a moved line
  is not a blocker.
