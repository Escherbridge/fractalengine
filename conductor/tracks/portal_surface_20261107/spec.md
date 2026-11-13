---
type: spec
title: Portal surface — signed web bundles in the Petal Portal, custom protocol, host-brokered IPC, repository nodes
track: portal_surface_20261107
tags: [feature, portal, fe-webview, fe-ui, fe-api, fe-format, security, ipc, qa-lane, portal_surface_20261107]
created: 2026-11-07
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: user grill 2026-11-07 (extension platform program; decisions D-X5..D-X8)
depends_on: [extension_platform_20261107 (public sibling track; P2 integration bus + WIT 2.0, P4 governance), plugin_host_revival_20261031 (S-PLUGIN signatures, trust roots, revocation list; P4)]
---

# Specification: Portal surface

**Track ID:** `portal_surface_20261107` · **Type:** feature · **Priority:** P2
**Crates:** `fe-webview` (bundle mount, `fe-portal` custom protocol, navigation allowlist, IPC bridge, fixed QA operations), `fe-ui` (repository-node inspector, portal chrome), `fe-api` (bundle ingest, directory listing, local admission read, QA routes), `fe-format` (portal manifest section + validator), `fe-query` (`repository` node kind). Cross-track touch points: the `fe-runtime` / `fe-database` scene-asset MIME gates and `tools/qa` (the portal QA lane).
**Session:** EXT-PORTAL, worktree `.omc/worktrees/ext-portal`, branch `track/portal-surface` (D-X8). **Build gate:** author-only while `.omc/state/build-gate.json` has `cargo_allowed: false` (D-X2).
**Citations computed at:** `cfd7fce6`. They were first read at `30163ca2`; the three P2P commits that landed during authoring (`fe1e6b75`, `11b44d4f`, `cfd7fce6`) moved no cited line. Sibling-track ids (R11, R13–R15, R23, R24, OD-E8) are cited from the uncommitted `extension_platform_20261107` draft present at authoring, so re-verify them after it commits. Symbol names are stable; line numbers are not, so re-grep before editing.

## 1. User vision (2026-11-07)

Websites render inside FractalEngine as **portals**, using the existing Petal Portal webview. Extensions and verse content can write to them:

- dashboards and documentation sites fed by node data;
- web-driven interactions back into the engine;
- **repository nodes** that hold a code bundle which renders in the portal.

A bundle is a signed package in the same sealed `.hexon` family as extensions (D-X1, D-X5). Each peer decides for itself whether a bundle gets the data and interaction API (D-X5). Unsigned or unadmitted bundles still render, but with no bridge.

The extension runtime, WIT 2.0, governance, authoring tools, the Extensions UI and the integration bus belong to the sibling track `extension_platform_20261107`. This track consumes them and never redefines them.

## 2. Problem & evidence: a working portal with no bundle, no protocol and no bridge

Verified at `cfd7fce6`:

- **The portal is one external-URL webview.**
  - fe-ui's `UiAction::OpenPortal { url }` writes `BrowserCommand::Navigate { url }` (`fe-ui/src/actions/mod.rs:1325-1329`).
  - `TauriBackend` builds a wry 0.54 webview inside a borderless popup owned by the Bevy window (`fe-webview/src/AGENTS.md:32-36`, `backends/tauri.rs:101-112`).
  - There is one backend instance (`WebViewBackendRes`, `fe-webview/src/plugin.rs:38-44`), so one portal is open at a time.
  - The URL lives on the model row: "This preserves the existing model.external_url storage contract" (`fe-ui/src/AGENTS.md:158-160`).
- **Every layer refuses anything that is not a public http(s) URL.** `is_url_allowed` admits only `http`/`https` (`fe-runtime/src/portal_url.rs:37`) and refuses any host whose last label is `localhost` (`:46-49`). The same policy runs in four places, and the doc says this is deliberate: "Enforced in **both** layers on purpose" (`fe-webview/src/AGENTS.md:78-79`):
  - at dispatch (`fe-webview/src/plugin.rs:244`);
  - in `TauriBackend::navigate` (`backends/tauri.rs:204-207`);
  - in wry's navigation callback, `decide_navigation` (`:36-70`);
  - in the native WebView2 owner on captured QA hosts, `qa_navigation::enforce` (`backends/qa_navigation.rs:84-89`).
- **There is no custom protocol and no IPC handler.**
  - The builder sets bounds, visibility, autoplay, the trust-bar init script and a page-load handler, and nothing else (`backends/tauri.rs:101-112`).
  - The only "commands" are two asset helpers that take a `tauri::AppHandle` (`fe-webview/src/tauri_commands.rs:13-18,49-50`).
  - The crate doc confirms both points: "Nothing registers them yet: there is no `tauri::Builder` in the binary; the embedded backend uses wry directly" (`fe-webview/src/AGENTS.md:86-89`), and "there is no webview-JS object mutation/query bridge at this seam" (`:91-93`).
- **The bundle container is a placeholder.**
  - `DIRECTORY_ASSET_CONTENT_TYPE = "application/x-fe-directory"` (`fe-api/src/assets.rs:29`) is a sentinel; `serve_asset_by_id` answers it with `501` (`:371-385`).
  - fe-api's doc reserves it as an extension point: "when directory assets are real, that branch is where a manifest/listing response slots in" (`fe-api/AGENTS.md:408-418`).
  - Uploads accept GLB only (`validate_glb`, fe-api §asset-ingest; `MAX_ASSET_BYTES` 256 MiB at `fe-api/src/upload.rs:24`).
- **Replication refuses anything that is not glTF.** Two gates enforce this. The P2P campaign changed the first one during authoring (`11b44d4f`) and is still active, so any edit here sequences behind it:
  - the scene-asset transfer admission keeps only `model/gltf-binary | model/gltf+json` rows (`fe-database/src/handlers/scene_asset_transfer.rs:72-77`);
  - the byte verifier bails with "unsupported scene asset MIME" for anything else (`fe-runtime/src/scene_asset.rs:94-112`).
- **The QA evidence surface runs only fixed scripts, by doctrine.**
  - `fixture_surface` runs host-owned DOM scripts and CapturePreview inside the real backend on captured Windows hosts. The rule: "No caller supplies JavaScript, selectors or URLs" (`fe-webview/src/AGENTS.md:161`, `backends/fixture_surface.rs:501-550`).
  - The driver chain is `fe.py portal-fixture` → `/api/v1/debug/portal-fixture/*`, which is Owner-only, input-gated, with a five-second host deadline (`fe-api/AGENTS.md:2257-2259`).
