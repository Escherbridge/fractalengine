---
type: FocusedEvidenceSourceTrace
date: 2026-11-03
audited_stage_source: 481e942bb8150b882a30a0826ad49d89f4a00b64
execution_status: READ_ONLY_SAVED_EVIDENCE
---

# QA30 pending and terrain diagnostics

The persistent pending count has a concrete failing-read explanation. The terrain diagnostic is a separate report-snapshot availability issue; its saved reason does not establish stale terrain assignment or tile loading. No app/API, network, Cargo or test was run. Inspection used exact stage-source Git reads and retained QA30 JSON/log files; current asset-sharing UI did not exist in this stage.

## Observed evidence

All38 parsed `qa/passes/30/*state.jsonl` records containing `pending` have exactly `{ui_known:4, db:null, sync:null}` and `settled:false`. Initial `owner-started-state.jsonl`, frame3621, has no selected node and terrain reason `selection_unavailable`. The other37 records have status `stale`, reason **`scene_petal_mismatch`**. Later sampled frames include79307,84716 and final88668 (`owner-restart-group-final-state.jsonl`), still at four. These are sampled observations, not a continuous frame trace.

The retained stdout identified by `log-summary.json` has SHA256 `f98d4b316c5968287abb519cc5c2d25111aa84ddb14d2c6453b48985fcdbd3e0`,1852 lines,347 WARN and0 structured ERROR/panic markers. Safe fixed-message filtering finds **280 warnings** with the exact message suffix `private replica current read deferred: The query was not executed due to a failed transaction`:70 for each of these four real nodes:

| Node | Historical fixture lead | Warnings |
|---|---|---:|
| `01M6FDKQWF0QVZ44DRZ3H9XDYC` | QA21 asset-bearing object |70|
| `01M6FDPE00CN429W03RQWYBSCR` | QA21 route |70|
| `01M6FDWKBAE253GYS7HY998BTK` | QA21 placed asset |70|
| `01M6F3SMGP4PX18QW85M1Y9WDZ` | QA18 Portal asset |70|

First matching warning:2026-11-03T09:30:03.975340Z; last:10:05:02.284510Z. This is sustained failure over about35minutes, not one startup retry. Filtering found no `unknown` or `unconfirmed` text. Absence of those words is not proof that every accepted-operation owner was empty. No raw stdout or credentials were copied into this report.

## Exact pending aggregation and its limits

At stage481e, `fractalengine/src/qa_debug_bridge.rs:450–470` adds:

1. `PendingAssetOps.0.len()`.
2. `PendingGpxOps.0.len()`.
3. `PendingPathOps.0.len()`.
4. `VerseManager::pending_scene_operations()`.
5. Host path `pending_current_projections()` when present.
6. GPX import `pending_outcomes()` when present.
7. Join request pending flag.
8. GLTF import request pending flag.
9. Active target-Access controller pending flag.

It does **not** publish the component values. Thus four cannot be algebraically attributed from the census alone. It also does not count every possible UI cache/operation (for example the editor-batch state is not directly added there). `db:null` and `sync:null` mean queue completion is unobserved, not zero or idle. `settled` is only `!gesture_active && no_camera_capture_lock && pending==0`; its own text explicitly excludes knowledge of DB/sync and terrain completion.

`fe-ui/src/verse_manager/replica_reads.rs:182` counts pending replica reads, queued scene-refresh intents, queued current-path projections, plus at most one role-refresh/deadline flag. Existing `fractalengine/src/gpx_bridge.rs:871` counts accepted track/waypoint/property import owners. Its `:1486` counts deferred current path projections/materialization refresh plus accepted path/annotation creates/property outcomes. These accepted-write owners can intentionally persist across unknown outcomes; they are distinct from the failed pure reads seen here.

## Why the four read intents can stay pending

`ReadReplicatedNodeCurrent` dispatch at `fe-database/src/lib.rs:1773–1783` calls `scene_publication::read_current_and_need`; a failure logs the actual internal error above and returns a correlated `ReplicatedNodeCurrentRead` error. It does not perform an adoption or object mutation.

