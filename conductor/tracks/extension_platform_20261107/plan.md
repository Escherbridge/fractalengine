---
type: plan
track: extension_platform_20261107
title: "Implementation Plan: extension platform"
created: 2026-11-07
resource: ./spec.md
computed_at_commit: cfd7fce6
partitions_confidence: planned
---

# Implementation plan: extension_platform_20261107

> Citations computed at `cfd7fce6`. Re-grep line numbers before editing; only symbol names are stable.
>
> **Build gate:** `.omc/state/build-gate.json` (`cargo_allowed=false` at authoring, D-X2).
> - **AUTHOR-ONLY** tasks are allowed now: specs, WIT/schema text, docs, AGENTS.md sections, journeys as text, scenario JSON, CLI/UI designs.
> - **BUILD** tasks wait for "gate open" on the bus, then follow `.omc/state/bus/SESSION-PROTOCOL.md` item 1: post `building`, run `cargo check -p` per slice, and run one serial gate per wave.

## 0. Assumptions (proceed unless overridden; reversal cost in brackets)

- **A-1** plugin_host_revival_20261031 runs in the EXT-PLATFORM session as this track's prerequisite lane. Its spec and plan stay authoritative for S0-S7. [low]
- **A-2** The spec §8 defaults (OD-E1..OD-E10) hold until ratified. P2.3 (withdrawal) waits for OD-E1. [per OD]
- **A-3** The release pipeline (release_ci_20260429) builds and signs first-party Wasm packages; dev builds trust a dev root. [low]
- **A-4** BUILD tasks start only after the P2P campaign merges and flips the gate. Partitions that touch fe-database, fe-runtime or fe-sync are re-verified then; HEAD moved twice during authoring. [low]
- **A-5** Python simulator code (`tools/qa/sim/*.py`) counts as BUILD, because it is not on the protocol's author-only list. Scenario JSON and schema text are AUTHOR-ONLY. [low]
- **A-6** Guests target `wasm32-wasip2`, with component-model bindgen on both sides. The wasmtime version is whatever plugin_host_revival S0 picks. [medium]
- **A-7** Priority is P1, because P0 belongs to the P2P campaign. [low]

## 1. Doctrine

- **Agents sketch, the session inks.** No agent runs cargo.
  - Per slice: `cargo check -p <crate>`.
  - Per wave: one serial clippy/test gate over the touched batches in `conductor/test-batches.md` — R11 (plugin/hexon/format), the R4 IoT sub-batch (fe-sim, ingest tests), R9 (API/MCP), R8 (sync, for P2.3) and R6/Misc (fe-ui shell wiring).
  - The full sweep runs once, at track end.
- **Tests are swapped or extended, not added.** Every task names the journey it extends (**E**) or swaps (**S**). New journey files (**N**) exist only for new public seams: `fe-api/tests/extensions_api_test.rs`, one bus journey, one slot journey, one panel journey, one registry-source journey, one CLI journey, and the two simulator scenarios.
- **Guests hold nothing.** Every effect is host-brokered and declared (spec §5).
- **One flush per instance per tick (R5).** A diff that adds a per-sample DB command is a bug.
- **Write as the local identity, never above it.** The extension path has no ElevatedQuery, no RawQuery and no caller SQL, and only the local DID may administer extensions (R12).
- **Leak discipline (public repo).** No business logic, accounts or catalogue operations. Registry access goes only through S-REGISTRY / S-ENTITLEMENT. Run the staged-diff leak grep before every commit (SESSION-PROTOCOL item 4).
- **Docs.** Terse doc-comments in code; rationale in the directory AGENTS.md. New directories get their own: `fe-plugin/src/wasm/AGENTS.md`, `fe-plugin/src/bus/AGENTS.md`, `fe-plugin/src/net/AGENTS.md`, `fe-ui/src/extensions/AGENTS.md` and `tools/fe-ext/AGENTS.md`.
- **Reviews.** Every phase closes with ADVOCATE, then CRITIC: each in a fresh context, each fixing its own findings, with one verdict line per finding in `review.md`. `/security-review` is mandatory at P1, P2 and P4; `/code-review high` at P1 and P2.

## 2. Phase / task table

Journey legend: **E** extends an existing journey · **S** swaps an obsolete one · **N** is one new journey for a new public seam. Slice ids match `metadata.json` → `partitions`. The review gates G0..G6 each append to `review.md`.

### P0 — Contract: WIT 2.0, manifest v2, integration bus (D-X6), governance text (all AUTHOR-ONLY)

