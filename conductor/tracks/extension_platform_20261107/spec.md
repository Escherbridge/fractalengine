---
type: spec
title: Extension platform — capabilities plug in as governed, signed extensions
track: extension_platform_20261107
tags: [feature, extension-platform, plugin-system, fe-plugin, fe-sdk, fe-sim, fe-ui, fe-api, fe-hexon, governance, integration-bus, extension_platform_20261107]
created: 2026-11-07
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: user grill 2026-11-07 (extension platform program; decisions D-X1..D-X4, D-X5..D-X8 recorded)
depends_on: [plugin_host_revival_20261031, hexon_entitlements_20261031, agent_input_surface_20261031]
---

# Specification: Extension platform

**Track ID:** `extension_platform_20261107` · **Type:** feature · **Priority:** P1
**Crates:** `fe-plugin` (component host, tick driver, bus broker, network brokers), `fe-sdk` (manifest v2 and batch types, shared route math), `fe-runtime` + `fe-database` (typed plugin DB bridge: batched commit, policy, audit, provenance), `fe-api` (extension endpoints, ingress mount, bus access), `fe-ui` (Extensions UI, slot host), `fe-format` + `fe-hexon` (sealed plugin package, registry sources), `fe-sim` (fleet model, lab hosting), `fractalengine` (bridge wiring). New: `tools/fe-ext` (authoring CLI and guest SDK), `extensions/fleet-sim`, `extensions/device-adapter`.
**Citations computed at:** `cfd7fce6`. HEAD moved from `30163ca2` while this was written, because the P2P campaign landed QA-0123, QA-0137, QA-0118, QA-0138 and QA-0139. Symbol names are stable and line numbers are not, so re-grep before editing.
**Build policy:** author-only until `.omc/state/build-gate.json` flips (D-X2). Every plan task is flagged AUTHOR-ONLY or BUILD.

## 1. User vision (2026-11-07 grill)

The extension surface becomes **the** way capabilities plug into FractalEngine: hardware and device adapters, simulated fleets, registries and BI connectors. Each one is governed and signed, authored with real tools, and managed from an in-app Extensions UI, on the path to a VS-Code-marketplace-like experience.

This repository builds the open half of that path (`conductor/open-core.md` §The rule: *anything a self-hoster needs to run, verify, or extend FractalEngine is open*): the runtime, the contract, the package format, governance, the authoring kit, and an Extensions UI whose registry endpoint is pluggable. Any hosted catalogue is reached only through S-REGISTRY / S-ENTITLEMENT and is not described here.

## 2. Product fit (`conductor/product.md`)

- **Primary persona (analysts, operations).** product.md promises that "an IoT reading is queryable as a spatial row within seconds of ingestion", and its core value is "BI egress, not BI lock-in".
  - Today the only ingestion door is one REST batch route, and fe-api `AGENTS.md` §iot-ingest records "a poll connector catalog is future work". Device adapters (HTTP, websocket, MQTT) close that gap.
  - BI connectors push the same guarded query rows to external platforms. Both go through the existing guards.
- **Secondary persona (world-builders)** "script the engine via the plugin system (Rhai/WASM)". D-X1 splits that: Wasm packages for extensions, Rhai for user macros.
- **Self-sovereign and role-precise.** Extensions run on the operator's host and write only as the operator's own identity (§4.11), never above it. Verse Managers decide which packages may act in their verse (R18).
- **Non-goals respected.**
  - Not a dashboard: UI contributions are declarative, host-rendered controls, and connectors hand data out.
  - No payment processing: licensed packages use S-ENTITLEMENT verification only.
  - No marketplace operations in this repo.
- **Stale line, corrected in P6 docs.** product.md "Built but not wired → Rhai + WASM plugin system ... No binary loads it" has been half-true since `2cc05f27`: the GUI binary installs the host (§4.4), but nothing admits a package.

## 3. Decisions register

Ratified decisions are not re-litigated (`.omc/state/bus/SESSION-PROTOCOL.md` item 11). New decisions are proposed in §8 and ratified through the orchestrator.

| ID | Decision (2026-11-07) | Owner | Effect in this track |
|---|---|---|---|
| **D-X1** | Package + ABI: third-party extensions ship as sealed `.hexon` packages (`HexonType::Plugin`) carrying a Wasm component against WIT `fractalengine:plugin`, bumped from 1.0.0 (`fe-plugin/wit/hexon-plugin.wit:7`) to 2.0. Native Rust via `fe-sdk` stays first-party only; Rhai stays for user macros. | this track | R1-R3 |
| **OD-9 closed by D-X1** | `plugin_host_revival_20261031` OD-9 ("plugin package shape on the registry wire") resolves to its recommended default: a sealed `.hexon` with a new `HexonType::Plugin`, so registry protocol v1, `PackageStore` and S-ENTITLEMENT apply unchanged. | this track | R1, R17, R29 |
| **D-X2** | Build policy: author only (specs, WIT, docs, test journeys as text) until the P2P convergence campaign flips `cargo_allowed=true`. | P2P-QA | plan flags |
| **D-X3** | Sessions. EXT-PLATFORM (worktree `.omc/worktrees/ext-platform`, branch `track/extension-platform`) owns runtime, SDK, governance, authoring tools, Extensions UI and first-party migrations. A second session on the private side builds its registry service and store client only against S-REGISTRY / S-ENTITLEMENT; only the seam contract is described here (R29). P2P-QA (orchestrator, main tree) is the merge gate. | P2P-QA | checkpoint §SESSION BUS |
| **D-X4** | Wave-1 migrations behind the extension surface: fe-sim fleets and scenarios (fe-sim becomes the first-party "simulation" extension; a fleet stays DATA — `FleetConfig`), IoT device adapters (HTTP, websocket, MQTT ingress; ONE adapter shape for simulated and real devices), the hexon registry client, BI egress connectors. Terrain is already the first-party extension (`fe-terrain/src/extension.rs`). | this track | R30-R34 |
| **D-X5** | Web bundles are signed packages (same sealed `.hexon` family, web content type), admitted per peer by extension governance before they get IPC. Unsigned or unadmitted bundles render in a no-IPC static sandbox; replicated rows never auto-execute. | `portal_surface_20261107` | R17-R19 admit them through the same verifier and policy, with no second admission path |
| **D-X6** | Host-brokered IPC subscriptions plus an integration bus: extensions expose streams and commands and consume hooks through the host broker under capability checks, so outside developers can integrate external platforms. | `portal_surface_20261107` (record); the bus is specified and built HERE | R13-R15 (P0 contract, P1 broker) |
| **D-X7** | Portals render in-app first (local blob store via a custom protocol). "Through the web" means a static export that talks to the public REST API, later. | `portal_surface_20261107` | none beyond R13-R15 |
| **D-X8** | The portal surface is its own public track, `portal_surface_20261107`, with a third session, EXT-PORTAL (worktree `.omc/worktrees/ext-portal`, branch `track/portal-surface`), and its own QA lane. | `portal_surface_20261107` | that track depends on this one (bus, WIT 2.0, governance); no portal work here |

## 4. Evidence (what exists at `cfd7fce6`)