`ReplicaReads::route` (`replica_reads.rs:290–358`) removes the exact matching in-flight read. On error it increments failures, calls `retry`, and reinserts the same node. `Read::retry` (`:71–78`) uses bounded exponential delay2,4,8,16,32seconds; attempts do not evict a known node. `pump` (`:359–450`) retains the intent while the current node/epoch/navigation remains known and submits another pure read with a fresh nonce after backoff. The30second parent deadline removes an unresolved-parent intent, not these known nodes. Capacity remains bounded256, and navigation/access invalidation clears the applicable ownership. This is an intentionally retained pure-read retry policy encountering a real persistent failure; it is not, by itself, an unbounded memory leak or an unknown accepted write.

The four continuously failing nodes and exact four pending units are a strong, source-consistent explanation for the observed count. It cannot be elevated to a component census proof because the snapshots expose only the sum. Normal successful reads remove their intents. Other queue producers could theoretically contribute between saved samples; their values were not recorded.

The precise failing SQL guard is **not established** by the retained error. `scene_publication/current.rs:26–137` reads the node, obtains actual Viewer/ancestry fences, adds exact node fingerprint/geometry/clock guards, calls `asset_state`, then runs a read-fence transaction. `asset_state` at approximately:187–222 adds asset-metadata snapshot guards. The four failing objects are asset-bearing, making that branch a justified next investigation target. But the generic Surreal transaction error omits which statement/guard failed; this trace does not assert a null/NONE mismatch, authorization failure, mutation race or a specific asset guard as the root cause. The current-read file has no committed diff between481e and inspected main284ef388, so a new build alone supplies no proof this failure is fixed.

Error replies take the retry branch without setting the pump's returned failure Boolean. The user advisory at `db_results/mod.rs:990–1002` is driven by that Boolean (timeouts/disconnect/unresolved parent), so explicit repeated DB error replies can remain largely a log-only retry loop while the public pending count stays nonzero. This is a relevant observability/UX limitation; do not silence it by omitting these reads from settled accounting.

## What `scene_petal_mismatch` actually means

`selected_terrain_diagnostic` at `fractalengine/src/qa_debug_bridge.rs:250–315` first requires a selected object and a `SceneAnalysisSnapshot`. It checks `scene.petal_id == active_petal` **before** reading `ActivePetalTerrain`, its assignment, terrain config, runtime origin or heights. A failed comparison immediately returns `stale/scene_petal_mismatch`. Actual terrain mismatch uses a different reason, `terrain_petal_mismatch`, later in the function. No saved QA30 state has that later reason.

The snapshot is intentionally report-demand-driven. `fe-ui/src/scene_analysis.rs:283–288` publishes `SceneAnalysisSnapshot::default()` whenever `!report.has_selection()`. `VolumeReportState::has_selection` (`panels/volume_report_panel.rs:272`) tests the volume-report selection key, not selected NodeManager objects. The default snapshot has `petal_id:None`; it therefore fails the QA diagnostic's scene-Petal comparison even while the active scene and terrain work normally. A real report selection produces a Petal-tagged snapshot at most every0.25seconds; no report selection means this diagnostic cannot establish current selected-object geometry.

QA30's no-volume-selection manipulation workflow and37identical reasons are consistent with the intentional default/inactive scene-report snapshot. The saved state does not contain the report selection key or actual snapshot Petal ID, so defaultNone is a source-supported explanation rather than a directly measured component value. There is no basis here to infer stale terrain tiles, terrain assignment, heights or volume-report replay. The diagnostic is request-time only and never promises a historical report state.

## Bounded conclusion

Carry forward a failing-current-read investigation for the four exact nodes, with statement-specific safe transaction diagnostics or an existing strict read journey when execution is authorized. Preserve the whole asset/authority/fingerprint fences while finding the failure; do not turn a failed current read into LocalBlob, Saved or an ordinary fallback. Improve per-producer pending visibility so future observations distinguish queued reads, backoff, role work and accepted unknown mutations. Treat the terrain field as **report geometry unavailable/inactive** for this workflow until an actual volume-report selection supplies a matching scene snapshot; do not label the terrain pipeline stale from this flag alone.
