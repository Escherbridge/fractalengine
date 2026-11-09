---
type: Review
date: 2026-11-09
track: portal_surface_20261107
gate: R-POR (turn 2: portal UI, API, sim and contracts)
reviewer: R-POR (fresh context, opus, adversarial review-and-fix, code-first: nothing built or run)
reviewed: P-DOC, P-QA, P-API, P-S0C, P-UI, B10f, I-POR-UI, I-POR-SIM, I-POR-CRATES (commits 62a30c23..7a236970)
verdict: PASS-WITH-FIXES (10 FIXED; binary wiring and host routes HANDED-OFF)
resource: ./plan.md
---

# R-POR: turn-2 review of the portal UI, API, sim and contract slices

One verdict line per finding. FIXED means the fix landed in an R-POR-owned file in this gate. KEPT gives the reason. HANDED-OFF names the owner. DEFERRED names the trigger that reopens it.

## Repository section and chrome (ux.md §2, §3, §5)

- **R-POR-01 FIXED (High, a dead primary flow).** In the Repository section, Admit… and Revoke dispatched `UiAction::PortalAdmitBundle` / `PortalRevokeBundle` without a node. The handler resolved the target as "the open bundle portal". But the inspector is hidden whenever any portal is open (`ui_shell/right_sidebar.rs:324-341`), so every section click was a silent no-op. Both variants now carry `{ node_id }`:
  - the section passes its own node (`portal/repository.rs::intent_command`);
  - the toolbar passes the open bundle's node (`PortalState::bundle_node_id`, `ChromeAction::ui_action(node_id)`);
  - `actions/portal.rs::queue_admission` dedups the queue and caps it at 8 undrained requests.

  The bundle journey in `actions/portal.rs` is extended to cover this.
- **R-POR-02 FIXED (Medium, a stale badge).** The section cached a summary per node and never refreshed it after an admission change. After Admit or Revoke, closing the portal showed the old Status. `apply_bundle_state` now invalidates that node's summary when the host state moves into or out of Admitted or Revoked. The journey asserts the re-read request.
- **R-POR-03 FIXED (Medium, the may-admit rule).** The summary's `admission` never carries `can_admit`: `summary_codes` emits `static_sandbox` with no reason. So the section offered Admit… for every static build, unsealed builds included, which can never be admitted (admission.md §2 rule 11).
  - `repository_view.rs::admission_to_state` now offers Admit only for a sealed build whose reason is not `cannot_admit`.
  - README §8.5 and fe-api `AdmissionView` / `LocalAdmissionSource` define `reason: "cannot_admit"` on `static_sandbox` as the may-admit channel. The table row is extended.
- **R-POR-04 FIXED (Low, contract).** ux.md §7 "Disabled: Admit… disabled with hover" contradicted §1 and §2. This is P3-09; the code follows §2. §7 now says "absent, not disabled", and reserves the disabled form for P3-11.
- **R-POR-05 FIXED (Low, accessibility).** The `portal.open` binding offer's Open button had no stable accessible name. It is now "Open bound bundle", and ux.md §5 lists it together with the existing "Bundle title: <value>" row name.
- **R-POR-06 KEPT.** The badge, section and toolbar names match ux.md §3 and §5. The badge name is ASCII (" - "); the visible text uses the em dash. Blocked and Revoked suffixes come from `BlockedReason::suffix` and `RevokedReason::suffix`. Admitted offers Close plus Revoke. Static offers Admit… only when `can_admit`. Revoked has no secondary action (P3-11).
- **R-POR-07 KEPT (Open without admission).** Open mounts under the host-decided state, and the static sandbox renders by design (admission.md §3). Only an explicit local Open mounts. `binding_offer.rs` maps a bound bundle to `UiAction::OpenBundle` and nothing else.

## Binary wiring (Admit/Revoke through governance; the summary feed)

- **R-POR-08 HANDED-OFF to I-POR-BIN / R-POR-IPC (`fractalengine/src/main.rs`).** Nothing drains three queues yet:
  - `UiManager::bundle_admission_requests`. Admit must open governance's consent dialog for the node's `(package_id, content_hash)` and write the record only on accept. Revoke withdraws the local record without withdrawing rows (OD-E1). Both then call `repository_view.invalidate(node_id)`.
  - `repository_view.take_wanted()`. For repository nodes only, read README §8.5 as the local identity and call `ingest(node_id, parse_summary(&data))`.
  - `bundle_asset_pick`. Open the asset picker filtered to the bundle MIME and write `portal.bundle_asset`, Editor or above.

  Until then the Repository section never appears and Admit does nothing. In fe-ui, no admission can bypass governance.
