---
type: checklist
title: Round 4 QA checklist (public tracks)
updated: 2026-11-13
---

Compiled from 29 of 210 slices (round-4 swarm; nothing below has been built or run yet).
Tick a box only after you saw the stated result. Re-running the scribe adds new slices and keeps every ticked line as it is. `(derived)` = the worker left no QA line, so the step was written from the slice summary and files.

## 1. How to run

Three ways to verify. Tests and simulator runs only when you name them (AGENTS.md test policy); never `cargo test --workspace`.

- **In the app.** Stage a build and launch an isolated run with a seeded fixture (`basic` = offline terrain on Petal B, `terrain` = on Petals A and B):
  - `tools\qa\stage.ps1 -Jobs 2` then `tools\qa\launch.ps1 -Fixture basic` (add `-ApiPort N` to pick the loopback API port)
  - drive it with `python tools/qa/fe.py <command>` (screenshots, camera read-back, synthetic input; `--run <run>` binds a specific run); stop with `tools\qa\stop.ps1`
  - details: `tools/qa/AGENTS.md` (§fe-py, "Fixture modes and oracle")
- **API / simulator.** The API listens on the run's loopback port (owner token is read by `fe.py`; use it as a Bearer token for `curl`). Simulator: `python tools/qa/sim/run.py validate` (no app), then `python tools/qa/sim/run.py p2p-core --peers owner,editor --steps 0-3 --allow-stale-stage` (2-peer dry run; full coverage needs owner, editor, viewer, editor2). Scenarios live in `tools/qa/sim/scenarios/`.
- **Test batches.** `tools\qa\test_batches.ps1 -Batches R1,R8` (add `-ListOnly` to print the chunks without running cargo). Batch ids and filters are in `conductor/test-batches.md`; section 6 lists the ones this round touches.
- Kinds used below: `app` = in the GUI, `api` = REST/MCP/env/process, `sim` = simulator scenario, `batch` = named test batch.

## 2. Checks by track

### map_scale_authority_20260716
- [ ] CITY-02 · app · (derived) Launch `-Fixture basic`, open Petal B (map scale 1:100), import a plain GLB, open the new node in the inspector -> scale shows 0.01 / 0.01 / 0.01. Repeat on a petal with no map -> scale 1/1/1.
- [ ] CITY-02 · app · (derived) Create a node by the Add-node action on Petal B -> it is placed at the same 0.01 scale and still has that scale after you restart the run with `launch.ps1 -ResumeRun`.
- [ ] CITY-03 · app · (derived) Open Hexon Manager -> Installed, click "Set petal map" on a tileset, then reopen the petal -> the shown world scale is a value inside the tileset's scale bounds (never 0, NaN or a huge number), and inspector metre readouts for a node on that petal still convert correctly (a 100 m Measure tape reads 100.0 m).

### road_builder_ux_20260716
- [ ] CITY-10 · app · (derived) With edit rights, press L in the viewport -> the toolbar highlights the new Road tool and the viewport hint changes; with Viewer rights, L does nothing.
- [ ] CITY-10 · app · (derived) With Road active, click an existing node and then empty ground -> the selection does not change either way (Road never selects or deselects).
- [ ] CITY-05 · app · (derived) Launch `-Fixture basic`, pick Pen, click on a raised part of Petal B's terrain -> the anchor point sits on the surface (not underground); on a petal with no terrain the anchor is at height 0.
  - not finished: `mod ground_snap;` is not yet declared in `fe-ui/src/node_manager/mod.rs` (CITY-15), so this does not compile yet; a Pen press-drag on a slope may count height in the drag threshold (not checked).
- [ ] CITY-08 · app · (derived) Import a GPX file, then draw a track by hand; open each track node's property list -> the imported one has `path_kind = recorded`, the hand-drawn one `path_kind = designed`; both keep their existing gpx_type, gpx_points and name properties.
- [ ] CITY-09 · app · (derived) Once the Road tool is wired (CITY-11/13), place three points on one road -> one track node is created after the first point and the other two points are appended to it in click order (reload the petal and compare).
  - not finished: the ordered-flush test is not written; `fe-ui` does not compile until `path_point_interaction.rs` is fixed.

