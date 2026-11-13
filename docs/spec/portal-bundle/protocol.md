# `fe-portal` protocol, origins, CSP and navigation

**Status:** draft, author-only, 2026-11-08. Task T1.1 of `portal_surface_20261107`; built by S1, spiked by SP (which appends a "Spike results" section at the end of this file and nothing else). Container rules are in [README.md](README.md).
**Pinned engine:** wry 0.54.4 (`fe-webview/Cargo.toml`, Cargo.lock), WebView2 on Windows. wry citations are to `wry-0.54.4/src/` in the cargo registry.

The design has one idea: **a bundle origin is admitted only by exact match against the live mount table, in both navigation layers, and the raw URL policy never learns that bundle origins exist.** `fe_runtime::portal_url::is_url_allowed` stays byte-for-byte unchanged and keeps refusing every bundle spelling, because the scheme is `fe-portal` or the last host label is `localhost` (`fe-runtime/src/portal_url.rs:37-49`).

## 1. Origins

1.1. **Labels.** b32 is RFC 4648 base32 with the lowercase alphabet `a-z2-7` and no padding; 26 characters carry 130 bits.
  - **Admitted:** `a-` + b32(BLAKE3(utf8(`publisher_did`) ‖ `0x00` ‖ utf8(`package_id`)))[..26]. One publisher's updates keep one origin, and with it their storage. Two publishers never share an origin, even for the same `package_id`.
  - **Static:** `s-` + b32(the 32 raw bytes of `content_hash`)[..26]. Every build gets its own static origin.
1.2. **Why two kinds.** A static rendering never shares an origin, and therefore never storage, cookies or a service worker, with an admitted one. Content a bundle planted while unadmitted cannot survive into the admitted state (*origin poisoning*). The `a-`/`s-` prefixes make the two sets disjoint by construction. The cost: storage does not carry over at admission (spec §10).
1.3. **Host form.** The host is `<label>.localhost` (OD-1). `*.localhost` never leaves the machine; a missed interception can only reach loopback over TLS, where the name check fails.
1.4. **Spellings.** wry rewrites custom-protocol URLs on Windows, so one origin has two spellings, and the code sees both:

| Where the URL appears | Spelling |
|---|---|
| Windows: the document URL, the IPC sender URL (`webview2/mod.rs:898-912`), the navigation callback URL | `https://fe-portal.<label>.localhost/<path>` |
| Windows: the protocol handler's request URI (wry reverts its workaround before calling the handler, `webview2/mod.rs:1093-1100`, `custom_protocol_workaround.rs:30-35`) | `fe-portal://<label>.localhost/<path>` |
| Elsewhere (SP-7, OD-10) | `fe-portal://<label>.localhost/<path>` |

1.5. **One parser.** `portal_bundle::origin::parse(raw) -> Option<Label>` accepts exactly those two spellings and returns the label only when it matches `(a|s)-[a-z2-7]{26}`. It refuses userinfo, any explicit port, a trailing dot, extra host labels and any other scheme. Every consumer — the protocol handler, both navigation layers, the IPC gate and the trust bar's host check — goes through it; no consumer compares strings on its own.
1.6. **The reserved prefix.** wry registers the WebView2 filter `https://fe-portal.*` (`custom_protocol_workaround.rs:41-43`, `webview2/mod.rs:933-950`) and routes every matching request to the handler, which answers `404` unless the host is a mounted label. A real website whose host begins `fe-portal.` is therefore unreachable inside the portal, and a request for one never touches the network (SP-1).
1.7. **The host loads the https spelling.** `WebView::load_url` does not apply wry's rewrite (`webview2/mod.rs:1391-1394`); only the builder's initial URL is rewritten (`:518-524`). On Windows the host always loads `https://fe-portal.<label>.localhost/<entry path without web/>`.

## 2. Mount table

2.1. **Entry.** `{label, mount_id, mode: Admitted | Static, node_id, asset_id, package_id, content_hash, publisher_did, archive, portal, csp, feeds, document}` where:
  - `mount_id` is a process-wide counter that never repeats;
  - `archive` is a `HexonArchiveIndex` opened under `RequireSealed { pin: Some(publisher_did) }` for Admitted, or `VerifyIfSealed` for Static (`fe-format/src/integrity.rs:71-77`). A tampered sealed bundle is refused in both modes, because `VerifyIfSealed` still enforces a seal that is present;
  - `portal` is the validated section; `csp` is the header value, composed once at mount (§5);
  - `feeds` is the static mount's feed set (`admission.md` §7), empty for Admitted;
  - `document` is `{generation, token}` (`ipc-v1.md` §1.3).
