---
type: plan
track: portal_surface_20261107
title: "Implementation Plan: portal surface"
created: 2026-11-07
resource: ./spec.md
computed_at_commit: cfd7fce6
partitions_confidence: planned
---

# Implementation plan: portal_surface_20261107

> Citations were computed at `cfd7fce6`. They were first read at `30163ca2`; P2P commits `fe1e6b75`, `11b44d4f` and `cfd7fce6` landed during authoring and moved no cited line. Re-grep line numbers before editing; only symbol names are stable.
> **Build gate:** at authoring time `.omc/state/build-gate.json` has `cargo_allowed: false` (D-X2).
> - **AUTHOR-ONLY** tasks run now: specs, schema text, docs, journeys as text, scenario JSON, fixture sources as text.
> - **BUILD** tasks wait until P2P-QA posts "gate open" on the bus.

## 0. Assumptions

This plan proceeds on the recommended defaults in spec §9 (OD-1..OD-13) and the assumptions below. Each assumption carries its reversal cost so whoever triages later can weigh it.

- **A-1. Integration bus (sibling track).**
  - `extension_platform_20261107` exposes a broker on the plugin worker. The GUI process can call it without blocking, over a channel.
  - The broker offers host streams `host/query`, `host/readings` and `host/node-changes` (sibling draft R13), plus declared extension streams, commands and hooks (R14), with limits, audit and external access (R15). Portal IPC is "one more client of the same broker".
  - P2 is written against this shape. It is re-verified when the sibling commits its spec and posts its bus contract `milestone`.
  - *Reversal: low before P2 BUILD, high after.*
- **A-2. Governance (sibling track).** Governance owns four things: per-peer admission records keyed `(package_id, content_hash)` with change notifications, the consent dialog, the per-verse policy check ("may this identity admit?"), and a local kill-switch action reachable over REST (Owner). Installation status follows sibling draft R11 (`installed | active | disabled | degraded | revoked`), and the kill-switch layers follow R23 (a)–(d). This track never stores admission. *Medium.*
- **A-3. Package format.** The web content type is a discriminator inside the `HexonType::Plugin` family that D-X1 introduces (sibling manifest v2). This track owns only the `portal` manifest section. *Low.*
- **A-4. Trust roots and revocation.** These are S-PLUGIN's (`plugin_host_revival_20261031`), used unchanged. *Low.*
- **A-5. Audit.** Canonical-log audit is the sibling governance's seam. *Low.*
- **A-6. Platform.** Windows/WebView2 is the acceptance platform (OD-10). *Low.*
- **A-7. One portal.** There is one webview, so one portal is open at a time. *Low.*
- **A-8. The P2P campaign goes first.** It is still active: it committed `11b44d4f` and `cfd7fce6` during authoring, and `tools/qa/sim/prober.py` was dirty in the main tree. Its edits merge before this track's BUILD touches the same files: `fe-runtime/src/scene_asset.rs`, `fe-database/src/handlers/scene_asset_transfer.rs`, `fe-sync/src/scene_assets/mod.rs`, `tools/qa/sim/*` and `tools/qa/fe.py`. *Low (sequencing only).*

## 1. Doctrine for every phase

- **Agents sketch; the session inks.** No phase agent runs cargo. After the gate opens:
  - run `cargo check -p <crate>` per slice;
  - run one serial batch gate per wave, using only the touched usability regions (`conductor/test-batches.md`);
  - run the full sweep once, at track end.

  Batches by area: portal (R6) is `cargo test -p fe-webview` plus the fe-ui `portal::` filter; fe-api routes are R9; asset reach is R7; ingest journeys are R4; fe-sync is R8; fe-format is R11.
- **Never build concurrently with another session.** Post `building EXT-PORTAL <what>` on the bus before any build. If another session's `building` entry is open, wait (SESSION-PROTOCOL rule 1).
- **Tests model real flows.**
  - Extend existing journeys; add new journey files only for the three new public seams: the custom protocol, the IPC bridge, and the bundle REST routes.
  - Pure logic with real branching (manifest validation, origin labels, navigation admission, CSP composition) gets one table each.
  - No tautologies. The state → badge mapping is a restated constant, so it is proven through ui-tree accessible names in the QA journeys, not unit-tested.