### relay_operability_20261031
- [ ] NET-03 · api · (derived) Start the API with `FE_API_USAGE_LOG=<file>` set, call `GET /api/v1/health` with no token and `GET /api/v1/petals/{petal_id}/gis/nodes` with a token -> the file gains two JSON lines; both show the route template (`{petal_id}`, not the real id), the second shows the caller, the first none. (needs the binary wiring from I-BIN to be in)
- [ ] NET-04 · api · (derived) Start with `FE_API_METRICS_BIND=127.0.0.1:9100`, then `curl http://127.0.0.1:9100/metrics` -> OpenMetrics text with `fe_api_request_duration_seconds_bucket`, labelled by route template only.
- [ ] NET-04 · api · (derived) Request 50 different nonexistent URLs, fetch `/metrics` again -> they add at most one `<unmatched>` series (no per-URL growth). With `FE_API_METRICS_BIND` unset nothing listens on 9100, and the normal API port answers `/metrics` with 404.
- [ ] NET-09 · api · (derived) As verse Manager: `POST /api/v1/verses/{verse_id}/relays` with `{"url":"https://relay.example.org/"}` -> 201 `{relay_id,url}`; then `DELETE /api/v1/verses/{verse_id}/relays/{relay_id}` -> 204.
- [ ] NET-09 · api · (derived) Same POST as Viewer -> 403 before any URL check; as Manager with `"url":"relay.example.org"` (no scheme) -> 400; with no node signer configured -> 503.
- [ ] NET-06 · batch · (derived) Run `tools\qa\test_batches.ps1 -Batches R8` -> the `__relays` book tests pass: a row from a below-Manager author is held back (quarantined), a row whose `advertised_by` is not its signer is refused, a Manager+ row is admitted, demoting that Manager withdraws it.
- [ ] NET-07 · batch · (derived) In the same R8 run, `a_forged_shards_row_from_a_stranger_never_reaches_the_ledger` passes and its `__relays` leg shows the stranger's row not admitted while the owner's row is.
  - not finished: `fe-sync/src/lib.rs` has no `mod relay_advertise;` yet (the slice does not compile until it is added); the WriteRowEntry test leg and closing the book on `CloseVerseReplica` are not done.
- [ ] NET-08 · api · (derived) Two-peer verse where a Manager advertises `https://relay.example.org/`: with `FE_SYNC_RELAY_ADVERTISE_POLICY` unset (dial-only) the advertised relay never enters the node's relay map; with `pinned` it does while advertised and is removed after the tombstone; with `deny` it is never even listed.
- [ ] NET-11 · api · (derived) Set `FE_MAX_DOWNLOAD_RATE=1000000` and download a ~5 MB tileset from a peer -> takes at least 5 s; unset, `0` or `abc` -> no throttling.
- [ ] NET-17 · api · (derived) With the `relay-server` feature, start the embedded relay with `FE_RELAY_SERVE=1` and `FE_RELAY_SERVE_ACCESS=open` -> it refuses to start with a named error; with `FE_RELAY_SERVE_METRICS_BIND=0.0.0.0:9200` -> refuses (`metrics_not_loopback`); with hosted verses unset -> refuses (`hosted_verses_unknown`).
  - not finished: process wiring (NET-24/NET-30) is not in; the three journey tests were never run.

### peer_networking_support_20261031
- [ ] NET-17 · app · (derived) With the relay running for one verse: a member peer connects through it; a non-member is denied; removing the member's role disconnects them within about a second, even if they reconnect at the same moment.

### per_key_clock_20261113
- [ ] CLOCK-02 · sim · (derived) Run `p2p-core` with revoke-then-regrant of the editor: an edit stamped before the revoke is kept; an edit stamped between the revoke and the regrant is refused on every peer; after regrant the editor can write again -> all peers show the same node values.
- [ ] CLOCK-02 · batch · (derived) `tools\qa\test_batches.ps1 -Batches R7,R10` -> role-grant window tests pass, including the changed assertion "X's row at 5 after revoke and re-grant is Editor, not None" and the new "at 10 is None" check.
  - not finished: `fe-database` does not compile until the NEEDS (CLOCK-03..) land; any older test that asserts "break restarts, old row refused" must be re-checked (R-CLOCK-1).