- **The trust bar is page content.** `TRUST_BAR_JS` prepends `'External Website: ' + location.hostname` (after a globe glyph) into the page DOM (`fe-webview/src/plugin.rs:11-16`). Page script can remove it, and on a bundle the text would be wrong.
- **The sealed `.hexon` already does what a bundle needs.**
  - A sealed archive carries `members.json` (path → BLAKE3) under a publisher signature, readable under `IntegrityPolicy::RequireSealed { pin }` (`fe-format/src/integrity.rs:62-77`).
  - `HexonArchiveIndex` checks every lazy `read_member` against that signed coverage (`fe-format/src/archive_index.rs:95-100,140-143`).
  - `HexonType` has no plugin or web variant yet (`fe-format/src/manifest.rs:6-30`); D-X1 adds the Plugin family in the sibling track.
- **A capability-binding pattern already exists.**
  - `PluginAdmission` is a "Host-minted capability binding; never deserialized from plugin request data" (`fe-runtime/src/plugin_host.rs:6-17`).
  - The plugin scene bridge's DB side "resolves current local identity and rechecks role, capability and stored ancestry at use" (`fractalengine/src/AGENTS.md:1706-1707`).
  - Portal actions reuse that pattern through the integration bus.

## 3. Product fit (against `conductor/product.md`)

- **Dashboards are an egress consumer, not a BI tool.** product.md says "FractalEngine never tries to be your dashboard" (`:59`) and lists as a non-goal "Not a dashboard/BI tool ... FractalEngine does not grow charting or report layouts" (`:170-171`). This track keeps that line:
  - It ships a *surface* (a bundle container, a per-package origin, a data path) and no charting, report layout or dashboard builder.
  - A dashboard is author-supplied code. It reads through the same guarded handlers BI egress uses: the `/api/v1/query` guards, the analytics query and signed share URLs.
  - It renders next to the spatial context it describes.
  - OD-11 asks the orchestrator for one clarifying line in product.md, so a reader sees no contradiction.
- **Rich extension UI lives here.** The sibling keeps extension UI declarative and host-rendered: "rich UI belongs to portals" (sibling draft OD-E8).
- **Documentation sites** are static bundles: operating procedures, sensor manuals and runbooks, placed as a repository node beside the twin they document. They need no bridge, so they work in the static sandbox from day one.
- **Repository nodes** extend the founding idea that "3D objects can carry an embedded in-world browser portal" (`product.md:142-143`). A bundle becomes an addressable node like any other (`GET /api/v1/nodes/{id}/address`, fe-api §endpoint-surface). Vocabulary: `BrowserInteraction` is listed as never-built, non-current vocabulary (`product.md:149-150`), so this track says **portal action**.
- **Open core.** "Anything a self-hoster needs to **run, verify, or extend** FractalEngine is open and lives here" (`conductor/open-core.md:20`). This track rides S-PLUGIN (`:54`). No business logic, catalogue operations or account model enter this repository.

## 4. Decisions register

These were ratified in the 2026-11-07 user grills. They are binding; do not re-litigate them (`.omc/state/bus/SESSION-PROTOCOL.md` rule 11). New decisions go to the orchestrator as a bus `question`.

| Id | Scope | Decision |
|---|---|---|
| D-X1 | shared | Third-party packages are sealed `.hexon` packages (`HexonType::Plugin` family) carrying Wasm components against WIT `fractalengine:plugin@2.0`; signed; trust roots; package-level kill switch. |
| D-X2 | shared | Author-only (specs, docs, journeys as text, scenario JSON) until the build gate flips `cargo_allowed=true`. |
| D-X5 | this track | **Bundle trust.** A web bundle is a signed package (same sealed `.hexon` family, a web content type) admitted **per peer** by the extension governance (trust roots, per-verse policy gated by RBAC roles, consent, kill switch) before it gets the IPC data/interaction API. Unsigned or unadmitted bundles still render, but in a no-IPC static sandbox (docs, read-only dashboards fed only by the public REST). Replicated rows never auto-execute on arrival. |
| D-X6 | this track | **Data path.** Host-brokered IPC subscriptions: the webview gets a small typed API over the IPC bridge. `subscribe(query \| readings \| node)` streams changes from the same handlers REST uses. Actions (`set_property`, invoke extension command) go back through the capability manifest and are judged like any write (role at write time). PLUS the integration bus (defined in `extension_platform_20261107`): extensions expose streams/commands and consume hooks through the host broker; portals are a consumer/producer on that bus. |
| D-X7 | this track | **Web reach.** Wave 1 renders bundles in-app from the local blob store via a custom protocol. "Through the web" is a later phase: a static export of the same bundle talking to the public REST with a scoped viewer token. Hosting is out of scope. |
| D-X8 | this track | **Ownership.** Its own public track with its own session (EXT-PORTAL, worktree `.omc/worktrees/ext-portal`, branch `track/portal-surface`) and a dedicated QA lane. Cross-session coordination: `.omc/state/bus/README.md`. |

## 5. Scope, non-goals, boundary

**In scope:**

- the bundle's web manifest section and its validator;
- bundle ingest and directory listing;
- repository nodes;
- the `fe-portal` custom protocol, with per-package origins, a host-authored CSP and a navigation allowlist;
- the IPC bridge, as a consumer and producer on the integration bus;
- portal chrome and the inspector UX;
- admission states, including the static sandbox;
- the dedicated QA lane;
- later, static export.

**Non-goals (hard):**

- **No charting, report layout or dashboard builder** (`product.md:170-171`). QA fixture dashboards are fixtures, not product.
- **No second governance.** Trust roots, the revocation list, the consent UI, per-verse install policy, the kill switch and canonical-log audit are seams of `extension_platform_20261107` / `plugin_host_revival_20261031`. This track consumes them.
- **No relaxation of the raw URL policy.** `is_url_allowed` stays byte-for-byte as it is. Bundle origins are admitted only by exact match against the live mount table. This mirrors "No QA private-origin exception is introduced" (`fe-webview/src/AGENTS.md:141`).
- **No Tauri command system.** There is no `tauri::Builder`, and adding one would hand Tauri the event loop Bevy owns (§8 item 1).
- **No remote code.** `script-src 'self'` only; a signed bundle cannot load code its signature does not cover.
- **No inline serving of bundle members from the fe-api origin.** REST keeps `Content-Disposition: attachment` + `X-Content-Type-Options: nosniff` (`fe-api/AGENTS.md:327-336`).
- **Static-export phase:** no hosting, CDN, custom domains, auth federation, accounts or payments.
- **No multi-portal.** One webview, one open portal.

