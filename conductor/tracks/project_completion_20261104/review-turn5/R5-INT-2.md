---
type: Review
gate: R5-INT-2
track: project_completion_20261104
reviews: [I5-API, I5-UI, I5-BIN, INT5-02, INT5-14]
security_review: true
---

# R5-INT-2: round-5 code integrations (fe-api, fe-ui and binary hubs)

Base `a85dc9ad`. This is an adversarial pass that tried to refute each upstream slice. No cargo was run.

## Findings

- FIXED - fe-api/src/mcp/mod.rs:305 - INT5-02's `tool!(get_network_diagnostics, ..)` row expanded to `tools::schema::get_network_diagnostics`, which does not exist in `mcp/tools.rs` (E0425, so the crate would not build). The row is now a literal `ToolSpec` with the same role, scope, seam and handler, and its schema is the new `no_arguments_schema` (mod.rs:200).
- HANDED-OFF (orchestrator, owner of fe-test-harness/Cargo.toml) - fe-test-harness/src/api.rs:304 - `spawn_with_hexon_registry` names `fe_hexon` in library code, but fe-hexon is only a `[dev-dependencies]` entry (Cargo.toml:49), so the lib would not build. Fix: add `fe-hexon = { path = "../fe-hexon" }` under `[dependencies]`.
- FIXED - fe-api/tests/mcp_integration.rs:1295 - the Viewer call leg checked confinement but not redaction. It now asserts `addrs == ["ipv4:ephemeral"]`, which pins the API-side redaction because the stub bridge ignores `call.detail`.
- FIXED - fe-api/tests/api_integration.rs:986 - the imported-history route had no test. The hexon journey's snapshot leg now checks that no token gets 401, an editor gets the existence-safe denial, and an owner gets 200 with `entries: []`. Removing the mount or lowering the Manager floor turns it red.
- FIXED - fe-ui/src/plugin.rs:891 - the two verdict systems were registered unordered, as (mirror, request). They now run as `(request, mirror).chain()`, the order settings.rs:862 tests.
- FIXED - fractalengine/src/plugin_bridge.rs:2168 - the `warn!` on a gone binding worker fired for every later signal, once per DB event. It now warns once per process, and later drops log at debug.
- KEPT (correct) - fe-api/src/lib.rs:204 - `announce_detail_opt_in()` is guarded by `std::sync::Once` (network.rs:135). Its only caller is `spawn_api_service_inner`, which covers both service entry points, so it runs once per process.
- KEPT (correct) - fe-api/src/server.rs:289 - imported-history is mounted inside the router that `auth_middleware` wraps (server.rs:553). The handler applies `manager_covers`, which fails closed on a missing or foreign petal (imported_history.rs:45), reads only `op_log_imported` filtered by petal, and binds every parameter.
- KEPT (correct) - fe-api/src/mcp/mod.rs:167 - the row is hidden, and calls answer "unknown tool", when `hosts.network_diagnostics` is None. The Viewer floor comes from the dispatcher (mod.rs `authorize`). The handler's rate gate and `collect_scoped_diagnostics` confine peers to the token's verses and redact unless Owner plus opt-in (network.rs:72).
- KEPT (correct) - fractalengine/src/main.rs:448 - the servable-verse thread is one-shot, sits outside and before `if app_settings.serve_relay` (main.rs:463), never joins the main thread, warns on a runtime or spawn failure, and runs a bound query limited to `created_by = $did`.
- KEPT (cheap) - fe-ui/src/panels/mod.rs:403 - `mark_verse_local_only_from_tree` returns early unless the dialog is EntitySettings. Otherwise it does a linear scan over the verses with no allocation (entity_access.rs:187).
- KEPT (cosmetic) - fe-api/src/lib.rs:37 - `pub mod imported_history;` is out of alphabetical order. Not worth a hub edit.
- DEFERRED (trigger: verse created after startup missing from the consent list) - fractalengine/src/main.rs:448 - the servable list is published once at startup and again only when the relay starts (relay_serve_bridge.rs:178). A verse created mid-session appears after a restart.
- DEFERRED (trigger: an App-level GardenerConsolePlugin test harness exists) - fe-ui/src/plugin.rs:891 - no test goes red if the verdict-system registration is removed. Only the systems themselves are tested (settings.rs:855).
- HANDED-OFF (I5-UI successor / UI gate) - fe-ui/src/actions/mod.rs - the INT5-09 panel-path leg was not written because `gardener_console` needs an egui context and about 20 params. The wrapper is covered at entity_access.rs:1386; the panel call is not.
- KEPT (sync side redacts first) - fe-api/src/network.rs:101 - the defensive redaction parses `SocketAddr` only, so an unparsable address string would pass through unredacted. The sync side already redacts, so this is defense in depth only (not owned, noted for R5-NET).
- SEC-INBOX 15: STILL-FIXED - fractalengine/src/main.rs:1225 - hosted verses are read with `WHERE created_by = $did`, and the new servable list uses the same filter (relay_serve_bridge.rs:73).

## Completeness

- INT5-02 COMPLETE - fe-api/src/mcp/mod.rs:148,167,305,813; fe-api/tests/mcp_integration.rs:1167,1248-1305
- INT5-14 STILL OPEN - the code and journey switch are in place (fe-test-harness/src/api.rs:215, fe-api/tests/api_integration.rs:851, local helper deleted), but the build needs fe-hexon in fe-test-harness `[dependencies]` - fe-test-harness/Cargo.toml:49
- INT5-06 COMPLETE - fe-ui/src/plugin.rs:891-899 (systems tested at fe-ui/src/settings.rs:855; registration itself is not test-guarded, DEFERRED above)
- INT5-09 STILL OPEN - the wiring is in place at fe-ui/src/panels/mod.rs:403, but the panel-path test leg is missing - fe-ui/src/actions/mod.rs (active_dialog_entity_settings_opens_and_closes)

Verdict: PASS-WITH-FIXES. One compile blocker fixed (MCP schema). One compile blocker handed off (fe-test-harness needs fe-hexon as a normal dependency). Security checks hold: auth mount, Manager floor, Viewer floor, confinement, redaction, no secrets logged.
