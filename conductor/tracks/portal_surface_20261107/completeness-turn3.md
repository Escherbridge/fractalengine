---
type: Review
date: 2026-11-13
track: portal_surface_20261107
gate: C-POR (turn 3 completeness critic)
reviewer: C-POR (fresh context, opus, code-first: nothing built or run)
reviewed_at: 8cacc6ac (+ working tree; owned files clean at start)
verdict: CODE-COMPLETE for R1-R15 and R17; R16 PARTIAL (runs pending); 1 FIXED, 0 MISSING, 2 HANDED-OFF
resource: ./spec.md
---

# C-POR: portal surface completeness at turn-3 HEAD

Every spec requirement (spec.md §7 R1-R17), every plan task (plan.md §7) and every portal item turn 3 put in
scope (turn3-plan.md §6.5) checked against code. "complete" means the code and its journey exist at the cited
lines; nothing here is built or tested yet (refine §11 does that), so every runtime acceptance item is still
unproven. File paths are repo-relative.

## Requirements (spec.md §7)

| Req | Status | Evidence |
|---|---|---|
| R1 manifest section + validator | complete | `fe-format/src/lib.rs:16` mod; `fe-format/src/portal_bundle.rs:146` `RefusalClass`, `:284` `validate_portal_bundle`, tables from `:925` |
| R2 ingest + directory mapping | complete | `fe-api/src/server.rs:254-260` routes; `fe-api/src/portal_bundles.rs:462` upload (413 path `:495`, refusal status `:364`), `:603` summary; file index `fe-api/src/assets.rs:372`; journey `fe-api/tests/portal_bundle_test.rs:230,356` |
| R3 repository nodes | complete | `fe-query/src/spatial_nodes.rs:34,43,52` `NodeKind::Repository`; `portal_bundle_test.rs:397` `?kind=repository` |
| R4 replicate without executing | complete | `fe-runtime/src/scene_asset.rs:9,102,171` bundle MIME + byte check; fe-sync journey `fe-sync/src/docs_engine.rs:1334,1628` (row arrives, blob fetched) |
| R5 `fe-portal` protocol | complete | `fe-webview/src/portal_bundle/protocol.rs:28` `serve`, `:34` Service-Worker refusal, `:48` no percent-decode, `:79` 410; journey `fe-webview/tests/portal_protocol_journey.rs` (tamper `:92`) |
| R6 origin isolation | complete | `fe-webview/src/portal_bundle/origin.rs:78` admitted label (publisher ‖ package), `:87` static label (hash) |
| R7 navigation in both layers | complete | one gate: `fe-webview/src/backends/tauri.rs:52-63` `decide_navigation` → `gate.decide`; `backends/qa_navigation.rs:87` same `NavigationGate`; `portal_bundle/navigation.rs:102` `admits`; `fe-ui/src/actions/portal.rs:303` refuses `fe-portal`/bundle spellings |
| R8 IPC ingress authorization | complete | `fe-webview/src/portal_bundle/ipc.rs:316-334` bridge only for Admitted mounts with a generation token; `:420,424` `dropped_origin`; `:454-458` delivery gated by visibility + generation |
| R9 subscriptions without polling | complete | `portal_bundle/limits.rs:16` 16 live subs, `:18` 250 ms coalescing (≤5/s), `:31-36` per-delivery bounds; host streams via the shared bus (`fractalengine/src/portal_host/broker.rs`) |
| R10 actions as installing identity | complete | `limits.rs:20` set_property 2/s burst 5 (≤7 in 1 s); ceiling `broker.rs:344`; one-op `PluginCommit` with provenance `{package_id, content_hash}` `broker.rs:839-859`; bus refuses a binding without provenance `fe-plugin/src/bus/broker.rs:1031`; denial audit `:1019` |
| R11 bus producer/consumer | complete | `broker.rs:510` invoke, `:521` emit; declared grants `:632-653`; bus side `fe-plugin/src/bus/broker.rs:913-921,2237` |
| R12 static sandbox | complete | no `/__fe/portal-v1.js` without an Admitted token (`ipc.rs:334`); `portal_bundle/feeds.rs:74` `/__fe/feeds.json`; share-route CORS `fe-api/src/portal_bundles.rs:131` (`server.rs:125`) |
| R13 admission states + chrome | complete | `fe-ui/src/portal/chrome.rs:109-115` Admit only when `can_admit`; `fe-ui/src/portal/repository_view.rs:100`; governance `fractalengine/src/portal_host/resolver.rs:450` `can_admit`; Repository call site `fe-ui/src/panels/inspector.rs:291,2493` (P3-03 closed); actions `fe-ui/src/actions/mod.rs:59,61,1375,1378` (P3-01/02 closed) |
| R14 revocation, update, close | complete | `fe-webview/src/portal_bundle/admission.rs:106,116,120` revoked verdicts; `mount.rs:398-419` refusal before bytes; kill switch `portal_host/resolver.rs:321` `set_kill_switch`, `:412` `host_disabled`; re-judge wakes on governance generation `portal_host/mod.rs:883` (R-POR-T3-08 closed) |
| R15 offline + size | complete (code) | Waiting `mount.rs:406-409`; cap `fe-format/src/portal_bundle.rs:22` (32 MiB, so 40 MB → 413); ladder render budgets are runtime-only (PJ5) |
| R16 QA lane | **partial** | code present: `tools/qa/sim/scenarios/portal-{core,replication,load}.json`, `tools/qa/sim/portal_probe.py`, driver ops `tools/qa/sim/driver.py:40,1034,1045`, routes `fe-api/src/server.rs:390-395` (kill switch `fe-api/src/qa_portal.rs:363`, bridge `fractalengine/src/qa_portal_bridge.rs:73`). Missing: `run.py validate` and PJ1-PJ9 receipts (T5.5, a run, not code) |
| R17 static export | complete (code) | `fe-api/src/server.rs:263-265`; `portal_bundles.rs:946` export (no page API, `fe-portal.json` + `__fe/feeds.json` `:914-915`, revoked → 409 before bytes); journey `portal_bundle_test.rs:439`; expired state in `tools/qa/portal/bundles/dashboard/web/app.js` |