**Boundary:**

- Bundle authoring and signing tools (`fe-ext` scaffold/pack/sign/validate) belong to `extension_platform_20261107`.
- Any external catalogue builds on S-REGISTRY and lives outside this repository (`conductor/open-core.md:20-26`).

## 6. Design

### 6.1 Bundle model

- **Container.** One sealed `.hexon` archive per bundle version, in the Plugin family, carrying the web content type D-X1/D-X5 assign. The discriminator lands in the sibling track's manifest v2 (A-3).
  - Body members live under `web/`.
  - The `portal` manifest section is the only new schema this track owns.
  - One archive is one blob with one content hash. That is the only shape that replicates whole today: scene transfer moves one root blob, and "transferring its root file does not certify those files" (`fe-sync/src/AGENTS.md:2027-2028`).
- **The `portal` manifest section, v1:**
  - `schema: "fe-portal-bundle/1"`;
  - `entry`: an HTML member under `web/`;
  - `title`;
  - `csp`: additions from a closed vocabulary — `connect`, `img`, `font`, `media` and `frame` (each a list of `https` origins that must also pass `is_url_allowed`), plus the booleans `wasm` and `inline_styles`;
  - `navigation`: `https` origins the top frame may leave to;
  - `subscriptions`: `node`, `readings`, and named `queries` (SQL text plus parameter types);
  - `commands`: `set_property` with a property-key scope, and extension commands by `(package_id, command)`;
  - `hooks` (consumed) and `streams` (produced);
  - `ui_intents`: `focus_node` and `select_node`.
- **The validator refuses:** a missing or non-HTML entry; any member under the reserved `__fe/` prefix; any non-https or policy-blocked origin; anything outside the vocabulary; a size over the cap (OD-4); and anything over fe-format's existing archive limits (`fe-format/src/limits.rs:8-14`: 64 MiB per entry, 1 GiB per archive, 200 000 entries).
- **Asset mapping.**
  - The archive is stored once in the blob store (BLAKE3), with an `asset` row whose `content_type` is the bundle MIME (OD-5).
  - That row has the same anchor-petal ownership and reach as any upload (`fe-api/AGENTS.md` §assets, DEC-C21).
  - The directory-asset branch at `fe-api/src/assets.rs:371-385` stops answering `501` for bundles. It returns the **file index** instead (member path, size, BLAKE3, MIME): the "manifest/listing response" the extension point reserved.
  - Members are never served inline from the API origin.
- **Repository node.** An ordinary `node` row tagged `properties.node_kind = "repository"` with `properties.portal.bundle_asset = <asset_id>` (OD-12).
  - It follows the tag convention `stamp` and `earthwork_region` already use (`fe-api/AGENTS.md:196-201`), so there is no schema change.
  - `GET /api/v1/petals/{p}/nodes?kind=repository` needs one `NodeKind` variant, because the vocabulary is a closed enum (`fe-query/src/spatial_nodes.rs:39-48`).
  - The reference replicates through the existing property path and is judged by the writer's role at write time (QA-0131, commit `b852c4b1`).
- **Per-peer admission** is a local record keyed by `(package_id, content_hash)` and owned by the sibling governance (A-2). Its states are `Admitted`, `StaticSandbox`, `Blocked` and `Revoked`. It is never replicated, and a row or blob arriving from a peer cannot change it (D-X5).

### 6.2 Serving: the `fe-portal` custom protocol

- **Registration.** `TauriBackend::create` registers one asynchronous custom protocol, `fe-portal`, via wry's `with_asynchronous_custom_protocol` (`wry-0.54.4/src/lib.rs:1120-1147`, default `protocol` feature) and `with_https_scheme(true)`.
  - A protocol cannot be added after the webview exists, so the handler resolves every request against a shared **mount table**, which `BrowserCommand::OpenBundle` fills.
  - `RequestAsyncResponder` is `Send` (`lib.rs:446-449`), so member bytes are read and verified off the Bevy main thread.
- **Origins.** On Windows, wry rewrites `fe-portal://<host>/p` to `https://fe-portal.<host>/p` behind a `https://fe-portal.*` WebResourceRequested filter (`lib.rs:1043-1044`, `webview2/mod.rs:932-950`). The `<host>` part is a label:
  - **admitted:** `a-<base32(BLAKE3(publisher_did ‖ 0x00 ‖ package_id))[..26]>.localhost`. A publisher's updates keep one origin, and with it their storage. Different publishers never share one.
  - **static:** `s-<base32(content_hash)[..26]>.localhost`. A static rendering never shares an origin (and so never storage or a service worker) with an admitted one. That rules out *origin poisoning*: content planted while unadmitted can never survive into the admitted state.
  - OD-1 covers the `.localhost` suffix and how it squares with `tech-stack.md:141`.
- **Response contract.**
  - GET/HEAD only. The origin label must be mounted, and the path must be a `web/` member or one of the reserved `__fe/` paths (§6.4, §6.6).
  - Bytes come from `HexonArchiveIndex::read_member`, under `RequireSealed { pin: publisher }` when admitted or `VerifyIfSealed` when static. A tampered member is a refusal; it is never served.
  - MIME comes from a fixed extension table, with no sniffing.
  - Requests carrying `Service-Worker: script` are refused.
  - Unknown paths return `404`, origins unmounted mid-flight return `410`, and both have empty bodies.
- **Host-authored headers on every response.**
  - `Content-Security-Policy`, composed from the manifest vocabulary:
    `default-src 'none'; script-src 'self' ['wasm-unsafe-eval']; style-src 'self' ['unsafe-inline']; img-src 'self' data: blob: [img]; font-src 'self' [font]; media-src 'self' [media]; connect-src 'self' [connect]; frame-src [frame | 'none']; worker-src 'self' blob:; form-action 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; upgrade-insecure-requests`.
  - Also `X-Content-Type-Options: nosniff`, `Cross-Origin-Opener-Policy: same-origin`, `Cross-Origin-Resource-Policy: same-origin`, `Referrer-Policy: no-referrer`, a deny-all `Permissions-Policy`, and `Cache-Control: no-store`.
  - A `<meta>` CSP inside the bundle can only narrow this policy, never widen it. This satisfies tech-stack's "Enforce Content Security Policy headers" (`conductor/tech-stack.md:142`).