| Task | Slice | Flag | Model | What | Files | Journey | Depends |
|---|---|---|---|---|---|---|---|
| P0.1 | E0 | AUTHOR-ONLY | opus | WIT `fractalengine:plugin@2.0.0`, world `extension` (spec §6.9), split per interface; `UNSTABLE — not yet bound` header until P1.1 | `fe-plugin/wit/hexon-plugin.wit`, `fe-plugin/wit/*.wit` (new) | — text; bound by P1.1's swapped journey | — |
| P0.2 | E0 | AUTHOR-ONLY | opus | Manifest v2 JSON Schema + field reference; R3's validation table written as the journey text P1.3 implements (J2) | `fe-sdk/schema/extension-manifest-v2.schema.json` (new), `docs/extensions/manifest-v2.md` (new) | — text (J2) | P0.1 |
| P0.3 | E0 | AUTHOR-ONLY | opus | **Integration bus contract (D-X6, P0 deliverable):** addressing; the three host streams and their source handlers; expose/consume; command floors; delivery and drop rules; limits; audit; REST/WS/MCP/portal-IPC mapping. This is the contract portal_surface_20261107 consumes. | `docs/extensions/bus.md` (new), `fe-plugin/wit/bus.wit` | — text (J4) | P0.1 |
| P0.4 | E0 | AUTHOR-ONLY | sonnet | Package format: `HexonType::Plugin` members, the `plugin/extension.json` header member, the `content_hash` definition, signature + counter-signatures | `docs/extensions/package-format.md` (new) | — text (J5) | P0.1 |
| P0.5 | E0 | AUTHOR-ONLY | opus | Governance contract: policy row schema and modes; precondition order and refusal codes; consent record + capability digest; audit `OpType`; kill-switch layers; provenance stamp. Also the S-PLUGIN amendment text for OD-E1, handed to the orchestrator and not applied. | `docs/extensions/governance.md` (new) | — text (J6, J7) | P0.2 |
| P0.6 | G0 | AUTHOR-ONLY | opus | ADVOCATE then CRITIC over P0, against plugin_host_revival, hexon_entitlements and the portal track's needs; fix own findings | `conductor/tracks/extension_platform_20261107/review.md` (new) + fix-only edits to E0 files | — | P0.1-P0.5 |

### P1 — Runtime: tick on the worker, write window, host imports, batched motion path, bus broker (D-X6)

