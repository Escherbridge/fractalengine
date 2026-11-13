---
type: review
track: extension_platform_20261107
gate: C-EXT
reviews: [R-ADMIN, R-GATE, R-API, R-BND, R-PLUG, R-STORE, R-GOV, R-BIN, R-UI, R-TOOL, R-POR, D-EXT, D-BATCH]
date: 2026-11-13
verdict: PARTIAL (34 complete, 3 partial, 0 missing requirements; 2 GUI write paths MISSING the scene door; 4 FIXED; 9 HANDED-OFF)
---

# C-EXT: completeness of extension_platform_20261107 at HEAD (turn 3)

Every spec requirement (R1-R37) and plan task (P0.1-P6.5) checked against the working tree on top of `8cacc6ac`,
with `file:line` evidence. Code-first turn: nothing was built or run, so "complete" means "written and wired", never
"tested"; the tests that would prove each AC are listed in `conductor/test-batches.md` (R11, R11-ADMIN/STORE/ENT, R9,
R2, R7, R8, R4-IoT) and the four `ext-*` sim scenarios, all deferred to the refine pass by user decision 2026-11-09.

## Fixed by C-EXT (owned files)

- C-EXT-01 FIXED (R-BIN-20, governance.md host-switch row) the Extensions panel had no Disable all / Enable all, so only
  `fe-ext admin` reached the host switch. New `ExtensionOp::SetHostSwitch { disabled }` (`fe-ui/src/extensions/panel.rs:172`),
  a two-step `Disable all extensions` / one-step `Enable all extensions` row under the installed list (`panel.rs:579`),
  J9 leg (`panel.rs:751`), and one shared `OpsWorker::set_host_switch` used by the panel arm and both admin verbs
  (`fractalengine/src/extension_host/ops_drain.rs:660`, `:1698`; audit `channel: gui|cli`, portal resolver told through
  `host_switch_changed`, Enable all refused `host-off` while `FE_EXT_DISABLE_ALL=1`). Names added to
  `docs/extensions/extensions-ui.md` and the GUI QA path (`fe-ui/src/extensions/AGENTS.md` §gui-qa-path step 6).
- C-EXT-02 FIXED (R-STORE-08 / R-GOV3-05, compile break under `--features entitlement`) `governance.rs:1835` called
  `encrypted_key_for(&content_key)`; now passes `&licensed.package_id` (package-bound wrap, steer 2026-11-13).
- C-EXT-03 FIXED (R-ADMIN-06) `read_package_file` opened the path before checking it: a FIFO/device path blocked the single
  ops worker. Now `metadata(path)?.is_file()` first (`ops_drain.rs:493`); the post-open check stays as the race guard.
- C-EXT-04 FIXED (R-UI3-11) `docs/extensions/extensions-ui.md`: names list gains `Load more extensions`,
  `Back to extension results`, `Reviewed <id>`; Store flows step 2 now says `Get licence` opens the licence page and
  `Check licence` requests and verifies the token (matches `fe-ui/src/extensions/details.rs:223,230`); the stale "drain is not
  written yet" line names `ops_drain.rs` `OpsWorker::dispatch`.

## Requirements