### 6.3 Navigation allowlist

One pure admission function, `portal_bundle::navigation::admits(raw, mount_table)`, is consulted in **both** existing layers: `decide_navigation` (the production wry callback, `backends/tauri.rs:36-70`) and `qa_navigation::enforce` (the native owner on captured hosts, `backends/qa_navigation.rs:84-89`).

- **Admitted top-frame destinations:**
  - the **same mounted origin**;
  - an `https` origin listed in `navigation` that also passes `is_url_allowed`. Leaving the bundle detaches IPC; the page is now an external website.
- **Refused:** any other bundle origin, unmounted origins, top-level `file:`, `data:` and `javascript:`, loopback/private/link-local addresses, undeclared origins, and redirects into a bundle origin.
- **New windows.** `window.open` is already swallowed: wry marks every unhandled `NewWindowRequested` as handled (`webview2/mod.rs:784-786`), and captured hosts deny explicitly (`backends/tauri.rs:114-116`).
- **Sub-frames** are bounded by `frame-src` (default `'none'`). Whether wry's callback also sees frame navigations is spike SP-2; either way, the CSP is authoritative.
- **Raw bundle URLs can never be persisted or dispatched.** `validate_url` (`fe-ui/src/actions/portal.rs:106-122`) and `BrowserCommand::Navigate` keep refusing them. Only the typed `OpenBundle` path mints them, from the mount table.
- **The host's own unload** (`load_html`, `wry-0.54.4/src/lib.rs:2190`) passes the guard through a one-shot host token, not a scheme exception (SP-5).

### 6.4 IPC API (consumer and producer on the integration bus)

- **Transport.**
  - Page → host: wry's `with_ipc_handler` (`window.ipc.postMessage`, `lib.rs:1150-1161`). On Windows the request URI is the sending frame's own URL, and that URL is the authorization input.
  - Host → page on Windows: WebView2 `PostWebMessageAsJson`, gated as described below. There is no `eval`, which honours "All JS↔Rust calls via a versioned typed command enum (typed IPC) — no raw eval" (`conductor/tech-stack.md:139`).
  - Host → page elsewhere: a fixed-template script whose only variable is a serde_json literal, the same approach as fixture_surface (`backends/fixture_surface.rs:510-526`). See OD-2.
- **The page API is host-served, not injected everywhere.**
  - An admitted bundle loads `/__fe/portal-v1.js` from its own origin. The protocol handler serves it only to admitted mounts. A static origin gets `404`, so `window.fe` never exists there.
  - wry still exposes `window.ipc` to every page, including arbitrary external sites. That is why authorization goes by sender, not by whether the API is present.
- **Authorization order.** Cheap checks come first, and nothing is decoded before checks 1–3 pass:
  1. The sender URL's origin is a currently **Admitted** mount.
  2. The portal is visible, and the **document generation** equals the one recorded at that mount's page load.
  3. The envelope is at most 64 KiB, `v == 1`, and the per-page token bucket allows it.
  4. Strict serde decode (`deny_unknown_fields`) succeeds.
  5. The manifest declares the requested capability.
  6. The broker or DB check at use passes, using the installing identity's role at write time.

  Failures at 1–2 are dropped with **no reply**, so an external page learns nothing, and they are counted. Failures at 3–6 answer a typed error.

  The document clock bumps only on navigation, history, reload, hide/close and unload, never on geometry. Do not reuse the fixture capture clock: it bumps on `reposition` (`backends/tauri.rs:289-290`) and would cut IPC on every resize.
- **Messages (`fe.portal.v1`, a serde-tagged enum: the "versioned typed command enum").**
  - Page → host:
    - `hello` → returns the granted capabilities;
    - `subscribe {stream: node | readings | query | <bus stream>, ...}` and `unsubscribe`;
    - `act {op: set_property, node_id, key, value}`;
    - `invoke {package, command, args}`: an integration-bus command;
    - `emit {stream, data}`: the portal as producer, declared streams only;
    - `intent {focus_node | select_node, node_id}`: local UI only, never a write.
  - Host → page:
    - replies: `{re, ok | error}`;
    - deliveries: `{sub, seq, kind: snapshot | delta | resync_required, data}`.
- **Delivery sources.** `node` and `readings` are push-only. `query` follows the sibling cadence (OD-3). The portal never polls REST on a timer.
  - **`node`:** the sibling's `host/node-changes`, which is "the scene change feed that the `/ws` subscription carries" (sibling draft R13). Underneath it are `entity_change_tx` / `transform_broadcast_tx` (`fe-api/src/server.rs:22,24`), with ws.rs's scope rules and lag handling → `resync_required` (`fe-api/AGENTS.md:15-30`).
  - **`query`:** the sibling's `host/query`, a single SELECT run through `query_guard` at a cadence of at least 1 s (sibling draft R13). The portal adds four rules:
    - only manifest-declared named queries, so consent shows the SQL;
    - a subscription pauses while the portal is hidden and ends at unload;
    - each re-run is charged, together with REST, to the identity's `QUERY_RATE_PER_SEC` budget of 10 (`fe-api/src/limits.rs:18`);
    - REST's caps apply: 10 000 rows and 8 MiB (`:4,6`).

    OD-3 asks the sibling for change-triggered re-runs, so an idle portal adds no reads.
  - **`readings`:** the sibling's `host/readings`, "rows accepted by the readings door, both local and replicated" (sibling draft R13). The tree has **no push source today**: `ApiState` carries only the two senders above, and `WsClientMsg` has no readings channel (`fe-api/src/ws.rs:24-53`). T2.3 adds one only if the sibling drops this stream (OD-13).
  - **Bus streams, commands and hooks:** `<package_id>/<stream>` and `<package_id>/<command>` (sibling draft R14). Portal IPC is "one more client of the same broker" (sibling draft R15) and calls commands with caller kind `portal`.