### 4.1 The lifecycle trait, and a tick that never fires
- `FractalExtension` (`fe-plugin/src/lib.rs:111-162`) declares `on_install`, `on_activate`, `on_deactivate`, `on_uninstall`, `on_scene_change` and `on_tick`. The default body of `on_tick` is `let _ = (ctx, delta_ms)` (`:159`), and `git grep on_tick` finds no caller anywhere.
- The plugin worker (`plugin_thread_loop`, `lib.rs:250-265`) only services commands, with a 100 ms `recv_timeout`. Nothing schedules ticks. `WasmEngine::refuel_for_tick` (`fe-plugin/src/wasm/mod.rs:182`) is called only from a unit test (`:337`).
- `PluginManager::new` is called only in a test (`fe-plugin/src/lifecycle.rs:405`), so no production path installs or activates a package.

### 4.2 Write and capability vocabulary
- `PluginTransaction` offers `set_property`, `create_node`, `delete_node` and `commit` (`fe-plugin/src/transaction.rs:27-70`) over the three `PendingOp` variants (`fe-sdk/src/plugin_host.rs:44-66`). Batches are bounded at 64 ops and 64 KiB, and queries at 200 nodes (`plugin_host.rs:9-13`). There is no transform op and no readings op.
- `CapabilityOperation` has `PropertyAccess`, `PetalAccess`, `VerseAccess` and `HttpRequest` (`fe-plugin/src/capability.rs:114-123`).
  - Named grants fail closed (`:93-97`).
  - An EMPTY scope list means wildcard (`:134-136`, `:148-150`, `:161-163`), so scopes fail open by omission.
  - `network_scope`/`external_http` are parsed and never enforced (plugin_host_revival OD-1).

### 4.3 Contract text versus the running ABI
- WIT `fractalengine:plugin@1.0.0` (`fe-plugin/wit/hexon-plugin.wit:7`) defines the world `fractal-plugin`. It imports `node-api`, `plugin-log` and `query-api`, and exports `scene-hooks`, including `on-tick: func(delta-ms: u32)` (`:120`, `:124-129`).
- `fe-plugin/src/AGENTS.md:131-134` calls the WIT "the aspirational contract, not the wired ABI". The host registers hand-written core-ABI imports under the namespace `fe` (`fe-plugin/src/wasm/host_imports.rs:39-42`), and there is no component-model bindgen.
- Sandbox limits today: pooled instances are capped at 16 MiB (`wasm/mod.rs:67`), fuel metering is on (`:61`), and default fuel is 10 000 initial / 5 000 per tick (`wasm/fuel.rs:38-44`).

### 4.4 Production wiring (since `2cc05f27`, 2026-11-01)
- `fractalengine/src/main.rs:511` calls `plugin_bridge::install` unconditionally in the GUI binary. The relay binary has no plugin host.
- `fractalengine/src/plugin_bridge.rs:103-141` binds each admitted context to a typed DB backend through a host-minted `PluginAdmission` and a shared `PluginLease` (fe-runtime `src/AGENTS.md` §Typed plugin DB bridge).
- The DB re-checks at use. `authorize_as` (`fe-database/src/handlers/plugin_host.rs:15-44`) verifies the lease, the named capability, the pinned petal ancestry, and the role floor of **the DB's local DID**. `commit` requires Editor plus `storage.write` (`:192-206`).
- The op-log intention records `plugin_id`, petal, scope, `source_did` and the ops (`:409`; `OpType::PluginTransaction`, `fe-database/src/types.rs:15-16`).
- Per `fractalengine/src/AGENTS.md` §Plugin scene bridge: "No automatic package loading or new rule engine is implied by installing the host."

### 4.5 SDK and UI slots
- There are six `UiSlot`s: `InspectorSection`, `SidebarTool`, `ViewportOverlay`, `ToolbarAction`, `ContextMenu` and `DashboardWidget` (`fe-sdk/src/ui/mod.rs:11-24`). `UiContribution` is `{slot, plugin_id, label, icon}` (`:28-37`).
- Nothing in fe-ui or either binary consumes `UiExtensionRegistry`.
- The first-party terrain extension (`fe-terrain/src/extension.rs:10`) has a `register_terrain_ui` function (`:44-68`) that is never called. All of this is still true at `cfd7fce6`.

### 4.6 Device ingest surfaces
- Routes: `POST /api/v1/petals/{petal_id}/iot/readings` (`fe-api/src/server.rs:387`), device formats (`:392`) and device payloads (`:396`). Limits: 1 000 readings per batch and 10 requests/s per DID (`fe-api/src/limits.rs:25`, `:27`). Guard order: Editor+ → ULID → petal scope → rate → batch cap (fe-api `AGENTS.md` §iot-ingest).
- `insert_readings_with_replication` (`fe-database/src/handlers/iot_reading.rs:109`) is append-only and safe to call off the DB thread (fe-database `src/AGENTS.md` §iot-readings). **On the GUI binary, though, readings ride the DB thread anyway:**
  - `fractalengine/src/main.rs:703` starts the API with `spawn_api_service`, which marks the reader command-writes-only (`fe-api/src/lib.rs:178-179`, since `8a43f89e`).
  - `fe-api/src/iot.rs:102` therefore filters the reader out, and every batch takes the `DbCommand::InsertIotReadings` fallback (`iot.rs:146-235`). The "reading write timed out (db thread)" reply is at `:232`.
  - Only the relay keeps the direct path (`fractalengine-relay/src/main.rs:446`, `spawn_api_thread`).
- `SourceClass` (`fe-sdk/src/source_class.rs:11-22`) is one of Device, Connector, Decoder, Manual or Simulation. Simulation is "never presented as field data", and the class is never inferred (fe-sdk `src/AGENTS.md` §source-class).

### 4.7 Motion and route sampling
- `persist_transform` (`fe-api/src/rest.rs:483-528`, shared by REST and MCP `update_transform`) sends one `TransformPersist` command per call and then spawns `check_tracking_and_alert` (`:2294`). For a node with `tracking_route_id` (`:2312`), that function:
  - snaps the position with `PathTracker`;
  - fires four `SetNodeProperty` commands — `route_progress`, `distance_remaining_m`, `deviation_m` and `nearest_segment_index` (`:2356-2380`);
  - fires `tracking_alert = "off_route"` when the deviation passes the threshold (`:2384-2400`).

  That is up to six DB-thread commands per tracked transform, and each send result is discarded with `let _ =` (root `AGENTS.md` anti-pattern 6).
- The pure route math exists twice: `PathTracker::snap_to_route` / `position_at_progress` (`fe-terrain/src/iot/path_tracker.rs:62`, `:131`) and the SDK's shared sampler `path_geometry::position_at_progress` (`fe-sdk/src/path_geometry.rs:190`). `TrackAnimator` (`fe-terrain/src/iot/animation.rs:23`) plays entities along routes.