| Req | Status | Evidence |
|---|---|---|
| R1 sealed plugin package | complete | `HexonType::Plugin` `fe-format/src/manifest.rs:30`; header member `fe-format/src/integrity.rs:31`; `verify_plugin_package` `fe-hexon/src/package.rs:394`. The tamper *table* (flipped component byte / edited extension.json / extra member) is not a named journey; `package.rs:1184` covers one tamper. Extend in refine (R11-STORE). |
| R2 WIT 2.0 only contract | partial | `fe-plugin/wit/fractal-plugin-2.wit:18`; bindgen `fe-plugin/src/wasm/component/bindings.rs:7`, `hooks.rs:17`; `unsupported-contract` `component/mod.rs:48`. The WIT still opens with `UNSTABLE - not yet bound` (`fractal-plugin-2.wit:1`) though P1.1 bound it: HANDED-OFF H-6. |
| R3 manifest v2 | complete | schema + examples `fe-sdk/schema/examples/*.extension.json`; `fe-ext validate` + `check_provides` (R-TOOL). |
| R4 worker tick | complete | `ticks_skipped` `fe-plugin/src/tick.rs:238`; injected `Clock` `tick.rs:33`; `refuel_for_tick` `fe-plugin/src/wasm/mod.rs:170`. |
| R5 one batched write | complete | `PendingOp::SetTransform` staged `fe-plugin/src/wasm/component/imports.rs:102`, applied `fe-database/src/handlers/plugin_host.rs:276,491`; one `PluginCommit` arm `fe-database/src/lib.rs:786`. |
| R6 readings door | complete | `door_readings` `plugin_host.rs:665`, all-or-nothing flush per steer 2026-11-13 (R-BND); residual delete-between-check race DEFERRED (R-BND-10). |
| R7 route sampling | complete | one implementation `fe-sdk/src/path_geometry.rs:190` (`position_at_progress`), `:284` (`snap_to_polyline`). |
| R8 ingress | complete | mount `fe-api/src/extensions.rs:850,854` (petal-scoped per bindings.md, a recorded contract change); credential headers stripped host-side, journey rows `fe-plugin/src/net/tests.rs:100,146`. |
| R9 egress | complete | `capability-denied` table `fe-plugin/src/net/tests.rs:450`; shared buckets wired (R-PLUG-06 closed by R-BIN). |
| R10 UI slots | complete | `ext:<pkg>/<id>` names `fe-ui/src/extensions/slots.rs:337`; streams `bound_slots.rs:261`; ui_registry/slot_drain from X-HOSTUI (R-BIN). |
| R11 status + headless | complete | `GET /api/v1/extensions[/{id}]` `fe-api/src/extensions.rs:444,450`; MCP `list_extensions` `fe-api/src/mcp/mod.rs:325`; relay hosting `fractalengine-relay/src/main.rs:310` (feature `plugin-host`). |
| R12 local identity only | complete | REST/MCP refusal `fe-api/src/extensions.rs:911`; bridge refuses every verb `fractalengine/src/plugin_bridge.rs:3660` (`admin-gui-only`); local channel `extension_host/admin_channel.rs:164`, spawned (R-ADMIN-04 closed by R-BIN). |
| R13 host streams | complete | `host/query` re-run `fe-plugin/src/bus/broker.rs:100,665`; `host/admission` `fe-plugin/src/bus/mod.rs:61`. |
| R14 streams/commands | complete | caller kind `broker.rs:222`. |
| R15 limits + external | complete | `POST .../commands/{command}` `fe-api/src/extensions.rs:846`; MCP `extension_command` `fe-api/src/mcp/mod.rs:327`. |
| R16 opt-in admission | complete | `FE_PLUGIN_HOST_ALLOW` `fe-plugin/src/lib.rs:205`; `host-off` `fe-plugin/src/lifecycle.rs:667,670`; `FE_EXT_DISABLE_ALL` `lifecycle.rs:29`. |
| R17 signed + trust roots | complete | `fe-plugin/src/trust_roots.rs:1`; dev flag `FE_EXT_DEV` `extension_host/governance.rs:38`. |
| R18 verse policy | complete | `DbCommand::SetExtensionPolicy { actor_did }` `fe-database/src/lib.rs:798`. |
| R19 preconditions in order | partial | all nine typed `fe-plugin/src/lifecycle.rs:47-56`; but `check_activation` tests `not-local-identity` before `host-off` (`lifecycle.rs:663` vs `:666`), the reverse of spec R19 / governance.md:130. Fails closed either way: HANDED-OFF H-7. |
| R20 capability prompts | complete | digest-bound dialog `fe-ui/src/extensions/consent.rs:65,141`; `consent-mismatch` 409 `fe-api/src/extensions.rs:375`; re-prompt on delta in J9. |
| R21 audit | complete | one `OpType::ExtensionAudit` `fe-database/src/handlers/extension_binding.rs:633`; `GET .../audit` `fe-api/src/extensions.rs:460`; CLI denials durable (`plugin_bridge.rs` `audit_admin_refusal`). |
| R22 role-at-write | complete | `plugin_host.rs` uses only bound `db.query` with parameters (`:172,413,532`); no `RawQuery`/`ElevatedQuery`. |
| R23 kill-switch layers | complete | (a)/(b) panel + CLI (C-EXT-01); (c) policy; (d) `extension_host/revocation_pull.rs:45`; `host/admission` published. |
| R24 provenance, stop not withdraw | complete | scene `stamp_provenance` `plugin_host.rs:264`; readings `ext_package_id/ext_content_hash` `fe-database/src/handlers/iot_reading.rs:229`; drain `fe-plugin/src/bindings/activation.rs:80`. |
| R25 fe-ext CLI | complete | `tools/fe-ext/src/main.rs:8` (new/validate/pack/inspect/sandbox/lab/admin/index); fingerprint `sandbox.rs:108`. |
| R26 guest SDK + docs | partial | guest `tools/fe-ext/guest/src/hooks.rs`; docs incl. `migration-1-to-2.md`, `cookbook.md`, `authoring-guide.md`. The AC's doc link check (P6 docs pass) has no script in the tree: HANDED-OFF H-8. |
| R27 examples | complete | `extensions/fleet-sim`, `extensions/device-adapter` (+ `bi-push`); kinds in each `extension.json:7`. |
| R28 Extensions panel | complete | `fe-ui/src/extensions/panel.rs:537` + browse/details/permissions; GUI QA path written (`fe-ui/src/extensions/AGENTS.md` §gui-qa-path), run in refine. |
| R29 RegistrySource | complete | trait `fe-hexon/src/registry_source.rs:192`; `LocalSource` `:536`; `StaticIndexSource` `:1820`. |
| R30 fe-sim first-party | complete | pure model `fe-sim/model/src/fleet.rs:1,49`; single driver claim `extensions/fleet-sim/src/lib.rs:86`; lab `tools/fe-ext/src/lab.rs:18`. |
| R31 device adapters | complete | `extensions/device-adapter/src/lib.rs:1`. |
| R32 Hexon client behind trait | complete | `fractalengine/src/terrain_bridge.rs:923,946` (`RegistrySource::list` + `fetch_hexon`). |
| R33 BI connectors | complete | built-ins never revocable `fe-ui/src/extensions/builtin/copy_for_bi.rs:30`; bound `egress.snippet` `bound_slots.rs:217`; push connector `extensions/bi-push/src/payload.rs:100` (CSV injection fix). |
| R34 terrain aligned | complete | manifest v2 constant `fe-terrain/src/extension.rs:13`. |
| R35 sim scenarios | complete (unrun) | `tools/qa/sim/scenarios/ext-fleet.json`, `ext-device.json`, `ext-install-gui.json`, `ext-bindings.json`; admin ops via `ext` surface (`driver.py:430,521`), policy/commands/ingress via REST. Two clean runs are P6.3 (refine). |
| R36 review gates | complete | `review.md`, `review-turn2/`, `review-turn3/` (11 gates, security reviews recorded). |
| R37 scoped bindings | complete | `fe-plugin/src/bindings/resolver.rs:246`; portal branch + `default_access`/`admitted_bundles` fed (R-BND-3 closed by R-BIN). |