- **Security doctrine.**
  - The raw URL policy is never relaxed.
  - Both navigation layers consult one admission function.
  - QA routes never accept caller-supplied JavaScript.
  - Host chrome is the only trust indicator.
- **Leak discipline.** This is a public repo:
  - no business logic;
  - never name private tracks;
  - run the leak grep before every commit.
- **Docs.**
  - Code carries terse doc-comments.
  - Rationale goes in `fe-webview/src/portal_bundle/AGENTS.md`, with a one-line pointer from `fe-webview/src/AGENTS.md`.
  - The author-only design text lives in `docs/spec/portal-bundle/` until code lands.
- **Reviews.**
  - Every phase closes with a recorded adversarial verdict (§4).
  - `/security-review` is mandatory for P1, P2 and P4.
  - Reviewers fix their own findings.
  - After each review, post `ready-to-merge` on the bus; the orchestrator merges.

## 2. Phase / task table

Mode **AUTHOR-ONLY** means the task runs while the gate is closed; **BUILD** means it needs `cargo_allowed=true` or a staged binary. Slice ids refer to `metadata.json` → `partitions`.

| Phase | Task | Slice | Mode | Model | What | Files | Journey (extends / new seam) | Depends |
|---|---|---|---|---|---|---|---|---|
| P0 | T0.1 | SD | AUTHOR-ONLY | opus | Bundle container + `portal` manifest section v1: member layout, reserved `__fe/`, origin labels, MIME table, asset mapping, caps; JSON Schema with three example manifests | `docs/spec/portal-bundle/README.md` (new), `docs/spec/portal-bundle/manifest-v1.schema.json` (new) | text only; T5.1's fixture manifests validate against it | none |
| P0 | T0.2 | S0 | BUILD | sonnet | `fe-format` types + pure validator for the `portal` section; refusal classes from spec §6.1 | `fe-format/src/portal_bundle.rs` (new); SI adds `pub mod` in `fe-format/src/lib.rs` | new table inside the module (R1): pure logic, real branching | T0.1; sibling discriminator (A-3) |
| P0 | T0.3 | S0b | BUILD | sonnet | Upload route (multipart; `store_*`/`register_asset` shape, so bytes never cross the DB channel); file-index branch for bundles in `serve_asset_by_id`; bundle summary + local admission read; `NodeKind::Repository` | `fe-api/src/portal_bundles.rs` (new), `fe-api/src/assets.rs`, `fe-query/src/spatial_nodes.rs`; SI wires `fe-api/src/server.rs` + `fe-api/src/lib.rs` | new `fe-api/tests/portal_bundle_test.rs` (new route seam). Extends `fe-api/tests/asset_ownership_test.rs` (anchor-petal reach of a bundle upload), `fe-api/tests/device_ingest_test.rs::html_payload_is_served_as_attachment_with_nosniff` (bundle members stay attachment + nosniff) and the `NodeKind::{as_tag, from_tag}` table in `fe-query/src/spatial_nodes.rs` | T0.2 |
| P0 | T0.4 | S0c | BUILD (cross-track) | sonnet | Bundle MIME branch in both scene-asset gates: ZIP magic + bounded sealed-header check; no texture guard | `fe-runtime/src/scene_asset.rs`, `fe-database/src/handlers/scene_asset_transfer.rs` | extends the QA-0118 prefetch journey (`fe-sync/src/scene_assets/mod.rs` tests: bundle row arrives → blob fetched) and the QA-0131 block in `fe-database/tests/role_propagation_inbound_test.rs` (a repository-node row written while revoked stays absent) | T0.2; A-8 |
| P1 | T1.1 | SD | AUTHOR-ONLY | opus | Protocol contract: request → response, CSP grammar, header set, origin derivation, navigation decision table, spike list SP-1..SP-7 with pass criteria | `docs/spec/portal-bundle/protocol.md` (new) | the decision table becomes T1.3's test table | T0.1 |
| P1 | T1.2 | SP | BUILD (spike) | opus | Run SP-1..SP-7 on real WebView2 in a scratch worktree; append results; reopen any OD that fails | `docs/spec/portal-bundle/protocol.md` (append §spike-results) | evidence only | gate open |
| P1 | T1.3 | S1 | BUILD | opus | Mount table, origin labels, async `fe-portal` protocol + `with_https_scheme`, CSP composer, `navigation::admits` consulted by `decide_navigation` AND `qa_navigation::enforce`, `BrowserCommand::{OpenBundle, CloseBundle}`, origin-aware trust bar, unload-on-close for bundles | `fe-webview/src/portal_bundle/{mod,origin,mount,protocol,csp,navigation}.rs` (new), `backends/tauri.rs`, `backends/qa_navigation.rs`, `ipc.rs`, `plugin.rs`, `lib.rs`, `fe-webview/Cargo.toml` | new `fe-webview/tests/portal_protocol_journey.rs` (new seam: serve + CSP + labels, against fixture archives built in-process). Extends `backends::tauri::tests::decide_navigation_blocks_localhost_and_emits_error` (its Windows `enforce` table gains mounted / unmounted / foreign-bundle rows) and `tests/security_test.rs::test_localhost_blocked` (the raw policy still refuses every bundle-origin spelling) | T0.2, T1.2 |
| P1 | T1.4 | S1 | BUILD | haiku | Delete the unregistrable `tauri_commands.rs` (OD-9); drop its `lib.rs` line | `fe-webview/src/tauri_commands.rs`, `fe-webview/src/lib.rs` | `tests/tauri_cutover_test.rs` stays green; no new test | T1.3 |
| P2 | T2.1 | SD | AUTHOR-ONLY (written against A-1 until the sibling milestone) | opus | `fe.portal.v1` message enum, page API, authorization order, budgets, delivery sources, audit mapping, consumer/producer mapping onto the bus (`node`/`readings`/`query` → sibling `host/node-changes`/`host/readings`/`host/query`; commands with caller kind `portal`); post the OD-3 cadence `question` on the bus | `docs/spec/portal-bundle/ipc-v1.md` (new) | the message table becomes T2.2's decode/authorize table | the sibling's bus contract (watch the bus) |
| P2 | T2.2 | S2 | BUILD | opus | IPC bridge: `with_ipc_handler`; sender-origin + document-generation gate; token buckets; strict decode; broker routing with a host-minted per-mount binding; Windows `PostWebMessageAsJson` delivery; host-served `/__fe/portal-v1.js` | `fe-webview/src/portal_bundle/{ipc,limits}.rs` (new), `fe-webview/src/portal_bundle/portal-v1.js` (new), `backends/tauri.rs`, `plugin.rs` | new `fe-webview/tests/portal_ipc_journey.rs` (new seam: R8–R11 tables with synthetic senders and the real broker) | T1.3; A-1 |
| P2 | T2.3 | SI | BUILD (conditional on the OD-13 fallback) | sonnet | `readings_ingested_tx` broadcast beside `entity_change_tx`, emitted at the ingest commit | `fe-api/src/server.rs`, `fe-api/src/iot.rs` | extends `fe-api/tests/iot_ingest_test.rs` (an ingested reading is observed on the broadcast) | T2.2 |
| P3 | T3.1 | SU | AUTHOR-ONLY | sonnet | Repository inspector section, chrome state table, open/admit/revoke/replace flows, accessible names, keyboard paths; applies `C:\Users\atooz\.gemini\UI-UX-guidance.md` | `docs/spec/portal-bundle/ux.md` (new) | its accessible names are the locators PJ1–PJ9 use | T0.1 |
| P3 | T3.2 | S3 | BUILD | sonnet | `PortalState` bundle arm; pure chrome model; Repository section; `open_bundle` + bundle event arms; toolbar badges. Chrome state rides an existing `SystemParam` bundle (16-param ceiling) | `fe-ui/src/portal/mod.rs`, `fe-ui/src/portal/chrome.rs` (new), `fe-ui/src/portal/repository.rs` (new), `fe-ui/src/actions/portal.rs`, `fe-ui/src/panels/portal_toolbar.rs`; SI wires `fe-ui/src/actions/mod.rs` (`UiAction::OpenBundle` + one arm) and the Repository call site in `fe-ui/src/panels/inspector.rs` | extends the `actions::portal` tests (the `validate_url` / `compute_open_portal` tables refuse bundle-origin URLs as persisted URLs and cover bundle open outcomes); `portal::tests::compute_portal_rect_*` unchanged; chrome is proven by ui-tree names in PJ1–PJ9 | T1.3, T3.1 |
| P4 | T4.1 | SD | AUTHOR-ONLY | opus | Admission state machine and transitions; revoke-mid-session sequence; update policy; static-sandbox feeds; CORS + private-network analysis for the share route | `docs/spec/portal-bundle/admission.md` (new) | the transition table becomes T4.2's table | T0.1, T2.1 |
| P4 | T4.2 | S4 | BUILD | opus | Mount consults governance (A-2); revoke listener → unmount + `load_html` unload + per-origin clear (SP-6); update re-consent; static `/__fe/feeds.json` from `portal.feeds`; fe-api CORS predicate for static origins on `GET /api/v1/shared/{token}` only | `fe-webview/src/portal_bundle/{admission,feeds}.rs` (new), `portal_bundle/mount.rs`, `backends/tauri.rs`, `plugin.rs`; SI: `fe-api/src/server.rs` | extends `fe-webview/tests/portal_ipc_journey.rs` (revoked / static / stale rows) and `fe-api/tests/portal_bundle_test.rs` (the admission read follows governance); native PJ2, PJ3, PJ9 | T2.2; A-2 |
| P5 | T5.1 | SQ | AUTHOR-ONLY | sonnet | Harness spec; PJ1–PJ10 as text; fixture bundle sources (docs-site, dashboard, actions, escape, storm) with fixed slot ids and probe elements; three scenario files | `tools/qa/portal/AGENTS.md` (new), `tools/qa/portal/journeys.md` (new), `tools/qa/portal/bundles/**` (new), `tools/qa/sim/scenarios/portal-{core,replication,load}.json` (new) | text; validated by `run.py validate` once T5.3 registers the probe | T0.1, T1.1, T3.1 (T2.1 for the IPC steps) |
| P5 | T5.2 | S5 | BUILD | sonnet | Deterministic bundle builder: text sources + ladder sizes from `tools/qa/sim/assets.py` `LADDER`; sealing through the staged `seal_hexon.exe` with a run-local seed (controlled-fixtures precedent); unsigned twins | `tools/qa/portal/build_bundles.py` (new) | fixture producer, no test | T5.1; a stage |
| P5 | T5.3 | S5 | BUILD | sonnet | `portal` probe family: admission read + fixed observation + IPC outcome counters, `portal_ms` cadence; analyzer classes for IPC outcomes; `upload_bundle` driver op | `tools/qa/sim/portal_probe.py` (new); SI hooks in `tools/qa/sim/{scenario,prober,analyze,driver}.py` | `run.py validate portal-core portal-replication portal-load` | T5.1; A-8 |
| P5 | T5.4 | S5 | BUILD | sonnet | Fixed bundle operations (observe title/digest/probe elements, numbered slots, IPC counters); routes `/api/v1/debug/portal/{observe,action,ipc-stats}` behind the portal-fixture gate; `fe.py portal` client | `fe-webview/src/backends/fixture_surface.rs`, `fe-api/src/qa_portal.rs`, `tools/qa/portal/client.py` (new); SI: `fe-runtime/src/qa_portal.rs` (`FixtureOp` variants), `tools/qa/fe.py` (`portal` subparser) | extends journey J02 (`tools/qa/AGENTS.md` §Native Portal fixture client: observe/capture plus the refused-link oracle, now on bundle pages too) | T1.3, T5.2 |
| P5 | T5.5 | SQ (run) | BUILD (run) | sonnet | Execute PJ1–PJ9 natively and as sim scenarios; triage into `qa/findings.jsonl` in the existing register format | run dirs only | receipts per journey | all of P0–P4 |
| P6 | T6.1 | SD | AUTHOR-ONLY | opus | Static export: directory layout, `fe-portal.json`, share-URL data path, CORS configuration, non-goals | `docs/spec/portal-bundle/static-export.md` (new) | none | T4.1 |
| P6 | T6.2 | S8 | BUILD (later) | sonnet | Export route; static-export mode of the page API (share-URL reads, no actions or hooks) | `fe-api/src/portal_bundles.rs`, `fe-webview/src/portal_bundle/portal-v1.js` | extends `fe-api/tests/portal_bundle_test.rs`; PJ10 | T4.2 |
| gates | W1..W5 | orchestrator | BUILD | n/a | One serial `cargo check` + touched-batch gate per wave | n/a | n/a | that wave's slices + SI |
| review | R-P1 | S6 | BUILD | opus | `/security-review` + `/code-review high` on the P1 diff, in fresh context; fixes its own findings | `review-p1.md` (new) + fix-only edits | n/a | W2 gate |
| review | R-P3 | S6 | BUILD | opus | `/code-review` + UI review against UI-UX-guidance (one primary action, no helper text, accessible names, states) | `review-p3.md` (new) + fix-only edits | n/a | W3 gate |
| review | R-P2 | S6 | BUILD | opus | `/security-review` on the IPC bridge (sender authorization, ABA across navigation, budgets, binding minting) | `review-p2.md` (new) + fix-only edits | n/a | W4 gate |
| review | R-P4 | S6 | BUILD | opus | `/security-review` on admission, revoke and static feeds (kill-switch latency, storage clear, CORS scope) | `review-p4.md` (new) + fix-only edits | n/a | W5 gate |
| docs | D | S7 | AUTHOR-ONLY (after code) | haiku | Move rationale into the directory AGENTS.md files; pointers; add the new test binaries to `conductor/test-batches.md` R6/R9; plan ticks | see `metadata.json` S7 | n/a | reviews done |

