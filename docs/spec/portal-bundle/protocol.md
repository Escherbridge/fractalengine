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
  2. A binary-injected resolver (the `HostEnv` injection idiom) reads the node's `portal.bundle_asset`, the `asset` row, the local blob path and the admission view (`admission.md` §2; Static until S4 lands). Missing bytes resolve to Waiting; a kill-listed or revoked pair resolves to Revoked and stops here (`admission.md` §4).
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