## GUI scene-write door census (steer 2026-11-13: list any write path outside the gate)

Routed through `submit_scene_write` / `try_submit_scene_write_on` at HEAD: undo replay and path commit
(`fe-ui/src/undo.rs:559,867`), editor batch (`editor_batch/mod.rs:756`), earthwork volumes (`actions/earthwork_volumes.rs:651`),
stamp promote (`node_manager/stamp_selection.rs:593`), stamp overrides (`actions/asset.rs:401`), node props, create, path,
callouts, terrain proposals, create-entity dialog.

- C-EXT-05 **MISSING** `fe-ui/src/dialogs/gltf_import.rs:39` sends `DbCommand::ImportGltf` (scene.create) through
  `send_or_toast` (`fe-ui/src/actions/mod.rs:1054`), a raw `try_send`: a bound `scene.create` hook never sees GUI glTF imports.
  HANDED-OFF H-1.
- C-EXT-06 **MISSING** `fe-ui/src/verse_manager/db_results/nodes.rs:71` sends the earthwork contract `SetNodeProperty` keys
  straight to `db_sender.0`. HANDED-OFF H-1.
- C-EXT-07 partial `fe-ui/src/node_manager/transform_broadcast.rs:178` still asks the gate with an `UpdateNodeTransform`
  proxy, and `UndoState::transform` now submits the checked commit through the door as well (`undo.rs:559`): a single gimbal
  move asks `scene.transform` twice (two hook calls, augments staged from the first only). The steer says drop the proxy.
  HANDED-OFF H-1.

## Plan tasks

