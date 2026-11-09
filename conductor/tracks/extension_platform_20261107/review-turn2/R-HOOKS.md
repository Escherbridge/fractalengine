---
type: review
track: extension_platform_20261107
title: R-HOOKS — hook ABI and scene/map/import hooks review, swarm turn 2
resource: ../metadata.json
---

# R-HOOKS — hook ABI and scene, map and import hooks (turn 2)

Review-and-fix over B3 `638e84b9`, B5 `d5c81593`, B10a `5eff94ca`, B10e `37f1f8da`, F-E18 `b546aa5e`,
F-BIPUSH `311b9099` and I-BIN2 `7ae7adad`, code-first (nothing built or run). One verdict line per
finding: **FIXED** (edited here), **KEPT** (checked, left as is, reason given), **HANDED-OFF** (owner
named), **DEFERRED** (trigger named). The input-cap refusal is the 2026-11-09 02:45 steer (1).
Rationale for the binary-side fixes: `fractalengine/src/AGENTS.md` §extension-host.

## Hook ABI (`hooks.rs`, WIT)

- R-HOOKS-01 FIXED `fe-plugin/src/wasm/component/hooks.rs`: a manifest with `provides` was never checked against the component, because nothing called `check_hooks_export`. `ExtensionComponent::exports_hooks` and `ExtensionComponent::check_provides(&manifest)` now give every install and activation path the `missing-export hooks` check in one call.
- R-HOOKS-02 HANDED-OFF the callers of R-HOOKS-01. The install and activation paths must call `component.check_provides(&manifest)` after `compile`: `fractalengine/src/plugin_bridge.rs` `InstanceStager` (R-GOV), the ops-drain install (R-GOV) and `tools/fe-ext/src/validate.rs` (R-X).
- R-HOOKS-03 HANDED-OFF no instance can answer a hook call. `InstanceHandle::call_hook`, which the bridge calls, does not exist. `ComponentHost::instantiate` also keeps no raw `Instance`, so `HooksGuest` cannot bind. Owners: `fe-plugin/src/wasm/component/mod.rs` (I-PLUGIN, then R-LIFE) declares `pub mod hooks` and binds `HooksGuest::bind_if_exported`; `instance.rs` has no turn-2 owner (R-X).
- R-HOOKS-04 FIXED `hook_limits` held a third hook list beside `KNOWN_HOOKS` and the B0 catalogue. It now serves only `KNOWN_HOOKS` ids and takes each deadline from `fe_runtime::extension_binding::hook_spec`. Hooks with no catalogue deadline get explicit bounds: 5 s for service calls, 30 s for after-commit calls. Every current value is unchanged.
- R-HOOKS-05 KEPT per-call caps: `config` and `input` are each capped at 64 KiB, `body` in and out at the §6 cap, and the output must be JSON. Fuel is refilled per call from `tick.fuel`, which is at most 20 M. The clock is frozen. A trap or fuel exhaustion discards the window and is counted.
- R-HOOKS-06 KEPT the deadline is checked after the call returns, but the call itself is bounded. Fuel caps compute, and the guest cannot sleep: `wasi:clocks/monotonic-clock` is not linked. Scene reads are capped at 64 per call. A late reply is dropped and counted as a timeout.
- R-HOOKS-07 DEFERRED a timed-out or refused hook call does not unwind its own staged writes. They stay in the shared tick window and flush with the next tick. They are token-authorized, so this is a consistency gap, not an escalation. Trigger: per-call staging isolation in `state.rs` (R-LIFE scope). Epoch interruption of a host call that overruns its deadline waits on the P6 measurement (G0-C26).
- R-HOOKS-08 KEPT the WIT `hooks` interface and `extension-with-hooks` world match bindings.md §7.3 byte for byte. Plain 2.0 components stay valid, and `on-call-result` is unchanged (E5 default).
- R-HOOKS-09 FIXED `tools/fe-ext/guest/src/hooks.rs` `route`: a hook with no registered handler answered `Refused`. For `scene.*` that refuses the user's write: the guest helper failed closed. It now answers `Failed`, so the built-in serves. The guest test asserts the new mapping.