### 4.8 Simulation
- `FleetConfig` is DATA: peers, an ingest peer, anchors and `SensorSpec`s (`fe-sim/src/fleet.rs:42-55`, `:76-95`).
  - `plan_fleet` is pure (`:215`), with caps of 100 000 planned readings and 16 peers (`:27`, `:30`).
  - The sensor models (sine, weather, random_walk) are pure functions of `(config, tick, at_ms)` (`fe-sim/src/sensors.rs:24-58`).
  - FleetConfig has no movers: today's fleets are sensors only.
- The lab rides the same `VerseReplicator` trait as production (`fe-sim/src/AGENTS.md:3-6`).
- A live lab session installs the `SimClock` as the **process-global** HLC source. That is why the sim bridge lives only in `fractalengine-relay`, behind the `sim-control` feature plus `FE_SIM_ALLOW=1`, and never in the GUI binary (`fe-sim/src/AGENTS.md:302-310`).

### 4.9 Packaging and the registry client
- `HexonType` (`fe-format/src/manifest.rs:6-29`) has no `Plugin` variant. `HexonManifest` carries `publisher_did`, `integrity` and `signature` (`:274-311`).
- D-76 sealing uses one ed25519 signature over the canonical manifest, whose integrity root commits every member. Header members are limited to `HEADER_MEMBER_PATHS` (`fe-format/src/integrity.rs:31`), and `seal` (`:432`) is always the last transform (fe-format `src/AGENTS.md` §integrity).
- The registry client is `RemoteRegistryClient` (`fe-hexon/src/remote.rs:48`: search, manifest, download), which talks to fe-hexon-registry.

### 4.10 Measured constraint (city-lite run 1)
- **The load:** four drones at 2 Hz on one peer. Per tick that is one MCP `update_transform` per drone plus one REST readings batch (`tools/qa/sim/driver.py:398-436`).
- **The result:**
  - The owner's DB thread timed out 68 writes, plus a placement before the flight.
  - Replication latency p95 reached 431 s.
  - Sources: `qa/sim/runs/city-lite-20261107-144555/report.md`, and the steer bullet dated 2026-11-07 15:21 in `conductor/tracks/project_completion_20261104/checkpoint.md` (line 11 at `cfd7fce6`).
- **Root cause, per `cfd7fce6`:** timeseries partials ran back to back on the single DB thread at 4-5 s each, starving transforms and readings. Transform and ingest commands were not the hot spot. Every DB command slower than 250 ms now logs `slow DB command` with the queue depth behind it (`fe-database/src/lib.rs:2227`).
- **Consequence for this track:**
  - The DB thread is one serial queue shared by every write, and on the GUI binary readings ride it too (§4.6).
  - Today's path costs one command per mover per sample, and up to six for a route-tracked mover (§4.7).
  - Fleet motion and adapter writes therefore ride **one batched command per instance per tick** (R5), so queue depth stops growing with fleet size. A bypass path stays a measured fallback (OD-E5).

### 4.11 The role rule extension writes obey
- §write-time-role (`fe-database/src/AGENTS.md:1999`, `b852c4b1`): a scene row is admitted only if its author holds Editor at its scope now AND held it when the row was written. A demotion re-judges one batch per verse (fe-sync `src/AGENTS.md:2265` §rejudge-on-demotion).
- Plugin writes are authored by the host's local DID (§4.4), so peers judge them exactly like that user's manual edits.
- Provenance stays local today. `plugin_id` appears only in the journal facts and in the op-log intention (`fe-database/src/handlers/plugin_host.rs:237`, `:372`, `:409`). Replicated scene rows carry no extension provenance.

## 5. Scope, non-goals, boundary

**In scope** (open capability, this repo): R1-R36 — contract, runtime, bus, governance, authoring tools, Extensions UI, first-party migrations and simulator QA.

**Non-goals (hard):**
- **Catalogue operations.** No marketplace hosting, no payments, no publisher verification (review queues, identity vetting, counter-signing authorities), no account systems, no catalogue curation. The Extensions UI reaches any catalogue only through S-REGISTRY / S-ENTITLEMENT.
- **Portals and web bundles.** These belong to `portal_surface_20261107` (D-X5, D-X7, D-X8).
- **Third-party native code** (D-X1). Rhai stays for user macros and is not a package format.
- **Guest-held I/O.** A Wasm guest never holds a socket, a file handle, a clock source or an SQL string. Every effect is host-brokered and declared in the manifest.
- **Guest-drawn UI.** Contributions are declarative and host-rendered (OD-E8); rich UI belongs to the portal track.
- **Raw SQL for guests** (the 1.0 `query-select`). Queries go through the bus `host/query` stream and the REST guard (OD-E10).
- **P2P or NAT-relay capability for extensions**, closed by plugin_host_revival §3.
- **The lab clock in production.** The `SimClock` HLC override never runs in a production process.
- **Re-specifying plugin_host_revival.** Its signing, trust roots, consent modal, revocation list and registry install are built on here (R16-R23), not rewritten.

**Boundary:** whatever the private side builds is reached through S-REGISTRY (registry protocol v1, `PackageStore`) and S-ENTITLEMENT (entitlement tokens). Nothing in this repo names it, depends on it or feature-flags it (`conductor/open-core.md` §The rule).

## 6. Requirements and acceptance

Acceptance criteria are observable through public seams: REST, MCP, the GUI (agent input surface plus accessible names), and the scripted simulator (`tools/qa/sim`). Unhappy paths are listed only where they are reachable, distinct and consequential. Validation variants collapse into one table per requirement.

### 6.1 Package and contract (P0 text; built in P1-P2)

**R1 — Sealed plugin package (D-X1).** `HexonType::Plugin` joins `fe-format/src/manifest.rs`.
- **Members.** A plugin hexon carries:
  - `plugin/extension.json` — manifest v2, added to `HEADER_MEMBER_PATHS` so the header phase can render consent before the body is fetched;
  - `plugin/component.wasm` — a body member committed in `members.json`;
  - optionally, the existing `license.json`.

  `seal` is the last transform.
- **Identity.** `package_id` = `hexon_id`. `content_hash` = blake3 over the canonical signed manifest bytes (signature blanked). Those bytes commit the integrity root and therefore every member, so there is one value per sealed build.
- **Signatures.** The publisher signature IS the hexon manifest signature. Counter-signatures — plugin_host_revival Phase 2's `{key_id, signature}` list — sign the same canonical bytes.

*AC:* after `fe-ext pack`, installing either example package over REST succeeds. A table of post-seal tampering (a flipped component byte, an edited `extension.json`, an uncommitted extra member) is refused at install with a typed reason, and the package store holds nothing new afterwards.

**R2 — WIT `fractalengine:plugin@2.0.0` is the only third-party contract.**
- The host binds it with component-model bindgen, so no hand-written ABI faces third parties. This closes plugin_host_revival OD-2 through the second branch of its AC-8 ("bindgen now wires the two together").
- The 1.0 core-ABI `fe` imports survive only for first-party test fixtures, until P1 swaps them out.
- Until P1.1 lands, the WIT file carries an `UNSTABLE — not yet bound` header (plugin_host_revival tripwire).

*AC:* a component built against 2.0 activates and ticks. A 1.0 core module packaged as `HexonType::Plugin` is refused with `unsupported-contract`.