### hexon_unification_20260716
- [ ] HEXON-02 · api · (derived) Build a hexon package, open it -> signature valid and integrity `Sealed`. Flip one asset byte, rename a manifest field, strip the seal, or swap the publisher DID in separate copies -> none of them reports a valid signature (a tampered sealed archive fails to open; an unsealed one opens as unverified).
- [ ] HEXON-03 · api · (derived) Against a dev registry, publish a built hexon with `RemoteRegistryClient::publish` (`POST /api/v1/hexons/publish`, raw bytes), search for it, download it -> found by id and tag, downloaded bytes are identical to what was published.
  - not finished: fe-hexon and fe-api do not compile until HEXON-04..07 rename `crate_id`/`HexonKind` users; `PluginHexonBuilder` still uses the zip 0.6 option type.

### hexon_scale_orchestration_20260712
- [ ] HEXON-19 · app · (derived) Launch `-Fixture basic`, draw a 100 m Measure tape on Petal B and promote it to a dimension callout -> a "100.0 m" label is drawn at the callout in the viewport, moves with the camera, and disappears when the callout is off-screen or behind the camera; the new node is named "Dimension 100.0 m".
- [ ] HEXON-19 · app · (derived) Do the same on a petal with no map -> the label reads in world units (no NaN).

## 3. Checks the plan already owed

Work that is not code: in-app looks, simulator runs, sweeps and decisions the plan classed blocked-user (plan section 6.2).
### CLOCK
- [ ] OWED-CLOCK-1 · sim · run `p2p-core` to verify the per-key clock (when you name it)

### NET (relay_operability, peer_networking, iroh_1_0_upgrade, p2p_asset_streaming)
- [ ] OWED-NET-1 · batch · relay_operability gates W1/W2/W3: serial build + tests per wave
- [ ] OWED-NET-2 · api · relay_operability AC-6: backup runbook round-trip against a running relay
- [ ] OWED-NET-3 · api · relay_operability AC-7: relay-assisted sync against the documented compose service
- [ ] OWED-NET-4 · batch · peer_networking gates W1..W5, including `cargo check -p fe-sync --features relay-server` and the relay/binary relay-server builds
- [ ] OWED-NET-5 · batch · peer_networking F3: run the P15 e2e log, `cargo deny check` over iroh-relay/server, full sweep
- [ ] OWED-NET-6 · api · production embedded relay on a real domain (ACME/operator cert), desktop relay serving, OD-3 self-signed phase (needs a provisioned host)
- [ ] OWED-NET-7 · batch · iroh 1.x sweep: fe-sync incl. `legacy_v035_fixture_migrates_and_reads_back`, fe-sim, fe-identity, fe-test-harness loopback
- [ ] OWED-NET-8 · api · iroh flag-day rollout of deployed peers/relays before 2026-12-31; commit/push; decide how to package the 7.1 MB redb fixture; remove the socket2 shim after a macOS CI run
- [ ] OWED-NET-9 · batch · Harness scenario 16 `two_peer_tileset_stream` (two-instance loopback transfer)
- [ ] OWED-NET-10 · app · tileset streaming: no spawn spike and frame-time delta on tile faults (NFR-1/NFR-2), in the app
- [ ] OWED-NET-11 · batch · conformance: `cargo test -p fe-hexon --all-features` (hosted and P2P sources pass the same suite)
- [ ] OWED-NET-12 · batch · p2p_asset_streaming single end-of-track sweep
- [ ] OWED-NET-13 · app · p2p_asset_streaming retro + archive after the sweep and NFR evidence
- [ ] OWED-NET-14 · api · decide the full FetchStrategy rebase for P2P downloads (see section 5)