## 3. QA-lane journeys (PJ1–PJ10)

All QA-lane journeys share four rules:
- They run on captured Windows hosts.
- They are driven by `fe.py portal ...`, simulator steps, or both.
- The evidence is native receipts, CapturePreview PNGs (reviewed independently, never self-approved), sim `observations.jsonl`, and `report.md`.
- No caller supplies JavaScript, selectors or URLs.

New journey files exist only for the new seams. Everything else extends a named journey.

- **PJ1 — Render an admitted bundle.** (R1, R2, R5, R6, R9, R13)
  - *Steps:*
    1. The owner uploads the signed `dashboard` and `docs-site` bundles (REST).
    2. The owner creates repository nodes for them (`PATCH …/properties`).
    3. The owner admits them on its own peer (A-2).
    4. The owner clicks **Open** (accessible name from T3.1).
  - *Oracle:*
    - The badge reads "Admitted — <publisher>".
    - The fixed observation shows the fixture's title and the expected DOM digest.
    - The CSP canary is blocked.
    - After a REST transform change, the dashboard's node probe element equals `GET /api/v1/nodes/{id}/transform` within budget.
    - One PNG is captured.
  - *Extends:* J02 observe/capture; `portal-core.json` step 1.
- **PJ2 — Unsigned or unadmitted bundles have no IPC.** (R8, R12)
  - *Table rows:*
    - an unsigned bundle;
    - a signed bundle not admitted on this peer;
    - an external site opened through Portal URL;
    - the admitted bundle's declared https iframe (frame origin ≠ bundle origin);
    - a hidden portal;
    - a stale document after reload;
    - control: an admitted, visible, current document.
  - *Oracle:*
    - Static rows: the fixed probe sees `/__fe/portal-v1.js` → 404, so `window.fe` is absent.
    - Every non-control post raises `dropped_origin` by one, with zero deliveries and no reply.
    - The control row gets a reply.
    - Mutation check: removing the sender-origin check turns the non-control rows red.
  - *Extends:* the `portal_ipc_journey.rs` table (headless) plus `portal-core.json` step 2 (native).
