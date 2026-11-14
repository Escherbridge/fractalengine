---
type: Review
gate: R5-INT-2
track: project_completion_20261104
reviews: [I5-API, I5-UI, I5-BIN, INT5-02, INT5-14]
security_review: true
---

# R5-INT-2: round-5 code integrations (fe-api, fe-ui and binary hubs)

Base `a85dc9ad`. Two adversarial passes tried to refute each upstream slice; the second pass re-checked everything at HEAD `1dda94d7` after the NodeCreated-scale and UninstallCrate-petal seams landed. No cargo was run.

## Findings (second pass, at HEAD)

- FIXED - fe-api/src/lib.rs:67, fe-test-harness/src/api.rs:306 - INT5-14's compile blocker was still open: the harness lib named `fe_hexon`, which is only a dev-dependency of fe-test-harness (Cargo.toml:49), and the first-pass Cargo NEEDS was never applied. fe-api, which already exposes the fe-hexon type in `ApiState.hexon_registry`, now re-exports `fe_hexon`, and the harness builds the registry through `fe_api::fe_hexon`. No manifest edit is needed.
- KEPT (seam form wins) - fe-api/src/mcp/mod.rs:167 - NET5-05's NEEDS asked for "tools/call with network_diagnostics None returns 'network diagnostics unavailable'". The planner chose INT5-02's form instead: without the seam the tool is hidden and answers "unknown tool". The leg at mcp_integration.rs:1302 asserts that.
- KEPT (rejected as tautology) - fe-ui/src/actions/mod.rs - I5-UI's NEEDS line (build a VerseManager, call `mark_verse_local_only_from_tree`, assert the field) only re-tests the wrapper, which entity_access.rs:1386 already covers. It also reads a private field (entity_access.rs:44), and it would stay green if the panels/mod.rs:403 call were removed.
- HANDED-OFF (UI5-01 successor, owner of fe-ui/src/dialogs/entity_access.rs) - fe-ui/src/actions/mod.rs - INT5-09 needs a panel-path test leg. Recipe: (1) add `pub(crate) fn verse_local_only(&self) -> bool` on `EntityAccessState`; (2) the I5-UI owner moves the panels/mod.rs:402-405 closure body into `pub(crate) fn render_entity_settings_frame(ctx, ui_mgr, hierarchy, db_tx, local_role, known_peers, api_endpoint)`; (3) extend `active_dialog_entity_settings_opens_and_closes` to run one headless egui frame per namespace (None, then Some) and assert the getter.
- HANDED-OFF (R5-NET, owner of fractalengine/src/relay_serve_bridge.rs) - fractalengine/src/relay_serve_bridge.rs:179 - the startup one-shot (main.rs:449) and the relay gate both call `record_relay_servable_verses`, and nothing orders them. If the startup DB read is slow, it overwrites the gate's served list with every owned verse; if the gate reads names first, they come back empty. The impact is display only (the served set stays the owned set, SEC-INBOX 15), so this is low severity.
- KEPT (correct) - fe-api/tests/mcp_integration.rs:423 - the `DbResult::NodeCreated` emulator literal carries `scale: [1.0; 3]` (pinned seam, CITY5-04). No owned file builds `UninstallCrate` or `ImportPetalSnapshot`.
- KEPT (correct, re-verified) - fe-api/src/mcp/mod.rs:312 - the literal `ToolSpec` matches the struct fields (mod.rs:118). A Viewer floor applies through `authorize` ("insufficient permissions", mod.rs:531). A hidden tool is unknown before authorization (mod.rs:464). The handler rate-limits, then confines and redacts through `collect_scoped_diagnostics` (tools.rs:1907). The leg at mcp_integration.rs:1294 pins `203.0.113.7:51820` -> `ipv4:ephemeral` (net_diagnostics.rs:62).
- KEPT (correct, re-verified) - fe-api/src/server.rs:289 - imported-history is behind `auth_middleware` and has a Manager+ floor, which returns the existence-safe 404 (petal_snapshot.rs:45). It reads only `op_log_imported`, bound per petal (imported_history.rs:22). The table is defined in schema.rs:215, so the owner leg's `entries: []` is reachable.
- KEPT (correct) - fe-api/src/lib.rs:206 - `announce_detail_opt_in` uses a `Once` guard (network.rs:135). `run_server` is private and reached only from `spawn_api_service_inner`, so the announcement happens once per process.
- KEPT (correct) - fractalengine/src/main.rs:449-460 - the servable-verse thread is a named one-shot. It sits before and outside `if app_settings.serve_relay` (main.rs:463) and is never joined. It warns if the runtime or thread fails to start. The signature matches `publish_servable_verses(&Db, String)` (relay_serve_bridge.rs:70), and the query binds `$did`.
- KEPT (correct) - fe-ui/src/plugin.rs:891-899 - `(request_self_test_verdict, mirror_self_test_verdict).chain()`. Both take `Option<Res<..>>` for the fe-sync resources. The request uses `try_send`, rate-limited to one per interval, so the frame never blocks. Only main.rs:994 adds the plugin (Time present).
- KEPT (cheap) - fe-ui/src/panels/mod.rs:403 - the wrapper exits early unless the dialog is EntitySettings, then does a linear scan with no allocation.
- KEPT (correct) - fractalengine/src/plugin_bridge.rs:2168 - a gone worker warns once per process and logs at debug after that (`AtomicBool` and `Ordering` are imported at :7).