**R3 — Manifest v2.** A superset of `CapabilityManifest` (see the §6.9 example), with:
- a `kind` field — the content-type discriminator inside the Plugin family: `simulation`, `device-adapter`, `bi-connector` or `general`. `web-bundle` is reserved for `portal_surface_20261107`, which owns its `portal` manifest section (D-X5);
- explicit scopes — an empty list grants nothing, and `*` must be written out;
- a tick budget;
- the readings `source_class`;
- ingress and egress declarations, replacing `network_scope`/`external_http`;
- bus `expose` and `consume` declarations;
- declarative UI contributions.

The JSON Schema text lives beside the SDK types.

*AC (one validation table; `fe-ext validate` and REST install agree):* each of these is refused with a distinct reason — an empty scope list; egress to a non-https host other than loopback; a tick cadence below the host floor; a simulation-kind package declaring `source_class: device`; a UI contribution naming an undeclared command; a consumed stream that is not declared.

### 6.2 Runtime (P1)

**R4 — Worker-driven tick with a declared cadence budget.** The plugin worker (`lib.rs:250`) runs a deadline scheduler. It waits on the command channel until the earliest due tick, refuels (`refuel_for_tick`), calls `on-tick`, and then flushes the instance's write window (R5).
- **Wall-clock schedule.** When an instance falls behind, the host skips to the latest due instant, counts `ticks_skipped`, and reports the true `delta-ms`. Ticks never queue.
- **One flush in flight.** One commit per instance may be outstanding; the next tick waits for its reply. That is backpressure, not a queue.
- **Injected clock.** The scheduler takes an injected clock: the wall clock in every production process, and the `SimClock` only inside fe-sim's lab (R30).
- **Not a Bevy system.** Ticking never runs as a Bevy system, so the reactive render loop (`fractalengine/src/AGENTS.md` §render-loop) wakes only for the DB results the writes produce.
- **Fuel exhaustion** degrades the instance, and that tick's window is discarded unflushed.

*AC:* a 500 ms instance run for 60 s reports `ticks_run + ticks_skipped` = 120 ± 1 (`GET /api/v1/extensions/{installation}`). With a slowed sink in the journey, ticks are skipped and counted, never queued. A fuel-exhausting tick leaves the status `degraded` and commits nothing.

**R5 — One batched write per instance per tick (the motion path).** Writes are staged into a per-instance window and flushed exactly once per tick as ONE `DbCommand::PluginCommit`, containing:
- up to 64 scene ops in the existing atomic batch: `set-property`, `create-node`, `delete-node` and the new `set-transform` (`PendingOp::SetTransform`);
- up to 1 000 staged readings (R6), in the same command.

Behaviour of the flush:
- **Unchanged checks.** The DB re-checks the lease, capability, ancestry and the local DID's Editor floor at use, as today.
- **Out-of-tick writes.** Writes staged outside `on-tick` (from ingress, events or commands) wait for the next flush, so a writing extension must declare a cadence.
- **Transport timeouts** surface as `unknown-outcome` in the next `tick-info` and are never retried automatically (fe-plugin `src/AGENTS.md` §Scene host).
- **No derived route properties.** Unlike `rest.rs:2294-2400`, the host never derives per-sample route properties. Guests call `routes.snap` and persist what they need inside the same batch.

*AC:* in the ext-fleet scenario (four movers at 2 Hz), the owner's status shows `commits ≤ ticks_run` and four ops per commit. Transforms agree on every peer (simulator `transform` probe). At that rate the owner's host log has no `slow DB command` line for `PluginCommit` and no 504.

**R6 — Readings ingest through the existing door semantics.** `readings.stage` validates exactly as `insert_readings_with_replication` does: anchors inside the bound petal, an RFC-3339 or epoch-ms time, a non-empty metric, and at most 1 000 per flush. The readings flush with R5's command.
- `source_class` comes from the manifest and is never inferred: `simulation` is mandatory for the simulation kind; adapters declare `device` or `connector`.
- `source_did` is the local DID.
- Rows replicate through the existing per-row emit seam.

*AC:* staged readings are queryable through `/api/v1/query` on every peer within the scenario budget. An unknown anchor rejects the whole flush (typed `invalid-input`), and nothing from that flush lands.

**R7 — Route sampling.** `routes.sample(route, progress)` and `routes.snap(route, position)` read the route node's source through the bound reader. Both run ONE shared implementation: the SDK sampler (`fe-sdk/src/path_geometry.rs:190`) plus `PathTracker`'s snap math, moved into `fe-sdk` and re-exported by fe-terrain. Results are read-only. Route geometry is cached per instance until a `host/node-changes` event invalidates it.

*AC:* a mover sampled at progress p lands where fe-terrain's existing `test_position_at_progress` journey says it should. There is one implementation, so a single assertion covers both callers.

**R8 — Host-brokered ingress (websocket, MQTT, HTTP server).** Only declared endpoints exist.
- `http-server` mounts on the existing API server at `/api/v1/extensions/{installation}/ingress/{endpoint}`, behind the existing bearer-token auth, with per-endpoint rate and size limits.
- `websocket` either dials a declared `wss://` URL (client mode) or accepts upgrades on that same mount (accept mode).
- `mqtt` makes the host an MQTT client of a declared broker and topic filter.

Frames up to the declared size reach the guest's `on-ingress` hook on the worker. Broker and egress credentials live host-side; they never enter the replicated store or the guest.

*AC (table):* a declared endpoint's frames become readings. An undeclared endpoint answers the uniform not-found (fe-api §existence-oracle). Over-rate → 429. Over-size → 413. A disabled installation's endpoints → not-found.

**R9 — Declared egress.** `egress.http-request`, `mqtt-publish` and `ws-send` reach only manifest-declared endpoints (https, or loopback), with caps on response size, time and rate.

*AC:* a call to an undeclared host returns `capability-denied` and writes one audit entry (R21).

**R10 — UI slots, rendered declaratively.** Manifest-declared contributions to the six existing slots render host-side; the guest draws no pixels.
- **Command buttons:** ToolbarAction, SidebarTool, and ContextMenu (filtered by node properties).
- **Property rows:** InspectorSection shows key/value rows bound to node properties inside the declared property scope.
- **Stream values:** DashboardWidget and ViewportOverlay show a label plus the latest value of a stream.

Accessible names are `ext:<package_id>/<contribution_id>`. fe-ui finally consumes `UiExtensionRegistry`. Contributions with no command, such as today's terrain labels, are listed in the Extensions details pane rather than rendered as dead buttons, until R34 binds them.

*AC:* the fleet example's "Start fleet" toolbar action is found by its accessible name through the agent input surface, and starts the fleet. Disabling the installation removes it on the next frame.

**R11 — Status, counters and headless hosting.** `GET /api/v1/extensions` and `GET /api/v1/extensions/{installation}` report the state (installed | active | disabled | degraded | revoked) and these counters: ticks run and skipped, commits, ops, readings accepted, refusals by reason, unknown outcomes, ingress frames, bus events published, delivered and dropped, and fuel exhaustions. MCP gains a read-only `list_extensions` tool. Once plugin_host_revival S1 wires the relay host, the headless relay hosts extensions with no UI slots.