- **PJ3 — The kill switch mid-session unloads the portal.** (R14)
  - *Steps:*
    1. Open the admitted dashboard with three live subscriptions and one queued `act`.
    2. Engage the governance kill switch, either through the local REST action or through a signed revocation list served by the controlled-fixture registry under a QA trust root.
  - *Oracle:*
    - The badge reads "Revoked — kill switch".
    - The fixed observation is refused (document unloaded).
    - IPC stats show zero live subscriptions; the queued `act` is refused.
    - A protocol request for the origin answers 410.
    - Re-opening still renders nothing.
    - After the switch is lifted and the bundle re-admitted, the bundle's `localStorage` key is absent (SP-6), or the residual is recorded.
  - *Extends:* the `portal_ipc_journey.rs` revoked rows; `portal-core.json` step 3.
- **PJ4 — The bundle replicates across peers and is judged by role at write time.** (R3, R4, R10)
  - *Peers:* owner, editor and viewer (N=3 under the current resource gate).
  - *Steps:*
    1. The editor uploads a bundle and creates its repository node.
    2. On the viewer's peer: the node row is present and the blob prefetched, but the state is `StaticSandbox`, with no auto-admit and no auto-open.
    3. The editor admits the bundle on its own peer. The bundle's declared `set_property` persists on every peer.
    4. The owner demotes the editor to viewer. The bundle's `act` is refused at write time, and the row is absent everywhere.
    5. The owner re-grants. The refused write stays absent: no resurrection.
  - *Extends:* p2p-core's "revoke, revoked writes, re-grant: rows must stay absent on every peer" step pattern; new `portal-replication.json`.