## Manifest (`manifest_v2.rs`, schema, bi-push)

- R-HOOKS-10 FIXED `KEYED_HOOKS` left out `petal.activate`, whose catalogue key is a name, so V18 refused `keys` on it as a "keyless hook". It is now listed (10 entries) and `pub` so a parity test can read it. The V-table journey gains a valid keyed `petal.activate` row.
- R-HOOKS-11 KEPT `KNOWN_HOOKS` equals the B0 served set minus `portal.open`, which web bundles bind without `provides`. Stretch hooks are V17.
- R-HOOKS-12 HANDED-OFF no test pins that parity; `fe-runtime/src/extension_binding.rs` only claims it in a comment. R-BND (owner) extends `catalogue_matches_the_documented_hook_set` with `KNOWN_HOOKS == served minus portal.open` and `KEYED_HOOKS == key_kind != None minus portal.open`.
- R-HOOKS-13 KEPT V17 and V18 match every "Needs" cell of bindings.md §6: `uses` kinds per hook, `scene.read`, `readings` + `tick`, `keys` on `asset.import`, and an egress plus a data consume on `egress.connector`. The schema patterns (`hookId`, `hookKeyPattern`, `localId`) match the Rust checks, and `web-bundle` forbids `provides`.
- R-HOOKS-14 KEPT `extensions/bi-push/extension.json` and `fe-sdk/schema/examples/bi-push.extension.json` are identical and pass V18: `uses` names the `sink` egress and the `latest` named query, and `config_schema` stays inside the args subset.

## Scene gate (`scene_hook_gate.rs`, `rest.rs`, `transform_broadcast.rs`, `main.rs` install)

- R-HOOKS-15 FIXED (steer 1) `SceneHookGate::begin` let a bound write proceed when the caller-sized hook input was over 64 KiB. It now answers `SceneVerdict::InputTooLarge`, counted as `too_large`. REST answers `413 input-too-large`, MCP and WS refuse through `gate_scene_write`'s fail-closed wildcard, and the GUI rolls the transform back. An out-of-scope key still passes untouched.
- R-HOOKS-16 FIXED `SceneCallerKind` gains `Ws`, so the `/ws` door is no longer reported as `rest` (hook input `caller.kind: "ws"`). HANDED-OFF: `fe-api/src/ws.rs` `WS_SCENE_CALLER` is still `Rest`; R-API is closed, so R-X flips it.
- R-HOOKS-17 KEPT the gate never runs on the DB thread, for `ApplyReplicatedRow` or for `PluginCommit`. Only doors call it. `bound` reads the published cache, `dispatch` hands the call to the binding worker, and nothing in fe-database references the gate.
- R-HOOKS-18 KEPT every fault fails open and is counted: a dropped call, a timeout, a decline, bad output, and an unbound or disabled gate. Only a guest refusal or an over-cap input stops a write.
- R-HOOKS-19 KEPT `property_scope`: an out-of-scope key never reaches the guest, augment ops are filtered to the scope and capped at 64, and `stage_augment` re-checks `set_property` + `tick` while the flush re-authorizes each op. `scene.create` and `scene.transform` inputs carry no properties.
- R-HOOKS-20 KEPT `main.rs` installs the process gate over `plugin_bridge::scene_hook_backend` after `install_with` and before `spawn_api_service`, and inserts it as the GUI resource.
- R-HOOKS-21 HANDED-OFF GUI door parity. Only the gimbal transform commit asks the gate. GUI `CreateNode` and `SetNodeProperty` issuers (`fe-ui/src/actions/{node,asset,dimension_callout}.rs`) write ungated. No turn-2 slice owns them; R-X records it, and the turn-3 fix is one gate in the shared send core (ledger lesson).

## `petal.map` (`petal_map.rs`, `terrain_map/mod.rs`, `terrain_bridge.rs`)

