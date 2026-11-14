# tools/qa/portal - portal surface QA lane

Harness spec for track `portal_surface_20261107` (spec section 6.7, plan P5). Authored text-only (D-X2); builder, probe and routes are written but not yet run (build gate closed). Journeys: [journeys.md](journeys.md). Fixture sources: `bundles/`. Scenarios: `../sim/scenarios/portal-{core,replication,load}.json`.

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
- Page code uses only the `fe.portal.v1` surface (`docs/spec/portal-bundle/ipc-v1.md` section 4): `hello()` resolves `{api, capabilities, context, limits}`; `subscribe(stream, params, cb)` calls `cb({kind, seq, data, dropped})`; `act({op:'set_property', node_id, key, value})` resolves `{node_id, key}`; `intent({...})`. Every promise rejects with `FePortalError {name, code, message}`, so fixtures read `error.code`, never a reply object.
- Delivery bodies are the `/ws` bodies: `node` snapshot parts carry `data.nodes[]` (final part `last: true`), deltas carry `data.changes[]` (`SceneChange`, tag `op`, ids as `node_id`); `readings` deltas carry `data.readings[]`; `query` snapshots carry `data.data[]` rows. `resync_required` is followed by a fresh snapshot on the same subscription, so a page waits and never subscribes again; `ended` (`{reason}`) is a subscription's last delivery. Subscribe params are exactly ipc-v1 section 5: `node` takes `{petal_id?, node_id?}`, `readings` `{node_id?, metric?}` (a `petal_id` there is `invalid`), `query` `{name, args?}`.
- `hello().context` fields: `package_id, content_hash, version, publisher_did, repository_node_id, petal_id` (`petal_id` is the binding petal). Fixtures use only `petal_id` and `repository_node_id`.
- Sealing: through the staged production `seal_hexon.exe` with a run-local publisher seed (controlled-fixtures precedent, `tools/qa/AGENTS.md` controlled fixtures). `unsigned` variants skip sealing to exercise the static sandbox. `extra_member` entries are written into the archive by the builder so each version has a distinct content hash.
- The `Probe Target` node (dashboard) is created by the scenario; the dashboard matches it by name.

## section: sim-contract

Ops and probe the portal scenarios assume. `scenario.py` and `scenario.schema.json` carry the `portal` probe and the `portal_ms` cadence, and `python tools/qa/sim/scenario.py validate portal-core portal-replication portal-load` passes (static check, 2026-11-09).

| Op (via rest unless noted) | Args | Result / binds |
|---|---|---|
| `upload_bundle` | `petal_id, path, name` | `data.id`, `data.content_hash`; 413 over cap |
| `create_repository_node` | `fractal_id, petal_id, name, bundle_asset, position` | node `id`; sets `properties.node_kind=repository` and `properties.portal.bundle_asset` |
| `portal_admit` | `package_id, content_hash, reconsent?` | via the GUI: clicks `Admit bundle` (Repository section or toolbar, `ux.md` section 5) for the selected or open node, then the consent confirm; local admission record (never replicated) |
| `portal_killswitch` | `engage` | `POST /api/v1/debug/portal/killswitch {"engage": bool}` (debug token, portal-fixture Owner+input gate): local governance kill switch, layer (a)/(b) |
| `portal_update_bundle` | `repository_node_id, bundle_asset` | replaces the node's bundle reference (an Editor property write) |
| `portal_slot` | `slot` | activates numbered slot in the mounted bundle (native tier) |
| `portal_ipc_post` | `row, package_id?, content_hash?` | `POST /api/v1/debug/portal/ipc-post` (`fe-api/src/server.rs:408`, handler `fe_api::qa_portal::portal_ipc_post`, 256-byte body limit), gated like `portal_killswitch`. `row` is one of the seven fixed `IpcPostRow` values in `fe-webview/src/portal_bundle/qa_rows.rs` (unsigned, unadmitted, external, foreign_frame, hidden, stale, control). The optional strings are a mount pin (`MountExpectation`, `fe-runtime/src/qa_portal.rs`): a malformed hash is 400; a pin that differs from the open mount is 409 `portal_fixture_mount_mismatch` (`fractalengine/src/qa_portal_bridge.rs:102`). For the unsigned and unadmitted rows the `content_hash` names the sender's static-origin build, which need not be open. Receipt: `{ipc_post: {row, dropped_origin_delta, delivered, replied}}` |
| `portal_observe` / `portal_ipc_stats` | none | read-only, used by the probe |

**Observation digest.** `observe.digest` is BLAKE3 (lowercase hex) over the UTF-8 text of the `data-fe-probe` elements only, in probe-name order, each text followed by one NUL byte; each probe text is capped at 256 chars, the set at 64 probes and 64 KiB. `client.py` recomputes it when the `blake3` module is importable and raises `portal_digest_mismatch` otherwise on drift. Anything outside probe elements (headings, attributes, scripts) never changes the digest.