- **PJ5 — Large bundles (size ladder).** (R2, R15)
  - *Bundles:* `tier_100kb`, `tier_1mb`, `tier_5mb` and `tier_25mb` (`tools/qa/sim/assets.py:17`), plus a 40 MB over-cap bundle.
  - *Oracle:*
    - Uploads return 200, or 413 for the 40 MB bundle, whose `asset_blob` stays absent.
    - Each tier replicates to the second peer within budget (`latency.csv`).
    - The entry renders within budget.
    - No frame stalls in the host band while a 5 MB member is served.
  - *Extends:* p2p-assets ladder steps; new `portal-load.json`.
- **PJ6 — Offline.** (R15)
  - *Steps:*
    1. Stop the providers with `host stop_peer`.
    2. Peer B opens a bundle that is already local. It renders.
    3. Peer B opens a bundle that is not local. It shows "Waiting for bundle bytes".
    4. Restart the providers. The second bundle renders after the prefetch re-arm (15 s or more).
  - *Extends:* the QA-0118 prefetch journey pattern; `portal-core.json` step 4.
- **PJ7 — Subscription storm and write burst are rate-limited.** (R9, R10)
  - *Steps:* the `storm` bundle opens 64 subscriptions, sends 500 msgs/s for 5 s, then issues 20 `set_property` calls in 1 s. A 60 s idle phase follows.
  - *Oracle:*
    - Subscriptions: at most 16 accepted; the rest get `too_many_subscriptions`.
    - Messages: `rate_limited` counts match the budget.
    - Writes: at most 7 accepted.
    - Zero "db thread" timeout lines appear in the peer log.
    - The idle phase shows zero reads for `node` and `readings` subscriptions; `query` re-runs stay at or below one per cadence period, with none while hidden.
  - *Extends:* city-lite's throughput reading (write ceiling ~8/s); `portal-load.json`.
