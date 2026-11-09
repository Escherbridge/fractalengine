# tools/qa/portal - portal surface QA lane

Harness spec for track `portal_surface_20261107` (spec section 6.7, plan P5). Authored text-only (D-X2); the builder, probe and routes land in T5.2-T5.4. Journeys: [journeys.md](journeys.md). Fixture sources: `bundles/`. Scenarios: `../sim/scenarios/portal-{core,replication,load}.json`.

## section: tiers

| Tier | Drives | Evidence |
|---|---|---|
| Headless | the real `portal_bundle` modules with synthetic page requests (webview is the only faked edge) | `fe-webview/tests/portal_ipc_journey.rs` rows, `fe-api/tests/portal_bundle_test.rs` |
| Native | captured Windows hosts, real WebView2, via `fe.py portal ...` | receipts, CapturePreview PNGs reviewed by a separate agent, never self-approved |
| Simulator | `tools/qa/sim` scenarios with a `portal` probe | `observations.jsonl`, `latency.csv`, `report.md` |

No caller supplies JavaScript, selectors or URLs on any tier. Every portal operation is a fixed, numbered or enumerated op.

## section: fixture-bundles

Sources live in `bundles/<name>/` as `portal*.json` (the `fe-portal-bundle/1` section, drafted against `docs/spec/portal-bundle/manifest-v1.schema.json`) plus `web/**`. `bundles/bundles.json` is the index `build_bundles.py` (T5.2) reads: package ids, versions, variants, journeys.

| Bundle | Package id | Purpose | Journeys |
|---|---|---|---|
| docs-site | `qa.portal.docs-site` | static docs; no API, CSP canary, image, second page | PJ1 PJ2 PJ5 PJ6 |
| dashboard | `qa.portal.dashboard` | node / readings / query subscriptions rendered into probe elements | PJ1 PJ2 PJ3 PJ6 |
| actions | `qa.portal.actions` | `set_property` on key scope, `localStorage` seed; variants v1, v2 (same caps), v3 (adds `qa_extra`) | PJ2 PJ4 PJ9 |
| escape | `qa.portal.escape` | ten numbered navigation slots, one declared origin `https://example.org` | PJ8 |
| storm | `qa.portal.storm` | 64 subscriptions, message flood, 20-write burst, 60 s idle | PJ7 |
| ladder | `qa.portal.ladder` | generated: docs-site shape plus `web/pad.bin` sized from `tools/qa/sim/assets.py` `LADDER`; plus a 40 MB over-cap build | PJ5 |

Conventions every fixture page follows (the fixed observation depends on them):
- `data-fe-probe="<name>"` marks an element whose text the host may read. The observation returns the title, a capped digest over probe elements only, and the named probe values. Nothing else in the DOM is observable.
- `data-fe-slot="<n>"` marks the n-th activatable control. `portal_slot` activates by number; there is no selector input.
- Page code uses only the draft `window.fe` surface: `hello()` returning `{capabilities, context:{petal_id, repository_node_id}}`, `subscribe(kind, params, cb)`, `act({op:'set_property',...})`, `intent({...})`. Reply errors carry `error.code`. See assumptions below.
- Sealing: through the staged production `seal_hexon.exe` with a run-local publisher seed (controlled-fixtures precedent, `tools/qa/AGENTS.md` controlled fixtures). `unsigned` variants skip sealing to exercise the static sandbox. `extra_member` entries are written into the archive by the builder so each version has a distinct content hash.
- The `Probe Target` node (dashboard) is created by the scenario; the dashboard matches it by name.

## section: sim-contract

Ops and probe the portal scenarios assume. T5.3 / T5.4 / SI implement them; `scenario.schema.json` needs `portal` added to the probe enums and `portal_ms` to `cadence` before `run.py validate portal-core portal-replication portal-load` passes (`NEEDS tools/qa/sim/scenario.schema.json`).

| Op (via rest unless noted) | Args | Result / binds |
|---|---|---|
| `upload_bundle` | `petal_id, path, name` | `data.id`, `data.content_hash`; 413 over cap |
| `create_repository_node` | `fractal_id, petal_id, name, bundle_asset, position` | node `id`; sets `properties.node_kind=repository` and `properties.portal.bundle_asset` |
| `portal_admit` | `package_id, content_hash, reconsent?` | local admission record (never replicated) |
| `portal_killswitch` | `engage` | local governance kill switch |
| `portal_update_bundle` | `repository_node_id, bundle_asset` | replaces the node's bundle reference (an Editor property write) |
| `portal_slot` | `slot` | activates numbered slot in the mounted bundle (native tier) |
| `portal_ipc_post` | `row, package_id?, content_hash?` | fixed PJ2 rows: unsigned, external, foreign_frame, hidden, stale, control |
| `portal_observe` / `portal_ipc_stats` | none | read-only, used by the probe |

Probe `portal`: `target` is a package id (or `*` for totals). `path` values used: `admission`, `mounted`, `badge`, `protocol_status`, `observe`, `observe.title`, `observe.probe.<name>`, `observe.api_script_status`, `origin_storage.<key>`, `ipc.<counter>` (`accepted refused_capability refused_role rate_limited dropped_origin invalid` plus `live_subscriptions`, `too_many_subscriptions`, `delivered_non_control`, `dropped_origin_total`), `writes_accepted`, `idle_reads.*`, `frame_stall_ms`, `peer_log.db_thread_timeouts`. Cadence `portal_ms`. The probe is read-only and never polls the hierarchy ("probing does not perturb", `tools/qa/AGENTS.md` sim section).

Native routes (T5.4): `/api/v1/debug/portal/{observe,action,ipc-stats}` behind the portal-fixture Owner/input gate and five-second host deadline; client `tools/qa/portal/client.py`; CLI `fe.py portal observe|slot|ipc-stats`.

## section: assumptions

- Gui accessible name for the repository **Open** action is `Open bundle` until `docs/spec/portal-bundle/ux.md` (T3.1) fixes it; scenarios mark every use with a note.
- The `window.fe` shape and the `hello().context` fields are drafts; `docs/spec/portal-bundle/README.md` wins when it lands and the fixtures follow it.
- Scenario references `$assets.portal.<key>.path` assume `build_bundles.py` registers its archives in the asset manifest under `portal`: `dashboard`, `dashboard_unsigned`, `docs_site`, `actions_v1..v3`, `storm`, `ladder_tier_*`, `ladder_tier_40mb_overcap`.
- `regrant_role` is a draft driver op; reuse the p2p-core re-grant step pattern if the driver names it differently.
- Run order for T5.5: headless tier first (cheap), native `fe.py portal` second, simulator last. Peers stay at N=3 (portal-replication) or 2 until QA-0119.
- Mutation check for PJ2: remove the sender-origin check (R8) and the non-control rows must go red.