2.2. **Ownership.** The table is shared state (`Arc<RwLock<…>>`). The protocol handler only reads it, off the main thread. Only the Bevy main thread writes it: open, close, leave and revoke.
2.3. **One mount.** One webview means one live mount (A-7). Opening a bundle while another is mounted closes the first (§2.5) before mounting the second.
2.4. **Open.**
  1. fe-ui writes `BrowserCommand::OpenBundle { node_id }` (§8). The command carries no URL, path or hash. Only the typed path mints bundle URLs; `validate_url` (`fe-ui/src/actions/portal.rs`) and `BrowserCommand::Navigate` keep refusing every bundle spelling.
  2. A binary-injected resolver (the `HostEnv` injection idiom) reads the node's `portal.bundle_asset`, the `asset` row, the local blob path and the admission view (`admission.md` §2; Static until S4 lands). Missing bytes resolve to Waiting; a kill-listed or revoked pair resolves to Revoked and stops here (`admission.md` §4), unless this peer's verse Owner has recorded the audited kill-list override (`admission.md` §4.7). Revocation of a mounted bundle unloads it after in-flight writes complete; rows it wrote stay (OD-E1).
  3. The archive opens under §2.1's policy and the validator runs (README §6). A refusal resolves to Blocked with its chrome reason.
  4. The mount is inserted, the CSP composed and a one-shot **entry token** minted for exactly the entry URL (§6.7).
  5. The host loads the entry URL (§1.7) and shows the portal.
  6. The host emits `BrowserEvent::BundleState` with the resulting state.
2.5. **Close.** `CloseBundle`, `Close` while a bundle is mounted, or a host hide:
  1. remove the mount first, so in-flight protocol requests answer `410`;
  2. bump the document generation (IPC goes dark, `ipc-v1.md` §1.3);
  3. mint the one-shot **unload token** and call `load_html` with a fixed empty document (`wry lib.rs:2190`), so no bundle script keeps running in a hidden popup — today an external site is only hidden (`backends/tauri.rs:267-285`);
  4. hide the popup and emit `BundleState { state: Closed }`.
2.6. **Leave.** A top-frame navigation to a declared `navigation` origin (§6 row 4) unmounts the bundle exactly like Close except that it does not hide: the page is now an external website, IPC is detached, and re-entering the bundle takes a new **Open**.
2.7. **Revoke** follows the same order inside one frame; see `admission.md` §5.

## 3. Request → response