### HEXON
- [ ] OWED-HEXON-1 · batch · hexon_unification phase 1: fe-format tests green, zero new deps, review the enum by eye (R11)
- [ ] OWED-HEXON-2 · batch · phase 2: fe-hexon and fe-hexon-registry tests (R11/R11-STORE); grep for `crate_id` and `.fecrate` is clean
- [ ] OWED-HEXON-3 · api · phase 3: cargo test -p fe-api (R9) and a manual `curl` publish against a dev node
- [ ] OWED-HEXON-4 · app · phase 4: export a real dev petal and decide with real data whether iot_reading rows stay in
- [ ] OWED-HEXON-5 · app · phase 5: export from node A, import on a wiped node, open it in the app
- [ ] OWED-HEXON-6 · batch · phase 6: sweep green, then conductor retro and close-out
- [ ] OWED-HEXON-7 · batch · hexon_entitlements gates W1/W2/W3 (cargo check)
- [ ] OWED-HEXON-8 · batch · hexon_entitlements full sweep, once
- [ ] OWED-HEXON-9 · app · hexon_entitlements: look at the Hexon Manager states, then retro + archive
- [ ] OWED-HEXON-10 · app · hexon_scale_orchestration phase 6: grid snaps, re-subdivides and fades; callout persists (real petal scale) plus the sweep

### EXT
- [ ] OWED-EXT-1 · app · extension_platform P4.4 GUI QA per `fe-ui/src/extensions/AGENTS.md` §gui-qa-path
- [ ] OWED-EXT-2 · sim · P6.3 two clean runs each of ext-fleet, ext-device, ext-install-gui, ext-bindings, then triage
- [ ] OWED-EXT-3 · app · plugin_host_revival sweep: workspace tests and the in-app consent/opt-in check
- [ ] OWED-EXT-4 · app · portal spikes SP-1..SP-9 on WebView2 (your Windows box)
- [ ] OWED-EXT-5 · sim · portal T5.5: run PJ1-PJ9 and the portal sim scenarios, `run.py validate portal-*` receipts, full sweep once (PJ8 is native-only)
- [ ] OWED-EXT-6 · app · RUNBOOK H4 earthwork petal authorization; R-GATE-11 GUI create augments; R-GATE-15 GUI queue test; door-parity journey; GPX serial-create deferral (owned by group NEW)
- [ ] OWED-EXT-7 · app · R-Z-16 / R-BIN-21: decide whether a bound `portal.open` bundle may mount by package without a repository node (see section 5)
- [ ] OWED-EXT-8 · app · mint a scoped share link from the Copy-for-BI card in-app (needs a GUI-to-API seam; documented instead by EXT-13)
- [ ] OWED-EXT-9 · app · QA-0174: GPX path vs 1:100 terrain (nodes at -27 m, focus-node 100x off); re-look after CITY-08 and the scale slices
- [ ] OWED-EXT-10 · sim · QA-0165 role-at-write decision and clock track (owned by CLOCK)
- [ ] OWED-EXT-11 · app · registry install shows real download progress (user decision 2026-11-13; sequence after EXT-16)
- [ ] OWED-EXT-12 · app · R-ADMIN-07: panel answers a concurrent CLI install with "consent declined" (kept, fails closed)
- [ ] OWED-EXT-13 · app · deferred, triggers not fired: R-UI3-16 context-menu property cache; R-UI3-17 App-level journey for draw_context_slot
- [ ] OWED-EXT-14 · api · deferred: R-STORE-16 constant-time token compare; R-STORE-17 fe-ext index verify reuse
- [ ] OWED-EXT-15 · api · deferred: R-TOOL-10 city-map example crate; R-TOOL-12 run.py staging a second example version
- [ ] OWED-EXT-16 · api · deferred: R-BND-10, R-PLUG-16 / R-NET-18 frame buffering, G0-C26, G5-C16, R-HOOKS-07, R-HOOKS-28, R-API-23, G3-C15
- [ ] OWED-EXT-17 · api · deferred by C-PHR: AOT `compile_cached` has no caller; `verify_revocation_list_fresh` has no production caller
- [ ] OWED-EXT-18 · app · deferred portal items: R-POR-IPC-21, R-POR-IPC-22, P1-09, P3-10 focusable badge, P3-11 "Admit after the cause is lifted"
- [ ] OWED-EXT-19 · app · out of scope by your default: M4 store experience, payment integration, KMS/HSM, default store endpoint, licensed-tileset row states, G2-C17, dropping tauri