- **PJ8 — Navigation escape attempts are blocked.** (R7)
  - *Escape slots:*
    1. an undeclared https link;
    2. loopback `http://127.0.0.1`;
    3. private `http://192.168.1.1`;
    4. another bundle's origin;
    5. `file:///C:/`;
    6. a `javascript:` URL;
    7. `window.open`, which is swallowed;
    8. a meta-refresh to an undeclared origin;
    9. a form POST, blocked by `form-action 'none'`;
    10. control: a declared https navigation.
  - *Oracle:*
    - Slots 1–9: the native refusal counter rises by one and the exact refused destination is reported, with the original page unchanged (the J02 refused-third-link oracle).
    - Control: the page navigates, and a later post from it raises `dropped_origin`.
  - *Extends:* J02 refused-third-link; the `decide_navigation` / `enforce` table.
- **PJ9 — Update escalation drops to static.** (R14)
  - *Steps:*
    1. v1 is admitted.
    2. v2 arrives with the same capabilities. It stays Admitted on the same origin, and the `localStorage` key persists.
    3. v3 arrives adding a `set_property` scope. It becomes `StaticSandbox` (badge: "Static — not admitted on this peer"), and posts are dropped.
    4. The user re-consents. The bundle is Admitted again.
  - *Extends:* the `portal_ipc_journey.rs` admission rows; `portal-core.json` step 5.