- **Actions are judged as the installing identity.** That is the local identity that admitted the bundle on this peer; each desktop peer has one identity.
  - Capabilities are the manifest's declarations intersected with that identity's role **at write time**, rechecked by the DB at use, exactly like the plugin bridge (`fractalengine/src/AGENTS.md:1706-1707`).
  - The host mints the binding per mount, following the `PluginAdmission` pattern (`fe-runtime/src/plugin_host.rs:6`). It is never taken from page data.
  - Replicas judge the resulting rows by the author's role when written (QA-0131).
  - Portal writes ride the sibling's extension write path. They therefore carry host-stamped provenance `{package_id, content_hash}` (sibling draft R24), and revocation stops the bundle after in-flight writes complete; the rows stay (OD-E1, user decision 2026-11-08 22:00; `docs/spec/portal-bundle/admission.md` §4.7, §5).
- **Rate limits (OD-8).**

  | Limit | Value |
  |---|---|
  | Messages per page | ≤ 50/s |
  | Live subscriptions | ≤ 16 |
  | Deliveries per subscription | coalesced to ≤ 4 Hz |
  | `set_property` | ≤ 2/s, burst 5 |
  | `invoke` | ≤ 5/s |

  The `set_property` budget is a quarter of the measured single-peer DB-thread ceiling of about 8 writes/s (commit `b66dfae7`; `conductor/tracks/project_completion_20261104/checkpoint.md:11`). A runaway bundle cannot starve the editor.
- **Audit.** Admission, revocation, and every admitted `act`/`invoke` are recorded through the sibling governance's canonical-log audit seam (A-5), attributed to `(package_id, content_hash)` plus the acting DID. There is no portal-private log.

### 6.5 Repository nodes and portal chrome (the UI-UX guidance applies)

- **Inspector "Repository" section.** Shown when the selected node has a bundle reference. It displays:
  - the bundle title;
  - the publisher (short DID plus trust-root name);
  - the version and a short content hash;
  - an admission badge;
  - the declared capabilities, as a plain list.

  It has one primary action, **Open**. Secondary actions appear only when allowed:
  - **Admit…** opens the sibling track's consent dialog; this track never builds a second one;
  - **Revoke on this peer**;
  - **Replace bundle** (Editor or above; it is a property write).

  There are no helper paragraphs. If a state needs explaining, the badge text carries it.
- **Portal chrome states are host-drawn** in the egui toolbar (`fe-ui/src/panels/portal_toolbar.rs:46-112`), outside the webview, so page content cannot spoof them. The in-page trust bar stays ("do not remove", `fe-webview/src/AGENTS.md:81-82`), but reads "Portal bundle" on bundle origins instead of "External Website".

| State | Rendered | IPC | Badge (accessible name) | Primary action |
|---|---|---|---|---|
| Admitted | yes | yes | "Admitted — <publisher>" | Close |
| Static sandbox | yes | no | "Static — not admitted on this peer" | Admit… (when allowed), else Close |
| Blocked | no | no | "Blocked — <reason>" | Close |
| Revoked | no (unloaded) | no | "Revoked — <reason>" | Close |
| Waiting for bytes | no | no | "Waiting for bundle bytes" | Close |
| Unavailable | no | no | "Embedded browsing unavailable" | Close |

- **Blocked reasons:** integrity failure, malformed manifest, too large, verse policy, unsupported platform. **Revoked reasons:** kill switch, revocation list, admission withdrawn.
- **Where states come from.** They derive from the sibling's installation status (`installed | active | disabled | degraded | revoked`, sibling draft R11), plus portal-only conditions: integrity, size, platform, and whether the bytes are present.
- **Layout does not change.** The native overlay keeps the measured remaining rect (`fe-ui/src/portal/mod.rs:37-47`). New chrome state rides an existing `SystemParam` bundle, because `gardener_ui_system` is already at Bevy's 16-parameter ceiling (`fe-ui/src/plugin.rs:1172-1174`).

### 6.6 Lifecycle

- **Install.** Upload (REST multipart, P0) stores one blob and an `asset` row. An Editor sets the repository-node reference, and both replicate. Bytes prefetch only for admitted rows whose author held Editor (QA-0133).
- **Open.** Only an explicit user action opens a bundle. The mount opens under the current admission state; arrival never opens anything.
- **Update** (OD-7, governance default):
  - a new content hash from the same publisher with no capability increase stays `Admitted` and keeps its origin;
  - any capability increase drops to `StaticSandbox` until re-consent;
  - a different publisher means a new origin.
- **Revoke mid-session.** Any sibling kill-switch layer triggers it: (a) local Disable, (b) `FE_EXT_DISABLE_ALL=1`, (c) removal from the verse policy, or (d) the S-PLUGIN signed revocation list (sibling draft R23). So does a role drop below verse policy. Within one frame of the decision:
  - the mount is removed, so the protocol answers `410`;
  - subscriptions close, and pending actions are refused;
  - the document is replaced through `load_html`;
  - the chrome shows Revoked;
  - on Windows, the origin's storage is cleared (SP-6). `clear_all_browsing_data` (`lib.rs:2195`) clears all origins, so it is not used.
- **Close.** Bundles unload on Close. External sites are only hidden today (`backends/tauri.rs:267-285`), but no bundle script may keep running in a hidden popup.
- **Offline.** A mounted bundle renders from the local blob with no network. Missing bytes show Waiting. The existing prefetch retry resolves it: 1–16 s, then re-arm at 15/30/60 s (`fe-sync/src/AGENTS.md:2232-2247`).
- **Size** (OD-4):
  - the default cap is 32 MiB per bundle, so the ladder's top tier (25 MB) fits;
  - over-cap uploads get `413`, and nothing is stored;
  - a larger bundle replicated from a peer with a higher cap shows `Blocked — too large`.

### 6.7 The dedicated QA lane (requirements here; design in plan P5)

- **Headless tier.** The real `portal_bundle` modules are driven with synthetic page requests, and the webview is the only faked edge. Covered: origin labels, navigation admission, CSP composition, protocol serving against sealed fixture archives, IPC authorization and rate limiting, and admission transitions.
- **Native tier.** Captured QA hosts with real WebView2, driven through new **fixed** operations in `fixture_surface`:
  - observe the title, a capped DOM digest and fixed probe elements;
  - activate numbered fixture slots;
  - read the IPC outcome counters.

  These are exposed as `/api/v1/debug/portal/*`, behind the same Owner/input gate and five-second host deadline as portal-fixture. `fe.py portal ...` drives them, and CapturePreview PNGs are the evidence. No caller supplies JavaScript, selectors or URLs.