### CITY
- [ ] OWED-CITY-1 · app · road_builder_ux phase 3: 3-segment chain, restart, track persists
- [ ] OWED-CITY-2 · app · phase 4 (AC-6.2): ghost colouring, guides vs radius, snap indicators, metre readout vs a known map distance, Alt suspend, screenshots
- [ ] OWED-CITY-3 · app · phase 5: confirm the manual verification outcomes; checkpoint commit
- [ ] OWED-CITY-4 · app · map_scale_authority phase 2: same GLB on 1:1 and 1:100 petals shows correct relative size
- [ ] OWED-CITY-5 · app · phase 5: the four in-app checks plus the sweep
- [ ] OWED-CITY-6 · app · phase 4: place a model on elevated terrain -> it sits on the surface

### UI
- [ ] OWED-UI-1 · app · ui_shell_architecture phase 0: FR-1/FR-2 repro matrix (plan.md:90)
- [ ] OWED-UI-2 · app · phase 1: click-claim behaviour (plan.md:124)
- [ ] OWED-UI-3 · app · phase 2: pixel-parity smoke (plan.md:161)
- [ ] OWED-UI-4 · app · phase 3: inject a panic in a scratch panel (plan.md:187)
- [ ] OWED-UI-5 · app · phase 4: every workflow from the right sidebar (plan.md:217)
- [ ] OWED-UI-6 · app · phase 5: tooltips, viewport wider (plan.md:241)
- [ ] OWED-UI-7 · api · decide release panic=unwind (ratified keep abort 2026-07-25; only if you ask)
- [ ] OWED-UI-8 · app · polish_lane W3 VISUAL / W6 VISUAL and your in-app look
- [ ] OWED-UI-9 · batch · terrain_proposal_delta_viz full sweep, once
- [ ] OWED-UI-10 · app · terrain_proposal_delta_viz: verify F5 in-app
- [ ] OWED-UI-11 · app · inspector_settings phase 1/3/4 manual checkpoints (plan.md:58,133,217)
- [ ] OWED-UI-12 · app · agent_vision AC-11 incl. minimized-window scene capture
- [ ] OWED-UI-13 · app · agent_input_surface: with `FE_API_INPUT_ALLOW=1`, Ctrl+V is refused
- [ ] OWED-UI-14 · batch · agent_vision / agent_input_surface full sweep, once

### DATA
- [ ] OWED-DATA-1 · app · RD1: typed value/draft/error/persistence proof across all layouts (staged build)
- [ ] OWED-DATA-2 · app · property grid at three sizes (Edit/Copy/Delete widths), fresh stage
- [ ] OWED-DATA-3 · app · tree adapter: final menu and destructive-action layout checks
- [ ] OWED-DATA-4 · app · RD4: node history v3 preview, chart/table, final coverage
- [ ] OWED-DATA-5 · sim · real Both scenario with QA-0035 repair, plus a same-stage restart (`tools/qa/rich_data_scenario.py`)
- [ ] OWED-DATA-6 · sim · two clean full-coverage passes; keep `catalog_source_fingerprint` null until the staged build
- [ ] OWED-DATA-7 · batch · final integrated checks, sweep, CI push and tagged artifact (you say when)
- [ ] OWED-DATA-8 · app · thin tree icons and primary/additional/partial row states: live multi/partial check
- [ ] OWED-DATA-9 · app · ordered selection and Inspector/Options/tool routing: alias, derived, concurrency-stream, QA-0069
- [ ] OWED-DATA-10 · app · group creation/membership, mixed properties, capability refusals, pivot, persistence
- [ ] OWED-DATA-11 · sim · committed-node publication/recovery: fresh paired run on two peers (`tools/qa/peers.ps1`)
- [ ] OWED-DATA-12 · app · closed-path witness: transform, Group, Undo, GPU picking proof on a rendered build
- [ ] OWED-DATA-13 · sim · local-copy adoption: owner adoption, peer bytes, failed-loader recovery, rendered GLB
- [ ] OWED-DATA-14 · sim · QA30 two-member Group baseline on the retained store restart; native Viewer/Editor observation gap
- [ ] OWED-DATA-15 · api · endpoint_api_surface phase 3: trigger op (decision, see section 5)
- [ ] OWED-DATA-16 · batch · endpoint_api_surface phase 6: sweep and in-app/endpoint verify
- [ ] OWED-DATA-17 · api · QA-0116, QA-0128, QA-0144 triage (see section 5)