## Findings (first pass, carried)

- FIXED - fe-api/src/mcp/mod.rs:200,312 - the `tool!` row expanded to a missing `tools::schema::get_network_diagnostics`. It is now a literal `ToolSpec` with `no_arguments_schema`.
- FIXED - fe-api/tests/mcp_integration.rs:1294 - the Viewer leg now also asserts API-side redaction.
- FIXED - fe-api/tests/api_integration.rs:986 - added an imported-history leg: tokenless 401, editor denial, owner 200.
- FIXED - fe-ui/src/plugin.rs:891 - the verdict systems are now ordered (request, then mirror).
- FIXED - fractalengine/src/plugin_bridge.rs:2168 - the gone-worker warn fires once per process.
- KEPT (cosmetic) - fe-api/src/lib.rs:37 - `pub mod imported_history;` is out of alphabetical order.
- DEFERRED (trigger: a verse created after startup is missing from the consent list) - fractalengine/src/main.rs:449 - the list is published at startup and when the relay starts.
- DEFERRED (trigger: an App-level GardenerConsolePlugin test harness exists) - fe-ui/src/plugin.rs:891 - no test guards the registration itself.
- KEPT (defense in depth) - fe-api/src/network.rs:101 - an unparsable address string skips API-side redaction. The sync side redacts first (R5-NET).

## Security review (fresh, prompted to refute)

- Authn/authz: imported-history is behind `auth_middleware` with a Manager+ floor and fails closed when the petal is missing or foreign. MCP diagnostics has the Viewer floor from the dispatcher and is hidden plus unknown without the seam. Nothing new bypasses a gate.
- Trust boundaries: diagnostics peers are confined to verses the token reads at Viewer+. Addresses are redacted unless the operator opted in and the caller is Owner. Imported history never reads live `op_log`.
- Fail closed: a missing reader returns 503, a missing bridge returns unknown tool, and a failed servable-list read leaves the consent list empty (warns).
- Secrets: nothing new logs a token, DID secret or address. The opt-in announcement carries only the gate text.
- SEC-INBOX 15: STILL-FIXED - fractalengine/src/main.rs:1225 (`WHERE created_by = $did`); the servable list uses the same filter (relay_serve_bridge.rs:73).

## Completeness

- INT5-02 COMPLETE - fe-api/src/mcp/mod.rs:148,167,312,813; fe-api/tests/mcp_integration.rs:1167,1248-1305
- INT5-14 COMPLETE - fe-test-harness/src/api.rs:215,306 (through fe-api/src/lib.rs:67); fe-api/tests/api_integration.rs:851 (local helper gone)
- INT5-06 COMPLETE - fe-ui/src/plugin.rs:891-899 (systems tested at fe-ui/src/settings.rs:855; the registration itself has no test guard, DEFERRED)
- INT5-09 STILL OPEN - the panel-path test leg is missing (wiring present at fe-ui/src/panels/mod.rs:403) - fe-ui/src/actions/mod.rs (active_dialog_entity_settings_opens_and_closes); recipe HANDED-OFF above

Verdict: PASS-WITH-FIXES. The last compile blocker (INT5-14's fe_hexon) is fixed inside the owned files. Security holds. One test leg (INT5-09) and one display-only race (relay servable list) are handed off.