| Task | Status | Evidence / note |
|---|---|---|
| P0.1-P0.5 contracts | complete | WIT 2.0; `docs/extensions/{manifest-v2,bus,package-format,governance,bindings,admin-channel}.md`. |
| P0.6, P1.9, P2.6, P3.5, P4.4 (review half), P5.7, P6.4 gates | complete | recorded verdicts in `review.md` and `review-turn2/`, `review-turn3/`. |
| P1.1-P1.8 runtime, commit, bus, API, bridge, slots | complete | R2, R4, R5, R13-R15, R11, R10 rows above. |
| P2.1-P2.4 package, governance, provenance, consent UI | complete | R1, R16-R24 rows (R19 order: H-7). |
| P2.5 open-core S-PLUGIN amendment | complete | `conductor/open-core.md:54` ("stops after its in-flight writes complete; rows it wrote stay"). |
| P3.1-P3.4 fe-ext, guest SDK, examples, docs | complete | R25-R27 rows (link check: H-8). |
| P4.0-P4.3 UI text, panel, RegistrySource, update flow | complete | R28, R29 rows; update flow in J9. |
| P4.4 GUI QA | partial | path written, not executed (test policy 2026-11-07; refine). |
| P5.1-P5.6 migrations | complete | R30-R34 rows. |
| P6.1-P6.2 scenarios, ops, probe | complete | R35 row. |
| P6.3 two clean runs + triage | partial | not run by user decision ("no test batches until verified complete"); refine. |
| P6.5 docs consolidation + retro | partial | docs consolidated under `docs/extensions/`; no retro section yet (plan.md:123 names it). Orchestrator writes it at turn end. |

## HANDED-OFF (named owners for R-Z)

- H-1 (fe-ui actions/dialogs/verse_manager owner, R-Z) C-EXT-05/06/07: route `gltf_import::submit_import` and the
  earthwork follow-ups through `try_submit_scene_write_on`, and drop the gimbal proxy gate in `transform_broadcast.rs`
  (keep the refusal rollback by reading the door outcome from the undo commit).
- H-2 (R-Z / orchestrator, compile break) `fractalengine-relay/src/main.rs:297` `BevyHandles { .. }` lacks
  `blob_fallback: Default::default(),` (R-PLUG-07 / R-BIN-18, still open).
- H-3 (R-Z, test compile break) `fe-hexon/tests/remote_registry_test.rs:186,225` `SourceTrust` literals lack
  `is_killed: fe_hexon::registry_source::no_kill_list(),` (R-STORE-07 / R-BIN-22, still open).
- H-4 (entitlement wiring, next turn) `BridgeOpsPorts::licence_identity` / `request_licence_token` stay on the
  `not-available` defaults (R-BIN-19): signing the token request needs the identity ed25519 key, which `main.rs` never hands
  the bridge (`ExtensionBridgeConfig` carries only `device_key`, `plugin_bridge.rs:421`). Needs main.rs to pass the identity
  key, then an `EntitlementClient` over a no-proxy, no-redirect transport writing into governance's token store.
- H-5 (blob.mirror tap owner) `ExtensionBlobMirror` (`plugin_bridge.rs:1087`) has no `MirrorQueue::after_commit` caller
  (R-BIN-17): blob.mirror bindings are inert until a blob-commit tap feeds it.
- H-6 (fe-plugin WIT owner) drop the stale `UNSTABLE - not yet bound` header (`fe-plugin/wit/fractal-plugin-2.wit:1-2`).
- H-7 (fe-plugin lifecycle owner) swap `check_activation` so `host-off` is tested before `not-local-identity`
  (`fe-plugin/src/lifecycle.rs:663-668`), or amend spec R19 / governance.md:130 to the code's order.
- H-8 (P6 docs pass) add the R26 doc link check (every code block in `docs/extensions/` points at an existing example file).
- H-9 (doc owners) `fe-ui/src/actions/AGENTS.md` has no `§scene-write-door` though `actions/node.rs:4` points at it
  (R-GATE-16); `fe-plugin/src/AGENTS.md:331` still shows `open_paid_entry(content_key, payload)` (R-STORE-19).

## KEPT

- R-ADMIN-07 (panel answers a CLI install that arrives during another review with `consent-declined`): fails closed and the
  CLI caller retries; a distinct `not-available` answer needs a reason field on `DiscardStaged` across panel, ops_drain and
  the settle path, which is larger than this critic's fix budget. Trigger: the first operator report of the misleading text.