- **R-POR-09 HANDED-OFF to P-RESOLVE (`fractalengine/src/portal_host/resolver.rs`).** `LocalAdmissionSource::admission` must return `reason: "cannot_admit"` on `static_sandbox` when the build is unsealed or the may-admit verdict is no. It must judge the archive digest alone when `package_id` and `content_hash` are empty (export pre-bytes check). It must pass the new `member_digests` to `kill_list_status`.
- **R-POR-10 HANDED-OFF to R-POR-IPC (`fe-webview/src/petal_portal.rs`).** `bound_bundle_open_system` writes `BrowserCommand::OpenBundle` without fe-ui's `PortalState::Bundle`. A bound bundle opened that way would mount with no host-drawn badge or Close, because fe-ui ignores `BundleState` events for a portal it did not open. Bound Opens should go through `UiAction::OpenBundle`, which needs the node selected, or `OpenBoundBundle` should be removed.
- **R-POR-11 DEFERRED (trigger: `show_offer` gains a caller).** `UiAction::OpenBundle` mounts only the selected node. The offer's caller must select the repository node first, or the "Select this repository node" advisory fires. This is noted in fe-ui `AGENTS.md` §portal-bundle.

## fe-api (export, quota, URL policy, MIME)

- **R-POR-12 FIXED (P0, compile).** P-API's `origin_allowed` names `url::Url`, but fe-api had no `url` dependency (E0433). `url = "2"` is added; it is already in Cargo.lock at 2.5.8, so it adds no new compile unit (steer 02:15).
- **R-POR-13 FIXED (Medium, security, the kill-list key set).** admission.md §4.2 matches an entry against the bundle's `content_hash`, its archive digest **or any member digest**. The fe-api `AdmissionQuery` carried no member digests, so a kill-list entry naming a member of a sealed bundle did not stop its export. `AdmissionQuery.member_digests` (the committed member digests) now goes into both the summary and the export re-check. The archive-only pre-bytes check passes `&[]`.
- **R-POR-14 KEPT (the export double check and reach).**
  - The order is: Viewer or above; `authorized_node_scope`; the node must be a repository node; `asset_reach`; the asset must have the bundle MIME. A uniform 404 covers all of these.
  - Revocation is then checked twice: on the archive digest before any byte is read (409 `revoked`, even while the bytes are absent), and again with the full build facts just before writing.
  - The CSP refuses an unsafe manifest source and drops a bad feed into `feeds_dropped`.

  fe-api `src/AGENTS.md` §portal-bundles was corrected: it described only the second check.
- **R-POR-15 KEPT (one quota).** The duplicate `check_quota` is gone. Bundle uploads call `upload::check_upload_quota` (`pub(crate)`) with `UploadOwner::new(&claims, &petal_id)`.
- **R-POR-16 KEPT (MIME parity).** `fe_format::portal_bundle::PORTAL_BUNDLE_MIME`, `fe_runtime::scene_asset::PORTAL_BUNDLE_MIME` and `portal_probe.BUNDLE_MIME` are all `application/vnd.fractalengine.portal-bundle+hexon`. `portal_bundle_test.rs` asserts the first two are equal.
- **R-POR-17 FIXED (steer 02:15).** `POST /api/v1/debug/portal/killswitch` is mounted in `server.rs`. It takes a 64-byte body and sits behind the portal-fixture gate beside `action`. The sim driver's `portal_killswitch` now calls it.
- **R-POR-18 HANDED-OFF to the orchestrator (`fe-api/src/qa_portal.rs`, no owner this turn).** `pub async fn portal_killswitch(State<Arc<ApiState>>, Extension<ApiClaims>, Uri, Bytes) -> Response` must:
  - take `{"engage": bool}` with `deny_unknown_fields` and no query string;
  - pass `QA_GATE.admit` and the Owner check, as `portal_action` does;
  - send a fixed `FixtureOp::PortalKillSwitch(bool)` to governance's local kill switch (layer a/b).

  fe-api does not compile until this lands.
- **R-POR-19 KEPT (S0c).** `scene_asset::inflate_bundle_member` reads through a `take(cap+1)` reader, and the earlier directory checks bound the archive. The docs_engine prefetch journey row (a replicated bundle row is a scene row, and its blob is fetched byte-identical) matches the task.

## Contracts against bus.md and the code

