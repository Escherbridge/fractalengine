---
type: Review
track: extension_platform_20261107
gate: C-EXT
round: 4
reviews: [T-EXT, R-EXT-1, R-EXT-2, R-EXT-3, R-INT-1, R-INT-2, I-DOC-UI, I-DOC-API, I-DOC-CORE, I-BATCH-1]
checked_at: b817b97e (plus the uncommitted working tree)
date: 2026-11-13
---

# C-EXT: extension_platform_20261107 completeness at round-4 HEAD

Every spec requirement (R1-R37), every plan task (P0.1-P6.5) and every round-4 item of this track (EXT-01..03,
EXT-08..14) checked against the tree at `b817b97e`. Code-first round: nothing was built or run, so "complete" means
written, wired and reached by a batch filter, never "tested". Scoper-excluded items are listed under their class at
the end, not as gaps. Sister files: `../plugin_host_revival_20261031/completeness-turn4.md` (EXT-04, EXT-16, EXT-17)
and `../portal_surface_20261107/completeness-turn4.md` (EXT-05..07).

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` (entries 4-15 and triaged 1-3) names a file of this
group (extension_host/, plugin_bridge.rs, export.rs, petal_readings, gis/readings, window_chrome, qa_portal,
portal_bundle, terrain_bridge, ruler_plugin). Nothing to confirm or refute.

## Round-4 items

| Item | Status | Evidence |
|---|---|---|
| EXT-01 licence token client (H-4 of turn 3) | complete | `fractalengine/src/extension_host/licence_client.rs:26` `LicenceTokens` (pinned keyring `:43`, governance store `:46`, one client per source `:52`, `request` `:65`, empty keyring sends nothing `:67`); checked POST `sources.rs:386,422` over the GET client (`Policy::none()` `:317`, `.no_proxy()` `:321`, `resolve_to_addrs` `:324`); `SourceSet::token_transport` `sources.rs:158`; caller `ops_drain.rs:843`; bridge `plugin_bridge.rs:3419-3434`, built only with `entitlement` + licence keys `:4449-4461`; identity key handed by `main.rs:571`. Journey `governance.rs:1873-1953` (R11-ENT `test-batches.md:471`). |
| EXT-02 blob.mirror tap (H-5) | complete | `extension_host/blob_mirror_tap.rs:19` `mirror_job_for` (only `GltfImported` with a materialization `:23`, `model/gltf-binary` `:13`), system `:45` calls `after_commit` `:57`; registered `plugin_bridge.rs:4716`; `BlobMirror::pending` `fe-plugin/src/bindings/egress.rs:161`; tests `blob_mirror_tap.rs:63` (R2 line `test-batches.md:404`). |
| EXT-03 R37 install/update offers | complete | `extension_host/registry_offers.rs:15-19` bounds (3 versions, 16 header fetches, 32 packages), `refresh` `:50`; ops worker `ops_drain.rs:50` (10 min), `:1429-1498` wanted set + refresh + `registry_offers_changed`; plugin worker reads it `plugin_bridge.rs:2580` (`registry: self.registry_offers.snapshot()`), one shared instance `:4718,4749,4782`; `gated_offer` `fe-plugin/src/bindings/resolver.rs:494`, `filter_offers` `ops_drain.rs:1464`. |
| EXT-08 R26 docs link check (H-8) | complete (unrun) | `tools/qa/docs_link_check.py` (R-EXT-1 F3-F5 fixes); registered `conductor/test-batches.md:534`; doc `tools/qa/AGENTS.md:1672` §docs-link-check. |
| EXT-09 P6.5 docs folds + retro | partial | Retro `plan.md:262` (§8); portal T2.3 annotated `portal_surface_20261107/plan.md:300`; `fe-plugin/src/AGENTS.md` §registry-install header-optional and licensed lines corrected (`:317-331`); plugin-host folds `fractalengine/src/AGENTS.md:2207,2216,2222`. Remaining: H4-1, H4-2 below. P6.5 stays unticked (`plan.md:258`). |
| EXT-10 measurements show the latest points | complete | `fe-database/src/handlers/node_measurements.rs:59` `ORDER BY recorded_at_ms DESC, reading_id DESC`, reversed `:84`; label "Showing the latest {} points." `fe-ui/src/object_data/measurements.rs:175`; test row `("m", 2, vec![11.0, 12.0], true)` `fe-database/tests/entity_property_test.rs:569` (R3 `test-batches.md:114`); doc `fe-database/src/AGENTS.md:702`. |
| EXT-11 petal readings summary read | complete | `fe-runtime/src/petal_readings.rs`, `lib.rs:25`; `DbCommand::ReadPetalReadings` `messages.rs:1179`, `DbResult::PetalReadingsLoaded` `:1587`; hub arm `fe-database/src/lib.rs:1961-1963`; lane `command_lane.rs:253`; scene_write `:678`; handler `fe-database/src/handlers/petal_readings.rs` (key (node, metric, units), skipped oversize names, R-EXT-3 R2-R4); test `entity_property_test.rs:622`. |
| EXT-12 Readings tab (live) | partial | Written but NOT COMMITTED at HEAD: `fe-ui/src/gis/readings.rs` is untracked; `gis/mod.rs:9,41,96,133`, `panels/gis_panel.rs:97,173` and `verse_manager/db_results/mod.rs:203-211` (poll + route) are working-tree edits (`git status`). Code matches the task (R-EXT-3 R12). HANDED-OFF H4-3. |
| EXT-13 snapshot-safe readings export + BI snippet | complete | `fe-api/src/export.rs:31` `as_of_ms`, sort + cutoff `:150-175`; `fe-ui/src/gis/egress_strings.rs:27` `EgressSource::Readings`, `readings_sql` `:134`, URL `:142`, test leg `:356`; card `fe-ui/src/panels/egress_card.rs:80-137` (Refresh snapshot `:137`), share line `:265`; `docs/bi-egress.md:171-180`; `fe-api/AGENTS.md:963-965`. |
| EXT-14 window title | complete | `fractalengine/src/window_chrome.rs:86-91` `FE_PEER_LABEL` read once, `window_title` `:108`, `sync_window_title` `:152` (writes on change, R-EXT-1 F2), table test `:187` (R6 `test-batches.md:222`); registered `fractalengine/src/main.rs:870` (R-EXT-1 F6 reached I-BIN-1); doc `fractalengine/src/AGENTS.md:1110`. |
| EXT-15 fleet-terrain scenario | complete (unrun) | `tools/qa/sim/scenarios/fleet-terrain.json` (fixture `terrain`), `tools/qa/sim/assets/roads/road-{a,b,c}.gpx`; doc `tools/qa/sim/AGENTS.md:68`. Bounds/height assertions DEFERRED by R-EXT-1 F14 (stated in the scenario description). |
| EXT-18 Genesis verse says why it cannot be shared | partial | Built in `fe-ui/src/dialogs/entity_access.rs:43,55,169` (`LOCAL_ONLY_REASON`, `mark_verse_local_only`), but `mark_verse_local_only` has no caller in `fe-ui/src` (R-UI-3-06 HANDED-OFF to I-UI-1, not applied), so the reason never shows. HANDED-OFF H4-4. |

## Requirements (spec R1-R37)

| Req | Status | Evidence |
|---|---|---|
| R1 sealed plugin package | complete | `fe-hexon/src/package.rs:394` `verify_plugin_package`; tamper table `:1063`, `:1207-1228`, control `:1275` (EXT-04). |
| R2 WIT 2.0 only | complete | `fe-plugin/wit/fractal-plugin-2.wit:1` "BOUND"; bindgen `fe-plugin/src/wasm/component/bindings.rs:7`. |
| R3 manifest v2 | complete | `fe-sdk/schema/examples/*.extension.json`; `fe-ext validate`. |
| R4 worker tick | complete | `fe-plugin/src/tick.rs:33,238`. |
| R5 one batched write | complete | `fe-database/src/handlers/plugin_host.rs:276,491`. |
| R6 readings door | complete | `plugin_host.rs:665` (R-BND-10 race is blocked-user, below). |
| R7 route sampling | complete | `fe-sdk/src/path_geometry.rs:190,284`. |
| R8 ingress | complete | `fe-api/src/extensions.rs:850,854`. |
| R9 egress | complete | `fe-plugin/src/net/tests.rs:450`. |
| R10 UI slots | complete | `fe-ui/src/extensions/slots.rs:337`, `bound_slots.rs:261`. |
| R11 status + headless | complete | `fe-api/src/extensions.rs:444,450`; relay `fractalengine-relay/src/main.rs` (feature `plugin-host`). |
| R12 local identity only | complete | `fe-api/src/extensions.rs:911`; `extension_host/admin_channel.rs:164`. |
| R13-R15 bus streams, commands, limits | complete | `fe-plugin/src/bus/broker.rs:100,222,665`; `fe-api/src/extensions.rs:846`. |
| R16 opt-in admission | complete | `fe-plugin/src/lib.rs:205`; `lifecycle.rs:29`. |
| R17 signed + trust roots | complete | `fe-plugin/src/trust_roots.rs`; `extension_host/governance.rs:38`. |
| R18 verse policy | complete | `DbCommand::SetExtensionPolicy`. |
| R19 preconditions in order | complete | turn-3 H-7 obsolete (plan §6.2): `lifecycle.rs:663`, `docs/extensions/governance.md` §Activation preconditions; retro `plan.md:262`. |
| R20 capability prompts | complete | `fe-ui/src/extensions/consent.rs:65,141`. |
| R21 audit | complete | `fe-database/src/handlers/extension_binding.rs:633`. |
| R22 role-at-write | complete | bound queries only in `plugin_host.rs`. |
| R23 kill-switch layers | complete | panel + CLI host switch (`ops_drain.rs` `set_host_switch`), `revocation_pull.rs:45`. |
| R24 provenance, stop not withdraw | complete | `plugin_host.rs:264`; `iot_reading.rs:229`. |
| R25 fe-ext CLI | complete | `tools/fe-ext/src/main.rs:8`. |
| R26 guest SDK + docs | complete (unrun) | docs under `docs/extensions/`; AC link check now `tools/qa/docs_link_check.py` (EXT-08), run in refine. |
| R27 examples | complete | `extensions/fleet-sim`, `extensions/device-adapter`, `extensions/bi-push`. |
| R28 Extensions panel | complete (code) | `fe-ui/src/extensions/panel.rs`; GUI QA run is blocked-user. |
| R29 RegistrySource | complete | `fe-hexon/src/registry_source.rs:192`. |
| R30-R31 fe-sim, device adapters | complete | `fe-sim/model/src/fleet.rs`; `extensions/device-adapter/src/lib.rs`. |
| R32 Hexon client behind trait | complete | `fractalengine/src/terrain_bridge.rs` `RegistrySource::list` + `fetch_hexon`. |
| R33 BI connectors | complete | `fe-ui/src/extensions/builtin/copy_for_bi.rs:30`; Readings source added (EXT-13). |
| R34 terrain aligned | complete | `fe-terrain/src/extension.rs:13`. |
| R35 sim scenarios | complete (unrun) | `tools/qa/sim/scenarios/ext-{fleet,device,install-gui,bindings}.json` + `fleet-terrain.json`. |
| R36 review gates | complete | `review.md`, `review-turn2/`, `review-turn3/`, `review-turn4/R-EXT-{1,2,3}.md` (all PASS-WITH-FIXES). |
| R37 scoped bindings + install offers | complete | resolver `fe-plugin/src/bindings/resolver.rs:246`; offers now fed (EXT-03 row). |

## Plan tasks

| Task | Status | Evidence |
|---|---|---|
| P0.1-P3.5, P4.0-P4.3, P5.1-P5.7, P6.1, P6.2, P6.4 | complete | ticked by T-EXT (`plan.md` §7); code as in the R rows. |
| P4.4 G4 gate + GUI QA | gate complete; GUI QA blocked-user | `plan.md:244-245`. |
| P6.3 two clean runs + triage | blocked-user | `plan.md:255-256`. |
| P6.5 docs consolidation + retro | partial | EXT-09 row. |

## Gate hand-offs: did each reach its owner?

- R-EXT-1 F6 (I-BIN-1, title system): reached, `main.rs:870`. F7 (I-BATCH-1): reached, `test-batches.md:534`.
  F8 (I-DOC-API / I-DOC-CORE): reached, `fractalengine/src/AGENTS.md:1110,2230`, `tools/qa/AGENTS.md:1672`,
  `tools/qa/sim/AGENTS.md:68`, `fe-terrain/src/AGENTS.md:567-568`. F9 (I-DOC-API): NOT reached, H4-1.
  F13 (CLOCK-12, multipart GPX driver op): NOT reached (driver.py has only `upload_asset` multipart, `:474`), H4-5.
- R-EXT-2 R2-04 (I-BATCH-2): reached, `test-batches.md:443,450,471`. R2-05 (I-BIN-2, warn on a dead binding worker):
  NOT reached, `plugin_bridge.rs:2167` still `tracing::debug!`, H4-6.
- R-EXT-3 R14 (I-DOC-CORE): NOT reached, `fe-database/src/AGENTS.md:704` still says "latest-per-(node, metric)", H4-2.
  R15 (I-DOC-API): NOT reached, `fe-api/AGENTS.md:963-965` lacks the anchor-position and share-link caveat, H4-2.
- R-UI-3-06 (I-UI-1, EXT-18 wiring): NOT reached, H4-4.
- R-INT-1 / R-INT-2: no EXT hand-off; their EXT lines are KEPT-verified (`ReadPetalReadings` arm, ipc-post route, licence fail-closed, shared RegistryOffers, blob tap).

## User-question defaults (turn4-plan.md §9)

- q18 bound portal.open, node-only: matches. No writer of `BrowserCommand::OpenBoundBundle`; `UiAction::OpenBoundBundle` only selects a node (`fe-ui/src/actions/mod.rs:1366`, `inspector.rs:2563`).
- q19 no GUI-to-API share seam: matches. `egress_card.rs` has no HTTP client; the card says "Parquet/CSV links need a petal-scoped token." (`:265`) and `docs/bi-egress.md:171` documents the mint path.
- q20 Genesis stays local-only: the default is coded (`entity_access.rs:55`) but not live (H4-4).
- q21 mirror local commits only: matches, `blob_mirror_tap.rs:23` (GltfImported is emitted by local imports only).
- q22 issuer unreachable, stored token valid -> Held: matches. `fe-hexon/src/entitlement_client.rs:400-401` keeps the held token; `licence_client.rs:81-87` answers `Held` and logs the renewal failure (warning is in the log, not the UI).
- q23 peer label from env only: matches, `window_chrome.rs:91`.
- q24 keep AZURE_TENANT_ID, allow-list it: matches, `conductor/RUNBOOK.md:818`; workflows `.github/workflows/release.yml:79,102`.

## HANDED-OFF (owner = next round)

- H4-1 (docs/extensions owner, I-DOC-API next round) `docs/extensions/extensions-ui.md:3` embeds a personal absolute home-directory path to the UI guidance file (R-EXT-1 F9); replace with a neutral reference.
- H4-2 (I-DOC-CORE, I-DOC-API next round) `fe-database/src/AGENTS.md:704`: key is (node, metric, units) and oversized names are skipped; `fe-api/AGENTS.md:963`: a readings snapshot does not pin anchor positions, share links rely on the cutoff in the SQL. Also `fe-plugin/src/AGENTS.md:336-337` still says the v1 client "moves to fe-hexon when E9/E13 land" (they landed; RemoteRegistryClient is the live path).
- H4-3 (orchestrator) commit EXT-12: `fe-ui/src/gis/readings.rs` (untracked), `fe-ui/src/gis/mod.rs`, `fe-ui/src/panels/gis_panel.rs`, `fe-ui/src/verse_manager/db_results/mod.rs` (only the readings hunks).
- H4-4 (fe-ui/src/panels/mod.rs owner) call `dialogs::entity_access::mark_verse_local_only` in the entity-settings dialog closure before render (R-UI-3-06 NEEDS text), so QA-0177's reason and the disabled Generate invite appear.
- H4-5 (tools/qa/sim/driver.py owner) a multipart GPX op so `assets/roads/*.gpx` become the single source of fleet-terrain's tracks (inline base64 is byte-identical today; R-EXT-1 F13).
- H4-6 (plugin_bridge.rs owner) `BindingSignals::send` (`plugin_bridge.rs:2165-2167`) logs a dead binding worker at debug; use `warn!` (AGENTS anti-pattern 6, R2-05).

## Excluded by the scoper (not gaps)

- blocked-user: P4.4 GUI QA; P6.3 two clean runs of ext-fleet/ext-device/ext-install-gui/ext-bindings; RUNBOOK H4 earthwork petal authorization, R-GATE-11, R-GATE-15, R-API door-parity journey + GPX serial create; R-Z-16 / R-BIN-21 OpenBoundBundle writer; turn-3 analytics finding 4 (in-app share link); QA-0174; QA-0165; registry install real download progress; R-ADMIN-07; R-UI3-16/17; R-STORE-16/17; R-TOOL-10/12; R-BND-10, R-PLUG-16 / R-NET-18, G0-C26, G5-C16, R-HOOKS-07, R-HOOKS-28, R-API-23, G3-C15; PHR #18 / R-LIFE-25 and `verify_revocation_list_fresh`; turn3-plan §9 scope flags.
- obsolete (verified at HEAD): turn-3 H-7 R19 order; H-1 / C-EXT-05/06/07 door gaps (`fe-ui/src/dialogs/gltf_import.rs:41`, `verse_manager/db_results/nodes.rs:70`); H-2 (`fractalengine-relay/src/main.rs:349`); H-3 (`fe-hexon/tests/remote_registry_test.rs:227,290`); H-6 (`fractal-plugin-2.wit:1`); H-9; C-PHR doc hand-offs; QA-0175; RUNBOOK H7 diagrams.

Verdict: PARTIAL - 37/37 requirements complete in code; round-4 items 9 complete, 3 partial (EXT-09 docs, EXT-12 uncommitted, EXT-18 unwired), 0 missing; 6 HANDED-OFF.