*AC:* every other AC in this spec reads its numbers from these routes. On the relay the routes behave identically, and `ui` contributions are reported as `headless`.

**R12 — Only the host's local identity administers extensions.** Install, consent, bind, enable or disable, update and uninstall are accepted only from the host's local DID — the identity the extension will write as — over REST or the GUI. Any other DID gets the uniform not-found. Otherwise a remote token holder could make the host write as the host.

*AC:* the same install call made with a remote editor's token returns not-found and leaves no trace except one `denied` audit entry.

### 6.3 Integration bus (D-X6; P0 contract, P1 broker)

**R13 — Host-brokered streams from the handlers REST already uses.** A broker on the plugin worker serves these host data streams, plus `host/admission` (R23). Each is scope-confined to the subscriber's binding and judged as the installing identity.
- **`host/query`** — a single SELECT, registered at subscribe time and re-run through fe-api's guarded core: `query_guard::guard_and_prepare_query` and `run_guarded_query_via_state`, the path `POST /api/v1/query` takes via `rest::execute_query`. It re-runs at a declared interval of at least 1 s; P0.3 decides whether a change-invalidation trigger also exists.
- **`host/readings`** — rows accepted by the readings door, both local and replicated.
- **`host/node-changes`** — the scene change feed that the `/ws` subscription carries (fe-api §websocket).

The binary injects these sources, mirroring the `HostEnv` injection idiom, so no new SQL path exists.

*AC:* for the same SQL and scope, a `host/query` subscription delivers exactly the rows `/api/v1/query` returns to the installing identity.

**R14 — Extension streams, commands and hooks.**
- **Publish.** An extension may publish only to streams it declares (`<package_id>/<stream>`): JSON events with a declared size cap and maximum rate.
- **Answer.** It may answer only commands it declares (`<package_id>/<command>`), each with an argument schema, a caller role floor and a rate.
- **Consume.** Through `on-event`, it receives only the host streams and foreign streams it declared under `consume` and that were consented. A producer may restrict a stream to named consumer packages.

Commands reach `on-command` with the caller's identity and kind (ui | rest | mcp | extension | portal). The caller's role floor is checked against the CALLER's DID at the binding scope. An extension caller is judged as its own installing identity.

*AC:* a consumer receives the fleet's `positions` stream. A caller below a command's floor gets the uniform not-found. A stream not declared under `consume` is refused at subscribe with `capability-denied`.

**R15 — Limits, delivery, audit and external access.**
- **Limits.** Each stream has an event-rate ceiling. Each caller has a command rate (default 10/s, the ingest precedent at `limits.rs:27`).
- **Delivery.** Subscriber queues are bounded. Streams drop the oldest event and count the drop; commands refuse with `rate-limited` once full. Nothing blocks the worker or the render loop.
- **Audit (R21).** Grants, subscriptions, command calls (actor DID, command, argument digest, outcome) and denials are audited. Individual stream events are not, because the data they carry is already in the log once it becomes rows.
- **External access** uses `POST /api/v1/extensions/{installation}/commands/{command}`, a `/ws` subscription target for extension streams under the existing deny-by-default rules, and the MCP tool `extension_command`. Portal IPC (`portal_surface_20261107`) is one more client of the same broker.

*AC:* driving a stream past its ceiling increments `bus.dropped` while the publisher's tick timing is unchanged. Each command call yields one audit entry. The WS and REST paths enforce the same floors.

### 6.4 Governance (P2) — built on plugin_host_revival and hexon_entitlements, not beside them

**R16 — Package loading stays opt-in.** The host thread already runs (§4.4), so plugin_host_revival OD-4's Cargo feature plus runtime flag now gates package admission instead.
- With the flag off, no package is admitted and the Extensions UI shows the off state.
- `FE_EXT_DISABLE_ALL=1` stops every instance on the host.
- Flipping the default to on is a later decision, taken only after plugin_host_revival S6's security review.

*AC:* with the flag unset, install is refused with `host-off`. Setting `FE_EXT_DISABLE_ALL=1` on a running host leaves every installation `disabled` within one tick.

**R17 — Signed packages and trust roots (plugin_host_revival S2, retargeted).** S2's ed25519 verification against `trusted_publishers.toml` (its OD-3) applies to the sealed hexon signature (R1). Unsigned packages are refused unless an explicit dev flag is set, and that flag is unreachable from release builds (plugin_host_revival tripwire). First-party packages are signed by the release pipeline's key (`release_ci_20260429`); dev builds use a dev trust root.

*AC:* plugin_host_revival AC-4's table, executed with `.hexon` packages.

**R18 — Per-verse install policy.** Each verse has a replicated policy row, written by Manager+ at verse scope (the `device_format` precedent, fe-api §device-ingest) and judged role-at-write on every replica like any row (§4.11).
- **Modes:**
  - `first-party-only` — the default when no row exists; admits packages signed by the release trust root;
  - `allow-list` — `package_id` plus an exact `content_hash` or a version range;
  - `trust-root` — anything a local trust root signed.
- **Capability ceilings** are part of the policy, for example `ingress: none`, `egress: none`, or bus external access off.
- **Local policy.** The host's own trust roots and local deny list also apply, and the stricter of the two wins. This is the S-RELAY-ADVERTISE pattern: a verse advertisement under local policy.

*AC:* a Manager can set the policy and an Editor is refused. A package outside the policy, or one that exceeds a ceiling, cannot be bound in that verse. Removing a package from the policy deactivates its instances in that verse on every peer within the scenario budget.

**R19 — Activation preconditions, in order, with typed refusals.**
1. `host-off`
2. `not-local-identity` (R12)
3. `package-invalid` — seal, integrity or manifest v2
4. `untrusted-signer`
5. `revoked`
6. `entitlement-required` — through S-ENTITLEMENT's `EntitlementVerifier`, with no new verifier code
7. `policy-denied`
8. `role-denied` — Editor+ at the petal for a writing extension, Viewer+ for a read-only one
9. `consent-missing` — no consent for this exact capability digest

Every later write is re-checked at use (§4.4).

*AC:* one table journey drives each refusal through REST and asserts its reason. The GUI shows the same reason text.