- **R-POR-20 FIXED (ipc-v1 §6.1 against OD-E1).** The text said revocation "withdraws a bundle's rows". The user decision is no withdrawal: rows stay. The text now says so.
- **R-POR-21 FIXED (ipc-v1 §5.1 and §3.3 against bus.md §2.1 and the code).** The text said `resync_required` ended the subscription and the page resubscribed. In fact the bus and `ipc.rs::absorb` keep the subscription, and a fresh snapshot follows. The code also delivers `ended {reason}`, which §3.3 did not list. Both sections are corrected.
- **R-POR-22 FIXED (ipc-v1 §5.3 and §11 against bus.md §2.3).** OD-3 is answered: re-runs happen only on a scope change, at most once per second, a table with no change feed is refused at subscribe, and the host suppresses unchanged results. The stale "page-side only" text is gone.
- **R-POR-23 FIXED (ipc-v1 §6.2 against bus.md §4).** A portal is a local caller, so a role below the floor answers `capability_denied` and no active instance answers `unavailable`, not `not_found`. §6.2 and §6.5 now name `role_ceiling` (steer 23:40).
- **R-POR-24 HANDED-OFF to R-POR-IPC (`fe-webview/src/portal_bundle/ipc.rs:219`).** The `PortalBroker::invoke` doc still says "below the role floor is `NotFound`"; it should map to `CapabilityDenied` per bus.md §4 check 4.
- **R-POR-25 FIXED (ipc-v1 §7.3, §8 and §9 against the code).** The text now states that `deliveries` counts subscription deliveries only, never replies, and that flat `ipc-stats` counters are process totals. The `AUDIT_REFUSAL_RATE` cap (2/s, burst 5) on audited check-5 refusals is now documented.
- **R-POR-26 KEPT.** admission.md §4.2 and §4.6 (hash under any id, with the residual narrowed to repackaging), §5 step 6 (storage clear on NavigationCompleted), README §1.8 (the `kv` ban; `kv` is a real manifest-v2 key) and ipc-v1 §1.1 (8 192-byte Source and 65 536-byte body, matching `limits.rs`) all agree with the code and with bus.md.

## QA fixtures, probe and sim

- **R-POR-27 FIXED (High, fixture against ipc-v1 §5.2).**
  - The dashboard and storm fixtures subscribed to `readings` with `{petal_id}`. `plan_subscription` allows only `{node_id, metric}`, so every readings subscribe answered `invalid`.
  - In the dashboard, that left the readings probe unfed and the rejection unhandled.
  - In the storm, 21 of 64 subscribes became `invalid`, so `too_many_subscriptions` could never reach the scenario's 48.
  - Both now send valid params. The dashboard counts rows from `data.readings` and shows rejections on its probes.
- **R-POR-28 FIXED (fixture against §5.1).** The dashboard resubscribed on `resync_required`, which leaked a second live subscription per lag. It now waits for the fresh snapshot.
- **R-POR-29 FIXED (High, probe keys against scenario assertions).** The evaluator resolves paths generically (`predicates.at_path`), but `portal_probe.read_path` was never called. So `badge`, `mounted`, `writes_accepted`, `observe.probe.<name>` (stored as `probes`), `observe = "refused"` and `admission = "StaticSandbox"` (stored as `{state: "static_sandbox"}`) could never match. `collect` now emits the scenario paths directly:
  - `mounted`;
  - `observe`, either the mount object with `probe` or `"refused"`;
  - `ipc`, with the total fallbacks filled in;
  - `badge` / `badge_name`, from ui-state;
  - `writes_accepted`: the host counter, else the page's `writes-ok`, never `accepted`, which also counts hello and subscribe;
  - `admission` as the BundleState name, with `admission_reason`.

  The dead `read_path` is removed.
- **R-POR-30 HANDED-OFF to I-POR-BIN / R-POR-IPC (`fractalengine/src/qa_debug_bridge.rs`).** The observe route's `label` is the origin label, not the badge. Unmounted states (Waiting, Revoked, Blocked) have no observe at all, so no current route serves the badge. The ui-state snapshot should gain `"portal": {"node_id", "state", "badge_text", "badge_name"}` from `fe_ui::portal::chrome::chrome_model` while a bundle portal is open. Line 560 should use `self.ui.portal.is_open()`. Until then, `badge` reads null.
- **R-POR-31 HANDED-OFF (no owner: `fe-webview/src/backends/fixture_surface.rs:255`).** `ipc-stats` answers only while a bundle is mounted. So `ipc.live_subscriptions equals 0 after kill` reads `"refused"`. The process totals should be answerable while unmounted.
- **R-POR-32 DEFERRED (trigger: the host exposes these routes; refine).** No probe produces `protocol_status`, `origin_storage.*`, `idle_reads.*`, `frame_stall_ms` or `peer_log.db_thread_timeouts`. The `absent origin_storage.qa_origin_key` expectation therefore passes vacuously; this is recorded in tools/qa/portal `AGENTS.md`.
- **R-POR-33 FIXED (PJ10 against the fixture).** PJ10's oracle said "node probes fill from the public REST", but the dashboard had no static path. The dashboard now reads `/__fe/feeds.json` with no `window.fe`; this is the one code path in static-export.md §3. A `feed` probe shows `200`, `expired` (on 410) or `error`. PJ10 is rewritten to match and requires the operator to send `fe-portal.json.csp` as a header (static-export.md §5).
- **R-POR-34 KEPT (I-POR-SIM).**
  - `python tools/qa/sim/scenario.py schema` regenerates `scenario.schema.json` byte-identical.
  - `validate portal-core portal-replication portal-load` passes (static).
  - `ASSET_MANIFESTS['portal']` and `assets.PORTAL_KEYS` match `build_bundles.py`'s keys.
  - `portal_admit` and `regrant_role` drive the GUI by the ux.md §5 names.