### REL
- [ ] OWED-REL-1 · api · pick the Windows signing provider, set the Azure signing secrets and variables, confirm a signed run
- [ ] OWED-REL-2 · api · release_ci 2.7: run the release workflow end to end on a tag (7 archives, no macOS)
- [ ] OWED-REL-3 · api · cut the first tag: `v0.1.0` or `v0.1.0-qa.YYYYMMDD` (your push)
- [ ] OWED-REL-4 · api · oss_release 5.3, 5.4, 5.5, 5.6, 6.2, 6.5 (disclosure, SECURITY.md contact, CI scope, publishing gate, register review, first public push)
- [ ] OWED-REL-5 · app · project_completion P0 feature-catalog reconciliation; P2/P3 live-acceptance (QA-0024, QA-0069, selection/group edges, same-object conflict, rich-data Both)
- [ ] OWED-REL-6 · app · P1 native/UIA acceptance on the interactive desktop (provider root, NAV23, QA-0072, accessibility helper, QA0053)
- [ ] OWED-REL-7 · app · require-existing startup: live restart and fixture checkpoints without reseeding (retained stores held)
- [ ] OWED-REL-8 · app · QA-0063 peer node delivery, QA0073 camera ownership, QA0074 layout, VIS-001..003, QA47 invalid insertion: fresh peer/render/restart acceptance
- [ ] OWED-REL-9 · batch · P4 fmt/test/harness/iot-bridge sweep; P5 push, tag, wait for Release, check downloads/hashes/GHCR, retro
- [ ] OWED-REL-10 · batch · crate_consolidation phase 4 close-out: sweep, docker image build, retro (nothing to run while G-1..G-3 are DEFER)
- [ ] OWED-REL-11 · api · Windows code signing (needs the secrets from OWED-REL-1)

## 4. Security verifications

- [ ] SEC-1 · api · `fe-runtime/src/relay_advertisement.rs`, `fe-api/src/relay_advertise.rs` (relay URL request forgery). As Manager, `POST /api/v1/verses/{verse_id}/relays` with each of `http://127.0.0.1:8080/`, `http://localhost/`, `http://[::1]/`, `http://169.254.169.254/`, `http://192.168.1.10/`, `http://10.0.0.5/`, `http://user:pw@relay.example.org/`, `ftp://relay.example.org/` -> every one is refused (400); only a public https host is accepted. Write the same URLs as a `__relays` row from a Manager peer -> none shows up as an advertised relay or is ever dialled. (Today only "http/https plus host" is checked; verify after R-NET-1 / NET-14.)
- [ ] SEC-2 · api · `fe-database/src/handlers/crud.rs`, `fe-format/src/scale.rs` (no global scale cap, f64 to f32 overflow). Set a petal map `world_scale` to `1e40`, `1e-40`, `0`, `-1` and a non-number, then import a GLB and create a node there -> the node's scale is always finite and inside the map's scale bounds (never `inf`, `NaN` or 0) in the inspector, in `GET` node responses and after restart.
- [ ] SEC-3 · batch · `fe-sync/src/relay_advertise.rs` (one global quarantine FIFO). Have one Viewer-role author write more than 1024 `__relays` rows, plus one row from a second Viewer -> the second author's held row is still there, `quarantine_evicted` counts only the flooder's rows, and admitted rows from Managers are unaffected (R8 + R-NET-2).

## 5. Defaults taken without asking

