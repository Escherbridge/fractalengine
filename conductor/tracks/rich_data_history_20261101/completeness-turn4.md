---
type: Review
track: rich_data_history_20261101
critic: C-DATA
round: 4
---

# C-DATA completeness — rich_data_history_20261101 (round 4)

Checked at public HEAD `733a3256` (working tree also carries uncommitted edits from other slices). No cargo, no runs: "complete" means the code and a batch-reachable test exist; live proof is listed separately.

## Spec requirements

| Requirement | Status | Evidence |
|---|---|---|
| 1. Typed nested input, invalid stays editable, confirmed persistence | complete (code) | fe-ui/src/panels/property_value.rs:16 `parse_selected` (semantic kinds, QA-0106), inspector.rs:1773 call site; confirmed-persistence arms per RD1 |
| 2. Versioned bounded descriptors enforced at the DB | complete | fe-sdk/src/property_schema.rs:57 `EnforcedPropertySchema`; fe-database/tests/entity_property_test.rs:399-421 SetFieldSchema + incompatible v2 refusal |
| 3. Atomic state + intention + node_log; per-object versions | complete | fe-database/src/handlers/node_history.rs:544 `commit_mutation`, :571 `commit_mutations_in_scope`, :208 `atomic_save` |
| 4. History reads authorize each fact; reply identities agree | complete (code) | node_history.rs:681 `require_committed_boundary`, :808 `replay`; scoped redaction per RD3 |
| 5. Scoped REST/MCP history + state-at-version; journal excluded from SQL | complete | fe-api/src/server.rs:210-218 history/state/mutations routes; fe-api/src/mcp/mod.rs:241 `get_node_history`, :243 `read_node_at_version`; fe-query/src/sql_guard.rs:1450 node_log refused |
| 6. Desktop history viewer + chart with readable table | complete (code) | fe-ui/src/object_data/history.rs:194 render; measurements.rs:240 chart, :214 accessible table |
| 7. Declarative driver with scrubbed receipts | complete (code) | tools/qa/rich_data_scenario.py; live Both scenario is blocked-user (below) |
| 8. Inspector Key/Type/Value/Actions grid, Edit/Copy/Delete | complete (code) | fe-ui/src/panels/property_grid.rs:11 `Columns`, :262 `clipboard_value`; Viewer gating (QA-0091) inspector.rs:2176 `PROPERTY_EDIT_DENIED`, :2208-2253 `can_edit` gates, :388 call site |
| 9. Tree Rename/Options/Copy ID/confirmed Delete; appearance window with Save/Cancel/Reset | complete (code) | fe-ui/src/panels/tree_entry.rs:151 Copy ID, :161 Rename/Options, :178 Delete; tree_appearance.rs:257 body ScrollArea with pinned action row (QA-0112), :362 Save |
| Selection and Petal groups extension | partial | Registry publish: fe-database/src/handlers/visibility_groups.rs:216 `publish_registry`, lib.rs:1842/1853/2105. Groups auto-read (QA-0111): editor_batch/mod.rs:626 `ReadEditorBatch`, ui.rs:480 `ready`, :518 dtos fallback, journey inspector.rs:5256-5456. Membership convergence deferred (scene_publication/AGENTS.md:120-122) |
| Completed-save playback, malformed baselines fail closed | complete (code) | node_history.rs:681; history.rs:30 `completed_version` |
| Acceptance boundaries (catalog entry + coverage passes) | partial | Census has zero unmapped/missing entries for DATA surfaces (qa/catalog-source-census.json `unmapped` / `missing_checkpoint_ids` list only extension/portal files); fingerprint null (qa/feature-checkpoints.json:4) |

## Plan tasks