Probe `portal`: `target` is a package id (or `*` for totals). `path` values used: `admission`, `mounted`, `badge`, `protocol_status`, `observe`, `observe.title`, `observe.probe.<name>`, `observe.api_script_status`, `origin_storage.<key>`, `ipc.<counter>` (the ten `ipc-stats` counters `accepted refused_capability refused_role rate_limited dropped_origin invalid failed too_many_subscriptions live_subscriptions deliveries`, plus the totals `ipc.delivered_non_control` and `ipc.dropped_origin_total` and `writes_accepted`), `idle_reads.*`, `frame_stall_ms`, `peer_log.db_thread_timeouts`. Cadence `portal_ms`.

Probe value shape (`sim/portal_probe.py collect`). The evaluator resolves every `path` generically (`predicates.at_path`, dotted lookup), so `collect` emits a value whose keys are the scenario paths: `mounted` (boolean); `observe` is the mount's `{label, mode, title, digest, probe: {<name>: text}, api_script_status}` or the string `"refused"` while no bundle is mounted, so `observe.probe.<name>` reads a probe text; `ipc` is the `ipc-stats` counters (flat values are process totals) or `"refused"`, with `delivered_non_control` filled from `deliveries` and `dropped_origin_total` from `dropped_origin` when the host omits them; `badge` is the host-drawn badge's visible text (em dash) and `badge_name` its ASCII accessible name, both read from `GET /api/v1/debug/ui-state` `portal.{badge_text, badge_name}`; `writes_accepted` is the host counter, else the page's `writes-ok` probe (never `accepted`, which also counts hello and subscribe); `admission` is the summary read's state as the `BundleState` name (`Admitted`, `StaticSandbox`, `Revoked`, `Blocked`) with `admission_reason` beside it. Counter classes follow `ipc-v1.md` section 7.1: `failed` counts `unavailable`, `backend` and `unknown_outcome`; `not_found` counts as `refused_role` (same uniform answer REST gives), so out-of-scope `node_id` writes raise `refused_role`, not a separate counter; `deliveries` counts subscription deliveries only (any `kind`), never replies. Paths no host route serves yet (`protocol_status`, `origin_storage.*`, `idle_reads.*`, `frame_stall_ms`, `peer_log.*`) stay absent, so an `absent` predicate on them proves nothing until those routes land. The probe is read-only and never polls the hierarchy ("probing does not perturb", `tools/qa/AGENTS.md` sim section).

Native routes (T5.4): `/api/v1/debug/portal/{observe,action,ipc-stats,killswitch}` behind the portal-fixture Owner/input gate and five-second host deadline; client `tools/qa/portal/client.py`; CLI `fe.py portal observe|slot|ipc-stats`.

## section: builder

`build_bundles.py` packs each `bundles/<name>/` (`portal*.json` plus `web/**`) into an unsealed plugin-family archive with fixed member order and timestamps, then seals it through the hash-verified staged `seal_hexon.exe` with a run-local seed passed only in the environment. Output: `.qa/assets/portal/*.hexon` and `manifest.json` (`fe-qa-portal-assets/1`, entries `{key, path, bytes, package_id, version, sealed, journeys, blake3|sha256}`). `unsigned` variants skip sealing; `actions` variants add a distinct `web/build-id.txt` so each version has its own content hash; `ladder` pads `docs-site` with an incompressible `web/pad.bin` to the `sim/assets.py` `LADDER` sizes plus a 40 MB over-cap build.
`sim/assets.py` locates the archives (`portal_manifest`, `portal_missing`, `python tools/qa/sim/assets.py --check-portal`) and `PORTAL_KEYS` is the key set scenarios reference as `$assets.portal.<key>.path`. `sim/run.py` `ASSET_MANIFESTS` maps that namespace to `.qa/assets/portal/manifest.json`.

## section: assumptions

- Gui accessible name for the repository **Open** action is `Open bundle` until `docs/spec/portal-bundle/ux.md` (T3.1) fixes it; scenarios mark every use with a note.
- The `window.fe` shape and `hello().context` follow `ipc-v1.md` sections 4-5; if the spec moves, the spec wins and the fixtures follow.
- Host `ipc-stats` may not yet report `writes_accepted`, `delivered_non_control` or `dropped_origin_total`; the probe falls back to the page's `writes-ok` probe, `deliveries` and `dropped_origin`, which is exact only while one bundle is mounted (A-7).
- `badge` needs the ui-state `portal` block (`{node_id, state, badge_text, badge_name}` from `fe_ui::portal::chrome::chrome_model`), which `fractalengine/src/qa_debug_bridge.rs` does not emit yet; until it does, `badge` reads null and the badge expectations stay unmet. `ipc-stats` also answers only while a bundle is mounted, so `ipc.live_subscriptions` after a kill reads `"refused"`, not 0.
- Scenario references `$assets.portal.<key>.path` assume `build_bundles.py` registers its archives in the asset manifest under `portal`: `dashboard`, `dashboard_unsigned`, `docs_site`, `actions_v1..v3`, `storm`, `ladder_tier_*`, `ladder_tier_40mb_overcap`.
- `regrant_role` is a draft driver op; reuse the p2p-core re-grant step pattern if the driver names it differently.
- Run order for T5.5: headless tier first (cheap), native `fe.py portal` second, simulator last. Peers stay at N=3 (portal-replication) or 2 until QA-0119.
- Mutation check for PJ2: remove the sender-origin check (R8) and the non-control rows must go red.