- R-HOOKS-22 FIXED `MapDecision` and `MapOutcome` derived `PartialEq` over `TerrainConfig`, which has none, so the binary would not compile. The derives are dropped and the journey matches on variants instead.
- R-HOOKS-23 FIXED the render loop waited for the guest. `BindingsMapCaller` blocked a Bevy system for up to 250 ms on `recv_timeout`, against the "never on the render loop" rule. It also failed every lazily started instance, because the 5 s catalogue deadline was cut to 250 ms. The call now starts on the binding worker (`start_map`), the built-in serves meanwhile (`awaiting`), and `PetalMapResolver::poll` applies the answer when it lands or degrades at 5 s.
- R-HOOKS-24 FIXED a binding change alone never re-ran `petal.map`. Install, binding row and role changes waited for the next DB echo. The bridge now re-reads the need every 500 ms (a cache read) and decides again on the last stored column.
- R-HOOKS-25 FIXED OQ-B6 rested on the resolver's `stored_terrain_petals` snapshot, so a stale snapshot let an inherited extension or `builtin` binding override a petal's own column. `column_wins` checks the live echo: a non-null column beats every binding not scoped to that petal.
- R-HOOKS-26 FIXED an extension's output could name tilesets this peer has not installed. That breaks "the tilesets it names must be installed locally" and lets a guest steer streamed-tileset fetches. Such an answer is now a fault, and the built-in serves.
- R-HOOKS-27 KEPT output is never written to the row: an assignment travels only as `TerrainAssignmentMsg`, `PetalMapState.terrain_json` stays the DB echo, and the stored config, `builtin` configs and extension output all pass `terrain_config_from_petal_json`.
- R-HOOKS-28 DEFERRED `PetalMapState.world_scale` (camera scale) follows the stored column, not an extension's effective map. Trigger: the Map column or bindings view starts rendering `effective_source` (R-UI, refine GUI QA).

## `asset.import` (`import_hooks.rs`, `upload.rs`)

- R-HOOKS-29 KEPT the converted bytes take `ingest_glb_with_limit` → `store_glb` → `validate_glb` with the 16 MiB cap, plus the owner quota and name checks. An unbound MIME falls through to `validate_glb` and is refused as before. A GLB-magic upload never calls the hook, and guest refusal text is stripped of control characters and capped.
- R-HOOKS-30 HANDED-OFF the seam is not wired, so the feature is inert (it fails closed): nothing implements `ImportHooks` or layers `ImportHooksHandle` onto the upload route. Owners: `plugin_bridge.rs` (R-GOV) for the implementation over the binding worker; `fe-api/src/server.rs` plus the `ApiConfig` field (R-X, since R-API is closed). `main.rs` passes it once the field exists.

## Terrain commands, binary wiring (`terrain_commands.rs`, `main.rs`, `terminal_reset`)

- R-HOOKS-31 KEPT the command mapping follows E18: `terrain-config` reveals `TerrainTools`, `gpx-import` raises the GPX dialog and then `GpxImportFile`, and `terrain-toggle` raises `ToggleTerrainVisibility`. Unknown ids are counted and ignored, and `TerrainCommandsPlugin` is registered in `main.rs`.
- R-HOOKS-32 FIXED `dispatch_terrain_commands` took, re-queued and change-marked every other package's pending commands every frame, a per-frame allocation (root anti-pattern 7). It now scans read-only and takes only the terrain package's commands. The GPX error notice used `now = 0.0` and now uses `NoticeClock`.
- R-HOOKS-33 HANDED-OFF other packages' slot commands are never dispatched; no bus drain exists. They now wait without per-frame cost, but they do pile up. Owner: `plugin_bridge.rs` (R-GOV, else R-X) drains them to `Bus` as `CallerKind::Ui`.
- R-HOOKS-34 FIXED (absorbs I-BIN's NEEDS) `main.rs` never bound `ExtensionBlobSlot`, so `ingest.decode` could not read payload files. It is now bound to the shared blob store right after `install_with`.
- R-HOOKS-35 FIXED (absorbs I-BIN's NEEDS) `terminal_reset::freeze_editor` stopped `PluginHostService` but not the extension workers, so a reset left them running. `ExtensionBindingService` and `RevocationPullService` now join the supervisor's services, and an absent or empty slot means nothing to stop (PHR-S1 rule).
