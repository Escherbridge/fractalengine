---
type: review
track: extension_platform_20261107
title: R-API — route mounting security review, swarm turn 2
resource: ../metadata.json
---

# R-API — route mounting (turn 2, over I-API `a21f357d`, with B10a `5eff94ca` for door parity)

Security review-and-fix, code-first (nothing built or run). One verdict line per finding:
**FIXED** (edited here), **KEPT** (checked, left as is, reason given), **HANDED-OFF** (owner named),
**DEFERRED** (trigger named). Refutation targets come from the slice task; door parity is the
2026-11-09 02:45 steer (2). Rationale for every fix: `fe-api/AGENTS.md` §extensions, §bindings,
§scene-gate-doors.

## Routes outside the auth layer

- R-API-01 KEPT `fe-api/src/server.rs` `build_router_with_hosts`: `extensions::routes`, `bindings::routes` and `bound_routes::router` merge into `authenticated` before `.layer(auth_middleware)`, so every extension, binding and alias path needs a bearer token; `/mcp` is inside the same layer.
- R-API-02 KEPT `/ws` stays public by design (it authenticates after the upgrade); `bus_subscribe` is only read after `auth_ok`, and every bus wake re-checks `credential_still_current` before writing events.
- R-API-03 KEPT `server::ApiHosts` is an `Extension` layered over the whole router, so `/mcp` and `/ws` always extract it; `build_router` (harness, `canonical_ws`) passes `ApiHosts::default()`.

## An absent host answering differently

- R-API-04 FIXED `server.rs` `plugin_host_routes` + `absent_hosts`: with `extension_host` or `binding_host` `None`, the routes were not mounted, so a relay answered axum's bare 404 (no JSON body) where a GUI host answered the uniform not-found JSON, which told any caller whether the node runs a plugin host. The routes now always mount; a not-found stand-in serves an absent host, including the policy read the trait default answered with 503 `not-available`.
- R-API-05 KEPT MCP with an absent host answers `unknown tool: <name>` (the tool is hidden from `tools/list`): it is the crate's rule for every unwired seam (sim, screenshot, input), pinned by `tools_list_inventory`, and names no resource.
- R-API-06 KEPT the stand-in admits every remote rate request, so a flood that a wired host would 429 never 429s on a relay; it tells a caller only that no plugin host runs, never whether an id exists.

## WS extension-stream denials

- R-API-07 FIXED `fe-api/src/ws.rs` `bus_subscribe`: every 404 from `subscribe` was coded `not_found`, but its text was the host's `ExtError.message`, so a host `NotAvailable(detail)` (404 `not-available`) carried detail that an unknown package did not. Every 404 now answers `not_found` + `NOT_FOUND_OR_DENIED`.
- R-API-08 FIXED `ws.rs` `bus_subscribe`: `params` reached `client_subscribe` unbounded; it now has the command-args ceiling (16 KiB, bus.md §6), checked only after the petal is authorized so the size error is no oracle.
- R-API-09 FIXED `ws.rs` `ws_handler`: the socket took axum's 64 MiB message / 16 MiB frame defaults, so one `bus_subscribe` or entity command could buffer 64 MiB before any check. Both are now 2 MiB, axum's default REST body limit for the same commands.
- R-API-10 KEPT `bus_unsubscribe` of an unknown `sub` is silently ignored (`ExtensionStreams::unsubscribe`), so it cannot probe other connections' ids.
- R-API-11 HANDED-OFF I-BIN (`plugin_bridge.rs`, the `ExtensionHost` implementer): `ws.rs` waits on `Notify::notified()` created each loop turn, so the host must wake clients with `notify_one` (stores a permit); `notify_waiters` would lose a wake between turns and delay events until the next one.

## MCP tools and petal scope

- R-API-12 FIXED `fe-api/src/mcp/mod.rs` + `tools.rs`: `list_bindings`, `set_binding` and `clear_binding` were `ScopeRule::None` and resolved their scope inside the handler, outside the one `authorize` gate (FR-5). New `ScopeRule::BindingTarget`: the dispatcher parses `verse_id`/`fractal_id`/`petal_id` (ULIDs) and resolves them with the REST routes' `bindings::authorized_target_scope`; the grep test now bans `authorized_target_scope` and `authorized_petal_scope` from `tools.rs`.
- R-API-13 KEPT `extension_command` is `PetalArg("petal_id")`: the dispatcher resolves and covers the petal before the core runs, and the core reads the same validated arg.
- R-API-14 KEPT `list_extensions` is `ScopeRule::None` with a Viewer floor: the core answers the uniform not-found to anyone but the node DID under an Owner token, matching `GET /api/v1/extensions`.
- R-API-15 KEPT the dispatcher resolves a binding tool's scope before the core's remote-rate `gate`: one DB read per call under `/mcp`'s concurrency limit, and the answer is the uniform not-found either way.
- R-API-16 FIXED `mcp/mod.rs` `table_rows_are_well_formed`: it still asserted 49 tools (now 54), and `list_extensions` failed its "description states the role" rule; both corrected, and the binding rows are pinned to `BindingTarget`.
- R-API-17 FIXED `fe-api/tests/mcp_integration.rs` `every_scoped_tool_rejects_a_foreign_scope_token`: it walked every table row with no hosts wired, so `extension_command` answered "unknown tool" and the journey went red. Both RBAC journeys now wire a recording plugin host; the foreign-scope count is 37, and no denied call may reach the host.