| Task | Status | Evidence / class |
|---|---|---|
| Inventory, user confirmation, RD1 source, tree appearance source, hierarchy cascade, backend approval, RD5 driver | complete | ticked [x]; code cited above |
| RD1 live all-layout proof | blocked-user (scoper) | code done; live proof needs a staged build |
| Property grid three-size check (+ DATA-01) | blocked-user (scoper) | DATA-01 code complete (inspector.rs:388, :2208; property_value.rs:16); R-DATA-1 D2/D13 |
| Tree/current-state review, final menu checks (+ DATA-03) | blocked-user (scoper) | DATA-03 code complete (tree_appearance.rs:257); test dialogs/mod.rs:223 at 1280x720 and 1024x640 x2 — NOT batch-reachable (see HANDED-OFF 1) |
| RD2, RD3, Viewer Join | done (scoper) | ticked by T-DATA (commit f0aea31f) |
| RD4 materialization/preview/chart | blocked-user (scoper) | node_history.rs:382 `materialization` |
| Real Both scenario | blocked-user (scoper) | local_catalog.rs:12 `publish_petal`, :113 `reemit` |
| Catalog reconciliation (DATA-06) | partial (scoper) | DATA-06 surfaces mapped (see Acceptance boundaries) but census `audited_source_commit` is still `09923b5d`, not round-4 HEAD, and the slice's tools/qa/catalog_source_rules.json edit (F010 for editor_batch/ui.rs, CheckSelectionSave, SelectionBatch) is uncommitted in the working tree |
| Final integrated checks / CI / tag | blocked-user (scoper) | campaign-end sweep |
| Thin icons and row states | blocked-user (scoper) | tree_entry.rs, tree_appearance.rs |
| Ordered selection and routing | blocked-user (scoper) | editor_batch/** adapters |
| Group lifecycle | partial (scoper) | fe-database/src/op_log.rs:2825-2880 CreateGroup/DeleteGroup journeys; handlers/editor_batch/groups.rs |
| Verify groups + reconcile replication gap (+ DATA-02) | partial (scoper) | spec sentence refreshed by T-DATA; DATA-02 code complete (above); live paired check blocked-user |
| Publication/projection, closed-path witness, local-copy adoption, QA30 baseline | blocked-user (scoper) | code under scene_publication/*, scene_asset_* |
| Completed-save playback | partial (scoper) | source done; malformed-journal journey live run pending |

## Gate hand-offs (R-DATA-1)

- D4 (I-DB stamp override fan-out): reached — fixed by R-INT-5 (733a3256), fe-database/src/handlers/mod.rs:80 `send_stamp_override_changes`, called at lib.rs:1090 and :2171.
- D10 (I-BATCH): NOT reached — conductor/test-batches.md has no `handlers::stamp_transform` and no `dialogs::tests::` filter.
- D11 (I-DOC-CORE): reached — fe-database/src/handlers/AGENTS.md:191 `## stamp-transform`.
- D12 (I-DOC-UI): half reached — fe-ui/src/panels/AGENTS.md:718 records QA-0112 (but names the buttons "Apply and Close"; they are Save/Cancel/Reset); the fe-ui/src/gis/AGENTS.md tombstone/escaping line is absent.
- SEC-INBOX: no entry in .omc/state/turn4-security-inbox.md names a file of this track or of this critic's owns.

## §9 defaults

- Q34 (coverage passes and fresh Both scenario stay out of round 4): code and plan agree; plan.md "Status 2026-11-13" paragraph defers them to the user-run campaign sweep.
- Q35 (QA-0116/0128/0144 left to QA triage): no DATA slice touched them; consistent.

## HANDED-OFF (owner = next round)

1. HANDED-OFF (I-BATCH, next round) - conductor/test-batches.md - add `handlers::stamp_transform` to the R1 fe-database lib command (line 58) and `dialogs::tests::` to the R6 fe-ui command (line 218); R-DATA-1 D10 never landed.
2. HANDED-OFF (orchestrator / DATA-06 follow-up) - tools/qa/catalog_source_rules.json - commit the uncommitted F010 mapping, re-run `python tools/qa/catalog_sources.py --source-commit <HEAD sha> --write`, and update the sha at tools/qa/AGENTS.md:1423-1426.
3. HANDED-OFF (I-DOC-UI, next round) - fe-ui/src/gis/AGENTS.md - one line: every copied/pasted read hides tombstones and escapes literals like the Query tab (QA-0127); fix panels/AGENTS.md:718 to say Save/Cancel/Reset.
4. HANDED-OFF (T-DATA, next round) - plan.md "Verify group creation..." annotation labels publish_registry as DATA-02 (DATA-02 is the QA-0111 auto-read) and cites lib.rs:1838/1849/2101 (now :1842/:1853/:2105); no plan line records DATA-02's actual change.
5. HANDED-OFF (campaign build-and-verify phase) - every blocked-user item above: staged build, fresh Both scenario, two clean full-coverage passes, live layout and paired group checks.

Verdict: PARTIAL — no code gap in round-4 DATA scope; one unrouted batch hand-off (D10), an uncommitted catalog rules edit, and two doc lines remain; everything else is blocked-user by design.