**R20 — Capability prompts (plugin_host_revival S3, extended).** S3's blocking consent modal renders manifest v2: the binding, scene scopes, the readings class, every ingress and egress endpoint, bus exposure and consumption, UI contributions, signer and trust state, the policy verdict and the licence state.
- Enable stays disabled until the full list has rendered once (S3's AC-5).
- An update whose capability digest differs re-prompts, and the old version keeps running until consent is given. An update with no delta does not prompt.
- REST consent (for headless hosts and QA) must echo the exact capability digest.

*AC:* updating to a build that adds one egress host re-prompts. A REST consent carrying a stale digest is refused.

**R21 — Audit through the log.** Each of these actions appends one op-log operation: install, update, consent, bind, unbind, enable, disable, uninstall, revocation, policy change, bus grant, command call and denial.
- **Path.** Entries go through `op_log::commit_operation`, the live log-first path (fe-database `src/AGENTS.md:580` §log-first-commit).
- **Shape.** A single new `OpType` variant with a typed action and `{installation, package_id, content_hash, actor_did, target, outcome}`.
- **Canonical log.** The canonical signed log receives these entries through whatever projection the op-log has. Its SurrealDB half is "parallel and dormant" today, apart from device enrolment (§canon-log-materialization, `:1491`).
- **Visibility.** The trail is local by default (OD-E7), readable at `GET /api/v1/extensions/{installation}/audit` and in the Extensions UI.

*AC:* each listed action yields exactly one audit entry carrying its actor DID and outcome, denials included.

**R22 — Role-at-write for extension rows.** Extensions write as the installing identity through the typed bridge and never with more authority. They use no `ElevatedQuery`, no `RawQuery` and no caller SQL — `plugin_host.rs` uses none today, and that must stay true. Peers judge these rows exactly like the identity's manual rows (§4.11).
- Demoting the identity refuses its next commits at use, and the existing demotion re-judge withdraws its rows on peers.
- A re-grant admits new writes, while rows from before the break stay out.

*AC (simulator):* demoting the installing editor mid-flight raises its instance's `refusals.role`, and peers drop its rows. After a re-grant, new rows converge and pre-break rows stay absent.

**R23 — Kill switch layers and arrival semantics.** There are four layers:
- (a) local Disable — one installation or one binding;
- (b) `FE_EXT_DISABLE_ALL=1` — the whole host;
- (c) removal from the verse policy — that verse, on every peer;
- (d) the S-PLUGIN signed revocation list (plugin_host_revival §4, OD-8 transport), keyed by `(package_id, content_hash)` and accepted only from revocation-capable trust roots.

When a host accepts a list that names an installed pair, it refuses activation AND stops running instances: the lease is revoked within one poll. A commit that was already admitted completes with its true receipt — revocation is not rollback (fe-runtime §Typed plugin DB bridge).

Admission records are local and keyed by `(package_id, content_hash)`; they are never replicated. Every admission change (admitted, disabled, policy-denied, revoked) is published on the bus as `host/admission`, so dependent surfaces such as portals react without polling.

*AC (simulator):* a published list stops the instance on every peer that trusts its root within the budget, and no row from the package lands afterwards.

**R24 — Host-stamped provenance and withdrawal (needs OD-E1).** Every scene version and reading written through the extension path carries host-stamped provenance `{package_id, content_hash}`. The guest can neither set nor strip it.
- **Where it lives.** In the versioned scene snapshot, beside the version's write clock; and in additive optional `iot_reading` fields that are omitted when absent (the `payload_id` precedent).
- **Withdrawal.** A host that accepts a revocation list re-judges the affected verses — one batch per verse, under the §rejudge-on-demotion queue discipline — treating versions with revoked provenance as inadmissible. The newest admissible version is restored, or the node is removed.
- **Readings** with revoked provenance are excluded at the one scope-substitution seam and on the distributed partial path.
- **Reversible.** Removing the trust root restores visibility; nothing is physically deleted.

This amends the S-PLUGIN contract text, which today only says "refuse activation", so it is a versioned seam change.

*AC (simulator):* after the list arrives, every peer that trusts the root shows the fleet's extension-created nodes removed and its moved nodes restored to their last non-extension version, and the fleet's readings are absent from `/api/v1/query`. A node property the installing user edited by hand survives.

### 6.5 Authoring tools (P3)

**R25 — `fe-ext` CLI.** It lives in `tools/fe-ext`, is written in Rust, and reuses fe-format sealing and the fe-plugin component host.

| Command | Does |
|---|---|
| `new <kind> <name>` | Scaffolds a guest crate, a manifest v2 file and a journey test |
| `build` | Compiles the component for `wasm32-wasip2` and checks it against the 2.0 world |
| `validate` | Runs the R3 table plus the host ceilings |
| `pack` | Builds the plugin hexon |
| `sign` / `countersign` | Signs the package |
| `inspect <pkg>` | Prints identity, signer, `content_hash` and capability digest |
| `run --sandbox --fixture <file>` | Runs the component in the real host against an in-memory scene backend (fe-plugin-test's mock host), with a deterministic tick clock, fed by fe-sim fixtures (fleet JSON, recorded ingress frames) |

`--lab` (multi-peer, via fe-sim) arrives with R30.

*AC:* both examples go from `new` to a signed package that installs on a dev host. `validate` exits non-zero with the table's reasons. Two `run --sandbox` executions of the same fixture print byte-identical fingerprints.

**R26 — Guest SDK and docs.**
- `tools/fe-ext/guest` gives Rust authors the generated 2.0 bindings plus helpers: the staged write window, `routes` snap-to-alert that matches REST's `tracking_alert` semantics, and readings batching.
- `docs/extensions/` holds the contract reference (WIT 2.0, manifest v2, bus), the governance and capability reference, the authoring guide, a cookbook, and a 1.0 → 2.0 migration note. Each guide links real example files rather than copying them.

*AC:* every doc code block points at a file that exists in the examples, checked by one link check in the P6 docs pass.

**R27 — Two examples.** These seed the first-party packages in R30 and R31:
- `extensions/fleet-sim` — simulation kind: route-following movers plus sensor readings, from a FleetConfig v2 document;
- `extensions/device-adapter` — device-adapter kind: HTTP, websocket and MQTT ingress → readings plus transforms, with actuation through egress.

`extensions/iot-bridge` stays as the Rhai macro reference (D-X1).

*AC:* both pass R25's sandbox run and drive the P6 scenarios.

### 6.6 Extensions UI (P4)

**R28 — Extensions panel.**
- **Views:**
  - Installed — name, version, publisher, signer and trust state, status, update available;
  - Details — description, bindings, counters, audit trail;
  - Permissions — consent record, the capability list in plain language, revoke consent.
- **Actions:** Enable / Disable per binding; Update (re-verify, and re-prompt on a capability delta); Uninstall; Install from file.
- **Design rules** (`C:/Users/atooz/.gemini/UI-UX-guidance.md`): one primary action per view, stable accessible names, and real empty, loading, error and disabled states.
- **Wiring.** fe-ui has no direct access to the host, so it queues extension operations and the binary bridge executes them. This is the Hexon Manager pattern (`fractalengine/src/terrain_bridge.rs` `drain_hexon_ops`; root `AGENTS.md` §petal-map).

*AC (GUI QA via the agent input surface):* install from file → consent → enable → disable → update with a capability delta (which re-prompts) → uninstall, with every state visible under its accessible name.

**R29 — Pluggable registry endpoint behind S-REGISTRY.** A `RegistrySource` trait with three operations:
- `list` / `search`;
- `header` — returns the header document with `plugin/extension.json`, so consent can render before download;
- `fetch` — returns sealed bytes, which are always verified locally.

Sources:
- **Public defaults:**
  - Local — a file or folder of `.hexon` packages;
  - Static signed index — an `index.json` listing `{package_id, version, content_hash, url, size}`, signed by a trust-root key with the revocation list's canonical-JSON rule, and cached for offline use.
- **Protocol-v1 registries** (fe-hexon-registry) plug in through plugin_host_revival S4's client, which checks `X-Hexon-Registry-Protocol` first.
- **Anything else** is reached only through this trait, or through a protocol-v1 endpoint the operator configures.

Licensed packages go through S-ENTITLEMENT unchanged.

*AC:* Browse lists a loopback static signed index. A tampered index, a bad package signature and a protocol-mismatched registry are each refused with a distinct reason.

### 6.7 First-party migrations (P5) — fe-sim first

**R30 — fe-sim becomes the first-party simulation extension; a fleet stays DATA.**
- **Model split.** The pure model (`fleet.rs`, `sensors.rs`) moves into a wasm-compatible crate under `fe-sim/` with no I/O dependencies.
- **FleetConfig v2** adds movers: `{node, route, speed_mps, loop, start_offset_s}`. The v1 lab fields (`peers`, `ingest_peer`, `start_ms`) stay lab-only.
- **Fleets are data.** A fleet lives as a petal node whose `fleet.config` property holds the document.
- **One driver.** Exactly one live instance drives a fleet, claimed with a `fleet.driver` property written in its own batch. A peer that sees another live driver stays passive.
- **Wall clock on live hosts.** On a live host the extension runs on the wall clock; the lab's `SimClock` never enters a production process (§4.8).
- **The lab hosts the same package.** The lab keeps SimNet, faults and `/api/v1/sim/*` (relay with `sim-control` plus `FE_SIM_ALLOW=1`), and hosts the SAME package under the `SimClock` through R4's injected clock. Lab and live therefore cannot drift — D3's same-trait principle, extended to the extension.

*AC:* one FleetConfig v2 document gives the same fingerprint on two lab runs. On a live three-peer swarm (the ext-fleet scenario) its movers converge within the budget.

**R31 — IoT device adapters: one adapter shape.** One manifest kind, `device-adapter`: ingress → decode → staged readings and transforms, with actuation through declared egress or bus commands.
- Simulated devices (fe-sim sensor models behind the simulator's virtual HTTP and MQTT devices) and real devices run the same package. Only the declared `source_class` differs: `simulation` versus `device` or `connector`.
- The REST ingest route stays as the no-extension door, with the same validation.

*AC:* identical frames sent through the adapter and through `POST .../iot/readings` produce identical reading rows, apart from provenance and `source_class`.

**R32 — Hexon registry client behind `RegistrySource`.** `RemoteRegistryClient` is re-homed behind R29's trait, so the Hexon Manager (tilesets) and the Extensions UI share one client, one trust model and one protocol check. It stays first-party and native.

*AC:* the existing Hexon Manager journeys pass unchanged through the trait.

**R33 — BI egress connectors.** A connector is an extension that consumes `host/query` or `host/readings` and delivers rows through declared egress. First-party wave 1:
- The Copy-for-BI targets become connector descriptors (target id, label, snippet builder) registered through the extension surface. The builders in `fe-ui/src/gis/egress_strings.rs` stay compiled-in built-ins that no package list can revoke.
- One push connector (JSON or CSV over HTTPS to a declared endpoint, on an interval) ships as a signed first-party package.

The egress API itself — `/api/v1/query`, export and share — stays core and unchanged.

*AC:* Copy-for-BI strings stay byte-identical (the existing `egress_strings.rs` journeys). The push connector delivers to a loopback sink the same rows `/api/v1/query` returns.

**R34 — Terrain aligned.** The terrain extension's metadata (`TerrainExtensionInfo`) becomes a manifest v2 constant, and its three contributions bind to the existing GPX-import and terrain actions through commands. It stays native and first-party.

*AC:* plugin_host_revival AC-2's registry assertion holds, and each terrain contribution renders and triggers its existing action.

### 6.8 Simulator QA (P6)

**R35 — Cross-peer scenarios.**
- **`tools/qa/sim/scenarios/ext-fleet.json`** — three peers, in order:
  1. policy → install → consent → bind;
  2. four movers at 2 Hz for 3 minutes;
  3. bus command floors through REST and WS;
  4. demote, then re-grant, the installing editor mid-flight;
  5. publish the revocation list.
- **`ext-device.json`** — an adapter with HTTP and MQTT ingress from virtual devices; parity with the REST door; disable, then enable.
- **New driver ops:** `install_extension`, `consent_extension`, `bind_extension`, `set_extension_policy`, `extension_command`, `ingress_post`, `mqtt_publish`, `publish_revocation`.
- **New probe:** one `extension` probe that reads R11's status.

Together the scenarios must hold the observables in R5, R18, R22, R23 and R24.

*AC:* two clean runs of each scenario (expectations met within budget, zero untriaged S1/S2), both schema-valid against `tools/qa/sim/scenario.schema.json`.

**R36 — Review gates.** Every phase ends with an ADVOCATE pass and then a CRITIC pass, each in a fresh context and each fixing its own findings.
- `/security-review` is mandatory for P1, P2 and P4: untrusted code, capabilities, IPC/bus, packaging, signing and ingress.
- `/code-review high` is mandatory for P1 and P2.
- One verdict line per finding goes in the track's `review.md`.

*AC:* a phase with no recorded verdict is not done.

### 6.9 Sketches (non-normative; P0 writes the normative text)

The WIT 2.0 world below shows the contract's intended names. P0.1 writes valid WIT across per-interface files.

```wit
package fractalengine:plugin@2.0.0;
// imported: types, scene (read), txn (staged writes), readings (staged), routes, egress, bus, kv, log
// exported: lifecycle, ingress-hooks, bus-hooks
world extension {
  import scene;     // get-node, query-nodes(NodeQuery json) - bound petal only
  import txn;       // stage-set-property, stage-set-transform, stage-create-node, stage-delete-node
  import readings;  // stage(list<reading>) - flushed with the tick's commit
  import routes;    // sample(route, progress) -> vec3; snap(route, vec3) -> snap
  import egress;    // http-request, mqtt-publish, ws-send - declared endpoints only
  import bus;       // subscribe(source, filter), unsubscribe, publish(stream, event), call(target, args)
  import kv;        // per-extension namespaced key/value
  import log;
  export lifecycle;     // on-install, on-activate(binding), on-deactivate, on-uninstall, on-tick(tick-info)
  export ingress-hooks; // on-ingress(endpoint, frame) -> reply
  export bus-hooks;     // on-event(sub, event); on-command(command, args, caller) -> result
}
// host-error: capability-denied | policy-denied | invalid-input | budget-exceeded |
//             rate-limited | revoked | not-available | backend | unknown-outcome
// tick-info:  { delta-ms, scheduled-at-ms, skipped, last-commit: committed(receipt) | refused(host-error) | unknown }
```

Manifest v2 (`plugin/extension.json`):

```json
{
  "manifest_version": 2,
  "id": "org.example.fleet-sim", "version": "0.1.0", "kind": "simulation",
  "contract": "fractalengine:plugin@2.0.0",
  "binding": "petal",
  "scene": { "read": true, "write": ["create_node", "set_transform", "set_property"],
             "property_scope": ["fleet.*"] },
  "readings": { "source_class": "simulation", "metrics": ["altitude_m"] },
  "routes": true,
  "tick": { "cadence_ms": 500, "max_ops": 64, "max_readings": 256, "fuel": 200000 },
  "ingress": [],
  "egress": [],
  "bus": {
    "expose": { "streams": [{ "id": "positions", "max_rate_hz": 4, "max_event_bytes": 4096 }],
                "commands": [{ "id": "start", "role_floor": "editor", "rate_hz": 1 }] },
    "consume": [{ "source": "host/node-changes" }]
  },
  "ui": [{ "slot": "ToolbarAction", "id": "start", "label": "Start fleet", "command": "start" }]
}
```

## 7. Unhappy paths (reachable, distinct, consequential)

| # | Path | Tested in | Requirement |
|---|---|---|---|
| U1 | Tampered sealed package | install tamper table | R1 |
| U2 | Unsigned package or untrusted signer, dev flag absent | plugin_host_revival AC-4 table | R17 |
| U3 | Revoked package: install refused, running instance stopped | ext-fleet scenario | R23 |
| U4 | Verse policy refusal, or a capability above the ceiling | activation table | R18, R19 |
| U5 | Administration from a non-local DID | extensions API journey | R12 |
| U6 | Installing identity demoted mid-flight | ext-fleet scenario | R22 |
| U7 | Undeclared egress host, ingress endpoint or bus stream | ingress / egress / bus tables | R8, R9, R14 |
| U8 | Ingress over rate or over size | ingress table | R8 |
| U9 | Tick overrun (skip and count) or fuel exhaustion (degrade, no flush) | tick journey | R4 |
| U10 | Commit transport timeout → `unknown-outcome`, no automatic retry | tick journey | R5 |
| U11 | Update with a capability delta | consent journey | R20 |
| U12 | Command caller below the floor (REST, WS, extension) | bus table | R14, R15 |
| U13 | Licensed package without an entitlement | S-ENTITLEMENT verifier path | R19 |

## 8. Proposed decisions (unratified — proceed on the default; post as bus `question`s)

| ID | Question | Recommended default | Reversal cost |
|---|---|---|---|
| OD-E1 | Should the kill switch withdraw rows (R24), amending the S-PLUGIN text from "refuse activation" to "refuse, stop and withdraw host-stamped extension rows"? | Yes, as a versioned seam change in `conductor/open-core.md` after ratification. If refused, R24 shrinks to stop-and-refuse and the scenario step adapts. | Medium (the provenance fields are additive, and withdrawal can be switched off) |
| OD-E2 | Should first-party behaviour extensions be native or Wasm? | Wasm for simulation and device adapters, which dogfoods the third-party path. Native for terrain, the registry client and the BI built-ins. | Medium |
| OD-E3 | Binding scope | One petal per activation, as `PluginContexts` / `DbSceneFactory` pin today. No verse-wide binding in v1. | Low |
| OD-E4 | Host tick constants | Floor 100 ms. No default tick: a writing extension must declare one. At most 64 ops and 1 000 readings per flush, one commit in flight, fuel per tick capped by the host. P1 measures. | Low |
| OD-E5 | Motion path | Batched (R5) now. Append-only motion samples with `TrackAnimator`-style display only if a post-fix measurement says batching is not enough. | Low |
| OD-E6 | Ingress surface and MQTT client | HTTP and websocket accept on the existing API server (one HTTP surface to secure). The MQTT client crate is chosen in P1 under `deny.toml` review. Credentials stay host-side and are never replicated. | Medium |
| OD-E7 | Audit visibility | Local op-log only. Replicating the audit trail to verse Managers is a later decision. | Low |
| OD-E8 | UI contributions | Declarative and host-rendered. No guest-drawn UI; rich UI belongs to portals. | Low |
| OD-E9 | How readings are withdrawn | Read-path exclusion, which is reversible, rather than physical deletion. | Medium |
| OD-E10 | Raw SQL for guests | Not in 2.0; `host/query` goes through the REST guard instead. | Low (can be added later) |

## 9. Risks

- **Untrusted code goes live at scale.** This inherits plugin_host_revival's wasmtime advisory risk. P1 waits on its S0 bump, and package loading stays off by default (R16).
- **Network capability is new attack surface.** It is host-brokered only, declared, consented, capped by policy, rate-limited and audited (R8, R9), and `/security-review` is mandatory.
- **The DB thread is shared.** Batched flushes stop per-instance queue growth, but not growth from many instances together. P6 measures host-wide ceilings (active instances × cadence) against the `slow DB command` log.
- **Provenance withdrawal touches the freshest P2P code** — the re-judge queues from QA-0121, QA-0131 and QA-0132. Its partition stays unpartitioned until the P2P campaign merges, and R24 is gated by OD-E1.
- **WIT 2.0 may churn before bindgen exists.** The file carries an UNSTABLE header until P1.1, and no third party is invited before P1's review passes.
- **Scope creep toward catalogue operations.** Any diff that adds business logic, accounts or curation is a non-goal violation (`conductor/open-core.md`).
- **Fleet driver claims can race** when two peers claim a fleet at once. The HLC decides last-writer-wins, and the loser stops within one tick. This is recorded, not hidden.

## 10. Dependencies

- **plugin_host_revival_20261031:**
  - S0 (wasmtime bump) before P1.1.
  - S1 (OD-1 enforcement, relay host wiring, the `register_terrain_ui` call, route mounting) before P1.
  - S2-S4 (signing, consent modal, revocation-list consumer, registry install) before P2.
  - S6's security review before any change to the default.

  It runs in the EXT-PLATFORM session as this track's prerequisite lane (plan A-1), and its spec and plan stay authoritative. Alignment proposals for its owner:
  - **AC-1 no longer matches the GUI binary.** Since `2cc05f27` the host thread always runs, so OD-4's flag should gate package admission (R16).
  - **S3's planned `fe-ui/src/panels/plugin_install_panel.rs`** should be created at `fe-ui/src/extensions/consent.rs`.
  - **S4's protocol-v1 client** should land as the protocol-v1 `RegistrySource` in fe-hexon. fe-plugin then stays fetch-free and only admits sealed bytes.
- **hexon_entitlements_20261031** — registry protocol v1, `PackageStore` and `EntitlementVerifier` (R19, R29). Its S5 format-spec 1.1.0 bump lands before this track's `HexonType::Plugin` doc edit.
- **agent_input_surface_20261031**, which runs inside agent_vision_20261031 as I1-I5 — GUI QA for R10 and R28.
- **Blocks:** `portal_surface_20261107`, which needs the bus (R13-R15), WIT 2.0 and governance (R16-R24).
- **Related, satisfied (archived):** plugin_host_20260509, extension_sdk_ui_20260509, plugin_testing_dx_20260509, analytics_extension_api_20260710, iot_extension_slice_20260710, simulation_lab_20261009, iot_spatial_reporting_20260714.
- **Related, active:**
  - project_completion_20261104 — the P2P campaign, which owns the build gate;
  - release_ci_20260429 — the first-party signing key;
  - relay_operability_20261031 — relay hosting;
  - auth_policy_pattern_20260710;
  - role_propagation_20261013.