- **Simulator.** A `portal` probe family in `tools/qa/sim`, with its own `portal_ms` cadence. It reads the local admission state, the fixed observation, and the IPC outcomes (`accepted`, `refused_capability`, `refused_role`, `rate_limited`, `dropped_origin`, `invalid`). "Probing does not perturb" (`tools/qa/AGENTS.md:1129`): no hierarchy polling, and only read-only fixed scripts.
- **Fixture bundles** are built from text sources: a docs site, a dashboard, actions, an escape bundle, a storm bundle, and a size ladder. They are sealed through the staged production `seal_hexon` with a run-local publisher, following the controlled-fixtures precedent (`tools/qa/AGENTS.md:876,888-893`). Unsigned twins exercise the static sandbox.
- **Journeys** PJ1–PJ9 (PJ10 with P6) are listed in plan §3.

### 6.8 Later: static export for the web (P6)

The same bundle is exported as a static directory plus `fe-portal.json` (API base URL, declared feeds).

- It reads data only through **signed share URLs**, the existing "signed scoped URL, NOT an embedded bearer JWT" (`fe-api/AGENTS.md:1013-1014`). These have a scope ceiling, a TTL of at most 24 h, public redemption, and a per-token rate limit. This realizes D-X7's "scoped viewer token" (OD-6).
- The operator lists the export origin in fe-api's `cors_origins` (`fe-api/src/server.rs:30-31`).
- Exports carry no IPC, no actions and no hooks.

**Non-goals:** hosting, CDN, custom domains, auth federation, accounts, payments, a hosted viewer.

## 7. Requirements and acceptance criteria

Every acceptance item is observable through a public seam: REST/MCP, `fe.py`, `BrowserEvent` messages, DB rows, native receipts or simulator expectations. Unhappy paths appear only where they are reachable, distinct and consequential, and validation variants are table-driven.

- **R1 — Portal manifest section and validator (P0).** `fe-format` parses `fe-portal-bundle/1` and refuses every class listed in §6.1.
  *Acceptance:* one table over fixture archives:
  - a valid archive parses;
  - each refusal class returns its typed error;
  - a remote `script` origin and a `__fe/` member are rows in that table.
- **R2 — Bundle ingest and directory mapping (P0).**
  - `POST /api/v1/petals/{p}/portal-bundles` (Editor+, multipart) stores one blob and one `asset` row (bundle MIME, `sealed_by`).
  - Bytes never cross the DB channel; the shape is `store_glb`/`register_asset`.
  - On a bundle, `GET /api/v1/assets/by-id/{id}` returns the file index instead of `501`.

  *Acceptance:*
  - a sealed fixture → `200 {asset_id, content_hash, sealed_by, title, entry}`;
  - the listing equals the archive's member set;
  - the raw blob by hash stays `attachment` + `nosniff`;
  - over-cap → `413`, and no blob is in the store;
  - a non-bundle archive → `422`, and nothing is stored.
- **R3 — Repository nodes (P0/P3).** `node_kind = repository` plus `portal.bundle_asset`, written through the existing property routes, plus a `NodeKind::Repository` variant.
  *Acceptance:*
  - an Editor's `PATCH /api/v1/nodes/{id}/properties` is readable on every peer;
  - `?kind=repository` lists the node;
  - a Viewer's PATCH gets the existing denial.
- **R4 — Bundles replicate without executing (P0, cross-track).** The scene-asset gates accept the bundle MIME. Instead of the glTF branch they run a bundle byte check: ZIP magic plus bounded sealed-header verification.
  *Acceptance:*
  - the extended fe-sync asset journey shows a bundle row arriving and its blob fetched within budget;
  - on the receiving peer, admission stays `StaticSandbox` and no portal opens.
- **R5 — `fe-portal` protocol serving (P1).** Implements the §6.2 contract.
  *Acceptance (headless):*
  - the entry returns `200` with the composed CSP;
  - a tampered member is refused;
  - traversal spellings (`..`, `%2e%2e`, backslash) return `404`;
  - `Service-Worker: script` is refused;
  - an unmounted origin returns `404`, and one unmounted mid-flight returns `410`.

  *Acceptance (native):* the dashboard's CSP canary (an inline script that should be blocked, reporting via `securitypolicyviolation`) shows as blocked in the fixed observation.
- **R6 — Per-package origin isolation (P1).** Uses the §6.2 labels.
  *Acceptance:*
  - a label table: the same package from another publisher gets a different label; the same publisher+package with a new hash keeps the same admitted label; static never equals admitted;
  - native: a `localStorage` key written by bundle A is absent in bundle B.
- **R7 — Navigation allowlist in both layers (P1).** Implements §6.3.
  *Acceptance:*
  - one decision table runs against both `decide_navigation` and `enforce`;
  - native escape journey: every forbidden class is refused, the native refusal counter rises by one per attempt, and the original page is unchanged;
  - `validate_url` refuses `fe-portal` and `*.localhost` spellings.
- **R8 — IPC ingress authorization (P2).** Implements the §6.4 authorization order.
  *Acceptance:*
  - a well-formed `subscribe` from an external site, a static bundle, a foreign frame, a hidden portal or a stale document gets no delivery and no reply, and `dropped_origin` rises by one;
  - the same message from an admitted, visible, current document gets a reply.
- **R9 — Subscriptions without polling (P2).**
  *Acceptance:*
  - an idle admitted bundle with `node` and `readings` subscriptions adds zero REST/DB reads over 60 s; a `query` subscription adds at most one guarded re-run per cadence period, and none while the portal is hidden;
  - a REST transform change appears in the bundle's fixed probe element within the replication budget;
  - 100 changes in 1 s produce at most 5 deliveries, and the last equals the final state;
  - subscribing to an out-of-scope node returns the same `not_found` as REST.
- **R10 — Actions judged as the installing identity (P2).**
  *Acceptance:*
  - a declared `set_property` on an Editor peer persists and replicates;
  - after the identity is demoted to Viewer, the same call is refused at write time, and no row exists;
  - an undeclared key is refused before the DB;
  - 20 calls in 1 s → at most 7 accepted, the rest `rate_limited`;
  - each accepted call has an audit record carrying `(package_id, content_hash)`.
