---
type: Review
slice: R5-DOC
track: project_completion_20261104
---

# R5-DOC review: round-5 doc integrations and batch registry

Scope: `git diff a85dc9ad` over the 28 owned AGENTS.md files, `fe-ui/src/gis/query.rs` and `conductor/test-batches.md`.

## Method

- Every added path reference (125) resolves to a tracked file; the four misses (`entries.json`, `license.json`, `plugin/manifest.json`, `snapshot/meta.json`) are archive member names, not repo files.
- Every added backticked symbol (228) exists at HEAD in a tracked source file (`egui_plot` is a crate name).
- About 30 `file:line` citations spot-checked against source (snapshot.rs:374, registry_source.rs:1869, package.rs:70/88/138/145, handlers/mod.rs:38, profile.rs:4/90, volume_report.rs:86, road_chain.rs:176/182/344/447, composite.rs:131/166, ipc.rs:382, server.rs:412, share.rs:29/308): all land on the named item (road_chain.rs:176/182 are the doc comment and `fn` line one above the body).
- Leak scan of added lines for the banned words and `cargo test --workspace`: no hits (the only workspace mention in `test-batches.md` is the existing prohibition).

## Findings

1. FIXED - fe-ui/src/actions/AGENTS.md:618-623 - unresolved "TODO: verify" about a retried dead letter. Verified against `undo.rs` (`RequestFailed` arm matches `p.request_id`, reconciles and clears pending) and `panels/unsaved_edits.rs:116` (toast only); the paragraph now states the fact.
2. FIXED - fe-api/AGENTS.md:967 - unresolved "TODO: confirm how share links carry the cutoff". Share links do not: `SharePayload` stores only SQL (`share.rs:29`), redemption calls `prepare_export` with `as_of_ms = None` (`share.rs:308`, `export.rs:147`). Text now says so.
3. KEPT (pre-existing, not added this round) - fe-terrain/src/AGENTS.md:1526 and :1537 - duplicate heading "Changed terrain space and placed objects" already present at a85dc9ad.
4. KEPT (reason: stated in the registry) - conductor/test-batches.md:87 - `handlers::editor_batch::` has no test module at HEAD and reaches nothing; the comment says so, and it will pick up tests when they land.
5. KEPT (harmless) - conductor/test-batches.md:224 - the R6 line lists both `actions::tests::active_dialog_entity_settings` and the wider `actions::tests::`; same line, so each test is still reached by exactly one batch line.
6. KEPT (valid) - every added filter names a module that exists (`terrain::tests`, `gpx::export_tree_tests`, `assets`/`import_hooks`/`input`/`portal_bundles`/`timeseries_query` `tests`, `fe-runtime input::tests`, `input_bridge::`, `namespace_id_tests`, `join_verse_dispatch_tests`, `subdir_tests`, `migration_tests`, `actions::tests`, `dialogs::tests`, `panels::tests`, `native_accessibility::` journey modules, `fleet-terrain.json` scenario). No filter is registered in two batches.
7. KEPT - `fe-ui/src/gis/query.rs:15` comment edit is a doc-comment only, no behaviour change; no rationale moved back into code.

## Completeness

- CITY5-06 PARTIAL - road-chain section COMPLETE fe-ui/src/node_manager/AGENTS.md:276; scale range rationale COMPLETE fe-format/src/AGENTS.md:176 area; spec amendments STILL OPEN - folded into T5-CITY (conductor files, not owned here) - conductor/tracks/road_builder_ux_20260716.
- DATA5-02 COMPLETE fe-ui/src/editor_batch/AGENTS.md:201, fe-ui/src/gis/AGENTS.md:84, conductor/test-batches.md:87.
- EXT5-01 COMPLETE tools/qa/portal/AGENTS.md:49, fe-webview/src/portal_bundle/AGENTS.md:347 (`## §qa-rows`).
- HEXON5-10 COMPLETE fe-format/src/AGENTS.md:423 (petal-snapshot), :210 (verify_strict), fe-hexon-registry/AGENTS.md:171.
- HEXON5-11 COMPLETE fe-hexon/src/AGENTS.md:247 (progress), :265 (wiring/entitlement), :294 (install gate), §petal-snapshot.
- HEXON5-12 STILL OPEN - conductor bookkeeping (tick plan tasks, decisions) is folded into T5-HEXON / I5-CONDUCTOR; no owned file carries it - conductor/tracks/hexon_unification_20260716.
- INT5-10 COMPLETE fe-ui/src/AGENTS.md:741, fe-ui/src/panels/AGENTS.md:1067, fe-ui/src/dialogs/AGENTS.md (Access record, Presets).
- INT5-15 COMPLETE conductor/test-batches.md:87, :328, :370 (every filter verified above).
- INT5-16 COMPLETE fe-sync/src/AGENTS.md:2497, fractalengine/src/AGENTS.md:1103, fractalengine-relay/AGENTS.md:5, fe-database/src/AGENTS.md:507.
- NET5-09 COMPLETE fe-runtime/src/AGENTS.md:377, fe-api/AGENTS.md:2474, fe-sync/src/AGENTS.md:2497.
- NEW5-01 COMPLETE fe-hexon/src/AGENTS.md:247, fractalengine/src/AGENTS.md:767 (`install_from_registry` with `fetch_hexon_with_progress`, registry_source.rs:1969).
- NEW5-02 COMPLETE fe-ui/src/actions/AGENTS.md:585 (`§scene-write-door`), retry/dead-letter paragraph now verified (finding 1).
- UI5-07 COMPLETE fe-ui/src/AGENTS.md:741, fe-database/src/AGENTS.md:1035, fe-ui/src/panels/AGENTS.md:1067; ledger lines belong to I5-CONDUCTOR.

## Verdict

PASS-WITH-FIXES: two unresolved TODOs in added doc text were resolved against code; the batch registry's filters all reach real modules; CITY5-06 spec amendments and HEXON5-12 bookkeeping are STILL OPEN by design (handed off to T5-CITY / T5-HEXON / I5-CONDUCTOR).