## Scene-hook door parity (steer 02:45, (2))

- R-API-18 FIXED `mcp/tools.rs`: `create_node`, `place_asset` (scene.create), `update_transform` (scene.transform, before the optimistic broadcast), `set_property` and `commit_node_mutation` `property_set` / `full_transform` now ask `rest::scene_gate_verdict` with `SceneCallerKind::Mcp` and stage augments after the write, as REST does (`mcp/scene_gate.rs`).
- R-API-19 FIXED `ws.rs`: `entity_command` `create_node` / `set_node_property` and every `transform_update` now ask the gate (transform before the optimistic broadcast); a WS transform's augment stages after its debounced `TransformPersist` is sent, and a superseded one is dropped.
- R-API-20 HANDED-OFF R-HOOKS (`fe-runtime/src/scene_hook_gate.rs`, `fe-plugin/wit/**`): `SceneCallerKind` has no `Ws` and WIT `caller-kind` has no `ws`, so `/ws` reports `Rest` (`ws.rs` `WS_SCENE_CALLER`). If the steer's own `Ws` kind is wanted, add it in both, then change that one constant.
- R-API-21 KEPT the MCP/WS wrapper treats any non-`Proceed` verdict as a refusal, so R-HOOKS' over-cap `input-too-large` (steer (1)) fails closed on these doors whichever shape R-HOOKS gives it.
- R-API-22 HANDED-OFF orchestrator (`fe-api/src/node_history.rs`, unowned) and R-HOOKS (`fe-api/src/rest.rs`, waypoint/GPX cores; `fe-api/src/endpoint.rs` promote): REST `POST /nodes/{id}/mutations`, `POST /petals/{p}/objects`, waypoint create/move, GPX import and `promote_instance` (REST and MCP) still write nodes without the gate. Gate inside those shared cores; MCP `commit_node_mutation` is already gated here.
- R-API-23 DEFERRED a WS transform stream under a bound `scene.transform` waits up to 250 ms per message (REST parity, not the GUI's commit-only rule in bindings.md §6). Trigger: a client or sim run that streams under a bound hook; the fix would be judging only the debounced persist plus a rollback frame.

## Body limits

- R-API-24 FIXED `server.rs` `plugin_host_routes`: the extension and alias routers now carry `DefaultBodyLimit(MAX_INGRESS_BODY_BYTES)` (1 MiB; the admin JSON bodies took axum's 2 MB default) and the binding routes `DefaultBodyLimit(MAX_BINDING_BODY_BYTES)` (64 KiB, which `parse_body` only checked after buffering 2 MB).
- R-API-25 KEPT ingress (`extensions.rs`) and the alias (`bound_routes.rs`) read the raw body through `to_bytes(body, MAX_INGRESS_BODY_BYTES + 1)`, so they are bounded whatever the mount says.
- R-API-26 HANDED-OFF R-NET (`fe-api/src/extensions.rs` `mount_ingress`): the ingress websocket sets `max_message_size(MAX_INGRESS_BODY_BYTES)` but keeps the 16 MiB `max_frame_size` default, so one frame buffers 16 MiB before the message cap; set `max_frame_size` to the same ceiling.

## Relay main and harness literals

- R-API-27 FIXED `fe-api/tests/token_lifecycle_test.rs` `owned_api_config`: I-API put `extension_host: None, binding_host: None` in the function's PARAMETER list, which does not parse; the two fields now sit in the `ApiConfig` literal.
- R-API-28 KEPT `fractalengine-relay/src/main.rs` and `fe-api/tests/api_harness_smoke.rs` set both fields to `None`.
- R-API-29 HANDED-OFF I-BIN2 (`fractalengine/src/main.rs`, pinned seam): the GUI `ApiConfig` literal (~line 728) still lacks `extension_host` and `binding_host`, which fails to compile until I-BIN2 sets both from the plugin bridge.
- R-API-30 KEPT `fe-api/Cargo.toml`: `serde_json` and `chrono` are already direct dependencies (E15); no new dependency was needed.
