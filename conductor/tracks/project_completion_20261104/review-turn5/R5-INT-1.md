---
type: Review
gate: R5-INT-1
track: project_completion_20261104
reviews: [I5-RUNTIME, I5-DB, I5-MANIFEST]
date: 2026-11-14
---

# R5-INT-1: code-integration gate (fe-runtime hubs, fe-database dispatch, manifests)

Base `a85dc9ad..HEAD` plus this gate's fixes. Static review only (no cargo, per round-5 rules).

## Findings

- FIXED - fe-database/tests/node_lifecycle_test.rs:657 - INT5-13 had no dispatch-level test: the import arm's hierarchy refresh could be deleted with every test green. Added to the duplicate journey: export via `ExportPetalSnapshot`, a refused re-import answers its correlation id and is followed directly by `Pong` (no refresh), and an accepted import on a second DB thread replies `PetalSnapshotImported` then `HierarchyLoaded` holding the petal and both nodes.
- FIXED - fe-database/tests/node_lifecycle_test.rs:731 - security inbox 16 had only a handler-level row (crate_registry.rs:163); the dispatcher's `Some`/`None` routing was untested. Added a dispatch row: install in petal-a and petal-b, uninstall `Some(petal-a)` leaves `["petal-b"]`, uninstall `None` leaves none.
- FIXED - fe-database/tests/node_lifecycle_test.rs:581 - CITY5-04 asked for the CreateNode echo scale (0.01) to be asserted; I5-DB asserted only the duplicate's echo. The create is now sent inline and its `scale` asserted.
- FIXED - fe-test-harness/src/peer.rs:1118 - harness `UninstallCrate` arm used `..` and ignored `petal_id` (HEXON5-01, R5-HEXON F4 NEEDS asked for routing). It now routes `Some` to `uninstall_crate_in_petal` and `None` to `uninstall_crate`, replying `Error` on failure.
- FIXED - fe-test-harness/src/peer.rs:318 - the CreateNode arm I5-RUNTIME touched swallowed both sends with `.ok()` (anti-pattern 6); both now `warn!`.
- KEPT (pre-existing harness stubs, not this round's change) - fe-test-harness/src/peer.rs - 48 other `.ok()` sends remain in the harness.
- KEPT (by design) - fe-database/src/lib.rs:2397 - the unsolicited `HierarchyLoaded` can satisfy a pending uncorrelated API `LoadHierarchy` waiter early; the data is the same full local tree it asked for and fe-ui documents "any HierarchyLoaded satisfies every request raised before it".
- KEPT (no publish path) - fe-database/src/lib.rs:2392 - import never replicates: `import_petal_snapshot_reply` takes no `repl_tx`, the arm sends only the reply and a local `HierarchyLoaded`.
- KEPT (provenance only) - fe-database/src/handlers/petal_snapshot.rs:330 - one `ImportPetal` marker (ids only) is committed to the live op_log; imported history itself stays in `op_log_imported`. Not an owned file; flagged for R5-DOC awareness, no change.
- KEPT (GUI-only families) - fe-runtime/src/app.rs:332 - ReadPetalReadings, ReadSceneMaterials, ReadNodeMeasurements, Get/SetPetalRequirements, ClaimOrphanedVerse and the checked-stamp commands carry correlation ids but no fe-api route waits on them through PendingApiRequests, so they are correctly absent from the correlation tables (INT5-01 step 3).
- KEPT (no caller) - fe-database/src/lib.rs:2380 - the global `uninstall_crate` runs only for `petal_id: None`; no construct site in fe-hexon, fe-api, fe-ui or fractalengine sends `None` (fe-hexon/src/registry.rs:408 sends `Some`).
- HANDED-OFF (I5-BATCH) - conductor/test-batches.md:59 - new legs live in `node_lifecycle_test` (existing R-filter `--test node_lifecycle_test`); no new filter needed.
- HANDED-OFF (I5-DOC-DB) - fe-database/src/AGENTS.md - note that the import arm refreshes the hierarchy and that the refresh/no-refresh split plus per-petal uninstall routing are asserted through the DB thread in node_lifecycle_test.
- DEFERRED (superseded) - fe-api/tests/export_share_test.rs - I5-DB's NEEDS for a HierarchyLoaded assertion there is covered by the dispatch-level leg above; drop it unless R5-INT-2 wants API-level coverage.

## Seam and construct-site census (read, not compiled)

- `DbResult::NodeCreated { .., position, scale }`: every literal carries `scale` (fe-database/src/lib.rs:925, :1758; fe-runtime/src/app.rs:1154; fe-test-harness/src/peer.rs:319; fe-api/tests/mcp_integration.rs:423; fractalengine/src/gpx_bridge.rs:3405/:3408/:3423); every pattern binds `scale` or uses `..` (fe-ui/src/verse_manager/db_results/mod.rs:733 passes `*scale` to `handle_node_created`, whose ten test callers pass a scale).
- `DbCommand::UninstallCrate { hexon_uri, petal_id }`: messages.rs:1359, lib.rs:2376, peer.rs:1119, fe-hexon/src/registry.rs:408/:871/:876; scene_write.rs:693 uses `..`.
- `DbCommand::ImportPetalSnapshot { rows, correlation_id, actor }`: messages.rs:1191, lib.rs:2392, scene_write.rs:869, app.rs:1359, fe-api/src/petal_snapshot.rs:300/:343 (`actor: Some(claims.sub)`), fe-api/tests/export_share_test.rs:1909.
- Correlation: export/import commands and the three replies are in `correlation_of_command`/`_result` and their own `ReplyKind`s (app.rs:366-367, :408-410, :482-485, :551-552, :623-626); a refused import reaches only its waiter and a DB failure (`RequestFailed`) reaches it by id (app.rs:909); test app.rs:1345.

## Security review (refute pass)

- Actor stamping: fe-api stamps `claims.sub`; the handler removes any archive `imported_by` before stamping and caps it at 256 bytes (petal_snapshot.rs:604-610). Never an authorization input. Holds.
- Authorization: the import is admitted at the REST door (fe-api/src/petal_snapshot.rs:303, scene_write.rs:533 binds the carried petal id and generates create/property hook inputs). DB arm trusts the door, same as every scene write.
- Fail closed: refusals write nothing (guards THROW before writes); an unexpected failure cascades its own rows and replies `RequestFailed`, which reaches the waiter.
- Secrets: new log lines carry only error text; no tokens or keys.
- SEC-INBOX 9: STILL-FIXED - scene_write.rs:533/:719 classify the import as a scene write with per-node create and property hook inputs.
- SEC-INBOX 16: STILL-FIXED - message, dispatcher and harness carry and honour `petal_id`; handler row crate_registry.rs:163 and dispatch row node_lifecycle_test.rs:731.

## Manifests

- fe-sync/Cargo.toml:12-13 `net-report = []`, `default = ["net-report"]`; no Cargo.toml in the workspace sets `default-features = false` on fe-sync, so fractalengine and fractalengine-relay manifests stay untouched (correct).
- fe-terrain/Cargo.toml:31 `fe-format` with `features = ["entitlement"]` (tiles/entitlement.rs:4 uses it).
- fe-hexon/Cargo.toml:41 `multipart` dropped: no `reqwest::multipart` use in fe-hexon or any dependent crate (fe-api's multipart is axum's, server side).
- fe-terrain/src/lib.rs:25-26 `#[cfg(feature = "render")] mod proposal_layer_mirror;` used by terrain_plugin.rs:623 (render-gated too).

## Completeness

- HEXON5-02 COMPLETE fe-runtime/src/app.rs:366
- INT5-01 COMPLETE fe-runtime/src/app.rs:408 (step 3: readings are GUI-only, fe-ui/src/gis/readings.rs:63, correctly uncorrelated)
- INT5-12 COMPLETE fe-database/src/lib.rs:2376 (superseded by HEXON5-01's `petal_id: Option<String>`, messages.rs:1359; dispatch row node_lifecycle_test.rs:731)
- INT5-13 COMPLETE fe-database/src/lib.rs:2392 (test leg node_lifecycle_test.rs:657)
- INT5-05 COMPLETE fe-sync/Cargo.toml:12 and fe-terrain/Cargo.toml:31

## Verdict

PASS WITH FIXES: all three message-shape changes are consistent at every construct site, the snapshot correlation families are exhaustive, and the missing INT5-13 / inbox-16 dispatch tests are now in place (unrun).