- **PJ10 (P6, later) — Static export.** (R17)
  - *Steps:* the exported directory is served by a static server whose origin is listed in `cors_origins`.
  - *Oracle:*
    - It renders from a share URL.
    - An expired share URL returns 410, and the page shows its expired state.
    - No page API is present.

## 4. Review-and-fix gates

Each review runs in a fresh context, never the agent that wrote the code (S6, opus). It fixes its own findings, records one verdict line per finding in `review-p<N>.md`, and ends with a `ready-to-merge` bus entry for the orchestrator.

1. **R-P1 (protocol):** `/security-review` is mandatory. It checks:
   - the raw policy is untouched;
   - the exact-origin admission is consulted in both layers;
   - traversal and encoding spellings are handled;
   - tampered members are never served;
   - the service-worker refusal works;
   - the CSP is composed only from the vocabulary;
   - the trust-bar change is correct;
   - the unload token is one-shot.
2. **R-P2 (IPC):** `/security-review` is mandatory. It checks:
   - authorization happens before decoding;
   - unauthorized senders get no reply;
   - the document clock is distinct from the capture clock;
   - the per-mount binding is host-minted;
   - budgets hold;
   - delivery is gated by generation plus envelope origin;
   - there are no `eval` paths with variable code.
3. **R-P3 (UX):** `/code-review` plus a UI review against the UI-UX guidance:
   - one primary action per state;
   - no helper text;
   - stable accessible names;
   - keyboard reachability;
   - unchanged layout insets.
4. **R-P4 (admission):** `/security-review` is mandatory. It checks:
   - kill-switch latency is within one frame of the decision;
   - no IPC survives a revoke;
   - the per-origin storage clear works, or its documented residual;
   - the CORS predicate is limited to the share route;
   - update re-consent cannot be bypassed.

A phase with no recorded verdict is unreviewed, not done.

## 5. Waves (DAG)

- **W0 — now, AUTHOR-ONLY, parallel-safe (disjoint files):** SD (T0.1, T1.1, T2.1, T4.1, T6.1), SU (T3.1), SQ (T5.1).
- **W1 — gate open:** SP (T1.2 spikes), S0 (T0.2), then SI.
- **W2:** S1 (T1.3, T1.4) and S0b (T0.3), then SI, gate, R-P1.
- **W3:** S3 (T3.2) and S5 (T5.2, T5.3, T5.4; sim files only after A-8), then SI, gate, R-P3.
- **W4:** S2 (T2.2, after the sibling bus milestone) and S0c (T0.4, after A-8); T2.3 only on the OD-13 fallback. Then SI, gate, R-P2.
- **W5:** S4 (T4.2, after the sibling governance), then SI, gate, R-P4.
- **W6:** T5.5 journey runs (PJ1–PJ9).
- **W7:** S7 docs, then the full sweep once.
- **Later:** S8 (P6).

