---
type: Design Note
track: mission_prodready_continuation_20261008
title: "F9 sim control surface — implementation map (recon 2026-10-09)"
timestamp: 2026-10-09T06:40:00Z
---

# F9 sim control surface — implementation map

Read-only recon output (sonnet explore agent, 2026-10-09) for the A20 surface:
`POST /api/v1/sim/*` + MCP sim tools driving fe-sim. Decision DEC-C7 in spec.md
ratifies the design below. File:line cites are at recon time (pre-F9-WaveB).

## REST surface

- Router: `fe-api/src/server.rs:65` `build_router()`; `public` + `authenticated`
  sub-routers merged at `server.rs:355-359`; auth middleware layered at
  `server.rs:350-353`. New endpoints: new module `fe-api/src/sim.rs`, routes in
  the authenticated block.
- Guard template for admin-ish, scope-less endpoints: `fe-api/src/hexon.rs:193-202`
  (`publish_crate`): `require_role(&claims, "owner")` then an `Option<Capability>`
  presence check that fails closed ("not configured" error string).
- Round-trip template: `fe-api/src/rest.rs:38-65` (`get_hierarchy`): build command
  → `oneshot` → `tokio::time::timeout` → 4-way match into `ApiResponse` envelope
  (`fe-api/src/types.rs:91,98,106`).

## MCP surface (pre-F13 shape)

- Flat match dispatcher: `fe-api/src/mcp.rs:296-731` `handle_tool_call`;
  `tool_definitions()` at `mcp.rs:57-222`; `tools/list` `mcp.rs:250-255`.
- Share one dispatch fn per verb between REST and MCP (precedent:
  `rest.rs::resolve_node_scope` reused from `mcp.rs:13,529`).
- `fe-api/tests/mcp_integration.rs:28-42` hardcodes `EXPECTED_TOOLS` (inventory
  check) — sim tools must extend it. F13 later replaces this dispatcher with the
  table-driven ToolSpec/ScopeRule design.

## Crate wiring (the load-bearing constraint)

- Precedent seam: `fe-api/src/lib.rs:65-68` `distributed_tx:
  Option<fe_runtime::distributed_query::DistributedQueryCallSender>` — types in
  fe-runtime (shared leaf), receiver owned by fe-sync. fe-api NEVER depends on
  fe-sync/fe-sim.
- Sim contract: new `fe-runtime/src/sim_control.rs`: `SimControlCommand`
  (`Start{script}`, `Stop`, `Status`, `Step{n}`, `InjectFault{event}`) in
  `SimControlCall{cmd, reply_tx}`; wire payloads stay `serde_json::Value`/strings
  (fe-runtime stays fe-sim-independent; `ScenarioScript::parse` happens on the
  receiver side).
- `ApiConfig`/`ApiState` gain `sim_control_tx: Option<SimControlCallSender>`;
  `ApiHarness::spawn` (`fe-test-harness/src/api.rs:88-105`) gains the `None`
  default — every new ApiState field needs a line there.
- Dependency facts: nothing depends on fe-sim (workspace `Cargo.toml:26-28`
  comment is explicit); fe-sim depends on fractalengine-test-harness as a REAL
  dependency (`fe-sim/Cargo.toml:11-15`), so pulling fe-sim into a binary drags
  the harness in — hence the default-off feature gate.
- Placement: `fractalengine-relay` only — `[features] sim-control = ["dep:fe-sim"]`,
  conditional wiring around the `ApiConfig` literal at
  `fractalengine-relay/src/main.rs:354`. GUI (`fractalengine/src/main.rs:373`)
  stays `None` permanently (process-global SCENARIO_RUN_LOCK/HLC-override vs a
  live editing session = correctness hazard).

## fe-sim backend (the real work)

- `run_scenario` (`fe-sim/src/scenario.rs:209`) is fully blocking: setup
  (`:230-377`) → merged action loop (`:422-484`) → settle/fingerprint/teardown
  (`:486-561`), under `SCENARIO_RUN_LOCK` (`clock.rs:38`, taken `scenario.rs:215-217`,
  process-global, held for the whole run).
- Required refactor: `ScenarioSession` (new `fe-sim/src/session.rs`) owning
  clock/net/peers/actions+cursor; lock taken once at `start`, released at `stop`;
  `step_one`/`step_n`, `inject_fault` (reuses the existing free fn `apply_event`,
  `scenario.rs:565-609`, which already covers PeerOffline/PeerOnline/Partition/
  Heal/SetLatency), `status`. `run_scenario` re-expressed over the session to
  avoid two drivers drifting.
- Concurrent-start MUST be rejected with a clean "session already running" error
  before touching `SCENARIO_RUN_LOCK` (guard: `Mutex<Option<Session>>` owned by
  the bridge thread that owns `sim_control_rx`) — otherwise the second start
  deadlocks on the process-global lock.

## Test idiom

- Lane 1 (full router, auth/role paths): `ApiHarness::spawn()`
  (`fe-test-harness/src/api.rs:38-117`), `h.post_json(...)`,
  `h.mint_token(scope, role)`.
- Lane 2 (channel-servicing): own the receiver + emulator task, call handlers
  directly — pattern `fe-api/tests/mcp_integration.rs:69-82` (`spawn_db_emulator`).

## CLI today

`fe-sim` bin verbs: (default) | `run <script.json>` | `print <script.json>`
(`fe-sim/src/main.rs:15-30`). No start/stop/step verbs — A20's interactive legs
land via the session + REST/MCP; the CLI stays the deterministic one-shot leg.