### CLOCK
- Peers upgrade together, no dual-format writing, no version negotiation (clocked rows unreadable by old peers) — say if you want it changed.
- A revoke closes the author's grant window at the highest write clock the revoker had received from that author, so honest concurrent edits can be dropped — say if you want it changed (alternative: close at the revoke's own clock).
- Residual risk accepted: a revoked author can forge an old-clock write at or below the frontier for a key nobody wrote since — say if you want it changed.
- Grant history capped at 8 closed windows per role statement — say if you want it changed.
- Every member's latest version of every key is kept forever, property deletions included (no GC yet) — say if you want it changed.
- Role statements gain an optional `windows` field; mixed old/new swarms disagree about past edits — say if you want it changed.
- Node transform is one clocked key (max of clock, then author DID); `edit_seq` retired for clocked rows — say if you want it changed.
- `verse.namespace_id` is treated as insert-once identity, not a governance key (CLOCK-03) — say if you want it changed.
- Revoke then re-grant: the closed window keeps the old role for edits stamped inside it (CLOCK-02) — say if you want it changed.

### NET
- An advertised relay becomes home-eligible when every verse you are in advertises it; Settings toggle default ON — say if you want it changed.
- Only the download-rate cap for P2P tileset downloads (per download, not global); no full FetchStrategy rebase — say if you want it changed.
- Gossip signing is compat (`FE_SYNC_GOSSIP_SIGNING` unset); no date set to require it — say if you want it changed.
- Unsigned v1 `__relays` rows stay admissible for one release (`FE_SYNC_RELAY_ADVERT_SIGNED=require` refuses them) — say if you want it changed.
- No provisioned production relay host before the 2026-12-31 flag day — say if you want it changed.
- Advertised relays default to dial-allowlist only (`FE_SYNC_RELAY_ADVERTISE_POLICY` unset = dial-only; `pinned` inserts, `deny` ignores) — say if you want it changed.
- Only a Manager or above can advertise a relay; relay id is ASCII alphanumeric, at most 64 bytes; note at most 256 bytes — say if you want it changed.
- The embedded relay refuses to start unless access is members-only, hosted verses are named and metrics bind is loopback — say if you want it changed.
- `FE_LOG_FORMAT` unknown values fall back to text silently — say if you want it changed.

### HEXON
- Licensed tilesets: an "entry" is a member path inside the archive (preview tiles via `free_entries`) — say if you want it changed.
- REST asset fetch of a licensed entry answers 403 `entitlement_required` (no token header) — say if you want it changed.
- Snapshot import needs Manager+ on an already-existing parent fractal; it never creates a fractal or verse — say if you want it changed.
- Snapshots include iot_reading rows and only roles scoped exactly at the petal — say if you want it changed.
- `crate_registry` / `crate_entry` rows change to composite ids with no migration — say if you want it changed.
- A built package is sealed, a publisher DID mismatch is silently corrected to the signer, and an unsealed archive opens as unverified — say if you want it changed.
- Hexon builder default licence is Free and `min_engine_version` is empty — say if you want it changed.

### EXT
- A bound `portal.open` bundle with no repository node stays "bundle not on this peer" (fail closed) — say if you want it changed.
- Copy-for-BI card does not mint share links in-app; docs explain the token route — say if you want it changed.
- The seeded Genesis Verse stays local-only, so Generate invite is disabled with a reason — say if you want it changed.
- `blob.mirror` mirrors only blobs committed on this host — say if you want it changed.
- An expired issuer with a still-valid stored token shows Held — say if you want it changed.
- Window title peer label comes from `FE_PEER_LABEL` only — say if you want it changed.
- The Azure signing secret keeps its current name and is allow-listed in the leak grep — say if you want it changed.

### CITY
- Road tool hotkey is L (B is Brush, A/D fly keys); alternatives T or O — say if you want it changed.
- Pen anchors also snap to the terrain surface — say if you want it changed.
- Map world scale applies to new nodes even when the petal's terrain is disabled — say if you want it changed.
- Road snap radius is 3 m in world space (not screen space) — say if you want it changed.
- Road needs edit rights; new tracks are named "Road N"; Backspace undoes in-flight points first — say if you want it changed.
- An unparsable world scale falls back to 1.0 — say if you want it changed.

### UI
- Cross-sections (UI-03..UI-05) ship now; answer "spin out" to drop them — say if you want it changed.
- Section line is a chord through the volume centroid at an angle you set — say if you want it changed.
- Access home stays as the Access tab of Entity Settings, linked from the hierarchy info card — say if you want it changed.
- "Managers only" preset sets default access none and revokes explicit grants below Manager at that scope, behind one confirm — say if you want it changed.