## Plan tasks (plan.md §7)

- **complete (code):** T0.1-T0.4, T1.1, T1.3, T1.4, T2.1, T2.2, T3.1, T3.2 (P3-03 now wired, `inspector.rs:291`), T4.1, T4.2, T5.1-T5.4, T6.1, D, R-P1..R-P4.
- **T6.2 static export:** code complete (`portal_bundles.rs:946`, test `:439`) although plan.md §7 still shows it unticked; tick it at turn end (plan.md is not this slice's file).
- **T2.3 readings broadcast:** not applicable; OD-13's fallback never triggered (readings arrive over the bus `host/readings`).
- **T1.2 WebView2 spikes, T5.5 journey runs, full sweep:** open by design; they are runs, owned by refine and the portal QA lane.

## Turn-3 in-scope portal items (turn3-plan.md §6.5)

- R-X-33 Disable all → portals: complete. `plugin_bridge.rs:451,4534` `portal_host_switch`; `resolver.rs:271,528` `host_disabled_handle`.
- R-X-04 `portal.open`: complete. `fe-plugin/src/bindings/resolver.rs:295,326-330`; feed `plugin_bridge.rs:477` `admitted_set`.
- Kill switch, R-POR-18/31: complete (see R14, R16).
- R-POR-09 `can_admit`, `learn_consent`: complete. `resolver.rs:238,450`; `portal_host/mod.rs:755,793`.
- OpenBoundBundle, R-POR-10/11/24: complete. `fe-webview/src/ipc.rs:17,27`; `fe-webview/src/plugin.rs:238,389`; offer `fe-ui/src/panels/inspector.rs:2472-2485` (R-POR-T3-06 closed).
- Registry install of web bundles: complete. `portal_host/mod.rs:5` `mod install`, `configure_install` at startup; caller `extension_host/ops_drain.rs:885` (R-POR-T3-12 closed).
- Governance generation counter: complete (R-POR-T3-07 fixed, R-POR-T3-08 closed at `mod.rs:883`).
- dev_token `client_did`, driver `portal_admit` button: complete (R-BIN 8cacc6ac; `driver.py:1045`).
- Portal spec:236 OD-E1 wording: complete (D-EXT 81a06ef3). Owned docs agree: `docs/spec/portal-bundle/admission.md:60,72,80`, `ipc-v1.md:126`. Kill-list match "by hash under any id": `admission.md:67`.

## Verdict lines

- **C-POR-01 FIXED (P-BROKER shared services).** `install_broker` built its own `NodeDidRoles` role source and left `BusPortalBroker::from_parts` dead, so the portal did not share the extension host's role source as P-BROKER requires. It now prefers `plugin_bridge::ExtensionBusParts` through `from_parts` (bus, roles and audit shared; governance view required), and keeps the old path only when the extension host did not build them (`fractalengine/src/portal_host/mod.rs:152-183`). Behaviour is unchanged today: both sources answer Owner for the node DID, the only DID a binding names.
- **C-POR-02 KEPT.** `track_portal_scopes` pins only `OpenBundle`. A bound open resolves its scope from the active petal (`broker.rs:146-157`) and refuses `NotFound` otherwise, so it fails closed when the user navigates away.
- **C-POR-03 HANDED-OFF (R-Z / orchestrator, docs).** `fractalengine/src/AGENTS.md:1861` §Broker "The role source" still names `NodeDidRoles` as the source. It should say the broker shares `ExtensionBusParts` via `from_parts` and that `NodeDidRoles` is the fallback when no extension host ran.
- **C-POR-04 HANDED-OFF (orchestrator, plan ticks).** plan.md §7: tick T6.2 (code-complete) and T3.2 (P3-03 wired). Mark T2.3 not applicable (OD-13 not triggered).
- **C-POR-05 KEPT (runs).** R16's `run.py validate portal-*`, PJ1-PJ9 receipts, and the T1.2 WebView2 spikes stay open until the refine and the portal QA lane run (turn3-plan.md §11).
- **Leak scan:** no blocklisted term in any owned path (fe-webview/src, portal_host, fe-ui/src/portal, docs/spec/portal-bundle, the fe-api portal files, qa_portal_bridge.rs, tools/qa/portal).