3.1. The handler is registered once at `TauriBackend::create` with `with_asynchronous_custom_protocol("fe-portal", …)` (`wry lib.rs:1120-1147`) and `WebViewBuilderExtWindows::with_https_scheme(true)` (`lib.rs:1763,1822`). A protocol cannot be added after the webview exists, which is why requests resolve against the shared mount table. `RequestAsyncResponder` is `Send` (`lib.rs:442-449`): reads and digest checks run on a worker thread and never block the Bevy main thread.
3.2. Each request runs these steps in order; the first that answers wins.
  1. **Method.** Not `GET` or `HEAD` → `405`.
  2. **Service workers.** A `Service-Worker: script` request header → `403` (SP-4).
  3. **Origin.** `origin::parse` the request URI (§1.5). No label, or a label not mounted when the request arrives → `404`.
  4. **Snapshot** the entry's `mount_id`.
  5. **Path.** Take the URI path, ignore the query, and never percent-decode. A path containing `%` or `\`, or one that breaks README §3.1 after the leading `/`, → `404`. Member names are unreserved characters only, so a browser never escapes them, and any `%` is an attack or a typo.
  6. **Reserved.** A path under `/__fe/` → §3.4.
  7. **Member.** Map per README §3.4. Absent → `404`.
  8. **Read** with `HexonArchiveIndex::read_member` (`fe-format/src/archive_index.rs:326-360`). An integrity error, or `ArchiveChanged`, → `500`, and the mount transitions to `Blocked — integrity failure` (§2.5 order). A tampered member is never served, not even partly.
  9. **Still mounted?** If the entry with the snapshot `mount_id` is gone → `410`.
  10. **Range.** One `bytes=` range → `206` with `Content-Range`; an unsatisfiable range → `416`; several ranges → `200` with the full body. Media seeking needs this.
  11. **Respond** with §4's headers. `HEAD` sends no body.
3.3. **Status table.** Every error body is empty.

| Status | When |
|---|---|
| 200 / 206 | served |
| 403 | service-worker script request |
| 404 | unknown or unmounted label, bad path, absent member, a reserved path the mode does not serve |
| 405 | any method but GET and HEAD |
| 410 | the mount went away while the request was in flight |
| 416 | unsatisfiable range |
| 500 | integrity failure; the bundle is now Blocked |

3.4. **Host-served paths.**

| Path | Admitted mount | Static mount |
|---|---|---|
| `/__fe/portal-v1.js` | `200`: the page API (`ipc-v1.md` §4) carrying this document generation's token | `404`, so `window.fe` never exists |
| `/__fe/feeds.json` | `404` | `200`: the feed set (`admission.md` §7.3) |
| anything else under `/__fe/` | `404` | `404` |

## 4. Headers

Every response, errors included, carries exactly these host-authored headers, plus `Content-Type` (README §4), `Content-Length` and, for `206`, `Content-Range`:

| Header | Value |
|---|---|
| `Content-Security-Policy` | the mount's composed policy (§5) |
| `X-Content-Type-Options` | `nosniff` |
| `Cross-Origin-Opener-Policy` | `same-origin` |
| `Cross-Origin-Resource-Policy` | `same-origin` |
| `Referrer-Policy` | `no-referrer` |
| `Permissions-Policy` | `accelerometer=(), camera=(), clipboard-read=(), clipboard-write=(), display-capture=(), geolocation=(), gyroscope=(), hid=(), magnetometer=(), microphone=(), midi=(), payment=(), publickey-credentials-get=(), serial=(), usb=(), xr-spatial-tracking=()` |
| `Cache-Control` | `no-store` |

No response ever carries `Access-Control-Allow-Origin`, so no other origin can read a bundle member, and `Cross-Origin-Resource-Policy` stops other origins from embedding one with a no-cors `<script>` or `<img>`. This satisfies tech-stack's "Enforce Content Security Policy headers" (`conductor/tech-stack.md:142`).

## 5. CSP composition

5.1. **Template.** Bracketed parts appear only when the manifest declares them (README §5):

```
default-src 'none';
script-src 'self'[ 'wasm-unsafe-eval'];
style-src 'self'[ 'unsafe-inline'];
img-src 'self' data: blob:[ <csp.img>…];
font-src 'self'[ <csp.font>…];
media-src 'self'[ <csp.media>…];
connect-src 'self'[ <csp.connect>…][ <feed origins>…];
frame-src <csp.frame>… | 'none';
worker-src 'self';
form-action 'none';
base-uri 'none';
object-src 'none';
frame-ancestors 'none'
```

5.2. **Typed input only.** The composer takes the validated manifest type, never strings, so no source expression outside the template can appear. `'unsafe-eval'` and `'unsafe-inline'` for scripts never appear.
5.3. **Feed origins** are added only on Static mounts that have feeds (`admission.md` §7.4). An Admitted mount never gets them: its data path is IPC.
5.4. **Deviations from spec §6.2** (recorded as contract changes):
  - `worker-src 'self'`, without `blob:`. A worker built from a Blob runs code the publisher signature does not cover, which is `eval` by another name.
  - No `upgrade-insecure-requests`. Every non-`'self'` source is `https` by construction, so it has nothing legitimate to upgrade, and it would rewrite a static feed's loopback share-route origin (`http://127.0.0.1:8765` by default, `fractalengine/src/main.rs:711`) to `https` and break it (SP-8).
5.5. A `<meta>` CSP in a bundle page can only narrow this policy.
5.6. **Residual.** With `csp.wasm`, a page can compile any bytes it can read, including bytes from its `connect` origins. "No remote code" therefore holds for JavaScript only; consent lists `wasm` and `connect` together, and authors SHOULD ship WebAssembly as members.

## 6. Navigation decision table

6.1. **One function.** `portal_bundle::navigation::admits(attempt, context) -> Decision`, pure.
  - `attempt`: `{raw_url, is_redirect, is_main_frame}`.
  - `context`: the mount table, the current top-frame document (a mounted label, or external, or none), the pending entry token and the pending unload token.
  - `Decision`: `AllowSame`, `AllowEnter`, `AllowLeave`, `AllowHostUnload`, `DeferToRawPolicy` or `Refuse(reason)`.
6.2. **Both layers call it first.** `decide_navigation` (the production wry callback, `backends/tauri.rs:36-70`) and `qa_navigation::enforce` (the native owner on captured QA hosts, `backends/qa_navigation.rs:62-110`) consult `admits` before anything else. `DeferToRawPolicy` falls through to their existing logic unchanged, so external-site behaviour does not move. A `Refuse` raises the native refusal counter (`clock.refuse`) on captured hosts and pushes `BackendEvent::Error` everywhere. No QA private-origin exception is introduced (`fe-webview/src/AGENTS.md:141`).
6.3. **Rows.** One table drives S1's test against both layers (R7). "Bundle origin" means any URL `origin::parse` accepts.