### DATA
- No trigger ops exposed on REST/MCP (T5-TRIGGER option a) — say if you want it changed.
- Final coverage passes and the fresh Both scenario stay out of round 4 — say if you want it changed.
- QA-0116, QA-0128, QA-0144 are left to QA triage — say if you want it changed.

### NEW
- Dead-letter Retry sends again without Undo history or selection re-bind — say if you want it changed.
- Retry bounds 8 attempts (50 ms doubling to 2 s), 32 dead letters, session only — say if you want it changed.
- Registry installs capped at 64 MiB, whole-package progress only — say if you want it changed.
- Earthwork/callout follow-up writes stay blocking with the node's petal — say if you want it changed.

### REL
- crate consolidation G-1/G-2/G-3 all DEFER — say if you want it changed.
- Linux release signing with minisign and a key you generate — say if you want it changed.
- Windows signing via Azure Trusted Signing; unsigned until you set the secrets — say if you want it changed.
- First tag is `v0.1.0-qa.YYYYMMDD`, `v0.1.0` only after QA passes — say if you want it changed.
- CI test sweep stays scoped and README says so — say if you want it changed.

## 6. Test batches to name

- **R1** hierarchy and navigation (CITY-02 placement default, `handlers::crud::`).
- **R2** scene editing (CITY-04 `node_manager::ground_snap`, CITY-10 dispatch/router, DATA-02 `editor_batch::`, NEW-01..04 door/undo/stamp journeys, HEXON-19 `actions::dimension_callout`).
- **R3** inspector (UI-06 `panels::hierarchy_info`, DATA-01 property form).
- **R4** terrain and maps incl. IoT sub-batch (UI-01..05 volume/section profile, EXT-10/11 readings, HEXON-15 tileset entitlement, fe-terrain scale tests; needs `--features render`).
- **R5** paths and stamps (CITY-05..13 road builder, CITY-08 gpx_bridge, CITY-09 id test in `gis/mod.rs`).
- **R6** dialogs and shell (CITY-10 toolbar/tool_options/tool_inspector, EXT-14 `window_chrome::`, NET-31 `dialogs::peer_debug`, UI-07/08, DATA-03, EXT-18).
- **R7** roles (UI-07 `role_propagation_durable_test`, CLOCK-04 `role_propagation_local_test`, CLOCK-02 role windows).
- **R8** P2P sync (NET-05/18/20 fe-runtime filters, NET-06/07/08 relay book, NET-11 tileset transfer, relay_serve with `--features relay-server`, NET-33 `peer_relay_e2e` slow, NET-30 `relay_serve_bridge`, CLOCK-11 replicator).
- **R9** API and MCP (NET-01 `usage::`, NET-04 `metrics::`, NET-09/25/32 relay and diagnostics routes, HEXON-12 `export_share_test`, EXT-05, EXT-13).
- **R10** identity and persistence (CLOCK `op_log::`, `handlers::replicated_row::`, `field_clock::merge::tests::`, HEXON-09/10/13 `petal_snapshot_test`).
- **R11** plugin, hexon, format (fe-format scale and hexon type tests via `-- --skip portal_bundle::`, fe-hexon package/manifest/signature tests, HEXON-01/02/03); **R11-STORE** (`fe-hexon --features remote --test remote_registry_test`, EXT-04, HEXON-03, NEW-05); **R11-ENT** (EXT-01 governance with `--features entitlement`, HEXON-14, `entitlement_gate::tests::`).
- **Portal QA lane** (not cargo): EXT-05 `portal_ipc_post` journey; EXT-06/07 spikes on your Windows box.
- **Misc**: EXT-08 docs link check (`tools/qa/docs_link_check.py`, no cargo).
- **Harness / Slow**: `two_peer_tileset_stream` and other two-peer scenarios; campaign-end only.
- **Simulator** (when you name it): `p2p-core` (CLOCK-12), `fleet-terrain` (EXT-15).
- **Full sweep**: `tools\qa\test_batches.ps1 -Batches All`, once, at campaign end.