| Task | Slice | Flag | Model | What | Files | Journey | Depends |
|---|---|---|---|---|---|---|---|
| P1.1 | E1 | BUILD | opus | Component host: bindgen for the 2.0 world; host impls of `scene`, `txn`, `readings`, `routes`, `kv` and `log`; instance store; the third-party core ABI is retired | `fe-plugin/src/wasm/**` (+ `wasm/AGENTS.md`, new), `fe-plugin/Cargo.toml` | **S** `wasm/host_imports.rs::register_host_imports_succeeds` → a 2.0 instantiate-and-call journey; **E** `wasm/mod.rs::fuel_exhaustion_marks_degraded` with a component | P0.6; plugin_host_revival S0; gate |
| P1.2 | E2 | BUILD | sonnet | SDK types: `PendingOp::SetTransform`; the staged readings type and its bounds in `validate_transaction`; manifest v2 serde + validation with schema parity; UI contribution v2 fields (command, binding); `PathTracker`'s snap math moved into `path_geometry` | `fe-sdk/src/**` (`manifest_v2.rs` new), `fe-sdk/Cargo.toml`; shared `fe-terrain/src/iot/path_tracker.rs` (re-export) | **E** `fe-terrain/src/iot/path_tracker.rs::test_snap_to_segment` + `::test_position_at_progress`, asserting through the shared functions; **E** `fe-sdk/src/ui/mod.rs::register_and_get_by_slot` (v2 fields) | P0.6; gate |
| P1.3 | E3 | BUILD | sonnet | Tick scheduler on the worker (`plugin_thread_loop`): deadline wait, injected clock, `refuel_for_tick`, skip-and-count, one flush in flight, counters. Staged write window. Manifest v2 → token (an empty scope grants nothing). `Disabled` and `Revoked` registry states. | `fe-plugin/src/{lib,context,transaction,scene_host,registry,capability,host_env}.rs`, `fe-plugin/src/tick.rs` (new), `fe-plugin/src/rhai/**`, `fe-plugin/src/AGENTS.md` | **E** `wasm/mod.rs::fuel_budget_refill` → J1; **E** `rhai/sandbox.rs::normal_script_runs_fine` (scene-host block: a SetTransform batch through `PluginContexts`); **S** `capability.rs::parse_manifest_defaults` + `::validate_property_access_allowed` → one v2 validation table (J2); **E** `registry.rs::activate_deactivate_lifecycle` | P1.2; plugin_host_revival S1 (`capability.rs`); gate |
| P1.4 | E4 | BUILD | opus | DB seam: `PluginCommit` carries the staged readings; `SetTransform` runs inside the atomic batch; readings go through `insert_readings_with_replication` validation with the manifest's `source_class`; the admission carries package identity (groundwork for provenance; stamping waits for OD-E1) | `fe-runtime/src/plugin_host.rs`, `fe-database/src/handlers/plugin_host.rs`; shared `fe-runtime/src/messages.rs`, `fe-database/src/types.rs` | **E** `fe-database/tests/db_test.rs::test_rename_entity_omits_node_scene_change` (dispatcher-plugin block → J3); **E** `fe-database/src/op_log.rs::a_failing_statement_rolls_back_the_op_log_row_too` (plugin batch block: a failing SetTransform rolls back the batch and no reading lands); **E** `fe-database/tests/iot_reading_test.rs::unknown_anchor_rejects_whole_batch` (door parity) | P1.2; gate; A-4 |
| P1.5 | E5 | BUILD | opus | **Bus broker (D-X6, P1 deliverable):** host-stream source traits, extension streams and commands, consume grants, limits, bounded queues, audit hooks. **Network brokers:** HTTP/WS accept through the API mount, WS dial, MQTT client, egress. | `fe-plugin/src/bus/**` (new + AGENTS.md), `fe-plugin/src/net/**` (new + AGENTS.md); shared `deny.toml`, `fe-plugin/Cargo.toml` | **N** one bus journey in `fe-plugin/src/bus/` (J4) | P1.3; gate |
| P1.6 | E6 | BUILD | sonnet | API surface: `/api/v1/extensions*` (status, audit, local-identity administration, commands, ingress mount); a `/ws` extension-stream target; MCP `list_extensions` + `extension_command` | `fe-api/src/extensions.rs` (new), `fe-api/tests/extensions_api_test.rs` (new); shared `fe-api/src/{server,mcp,ws,lib}.rs`, `fe-api/AGENTS.md` (§extensions) | **N** `extensions_api_test.rs` (J6); **E** `fe-api/tests/mcp_integration.rs::tools_list_inventory`; **E** `fe-api/tests/ws_integration.rs::ws_unknown_and_foreign_ids_get_identical_denials` | P1.4, P1.5; plugin_host_revival S1 (`server.rs` order) |
| P1.7 | E7 | BUILD | sonnet | Integration slice: `plugin_bridge` builds the `PluginManager` on the worker; injects the bus sources (queries through fe-api's guard, readings-door events, the scene feed); wires the API handle. Relay hosting comes through plugin_host_revival S1. | `fractalengine/src/plugin_bridge.rs`; shared `fractalengine/src/main.rs` (ApiConfig field), `fractalengine/src/AGENTS.md` (§Plugin scene bridge) | no unit seam in the bridge; covered by J6 through the bridge trait and by the P6 scenarios | P1.1, P1.3-P1.6 |
| P1.8 | E8 | BUILD | sonnet | UI slot host: renders declared contributions in the six slots with `ext:<package>/<id>` accessible names; fe-ui consumes `UiExtensionRegistry` (the terrain registration call is plugin_host_revival S1's) | `fe-ui/src/extensions/{mod,slots}.rs` (new), `fe-ui/src/extensions/AGENTS.md` (new); shared `fe-ui/src/{lib,plugin}.rs`, `fe-ui/src/panels/{toolbar,sidebar,inspector}.rs`, `fe-ui/src/dialogs/context_menu.rs` (one-line slot calls) | **N** one slot journey in `slots.rs`; **E** `fe-sdk/src/ui/mod.rs::unregister_all_removes_only_target_plugin` (disabling removes contributions) | P1.2, P1.7 |
| P1.9 | G1 | BUILD | opus | P1 gate: ADVOCATE then CRITIC, `/code-review high` + `/security-review` (component host, imports, ingress, bus); fix own findings | `review.md` (append) + fix-only edits to P1 files | reruns the touched batches once | P1.1-P1.8 |

### P2 — Governance

| Task | Slice | Flag | Model | What | Files | Journey | Depends |
|---|---|---|---|---|---|---|---|
| P2.1 | E9 | BUILD | sonnet | Sealed plugin package: `HexonType::Plugin`; `plugin/extension.json` in `HEADER_MEMBER_PATHS`; `content_hash`; plugin_host_revival S2's verifier applied to the sealed signature; package store | `fe-format/src/{manifest,integrity}.rs`, `fe-format/src/AGENTS.md`, `fe-hexon/src/{publisher,package}.rs`; shared `fe-plugin/src/lifecycle.rs`, `docs/hexon-format-spec.md`, `fractalengine/tests/security_harness.rs` | **E** `fe-hexon/src/package.rs::test_tampered_package_fails_signature` (J5 tamper table); **E** `fe-format/src/integrity.rs::seal_enforces_asset_hash_and_verifies_a_self_certifying_asset` (plugin header member); **S** `fe-plugin/src/lifecycle.rs::install_plugin_signature_mismatch` (BLAKE3 self-check → sealed admission); **E** `fractalengine/tests/security_harness.rs::test_tampered_ed25519_signature` | P0.6; plugin_host_revival S2; hexon_entitlements S5 (doc only); gate |
| P2.2 | E10 | BUILD | opus | Verse policy row (Manager+, replicated, role-at-write) merged with local policy; activation preconditions in order (R19); consent record; audit `OpType` via `op_log::commit_operation`; kill-switch arrival stops running instances (R23); `FE_EXT_DISABLE_ALL` | `fe-database/src/handlers/extension_policy.rs` (new); shared `fe-database/src/types.rs`, `fe-database/src/handlers/AGENTS.md`, `fe-plugin/src/lifecycle.rs`, `fe-api/src/extensions.rs` (policy + audit routes), `fe-api/tests/extensions_api_test.rs` | **E** `extensions_api_test.rs` (J6 refusal table + audit trail); **E** `fe-database/tests/role_propagation_inbound_test.rs::a_not_yet_managers_statement_is_deferred_then_admitted_on_promotion` (an inbound policy row from a not-yet-Manager is deferred, then admitted) | P1.7, P2.1; plugin_host_revival S2-S4 |
| P2.3 | E10W | BUILD | opus | Host-stamped provenance on extension scene versions and readings; revocation re-judge (one batch per verse, under the §rejudge-on-demotion discipline); readings excluded at the scope-substitution seam and in the distributed partials; reversible | **unpartitioned** — recompute after the P2P merge. Candidates: `fe-database/src/handlers/{scene_publication/**,replicated_row,iot_reading,ts_partial}.rs`, `fe-sync/src/{sharding,sync_thread,distributed_query}.rs`, `fe-api/src/query_guard.rs` | **E** the `fe-database/src/op_log.rs` QA-0121/QA-0131 re-judge block (J7); **E** `fe-api/tests/iot_ingest_test.rs::guarded_query_reads_iot_rows_scope_filtered` (revoked provenance excluded) | P2.2; OD-E1 ratified; A-4 |
| P2.4 | E11 | BUILD | sonnet | Consent v2: plugin_host_revival S3's modal, created at `fe-ui/src/extensions/consent.rs` and rendering manifest v2; re-prompt on a capability delta; REST consent echoes the digest | `fe-ui/src/extensions/consent.rs` (new) | **E** S3's planned `install_dialog_renders_capability_list_before_confirm_enabled` (v2 fields + delta re-prompt) | P1.8, P2.2; plugin_host_revival S3 |
| P2.5 | D1 | AUTHOR-ONLY | haiku | Only if OD-E1 is ratified: amend the S-PLUGIN row in `conductor/open-core.md`, and finalize `docs/extensions/governance.md` | `conductor/open-core.md`, `docs/extensions/governance.md` | — | OD-E1; P0.5 |
| P2.6 | G2 | BUILD | opus | P2 gate: ADVOCATE then CRITIC, `/code-review high` + `/security-review` (signing, trust roots, policy, provenance, revocation) | `review.md` (append) + fix-only edits | reruns the touched batches once | P2.1-P2.4 |

### P3 — Authoring tools

| Task | Slice | Flag | Model | What | Files | Journey | Depends |
|---|---|---|---|---|---|---|---|
| P3.1 | E12 | BUILD | sonnet | `fe-ext` CLI: `new`, `build`, `validate`, `pack`, `sign`/`countersign`, `inspect`, and `run --sandbox --fixture` (the real host + fe-plugin-test's mock backend + a deterministic clock + fe-sim fixtures) | `tools/fe-ext/**` (new); shared root `Cargo.toml` (workspace member) | **N** one CLI journey (J8) | P1.1, P2.1 |
| P3.2 | E12 | BUILD | sonnet | Guest SDK for Rust: generated 2.0 bindings, write-window helpers, a snap-to-alert helper (REST `tracking_alert` semantics), readings batching | `tools/fe-ext/guest/**` (new) | covered by J8 (the examples build against it) | P3.1 |
| P3.3 | E12X | BUILD | sonnet | Examples: `extensions/fleet-sim` (movers + sensors from FleetConfig v2) and `extensions/device-adapter` (HTTP/WS/MQTT ingress → readings + transforms, actuation through egress), each with sandbox fixtures | `extensions/fleet-sim/**`, `extensions/device-adapter/**` (new) | covered by J8's `run --sandbox` on each example | P3.2, P5.1 |
| P3.4 | E12D | AUTHOR-ONLY | haiku | SDK docs: README, authoring guide, cookbook (bus, ingress, fleets), 1.0 → 2.0 migration note. Link example files; never copy them. | `docs/extensions/{README,authoring-guide,cookbook,migration-1-to-2}.md` (new) | — (link check in P6.5) | P0.6 |
| P3.5 | G3 | BUILD | opus | P3 gate: ADVOCATE then CRITIC | `review.md` (append) + fix-only edits | reruns the touched batches once | P3.1-P3.4 |

### P4 — Extensions UI

| Task | Slice | Flag | Model | What | Files | Journey | Depends |
|---|---|---|---|---|---|---|---|
| P4.0 | E13 | AUTHOR-ONLY | sonnet | UI design text applying the UI-UX guidance: views, states, one primary action per view, accessible names, copy | `docs/extensions/extensions-ui.md` (new) | — | P0.6 |
| P4.1 | E13 | BUILD | sonnet | Extensions panel (Installed, Details, Permissions, Install from file, Enable/Disable per binding, Uninstall) over a queued `ExtensionOp` that the bridge drains | `fe-ui/src/extensions/{panel,details,permissions}.rs` (new); shared `fractalengine/src/plugin_bridge.rs` (drain, after P1.7) | **N** one panel journey in `panel.rs` (J9, fake bridge) | P1.8, P2.4 |
| P4.2 | E13 | BUILD | sonnet | `RegistrySource` trait + Local + Static signed index (verified with the revocation list's canonical-JSON rule); protocol-v1 source = plugin_host_revival S4's client, relocated per spec §10; Browse view | `fe-hexon/src/registry_source.rs` (new), `fe-ui/src/extensions/browse.rs` (new) | **N** one registry-source journey in `registry_source.rs` (good index, tampered index, bad package signature, protocol mismatch) | P2.1; plugin_host_revival S4 |
| P4.3 | E13 | BUILD | sonnet | Update flow: re-verify, re-prompt on a capability delta, keep the previous version until the new one is active (rollback) | `fe-ui/src/extensions/details.rs`; shared `fe-plugin/src/lifecycle.rs` (after P2.2) | **E** J9 (update with and without a delta) | P4.1 |
| P4.4 | G4 | BUILD | opus | P4 gate: ADVOCATE then CRITIC, `/security-review` (consent, registry sources, index signatures); GUI QA pass through the agent input surface (R28 AC) | `review.md` (append) + fix-only edits | reruns the touched batches once | P4.1-P4.3 |

### P5 — First-party migrations, fe-sim first

| Task | Slice | Flag | Model | What | Files | Journey | Depends |
|---|---|---|---|---|---|---|---|
| P5.1 | E14a | BUILD | sonnet | fe-sim model split: `fleet.rs` + `sensors.rs` into a wasm-compatible crate under `fe-sim/`; FleetConfig v2 movers; the v1 lab fields stay lab-only | `fe-sim/src/{fleet,sensors,lib}.rs`, `fe-sim/Cargo.toml`, `fe-sim/model/**` (new); shared root `Cargo.toml` | **E** `fe-sim/src/fleet.rs::validation_rejects_broken_configs` (v2 mover rows in the same table); **E** `::plan_is_pure_cadenced_and_totally_ordered` (mover plan); **E** `fe-sim/src/sensors.rs::models_round_trip_through_declarative_json` | P0.6; gate (runs in parallel with P1) |
| P5.2 | E14b | BUILD | sonnet | Simulation extension: fleets as `fleet.config` nodes, a `fleet.driver` claim, the wall clock on live hosts. The lab hosts the same package under the `SimClock` (R4's injected clock). Adds `fe-ext run --lab`. | `fe-sim/src/{session,scenario}.rs`, `fe-sim/src/AGENTS.md`, `docs/simulation-lab.md`; shared `extensions/fleet-sim/**` (after P3.3) | **E** `fe-sim/src/session.rs::stepped_session_matches_one_shot_run_exactly` (extension-hosted fleet); **E** `fe-sim/src/scenario.rs::sim_fleet_writes_real_iot_reading_rows_on_cadence` (readings through the extension path, class simulation) | P3.3, P5.1, P2.2 |
| P5.3 | E15 | BUILD | sonnet | Device adapters with one shape: WS dial/accept, MQTT, a declarative frame → reading/transform mapping, `source_class` from the manifest | shared `extensions/device-adapter/**` (after P3.3); `fe-api/tests/iot_ingest_test.rs` | **E** `iot_ingest_test.rs::ingest_batch_persists_and_reads_back` (parity: the same frames through the adapter and through the REST door give identical rows, apart from provenance and class) | P3.3, P1.5 |
| P5.4 | E16 | BUILD | sonnet | The hexon registry client moves behind `RegistrySource`; the Hexon Manager uses it | `fe-hexon/src/remote.rs`, `fe-hexon/src/AGENTS.md`, `fe-hexon/tests/remote_registry_test.rs`; shared `fe-ui/src/dialogs/hexon_manager.rs`, `fractalengine/src/terrain_bridge.rs` | **E** `fe-hexon/tests/remote_registry_test.rs::search_download_verify_roundtrip` (through the trait); the existing `fe-api/tests/hexon_tileset_test.rs` journeys stay unchanged | P4.2 |
| P5.5 | E17 | BUILD | sonnet | BI connectors: the Copy-for-BI targets become connector descriptors (the builders stay compiled-in); one push connector package (JSON/CSV over HTTPS on an interval) | `fe-ui/src/extensions/builtin/**` (new), `extensions/bi-push/**` (new); shared `fe-ui/src/panels/egress_card.rs` | **E** `fe-ui/src/gis/egress_strings.rs::petal_scope_sql_exact` + `::export_url_parquet_encodes_query_param` (byte-identical through the descriptor); push parity is asserted in J10 | P1.8, P1.5 |
| P5.6 | E18 | BUILD | haiku | Terrain aligned: a manifest v2 constant; contributions bound to the existing actions through commands | shared `fe-terrain/src/extension.rs` (after plugin_host_revival S1) | **E** `fe-terrain/src/extension.rs::register_terrain_ui_adds_three_contributions` (commands bound) | P1.8 |
| P5.7 | G5 | BUILD | opus | P5 gate: ADVOCATE then CRITIC | `review.md` (append) + fix-only edits | reruns the touched batches once | P5.1-P5.6 |

### P6 — Simulator QA

| Task | Slice | Flag | Model | What | Files | Journey | Depends |
|---|---|---|---|---|---|---|---|
| P6.1 | E19 | AUTHOR-ONLY | sonnet | Scenario JSON for `ext-fleet` and `ext-device` (J10); the `extension` probe added to the schema text | `tools/qa/sim/scenarios/ext-fleet.json` (new), `tools/qa/sim/scenarios/ext-device.json` (new); shared `tools/qa/sim/scenario.schema.json` | **N** two scenarios (new seam) | P0.6 |
| P6.2 | E19 | BUILD | sonnet | Driver ops and the `extension` probe; a loopback static server (signed index, revocation list); a loopback MQTT broker and HTTP sink for the virtual devices and the push connector | shared `tools/qa/sim/{driver,prober}.py` (the P2P-QA harness; only after its QA-0140 work lands) | exercised by P6.3 | P6.1 |
| P6.3 | E19 | BUILD | sonnet | Two clean runs of each scenario; triage anomalies into findings; fix loop through the owning slices | run artifacts under `qa/sim/runs/` | — | P6.2, P5.2-P5.5, P2.3, P2.4, P4.1-P4.3 |
| P6.4 | G6 | BUILD | opus | Final ADVOCATE then CRITIC over the whole track, `/code-review high` + `/security-review`; fix own findings | `review.md` (append) + fix-only edits | full sweep once | P6.3 |
| P6.5 | D2 | AUTHOR-ONLY | haiku | Docs consolidation: AGENTS.md folds; product.md's stale plugin line; open-core.md (if OD-E1); `conductor/tracks.md` status; retro; plan ticks; docs link check | listed under D2 in `metadata.json` | — | P6.4 |

## 3. Journeys as text (AUTHOR-ONLY now; the named BUILD task implements each)

- **J1 tick** (P1.3, extends `wasm/mod.rs::fuel_budget_refill`).
  1. Instantiate a 2.0 test component that stages one `set-transform` per tick, at a 500 ms cadence on a manual clock.
  2. Bind it to a fake sink that answers each flush after 750 ms.
  3. Advance through 20 cadences.
  4. Assert `ticks_run + ticks_skipped == 20`, never more than one flush in flight, each flush carrying exactly the staged op, and a true `delta-ms`.
  5. Shrink fuel below one tick's cost: the state turns `degraded`, and the sink sees no flush for that tick.
- **J2 manifest table** (P1.3 swap). One table over R3's rows, refused and accepted: an empty scope, non-https egress, a sub-floor cadence, a simulation kind declaring `device`, an undeclared UI command, an undeclared consumed stream, plus one valid manifest. `fe-ext validate` (J8) runs the same table.
- **J3 batch** (P1.4, the dispatcher-plugin block of `db_test.rs`).
  1. Send one `PluginCommit` with four `SetTransform` ops and eight readings.
  2. The receipt reports four ops and eight readings.
  3. The transforms read back.
  4. Eight `iot_reading` rows carry the local `source_did`.
  5. Replication events cover four node rows and eight reading rows.
  6. Variant — one unknown anchor: no reading from that flush lands, and the scene batch outcome follows R6's whole-flush rule.
- **J4 bus** (P1.5, new).
  1. `host/query` returns the rows the guarded core returns for the same SQL and scope.
  2. A stream pushed past its ceiling drops the oldest events, counted in `dropped`.
  3. A full command queue answers `rate-limited`.
  4. An undeclared consume is refused.
  5. A grant, a call and a denial each produce one audit entry.
- **J5 package tamper table** (P2.1). A flipped component byte, an edited `extension.json`, an uncommitted member, an unsigned package without the dev flag, and a package signed with an untrusted key are each refused with a distinct reason. The package store is unchanged.
- **J6 extensions API** (P1.6 new; P2.2 extends).
  1. With the local token: list, install, consent with the digest, bind, read the status counters.
  2. With a remote editor token: install answers the uniform not-found and writes one `denied` audit entry.
  3. Command floors: a viewer is refused, an editor is allowed.
  4. Ingress table: declared → 2xx, undeclared → not-found, over-rate → 429, over-size → 413, disabled → not-found.
  5. P2.2 adds the R19 refusal table, in order, and the audit trail.
- **J7 withdrawal** (P2.3, the op_log re-judge block).
  1. Seed an extension-provenance version over a manual baseline.
  2. Accept a revocation list: the baseline is restored, an extension-created node is removed, and a manual property edit survives.
  3. Remove the trust root: the withdrawn versions come back.
- **J8 CLI** (P3.1, new). `new` → `build` → `validate` (the J2 table) → `pack` → `sign` → `inspect` → `run --sandbox` twice, with byte-identical fingerprints.
- **J9 panel** (P4.1 new; P4.3 extends). Through the queued ops and a fake bridge: install from file → consent → enable → disable → update with a delta (re-prompts), then without → uninstall. Each step asserts the visible state by accessible name.
- **J10 scenarios** (P6.1).
  - **ext-fleet** (three peers):
    1. The Owner sets an allow-list policy; the Editor installs, consents to and binds `fleet-sim`.
    2. Create a fleet: four movers, a route from a GPX import, two sensors.
    3. Fly at 2 Hz for 3 minutes. Probes: transforms agree, readings via query, the Editor's status shows commits ≤ ticks, zero 504s.
    4. Check the command floors through REST and WS.
    5. The Owner demotes the Editor mid-flight, then re-grants (R22).
    6. A push connector consumes `host/query` into a loopback sink that equals `/api/v1/query`.
    7. Publish the revocation list from the loopback trust-root URL, with a short pull interval. The instance stops everywhere, extension rows are withdrawn, and the hand edit survives (R23, R24).
    8. Fingerprint.
  - **ext-device** (the adapter on the Owner; HTTP ingress from a virtual device, MQTT from the loopback broker):
    1. Readings and transforms agree on every peer.
    2. Parity with the REST door.
    3. The undeclared, over-rate and over-size rows.
    4. Disable → the endpoints answer not-found; enable → 2xx.

## 4. Review gates (G0-G6)

Every phase ends with ADVOCATE (patterns, reuse, ergonomics), then CRITIC (defects, anti-patterns, tests that cannot fail). Each runs in a fresh context and fixes its own findings, with one verdict line per finding in `review.md`. Security depth per phase:
- **P1:** sandbox and component host; host-import argument bounds; ingress parsing; bus floors; guests can never hold I/O.
- **P2:** signature and trust-root loading; policy row admission (role-at-write); the precondition order; guests cannot forge provenance; revocation-list signer scope (only `revocation = true` roots).
- **P4:** consent cannot be bypassed; the index signature; the protocol check.

Each gate also re-checks that the phase still serves `conductor/product.md` and that no catalogue-operations wording leaked in.

## 5. Docs + retro (P6.5)

- **AGENTS.md:**
  - `fe-plugin/src/AGENTS.md`: replace the "aspirational contract" and dormant framing with the bound 2.0 host; add §tick and §write-window; point to `wasm/`, `bus/` and `net/`.
  - `fe-sdk/src/AGENTS.md`: §manifest-v2, `SetTransform`, the route-math move.
  - `fe-api/AGENTS.md`: §extensions.
  - `fractalengine/src/AGENTS.md`: the bridge now admits packages.
- **conductor files:** product.md's stale line; open-core.md's S-PLUGIN row (if OD-E1 is ratified); the `conductor/tracks.md` status line.
- **Retro:** record which OD-E defaults were overridden.

## 6. Cross-track and cross-session sequencing

- **plugin_host_revival_20261031** runs first, in this session (A-1).
  - `fe-plugin/Cargo.toml`: S0 → S5 → S4 → P1.1 → P1.5.
  - `capability.rs`: S1 → P1.3.
  - `lifecycle.rs`: S2 → S4 → P2.1 → P2.2 → P4.3.
  - `fe-terrain/src/extension.rs`: S1 → P5.6.
  - Proposals for the session to confirm (spec §10): its S3 modal is created at `fe-ui/src/extensions/consent.rs`, and its S4 client becomes the protocol-v1 `RegistrySource`.
- **hexon_entitlements_20261031:** `docs/hexon-format-spec.md` changes only after its S5 1.1.0 bump.
- **P2P-QA (main tree, merge gate):**
  - It owns the build gate.
  - These files are under active P2P change: `fe-runtime/src/messages.rs`, `fe-database/src/{types,op_log}.rs`, `fe-database/tests/db_test.rs`, the re-judge seams, and `tools/qa/sim/*.py`. Touch them only after the campaign merges, then re-grep.
  - Post `ready-to-merge` on the bus; never merge your own branch.
- **Shared API and binary files:** `fe-api/src/{server,mcp,ws,lib}.rs` and `fractalengine/src/main.rs` follow the agent_vision → relay_operability → plugin_host_revival S1 order, and this track goes last. Re-grep `build_router` and `ApiConfig` first.
- **Shared fe-ui files:** `panels/{toolbar,sidebar,inspector,egress_card}.rs`, `dialogs/{context_menu,hexon_manager}.rs`, `lib.rs` and `plugin.rs` belong to the polish lane and the UI owners. This track adds one-line render calls only.
- **portal_surface_20261107 (EXT-PORTAL)** depends on P0.3 (bus contract), P1.5 (broker), P0.1 (WIT 2.0) and P2.2 (governance). Post a `milestone` on the bus as each lands.

## 7. Task checklist (43 tasks: 11 AUTHOR-ONLY, 32 BUILD)

- [ ] P0.1 WIT 2.0 world `extension` (AUTHOR-ONLY)
- [ ] P0.2 Manifest v2 schema + reference (AUTHOR-ONLY)
- [ ] P0.3 Integration bus contract, D-X6 (AUTHOR-ONLY)
- [ ] P0.4 Plugin package format (AUTHOR-ONLY)
- [ ] P0.5 Governance contract + OD-E1 amendment text (AUTHOR-ONLY)
- [ ] P0.6 G0 advocate + critic (AUTHOR-ONLY)
- [ ] P1.1 Component host + 2.0 bindgen (BUILD)
- [ ] P1.2 SDK types + shared route math (BUILD)
- [ ] P1.3 Tick scheduler + write window + capability v2 (BUILD)
- [ ] P1.4 Batched PluginCommit: transforms + readings (BUILD)
- [ ] P1.5 Bus broker + network brokers, D-X6 (BUILD)
- [ ] P1.6 Extensions API + MCP + WS (BUILD)
- [ ] P1.7 Bridge integration + relay hosting (BUILD)
- [ ] P1.8 UI slot host (BUILD)
- [ ] P1.9 G1 review gate (BUILD)
- [ ] P2.1 Sealed plugin package (BUILD)
- [ ] P2.2 Verse policy, preconditions, consent record, audit, kill-switch arrival (BUILD)
- [ ] P2.3 Provenance + withdrawal, needs OD-E1 (BUILD)
- [ ] P2.4 Consent v2 UI (BUILD)
- [ ] P2.5 open-core.md S-PLUGIN amendment if OD-E1 (AUTHOR-ONLY)
- [ ] P2.6 G2 review gate (BUILD)
- [ ] P3.1 fe-ext CLI (BUILD)
- [ ] P3.2 Guest SDK (BUILD)
- [ ] P3.3 Examples fleet-sim + device-adapter (BUILD)
- [ ] P3.4 SDK docs (AUTHOR-ONLY)
- [ ] P3.5 G3 review gate (BUILD)
- [ ] P4.0 Extensions UI design text (AUTHOR-ONLY)
- [ ] P4.1 Extensions panel (BUILD)
- [ ] P4.2 RegistrySource + Local + static signed index (BUILD)
- [ ] P4.3 Update flow (BUILD)
- [ ] P4.4 G4 review gate + GUI QA (BUILD)
- [ ] P5.1 fe-sim model split + FleetConfig v2 (BUILD)
- [ ] P5.2 Simulation extension + lab hosting (BUILD)
- [ ] P5.3 Device adapters, one shape (BUILD)
- [ ] P5.4 Hexon registry client behind RegistrySource (BUILD)
- [ ] P5.5 BI connectors (BUILD)
- [ ] P5.6 Terrain aligned (BUILD)
- [ ] P5.7 G5 review gate (BUILD)
- [ ] P6.1 Scenario JSON ext-fleet + ext-device (AUTHOR-ONLY)
- [ ] P6.2 Simulator ops + probe + loopback fixtures (BUILD)
- [ ] P6.3 Two clean runs each + triage (BUILD)
- [ ] P6.4 G6 final advocate + critic + security review (BUILD)
- [ ] P6.5 Docs consolidation + retro (AUTHOR-ONLY)