| # | Current top-frame document | Destination | Decision |
|---|---|---|---|
| 1 | bundle A | same origin A, any path, including reload, back and forward within A | `AllowSame` |
| 2 | none or external | bundle origin A, URL equal to the pending entry token's URL | `AllowEnter`; the token is consumed |
| 3 | anything | bundle origin, with no matching entry token (page-initiated entry, history back into A after leaving, a second use of a token) | `Refuse(enter_without_open)` |
| 4 | bundle A | `https` origin in A's `navigation` that passes `is_url_allowed` | `AllowLeave`; the bundle unmounts (§2.6) |
| 5 | bundle A | `https` origin not in A's `navigation` | `Refuse(undeclared)` |
| 6 | bundle A | bundle origin B ≠ A, mounted or not | `Refuse(other_bundle)` |
| 7 | bundle A | `file:`, `data:`, `javascript:`, `blob:`, `about:` or any non-`https` scheme | `Refuse(scheme)`, except row 9 |
| 8 | bundle A | loopback, private, link-local or `*.localhost` host | `Refuse(blocked_host)` |
| 9 | bundle A or none | the unload document's URL (SP-5), with a pending unload token | `AllowHostUnload`; the token is consumed |
| 10 | anything | a redirect whose destination is a bundle origin | `Refuse(redirect_into_bundle)`; the protocol never redirects, so this is always an attack |
| 11 | bundle A | a redirect from a declared origin to an undeclared one | `Refuse(undeclared)`; rows 4–5 apply to every hop |
| 12 | external | non-bundle destination | `DeferToRawPolicy` (today's behaviour) |
| 13 | bundle A | a declared origin that fails `is_url_allowed` (validation prevents it; defence in depth) | `Refuse(blocked_host)` |

6.4. **New windows.** `window.open` and `target=_blank` stay swallowed: wry marks every unhandled `NewWindowRequested` as handled, and captured hosts deny explicitly (`backends/tauri.rs:114-116`).
6.5. **Downloads.** S1 registers a download handler that refuses any download started from a bundle origin (a navigation to a `bin` member, an `<a download>`). Without it, WebView2's default would write the file into the user's Downloads folder.
6.6. **Sub-frames.** CSP `frame-src` is authoritative (default `'none'`). If SP-2 shows that the callback also sees frame navigations, a frame attempt gets the frame rule instead of rows 1–13: a declared `csp.frame` origin passes the raw policy; a bundle origin and everything else is refused.
6.7. **Host tokens.** The entry and unload tokens are host state, minted by Open and by Close/revoke, consumed by the first matching attempt, and voided by the next Open or Close. A URL can never carry or forge one, and neither is a scheme exception (plan review R-P1: "the unload token is one-shot").

## 7. Trust bar and chrome

- `TRUST_BAR_JS` stays on every page (`fe-webview/src/plugin.rs:11-16`; "do not remove", `fe-webview/src/AGENTS.md:82`). On a bundle origin, checked with the same label pattern as §1.5, its text reads `Portal bundle` instead of `External Website: <hostname>`.
- The in-page bar is page content, and page script can remove it. The host-drawn badge in the egui toolbar is the only trust indicator (`admission.md` §2, `ux.md` §3). Admission state is never derived from page content.
- For a bundle, the toolbar's hostname line shows the label, and "Open externally" is hidden: a bundle origin is not an external URL.

## 8. Commands and events (draft shape)

S3 codes against this shape and S1 may rename it; a rename is recorded in `fe-webview/src/portal_bundle/AGENTS.md`.

```rust
// fe-webview/src/ipc.rs
BrowserCommand::OpenBundle { node_id: String }
BrowserCommand::CloseBundle
BrowserEvent::BundleState { node_id: String, state: BundleState }

pub enum BundleState {
    Admitted { publisher: String },
    StaticSandbox { can_admit: bool },
    Blocked(BlockedReason),   // Integrity | MalformedManifest | TooLarge | VersePolicy | UnsupportedPlatform
    Revoked(RevokedReason),   // KillSwitch | RevocationList | AdmissionWithdrawn
    WaitingForBytes,
    Unavailable,
    Closed,
}
```

The variants and their reason suffixes are the chrome table of `admission.md` §2 and `ux.md` §3; the accessible names live in `ux.md` §5.

## 9. Fixed QA operations

The QA lane's native tier drives bundles only through fixed, host-owned operations, exposed as `/api/v1/debug/portal/{observe,action,ipc-stats}` behind the portal-fixture Owner/input gate and five-second host deadline (`fe-api/AGENTS.md` §QA Portal evidence), and implemented in `backends/fixture_surface.rs` (S5).

| Operation | Caller input | Result |
|---|---|---|
| observe | none | `{label, mode, title, digest, probes: {name: text}}` — `digest` is BLAKE3 over the text of `data-fe-probe` elements only, capped at 64 KiB; each probe value capped at 256 chars |
| action | a slot number, 1–32 | activates the element marked `data-fe-slot="<n>"` through a fixed script; `{activated: bool}` |
| ipc-stats | none | the host's IPC outcome counters (`ipc-v1.md` §7.3); host state, never page state |

- **No caller supplies JavaScript, selectors or URLs.** Inputs are numbers and nothing else. This is the fixture-surface doctrine (`fe-webview/src/AGENTS.md:161`) applied to bundles.
- Each fixed script embeds the mounted label and re-checks `location.origin` inside the document before it reads anything. Output is size-capped and strictly deserialized before it is exposed.
- The probe and slot attribute conventions belong to the QA lane (`tools/qa/portal/AGENTS.md` §fixture-bundles).

## 10. Spikes

Run on real WebView2 at gate-open, before P1 code merges. Each spike passes or reopens the named decision; SP appends the results below this file's last section.

| Spike | Question | Pass when | If it fails |
|---|---|---|---|
| SP-1 | Does interception win for `https://fe-portal.<label>.localhost`, so a miss never dials loopback? | With the handler answering `404` for an unmounted label, a navigation and a `fetch` to that URL, and to `https://fe-portal.example.com`, produce no DNS query and no connection to 127.0.0.1:443 (a loopback listener on 443 sees nothing) | OD-1 reopens (`.invalid` host form) |
| SP-2 | Does wry's navigation callback see sub-frame navigations? | Either answer is recorded; it selects whether §6.6's frame rule is wired into the callback. CSP `frame-src` blocks an undeclared frame in both cases | §6.6 wiring changes only |
| SP-3 | How is `PostWebMessageAsJson` ordered against NavigationStarting? | A page at origin B (declared `navigation`) listening on `chrome.webview` receives zero of 1 000 messages posted to bundle A while A navigates to B | OD-2 reopens: deliveries move to an async `/__fe/v1/stream` long-poll on the protocol, which is origin-bound by construction |
| SP-4 | Do service-worker requests reach the handler (`ICoreWebView2_22` source kinds, `webview2/mod.rs:938-946`)? | `navigator.serviceWorker.register('/sw.js')` reaches the handler with `Service-Worker: script`, the `403` fails registration, and no worker controls the page afterwards | §5 drops dedicated workers too (`worker-src 'none'`) until another route exists |
| SP-5 | How does a `load_html` unload present to the navigation guard? | The exact URL the guard sees is recorded; the unload token admits that one attempt; a page-initiated navigation to the same URL afterwards is refused | §6 row 9 adapts to the recorded URL |
| SP-6 | Can storage be cleared for one origin? | DevTools `Storage.clearDataForOrigin` through `ICoreWebView2::CallDevToolsProtocolMethod` (`webview2-com`, already a dependency) clears bundle A's `localStorage` and IndexedDB while bundle B's key survives | the residual in `admission.md` §9 stands |
| SP-7 | Does the non-Windows form `fe-portal://<label>.localhost` (`lib.rs:636`) pass `origin::parse`, the navigation callback and the IPC sender URL? | All three see the §1.4 spelling | OD-10 stands: "Blocked — unsupported platform" off Windows |
| SP-8 | Can a static mount read the local share route? | `fetch` from a static mount to `http://127.0.0.1:<port>/api/v1/shared/<token>` succeeds under `admission.md` §8's CORS rule with no mixed-content block; whether a private-network preflight or permission request fires is recorded and answered; an admitted mount and an external site cannot read the response | OD-6 falls back to its alternative: a host-served same-origin feed proxy, `/__fe/feeds/<name>` |

SP-8 is new in this contract (the static-sandbox data path depends on it).

## 11. Test tables this contract seeds (S1)

- **Labels (R6):** same package, two publishers → two admitted labels; one publisher and package, two hashes → one admitted label; a static label never equals an admitted one; `origin::parse` over both spellings plus the refused variants (userinfo, port, trailing dot, extra label, uppercase-only variants that normalize, a wrong prefix, 25 or 27 characters).
- **Requests (R5):** entry `200` with the composed CSP; tampered member `500` then Blocked; `..`, `%2e%2e`, `%2E%2E`, `.%2e`, `\` and `%5c` → `404`; `Service-Worker: script` → `403`; `POST` → `405`; unmounted label → `404`; unmounted mid-flight → `410`; `/__fe/portal-v1.js` on a static mount → `404`.
- **Navigation (R7):** §6.3 rows 1–13, run against both `decide_navigation` and `qa_navigation::enforce`, extending `backends::tauri::tests::decide_navigation_blocks_localhost_and_emits_error`; `tests/security_test.rs::test_localhost_blocked` gains every bundle spelling as a refused raw URL.

## 12. Spike results

**Run status, 2026-11-08: not run.** Two independent blocks apply:
- the swarm runs code-first, with no builds or runs until the refine pass;
- the build gate reads `cargo_allowed: false` (closed 15:46 for memory).

This section is therefore a **desk check** against the pinned sources: wry 0.54.4, webview2-com and webview2-com-sys 0.38.2, and http 1.4.0, with this machine's WebView2 Runtime at 157.0.4341.42. A desk verdict reports only what the source decides, never runtime behaviour. An OD reopens here only when the source alone settles it, and none did. Until the run fills §12.6, every pending decision stays on its recommended default. Paths below are relative to `wry-0.54.4/src/` unless they name another crate or `fe-webview/src/`.

12.1. **Summary.**

| Spike | Desk verdict | Decision now |
|---|---|---|
| SP-1 | Routing confirmed; whether the network stays silent is pending | OD-1 stands |
| SP-2 | **No** on Windows, from source; the run confirms | §6.6's frame rule stays out of the callback; CSP `frame-src` is the frame control |
| SP-3 | Pending | OD-2 stands |
| SP-4 | Plumbing confirmed; a runtime without `ICoreWebView2_22` escapes interception | P-1 |
| SP-5 | Pending; two candidate URLs named | Row 9 waits for the recorded URL; P-4 |
| SP-6 | API path confirmed; the effect is pending | `admission.md` §9's residual stands; the clear runs after the unload commits |
| SP-7 | **Fails** on Linux, from source; macOS cannot run on this machine | OD-10 stands (its default) |
| SP-8 | Pending | OD-6 stands |
| SP-9 (new) | Crash path confirmed in source | P-2 |

12.2. **Evidence.**
- **SP-1.**
  - With `with_https_scheme(true)`, wry registers the filter `https://fe-portal.*` for every resource context. When the `ICoreWebView2_22` cast succeeds, the filter also covers every request source kind (`webview2/mod.rs:933-950`).
  - wry matches a request to the handler on the URI prefix alone (`custom_protocol_workaround.rs:12-19`). So `https://fe-portal.example.com/` reaches the handler as `fe-portal://example.com/`, and `origin::parse` answers `404` (§1.6).
  - The filter is https-only, so `http://fe-portal.<label>.localhost` is never intercepted.
    - A bundle cannot fetch it, because CSP `'self'` does not match the http spelling.
    - The raw policy refuses it as a top-frame destination.
    - An external page's subresource fetch to it still reaches loopback port 80, like any `*.localhost` URL today. This is not specific to bundles; the run records it.
  - Whether Chromium resolves or preconnects a host before `WebResourceRequested` fires is runtime behaviour.
- **SP-2.**
  - The production callback is wry's `ICoreWebView2::add_NavigationStarting` (`webview2/mod.rs:674-694`), and the captured-host owner registers the same event (`fe-webview/src/backends/qa_navigation.rs:154`). WebView2 raises `NavigationStarting` for the top frame only.
  - Child frames raise `FrameNavigationStarting`. Neither layer registers it, although the binding exists (`webview2-com-sys-0.38.2/src/bindings.rs:1195`).
  - A native `FrameNavigationStarting` guard is optional defence in depth, not a requirement.
  - Off Windows, wry's callback is expected to see every frame, because neither WebKit decision is filtered to the main frame (`wkwebview/navigation.rs:50-80`, `webkitgtk/mod.rs:548-575`). That matters only once OD-10 lifts.
- **SP-3.**
  - `ICoreWebView2::PostWebMessageAsJson` is bound (`bindings.rs:1402`) and reachable through `WebViewExtWindows::webview()`, as `backends/fixture_surface.rs:388` already does.
  - Delivery order against a cross-origin commit is runtime behaviour. The send-time gate and per-document token of `ipc-v1.md` §1.2 ship either way.
- **SP-4.**
  - With `ICoreWebView2_22`, the filter covers service-worker and shared-worker sources (`webview2/mod.rs:941-946`).
  - wry copies every request header into the handler's request (`:1058-1070`). Any `Service-Worker` header that WebView2 exposes therefore reaches §3.2 step 2.
  - Without `ICoreWebView2_22`, wry silently falls back to the document-only filter (`:947-949`).
    - A worker script fetch is then not intercepted. It goes to `fe-portal.<label>.localhost:443`, which is loopback.
    - Registration still fails, but only after dialling loopback, which §1.3 says a bundle never does.
- **SP-5.**
  - `load_html` is `NavigateToString` (`webview2/mod.rs:1400-1403`), so both layers see the unload as a top-frame `NavigationStarting`.
  - The URI it reports is runtime behaviour. The two candidates are `about:blank` and a `data:text/html…` URI.
  - Today's guards refuse both. `security::embedded_allowed` admits only http(s) (`fe-runtime/src/portal_url.rs:36-38,70-74`), and `qa_web::allows` admits only a fixture permit's exact URLs. Without row 9, the unload is cancelled and the revoked document keeps running.
- **SP-6.**
  - `CallDevToolsProtocolMethod` is bound (`bindings.rs:1455`), with `webview2_com::CallDevToolsProtocolMethodCompletedHandler` (`webview2-com-0.38.2/src/callback.rs:314`).
  - The call is `Storage.clearDataForOrigin` with `{"origin":"https://fe-portal.<label>.localhost","storageTypes":"all"}`. It uses the https spelling (§1.4), because the browser keys storage on the document's origin, never on `fe-portal://`.
  - The parameters are built from a parsed label only; no caller supplies text.
  - Ordering matters. If the clear starts before the unload document commits, the dying document can write again in between, so the clear starts on the unload's `NavigationCompleted`.
- **SP-7.**
  - **Linux:** the IPC request URI is `webview.uri()` (`webkitgtk/mod.rs:644-647`). That is the top document when the message is handled, not the document that sent it, which breaks the premise of `ipc-v1.md` §1.1. Lifting OD-10 on Linux needs a wry change that reports the sending frame.
  - **macOS:** the sender is the frame's request URL (`wkwebview/class/wry_web_view_delegate.rs:50-53`). It is origin-correct but carries no main-frame flag.
  - Both navigation callbacks receive the URL as `fe-portal://<label>.localhost/…`, without rewriting.
  - This machine has no macOS or Linux host.
- **SP-8.**
  - The pinned SDK names no local-network permission kind (`bindings.rs:572-596`).
  - wry handles `PermissionRequested` only for clipboard reads, and only when that feature is enabled (`webview2/mod.rs:497-513`).
  - Any local-network request would therefore arrive unnamed, and the runtime default would decide it.
  - Chromium has been moving from private-network preflights to a local-network permission prompt. Which of the two runtime 157 applies is what the run records.
- **SP-9 (new: IPC sender-URL abort).**
  - wry builds each IPC request with `Request::builder().uri(url).body(js).unwrap()` (`webview2/mod.rs:912`). `http` 1.4.0 refuses any URI longer than 65 534 bytes (`http-1.4.0/src/uri/mod.rs:144,295`).
  - wry reads the sender only when an IPC handler is registered (`:894-896`), and fe-webview registers none today.
  - Once S2 registers `with_ipc_handler`, any top-level page can trigger the crash. That includes an external site in the portal and a bundle. The page calls `history.replaceState` to push its own URL past that length, then calls `window.ipc.postMessage`; wry defines `window.ipc` on every document.
  - The unwrap then panics inside a COM callback, and the process aborts. In release, the abort comes from `panic = "abort"` (`Cargo.toml:112`). In debug, the unwind reaches the `extern "system"` callback boundary, which also aborts.
  - The run confirms that the replaced URL reaches `Source`.

12.3. **Findings for S1 and S2.** These are not spikes; they surfaced during the desk check and bind the code.
- **F-1 — headers are silently dropped.**
  - wry forwards only header values that pass `HeaderValue::to_str()` and drops the rest without an error (`webview2/mod.rs:1117-1124`). A CSP value with one non-ASCII byte would be served with no CSP at all.
  - S1 composes ASCII only; the validator already holds IDN origins in punycode. S1 builds every value with `HeaderValue::from_str` and treats a failure as Blocked, never as "serve without".
  - R5 gains a row: every header on every handler response round-trips through `to_str()`.
- **F-2 — wry can answer `400` itself.**
  - When `prepare_request` fails, wry answers `400` without calling the handler (`webview2/mod.rs:983-990`, `:1136-1145`). One example is a request URI that `http` refuses.
  - That response carries none of §4's headers and has an empty body, so nothing runs. It is an accepted residual. §3.3's table describes the handler's responses, not wry's.
- **F-3 — the revert replaces every match.**
  - `revert_uri_work_around` uses `str::replace` (`custom_protocol_workaround.rs:30-35`), which rewrites every `https://fe-portal.` in the URI, including the query.
  - This is benign. A rewritten path gains a `:`, which README §3.1 refuses, and the query is ignored.
  - `origin::parse` and §3.2 step 5 take the URI exactly as the handler receives it.
- **F-4 — the navigation callback receives only a URL.**
  - wry's callback is `Fn(String) -> bool` (`lib.rs:1281`), so production `decide_navigation` has no `is_redirect`. The captured-host owner reads `IsRedirected` natively (`backends/qa_navigation.rs:37`).
  - Without that flag, row 10 collapses into row 3: the same refusal, with a different reason.
  - Row 11 holds only if a leaving context outlives the redirect chain. → P-3.
- **F-5 — responses complete on the UI thread.**
  - Worker-thread responses reach WebView2 through `PostMessageW` to the UI thread (`webview2/mod.rs:1014-1017`), so they complete only while the Bevy main thread pumps window messages.
  - The handler answers on every path. A dropped responder never completes the request's deferral.

12.4. **Proposed contract changes.** All are pending the run except P-2, which the source already decides.
- **P-1 (§3.1; `ux.md` chrome).**
  - Bundles require the `ICoreWebView2_22` cast at `TauriBackend::create`. Without it, every bundle shows `Blocked — unsupported platform`.
  - Reason: SP-4's fallback path dials loopback.
- **P-2 (`ipc-v1.md` §1.1; plan T2.2).**
  - Page → host uses a native `ICoreWebView2::add_WebMessageReceived` handler, registered the way `qa_navigation.rs:139-185` registers its navigation guard, and never wry's `with_ipc_handler`.
  - The handler reads `Source` capped at 8 192 bytes (the `qa_navigation.rs:41` bound) and the body capped at 65 536 bytes (`ipc-v1.md` §2 check 3). Anything over either cap is dropped as `dropped_origin` before any parse.
  - Reason: SP-9.
- **P-3 (§6.2).**
  - Production registers the same native `NavigationStarting` owner that captured hosts use, in place of `with_navigation_handler`. Both layers then see `IsRedirected`, and rows 10–11 hold as written.
  - Reason: F-4.
- **P-4 (§2.5 step 3; `admission.md` §5 step 4).**
  - If the unload navigation is refused or never completes, the host takes the `qa_navigation::disable` path (Stop, then controller Close) and recreates the webview. A revoked bundle never keeps running.
  - If SP-5 shows the `NavigateToString` URI varies from call to call, the unload uses `load_url("about:blank")` instead (`webview2/mod.rs:1391-1394`), whose URI is fixed.

12.5. **Run procedure.** Run once, at gate-open, while the machine is quiet.
- **Setup.**
  - Use a throwaway worktree with one example binary, `fe-webview/examples/portal_spikes.rs`. It is never merged.
  - The binary opens a winit window holding a wry webview built with `with_https_scheme(true)` and an asynchronous `fe-portal` handler. The handler serves fixed in-memory pages for labels A, B and S (static), and answers `404` for an unmounted label C.
  - Native hooks log JSONL observations for `NavigationStarting` (with `IsRedirected`), `FrameNavigationStarting`, `WebResourceRequested` (URI, headers, source kind), `WebMessageReceived`, `PermissionRequested` and `ServerCertificateErrorDetected`.
  - The first log line is the runtime version and the result of the `ICoreWebView2_22` cast.
- **Steps.**

| Spike | Steps | Pass |
|---|---|---|
| SP-1 | Start listeners on `127.0.0.1:443` and `[::1]:443` and a `pktmon` capture on port 53. Navigate to and `fetch` C's URL and `https://fe-portal.example.com/`. | Zero accepts and zero DNS packets for both names. |
| SP-2 | A declares one frame origin and embeds it beside an undeclared one. | Record which callbacks fire; the undeclared frame is blocked by CSP. |
| SP-3 | B, an external page that A declares in `navigation`, logs every `chrome.webview` message. Post 1 000 envelopes to A, then navigate A to B. Repeat with the posts issued inside the `NavigationStarting` callback. | B logs zero messages. |
| SP-4 | A registers `/sw.js`; reload A. Repeat with the filter forced to the pre-`_22` call. | The handler saw `Service-Worker: script`, the registration was rejected, and `navigator.serviceWorker.controller` is null after the reload. |
| SP-5 | While on A, the host calls `load_html` with the fixed document. Then a page-initiated navigation to the recorded URI. Then `history.back()` into A. | Record the URI; the second and third attempts are refused, and nothing restores from the back-forward cache (`Cache-Control: no-store`). |
| SP-6 | A and B each write a `localStorage` key and an IndexedDB database. After A's unload commits, clear A's origin. Reopen both. | A's data is gone; B's is intact. |
| SP-7 | On a macOS host and a Linux host, repeat SP-2 and an IPC post from A and from a frame. | Record the navigation-callback and sender URLs. |
| SP-8 | S's feed is a share URL on the local fe-api. Repeat the fetch from A and from an external page. | S reads the body with no mixed-content error; record any preflight and any permission kind. A and the external page cannot read it. |
| SP-9 | With `with_ipc_handler` registered, a page replaces its URL with a 70 000-byte one and posts. Repeat with P-2's native handler. | The first run aborts the process; the second survives and drops the message. |

12.6. **Results ledger.** The run fills one row per spike, plus the WebView2 runtime version it used.

| Spike | Runtime | Observed | Verdict | Date |
|---|---|---|---|---|
| SP-1 | | not run | pending | |
| SP-2 | | not run | desk: no (Windows) | |
| SP-3 | | not run | pending | |
| SP-4 | | not run | pending | |
| SP-5 | | not run | pending | |
| SP-6 | | not run | pending | |
| SP-7 | | not run | desk: fails on Linux | |
| SP-8 | | not run | pending | |
| SP-9 | | not run | desk: crash path confirmed | |
