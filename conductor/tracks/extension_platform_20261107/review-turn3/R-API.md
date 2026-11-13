---
type: review
gate: R-API
track: extension_platform_20261107
reviews: [I-API]
security: true
date: 2026-11-13
---

# R-API: API gate review (turn 3)

Scope: every I-API file (fe-api central, REST/MCP/WS doors, `mcp/**`, `extensions.rs`, upload + import hooks,
dev_token, test literals, relay main). Security review run adversarially (refute door parity, admin refusal,
mount placement, ingress caps). Code-first: nothing built or tested; the refine pass compiles.

## Verdicts

- R-API-31 (HIGH, security) waypoint create bypassed `scene.property`: lat/lon/ele/waypoint/description/symbol were written after the create through the ungated `set_property_core` — FIXED: `rest.rs` `create_waypoint_core` is one atomic `CreateObject` via `node_history::create_core` (census asks `scene.create` + one `scene.property` per key); a waypoint now needs a non-empty name.
- R-API-32 (HIGH, security) waypoint move wrote lat/lon/ele through ungated `SetNodeProperty` after an admitted transform — FIXED: `move_waypoint_core` admits the transform and each `CommitNodeMutation` (`node_history::admit_mutation`) before any write, then sends (`send_admitted`); all-or-nothing at the gate.
- R-API-33 (MEDIUM) `set_property_core` (ungated legacy SetNodeProperty) stayed reachable from non-test code — FIXED: now `#[cfg(test)]`; every API property door goes through `set_property_as` / `commit_core`.
- R-API-34 (MEDIUM, parity) REST `DELETE .../properties`, waypoint create/move and `promote_instance` answered a hook refusal as 200 `ok:false` while PUT answered 409/413 — FIXED: all four use `scene_denied_response` / `write_failure_response` (uniform 409 `refused` / 413 `input-too-large`).
- R-API-35 (LOW, security) admin denial audit row embedded the caller-sized `{installation}` path segment unbounded — FIXED: `extensions.rs` `administer` keeps a 128-char control-free prefix.
- R-API-36 (LOW, oracle) admin routes parsed the body before refusing, so a malformed body (and an uncharged remote request) answered 400 instead of the refusal — FIXED: `admin_response` refuses whatever the body; the rate gate always runs first.
- R-API-37 (test) admin refusal journey omitted Disable, Disable all and Enable all — FIXED: `extensions_api_test` loop covers all 11 verbs.
- Steer 2026-11-13 (2) GPX / .hexon imports outside the door — KEPT (already fixed at 450ca722: `gpx.rs` / `format.rs` admit every planned create first; files outside this gate's partition, re-verified).
- Door parity REST/MCP/WS — KEPT (verified): create, place asset, transform, property set/delete, mutation, object, waypoint, promote and `/ws` entity/transform frames all call `mcp/scene_gate.rs::admit*` with the petal from the DB-resolved, token-covered scope (never caller args); MCP petal/node come from the dispatcher's `authorize`.
- Admin verbs over REST/MCP/WS — KEPT (verified): every `AdminVerb` route reaches `administer` → 403 `admin-gui-only` (local Owner) or uniform 404 (others); no MCP or `/ws` admin surface exists; hook bindings (`/api/v1/bindings`, MCP `set_binding`) stay API surfaces per bindings.md §REST.
- Route mounting — KEPT (verified): `plugin_host_routes` and first-party extension routes merge before `auth_middleware` in `server.rs`; an unwired host answers through `AbsentExtensionHost` (uniform not-found).
- ApiConfig literals — KEPT (verified): `import_hooks` set in relay main, api_harness_smoke, token_lifecycle_test and fractalengine main.
- R-API-26 ingress frame cap — KEPT (verified): ingress upgrade sets `max_message_size` + `max_frame_size` to 1 MiB; HTTP ingress reads at most 1 MiB + 1; credential headers stripped inbound, outbound carries only status, content-type, nosniff and CSP sandbox.
- Tracking metrics (`check_tracking_and_alert` SetNodeProperty) outside the door — KEPT: host-derived from an admitted transform, not caller input; documented in fe-api/AGENTS.md §scene-gate-doors.
- dev_token shared client DID — HANDED-OFF (I-MAIN / R-BIN): `spawn_dev_token_service_as` exists but `fractalengine/src/main.rs:809` still calls `spawn_dev_token_service` (fresh DID).
- Door-parity integration journey (a refusing installed gate driven through REST, MCP and `/ws`) — DEFERRED (trigger: refine pass, needs a router + DB harness with `SceneHookGate::install`; batch R9).
- GPX import admits up to 10,000 creates serially, each up to its hook deadline — DEFERRED (trigger: a bound `scene.create` hook on a petal used for GPX; batch verdicts in `scene_write`, X-GATE's file).
