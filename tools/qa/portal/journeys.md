# Portal QA journeys PJ1-PJ10

Runnable text for track `portal_surface_20261107` (plan section 3 is the source; this file is the operational form). Harness: [AGENTS.md](AGENTS.md). Receipts, PNGs and `report.md` go to the run directory; findings go to `qa/findings.jsonl` in the existing register format. No caller supplies JavaScript, selectors or URLs.

Locator rule: slots are numbered (`data-fe-slot`), probes are named (`data-fe-probe`), chrome is read by accessible name from `docs/spec/portal-bundle/ux.md`.

| Id | Requirements | Bundles | Tier | Scenario step | Receipt |
|---|---|---|---|---|---|
| PJ1 | R1 R2 R5 R6 R9 R13 | dashboard, docs-site | native + sim | portal-core 1 | badge, title, digest, canary, 1 PNG |
| PJ2 | R8 R12 | dashboard (unsigned), actions | headless + native | portal-core 2 | `dropped_origin` x5, control reply |
| PJ3 | R14 | dashboard | headless + native | portal-core 3 | Revoked badge, 410, 0 subs |
| PJ4 | R3 R4 R10 | actions | sim (3 peers) | portal-replication 1-4 | per-peer rows and admission |
| PJ5 | R2 R15 | ladder | sim | portal-load 1-2 | `latency.csv`, 413 for 40 MB |
| PJ6 | R15 | dashboard, docs-site | sim | portal-core 4 | Waiting badge then render |
| PJ7 | R9 R10 | storm | native + sim | portal-load 3 | refusal counters, peer log |
| PJ8 | R7 | escape | headless + native | native only (table) | refusal counter x9 + control |
| PJ9 | R14 | actions v1-v3 | headless + native | portal-core 5 | badge transitions, storage key |
| PJ10 | R17 | exported dashboard | static server | P6, later | share-URL render, 410 expired |

## PJ1 Render an admitted bundle
1. Owner uploads signed `dashboard` and `docs-site` (REST); creates repository nodes (`PATCH .../properties`); admits both locally.
2. Owner clicks **Open** on the dashboard repository node.
3. Oracle: badge "Admitted - <publisher>"; fixed observation title `QA Dashboard` and expected digest; probe `csp` = `canary-blocked`; after a REST transform change of `Probe Target`, probe `node-position` equals `GET /api/v1/nodes/{id}/transform` within budget; one PNG.

## PJ2 Unsigned or unadmitted bundles have no IPC
Rows: unsigned bundle; signed but unadmitted on this peer; external site via Portal URL; the admitted bundle's declared https iframe (frame origin differs from bundle origin); hidden portal; stale document after reload; control (admitted, visible, current).
Oracle: static rows see `/__fe/portal-v1.js` 404 so `window.fe` is absent; every non-control post raises `dropped_origin` by one with zero deliveries and no reply; the control row is answered. Mutation check: removing the sender-origin check turns the non-control rows red.

## PJ3 Kill switch mid-session unloads the portal
1. Open the admitted dashboard with three live subscriptions and one queued `act`.
2. Engage the kill switch (local REST, or a signed revocation list from the controlled-fixture registry under a QA trust root).
3. Oracle: badge "Revoked - kill switch"; observation refused; zero live subscriptions; queued `act` refused; protocol answers 410; re-open renders nothing. After lifting and re-admitting, the `qa_origin_key` localStorage key is absent (SP-6) or the residual is recorded as a finding.

## PJ4 Replication, judged by role at write time
Peers: owner, editor, viewer.
1. Editor uploads the bundle and creates its repository node.
2. Viewer: row present, blob prefetched, state `StaticSandbox`, no auto-admit, no auto-open.
3. Editor admits on its own peer; the declared `set_property` persists on every peer.
4. Owner demotes the editor; the bundle's `act` is refused at write time and the row is absent everywhere.
5. Owner re-grants; the refused write stays absent (no resurrection).

## PJ5 Large bundles (size ladder)
Bundles `tier_100kb`, `tier_1mb`, `tier_5mb`, `tier_25mb`, plus a 40 MB over-cap build.
Oracle: uploads return 200 (413 for 40 MB with `asset_blob` absent); each tier replicates within budget (`latency.csv`); the entry renders within budget; no host-band frame stall while a 5 MB member is served.

## PJ6 Offline
1. `host stop_peer` on the providers.
2. Peer B opens a local bundle: renders. Opens a non-local one: "Waiting for bundle bytes".
3. Restart providers: the second renders after the prefetch re-arm (15 s or more).

## PJ7 Subscription storm and write burst
`storm` opens 64 subscriptions, sends ~500 msgs/s for 5 s, then 20 `set_property` in 1 s, then idles 60 s.
Oracle: at most 16 subscriptions accepted, the rest `too_many_subscriptions`; `rate_limited` counts match the budget; at most 7 writes accepted; zero "db thread" timeout lines in the peer log; idle phase: zero reads for `node` and `readings`, `query` re-runs at most one per cadence period and none while hidden.

## PJ8 Navigation escape attempts
Slots on `escape`: 1 undeclared https; 2 loopback; 3 private 192.168.1.1; 4 another bundle origin; 5 `file:///C:/`; 6 `javascript:`; 7 `window.open` (swallowed); 8 meta refresh to undeclared origin; 9 form POST (blocked by `form-action 'none'`); 10 control, declared https.
Oracle: slots 1-9 raise the native refusal counter by one with the exact refused destination reported and the page unchanged (J02 refused-third-link oracle); slot 10 navigates, and a later post from it raises `dropped_origin`.

## PJ9 Update escalation drops to static
1. v1 admitted. 2. v2 (same caps) stays Admitted on the same origin; `localStorage` key persists.
3. v3 adds a `set_property` key scope: becomes `StaticSandbox` (badge "Static - not admitted on this peer"), posts dropped.
4. Re-consent: Admitted again.

## PJ10 (P6, later) Static export
Bundle: the `dashboard` build exported with `static-export.md` (the export carries no host bridge; `window.fe` is never defined).
1. Attach one share URL minted on a shared fe-api as the repository node's `portal.feeds` entry, export the dashboard (`GET /api/v1/nodes/{node_id}/portal/export.zip`), and serve the directory at the root of a static server whose origin is listed in that fe-api's `cors_origins`, sending `fe-portal.json.csp` as the `Content-Security-Policy` header.
2. Open the export's entry page. Oracle: `status` reads `api-absent` (no `__fe/portal-v1.js` in the export), `feed` reads `200` (the share URL from `__fe/feeds.json`, fetched by the same code path the in-app static sandbox uses), and `csp` reads `canary-blocked`.
3. Expire the share (clock advance past its TTL). Oracle: the feed answers 410 and `feed` reads `expired`; no request carries a host credential.
4. Mutation check: serve the export without the `cors_origins` entry; the feed fetch fails and `feed` reads `error`.
Receipt: page probe values, the 410 status line, one PNG. Tier: static server only (no host); native tier not involved.

## Extends
J02 observe/capture (PJ1, PJ8); `portal_ipc_journey.rs` table (PJ2, PJ3, PJ9); p2p-core revoke pattern (PJ4); p2p-assets ladder (PJ5); QA-0118 prefetch pattern (PJ6); city-lite write ceiling of about 8/s (PJ7).