SI, the integration wiring slice, runs serially at the join of every BUILD wave: after that wave's leaves, before the wave gate. Central files are touched only there (see `metadata.json` `shared_writes`).

## 6. Cross-track sequencing

| File(s) | Other owners | Rule |
|---|---|---|
| `fe-api/src/server.rs` | `agent_input_surface_20261031`, `relay_operability_20261031`, `plugin_host_revival_20261031` S1, `extension_platform_20261107` | All register routes in `build_router`. SI adds one-line registrations (and, in W5, the share-route CORS predicate) after the predecessors land. Re-grep every time. |
| `fe-ui/src/actions/mod.rs`, `fe-ui/src/panels/inspector.rs` | Central UI dispatch, many tracks | SI adds one `UiAction` variant, one delegating arm and one section call. |
| `fe-format/src/manifest.rs` | Sibling track (`HexonType::Plugin` family + web discriminator), after `hexon_entitlements_20261031`'s format-spec 1.1.0 bump | **Not this track's.** SI only adds `pub mod portal_bundle;` to `fe-format/src/lib.rs` after the sibling's edit lands. |
| `fe-runtime/src/scene_asset.rs`, `fe-database/src/handlers/scene_asset_transfer.rs` | P2P campaign (`scene_asset_transfer.rs` changed in `11b44d4f` during authoring; the campaign is still active) | S0c waits for the campaign (A-8), then re-greps. |
| `tools/qa/fe.py`, `tools/qa/sim/{scenario,prober,analyze,driver}.py`, `tools/qa/AGENTS.md` | P2P-QA simulator campaign (active; `tools/qa/sim/prober.py` was dirty in the main tree during authoring) | Registration hooks only, after its sim edits merge. Portal logic lives in this track's own new modules. |
| `conductor/tracks.md`, `conductor/accountability-log.md` | Orchestrator | Never edited by this track. |

## 7. Task checklist

- [ ] T0.1 bundle container + `portal` manifest section v1 (AUTHOR-ONLY)
- [ ] T1.1 protocol / CSP / allowlist contract + spike list (AUTHOR-ONLY)
- [ ] T2.1 `fe.portal.v1` IPC contract, assumption-marked until the sibling bus milestone (AUTHOR-ONLY)
- [ ] T3.1 repository-node UX + chrome states (AUTHOR-ONLY)
- [ ] T4.1 admission state machine + static-sandbox feeds (AUTHOR-ONLY)
- [ ] T5.1 QA lane: harness spec, PJ1–PJ10 text, fixture sources, scenario JSON (AUTHOR-ONLY)
- [ ] T6.1 static export spec (AUTHOR-ONLY)
- [ ] T1.2 spikes SP-1..SP-7 (BUILD)
- [ ] T0.2 fe-format validator (BUILD)
- [ ] T0.3 fe-api bundle routes + listing + `NodeKind::Repository` (BUILD)
- [ ] T1.3 custom protocol + CSP + navigation allowlist (BUILD)
- [ ] T1.4 delete `tauri_commands.rs` (BUILD)
- [ ] R-P1 protocol security review (BUILD)
- [ ] T3.2 repository inspector + chrome (BUILD)
- [ ] T5.2 bundle builder (BUILD)
- [ ] T5.3 sim `portal` probe (BUILD)
- [ ] T5.4 fixed bundle QA operations + `fe.py portal` (BUILD)
- [ ] R-P3 UX review (BUILD)
- [ ] T2.2 IPC bridge (BUILD)
- [ ] T0.4 scene-asset MIME branch, cross-track (BUILD)
- [ ] T2.3 readings broadcast, only on the OD-13 fallback (BUILD)
- [ ] R-P2 IPC security review (BUILD)
- [ ] T4.2 admission integration, revoke, static feeds (BUILD)
- [ ] R-P4 admission security review (BUILD)
- [ ] T5.5 run PJ1–PJ9 + sim scenarios (BUILD)
- [ ] D docs consolidation + test-batches rows; full sweep once
- [ ] T6.2 static export (BUILD, later)