- **R11 — Bus producer/consumer (P2; depends on the sibling contract).**
  *Acceptance:*
  - an extension subscribed to a bundle's declared stream receives its `emit`s;
  - an undeclared `emit` or `invoke` is refused;
  - a declared hook reaches the page.
- **R12 — Static sandbox (P4).** Renders with no bridge; data comes only from the public REST.
  *Acceptance:*
  - `window.fe` is absent (`/__fe/portal-v1.js` → `404`);
  - posts are dropped as in R8;
  - with Editor-attached share-URL feeds (OD-6), the static fixture dashboard renders values from `GET /api/v1/shared/{token}`;
  - without feeds, an API fetch is a CSP violation.
- **R13 — Admission states and chrome (P3/P4).** Implements the §6.5 table.
  *Acceptance:*
  - in each journey, `fe.py` ui-tree shows the badge's accessible name for the state reached;
  - Admit appears only when governance allows the local identity;
  - one CapturePreview is taken per state.
- **R14 — Revocation, update, close (P4).** Implements §6.6.
  *Acceptance:*
  - a kill switch mid-session produces: the Revoked badge, an unloaded document (the fixed observation is refused), zero live subscriptions, dropped stale-page posts, and protocol `410`; reopening still renders nothing;
  - a capability-increasing update → StaticSandbox until re-consent;
  - a same-capability update keeps the origin (A's storage key is still present).
- **R15 — Offline and size (P4/P5).**
  *Acceptance:*
  - with providers stopped, a local bundle renders;
  - a non-local bundle shows Waiting, then renders after providers return;
  - the ladder tiers (100 KB / 1 MB / 5 MB / 25 MB) render their entry within budget;
  - a 40 MB bundle → `413`.
- **R16 — QA lane (P5).** Implements §6.7.
  *Acceptance:*
  - `run.py validate portal-core portal-replication portal-load` passes;
  - PJ1–PJ9 each leave a receipt;
  - mutation check: with the R8 origin check removed, PJ2 goes red.
- **R17 — Static export (P6, later).** Implements §6.8.
  *Acceptance:*
  - the exported directory, served by any static server whose origin is in `cors_origins`, renders the fixture dashboard from a share URL;
  - an expired URL → `410` → a visible expired state;
  - the export contains no `/__fe/portal-v1.js`.

## 8. Feasibility with the current fe-webview backend

**Verdict:** **feasible on Windows/WebView2 with the pinned wry 0.54.4** (Cargo.lock; `fe-webview/Cargo.toml:23`), given the changes below. Five things are **not possible or not advisable** as originally framed, and the design works around each:

1. **Registering Tauri commands is not possible here.**
   - `resolve_asset` and `get_asset_base_url` need a `tauri::AppHandle` (`tauri_commands.rs:17,50`), which exists only inside a running `tauri::Builder` app.
   - That app would own the event loop Bevy/winit already own: "T1 — Bevy main (ECS scheduler + Tauri/wry portal event loop)" (`conductor/tech-stack.md:57`).
   - The real bridge is wry's `with_ipc_handler` plus the custom protocol. Recommendation: delete `tauri_commands.rs` (OD-9).
2. **Bundle origins fail today's policy by construction** (`portal_url.rs:37,46-49`; `tauri.rs:204-207`; `plugin.rs:244`). The only feasible route is a separate exact-origin admission, consulted in both guards. Loosening `is_url_allowed` is not advisable.
3. **Bundles cannot replicate until two glTF-only gates change** (`scene_asset_transfer.rs:72-77`, `scene_asset.rs:94-112`). The P2P campaign changed the transfer gate during authoring (`11b44d4f`) and is still active, so R4 sequences behind it.
4. **`readings` subscriptions have no push source** (`server.rs:22,24`; `ws.rs:24-53`). Polling REST instead is not advisable: QA-0125 found that "Every REST GET /api/v1/hierarchy also rebuilds the GUI tree" under polling (`qa/findings.jsonl:116`). The `readings` part of R9 therefore comes from the sibling's `host/readings` (OD-13).
5. **Per-origin storage clearing is outside wry's public API.**
   - `clear_all_browsing_data` clears all origins (`lib.rs:2195`).
   - The likely route is the WebView2 DevTools protocol (`Storage.clearDataForOrigin`) through `webview2-com`, which is already a dependency (`fe-webview/Cargo.toml:43`) (SP-6).
   - Without it, a revoked origin keeps its storage, but it is never mounted again until re-admitted.

**Spikes.** Run at gate-open, before P1 code merges. Each one either confirms the design or reopens an OD:

| Spike | Question |
|---|---|
| SP-1 | Does interception take precedence for `https://fe-portal.<label>.localhost`, so a miss never dials loopback? |
| SP-2 | Does wry's navigation callback see sub-frame navigations? |
| SP-3 | How is `PostWebMessageAsJson` ordered against NavigationStarting? |
| SP-4 | Do service-worker fetches reach the handler (`ICoreWebView2_22` source kinds, `webview2/mod.rs:938-946`)? |
| SP-5 | How does a `load_html` unload present to the navigation guard? |
| SP-6 | Can storage be cleared for a single origin? |
| SP-7 | Does the non-Windows origin form `fe-portal://<label>` (`lib.rs:636`) pass the admission function (OD-10)? |

## 9. Open decisions

Each entry gives the recommended default first and the reversal cost last.

- **OD-1 — Origin host form.**
  - *Recommended:* `with_https_scheme(true)` + `<label>.localhost`, following Tauri's convention. A missed interception can then only reach loopback over TLS, where the name check fails, and nothing goes to DNS.
  - This needs one clarifying line beside `tech-stack.md:141`: "raw URLs: localhost blocked unconditionally; in-process bundle origins admitted only by exact mount match".
  - *Alternative:* `.invalid`, which means a resolver lookup on a miss.
  - *Reversal:* medium. A change of origin orphans per-origin storage.
- **OD-2 — Host→page transport.**
  - *Recommended:* on Windows, `PostWebMessageAsJson`, gated by the document clock plus an envelope-origin check in the page API; elsewhere, a fixed-template `evaluate_script`.
  - *Alternative:* an async custom-protocol long-poll on `/__fe/v1/stream`.
  - *Reversal:* low; the page API hides it.
- **OD-3 — Query subscription cadence.**
  - *Recommended:* consume the sibling's `host/query` unchanged. Portals may only subscribe to manifest-declared named queries, and they pause while hidden. Ask the sibling, as a bus `question`, to skip a re-run when no change touched the query's scope, so an idle portal adds no reads.
  - *Alternative:* a portal-side analytics path re-evaluated on SceneChange. That would be a second query path, so it is rejected unless the sibling declines.
  - *Reversal:* low.
- **OD-4 — Bundle size cap.**
  - *Recommended:* 32 MiB, configurable per node (`FE_PORTAL_BUNDLE_MAX_BYTES`).
  - *Reversal:* low.
- **OD-5 — Bundle MIME.**
  - *Recommended:* a dedicated `application/vnd.fractalengine.portal-bundle+hexon`, sharing the listing branch with the generic `application/x-fe-directory` sentinel.
  - *Reversal:* medium; replicated rows carry the MIME.
- **OD-6 — Static-sandbox data path and the "scoped viewer token".**
  - *Recommended:* Editor-attached signed share URLs (`portal.feeds`), fetched from the public share route, with fe-api CORS accepting static bundle origins on `GET /api/v1/shared/{token}` only.
  - *Alternatives:* a host-served same-origin feed proxy, or no data path for static bundles in wave 1.
  - *Reversal:* low.
- **OD-7 — Update policy.**
  - *Recommended:* the same publisher with no capability increase stays Admitted; otherwise StaticSandbox until re-consent. The sibling governance can override this.
  - *Reversal:* low.
- **OD-8 — Budgets.**
  - *Recommended:* the numbers in §6.4.
  - *Reversal:* low.
- **OD-9 — `tauri_commands.rs` and the `tauri` crate.**
  - *Recommended:* delete the file in P1, and decide on dropping the `tauri` dependency at the root lock gate.
  - *Reversal:* low.
- **OD-10 — Platforms.**
  - *Recommended:* Windows/WebView2 is the wave-1 acceptance platform. Elsewhere, bundles show `Blocked — unsupported platform` until SP-7 passes.
  - *Reversal:* low.
- **OD-11 — Product/tech-stack wording.**
  - *Recommended:* the orchestrator adds one clarifying line each to product.md (`:59`, `:170-171`) and tech-stack.md (`:139-144`), user-ratified.
  - *Reversal:* low.
- **OD-12 — Repository-node encoding.**
  - *Recommended:* `node_kind` plus `portal.*` properties.
  - *Alternative:* dedicated columns.
  - *Reversal:* medium.
- **OD-13 — Readings push source.**
  - *Recommended (now matches sibling draft R13):* the bus's `host/readings`.
  - *Fallback:* only if the sibling drops that stream does this track add `readings_ingested_tx` beside `entity_change_tx` (T2.3), as a sequenced shared write.
  - *Reversal:* low.

## 10. Risks

- **One shared WebView2 profile.** External sites and bundles share one webview, so `window.ipc` exists on every page. Sender-origin authorization is the only line of defence, which is why R8 carries a mutation check.
- **Silent capability creep through updates.** A same-origin update that adds commands would inherit trust. OD-7 forces re-consent.
- **Two origins per package history** (static versus admitted). This is deliberate, to prevent origin poisoning, but it means storage does not carry over at admission. Authors must not rely on storage written before admission.
- **DB-thread starvation.** Portal writes share the single-peer ceiling of about 8 writes/s. The budgets keep portal writes at a quarter of it or less, and PJ7 measures it.
- **Cross-track collisions.** fe-api `server.rs`, fe-ui `actions/mod.rs`, `tools/qa/sim/*` and `tools/qa/fe.py` are central files, and the scene-asset gate files are in flight now. Every touch is a sequenced shared write (see the metadata partitions).
- **Contract drift.** `fe.portal.v1` and the manifest section are public contracts that bundle authors implement. They are versioned and never changed in place.
- **Unaudited surface.** A web runtime holding a data bridge is new attack surface. `/security-review` closes P1, P2 and P4 (plan §4).

## 11. Dependencies

- **`extension_platform_20261107`** (public sibling; its draft was present but uncommitted at authoring). It provides:
  - the integration bus, WIT 2.0 and capability manifest v2;
  - governance: the admission store, consent UI, per-verse policy, kill switch and canonical-log audit;
  - the `fe-ext` tools.

  P2 is blocked on its bus-contract milestone, and P4 on its governance. P0, P1, P3 and P5 do not wait.
- **`plugin_host_revival_20261031`:** S-PLUGIN publisher signatures, trust roots and the signed revocation list, consumed through the governance layer.
- **Related, with no functional dependency:**
  - `hexon_entitlements_20261031` (format-spec bump sequencing);
  - `role_propagation_20261013` (write-time role);
  - `p2p_asset_streaming_20260718` (scene-asset transfer);
  - `agent_input_surface_20261031` (QA input gate);
  - `auth_policy_pattern_20260710` (policy engine).

## 12. Evidence corrections

These inputs differ from the tree at `30163ca2`:

1. **There is no crate-root `fe-webview/AGENTS.md`.** The crate doc is `fe-webview/src/AGENTS.md`, which is what `fe-webview/tests/tauri_cutover_test.rs:185-192` asserts.
2. **`fe-ui/src/portal.rs` does not exist.** The module is `fe-ui/src/portal/mod.rs`. The live portal code is in `fe-ui/src/actions/portal.rs` and `fe-ui/src/panels/portal_toolbar.rs`.
3. **`fixture_surface` and `fixture_stream` are not in-process fake backends.** They are fixed-script and bounded-capture surfaces inside the real `TauriBackend`, compiled only on Windows (`backends/mod.rs:9-15`). The headless seam is the pure decision functions plus `StubBackend`.
4. **`application/x-fe-directory` is not a working multi-file container.** It is a placeholder that answers `501`.
5. **`petal_portal`'s `Selected` / `ModelUrlMeta` activation has no producer in the app.** fe-ui never inserts `Selected`, and only `fractalengine/src/main.rs:975` registers the plugin. Repository nodes therefore attach to fe-ui's `UiAction` path.
6. **`current_url` / `load_url` are `TauriBackend` details** (`backends/tauri.rs:25,216`), not members of the `WebViewBackend` trait.
