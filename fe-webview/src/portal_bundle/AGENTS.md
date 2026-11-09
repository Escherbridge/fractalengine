# fe-webview/src/portal_bundle — module notes

Signed web bundles rendered in the Petal Portal (track `portal_surface_20261107`, T1.3/T1.4).
The contracts are `docs/spec/portal-bundle/{README,protocol,admission,ipc-v1}.md`; this file
records why the code looks the way it does and where it departs from those drafts. Source files
carry one-line doc comments that point here.

Ownership: S1 wrote `origin`, `mount`, `protocol`, `csp`, `navigation`. S2 adds `ipc.rs`,
`limits.rs` and `portal-v1.js` (and serves `/__fe/portal-v1.js`); S4 adds `admission.rs`,
`feeds.rs`, the revoke listener and `/__fe/feeds.json`.

## §origins — origin.rs

- **Two label families, disjoint by prefix.** `a-` = b32(BLAKE3(publisher ‖ 0 ‖ package))[..26]
  survives updates (storage follows the publisher's package); `s-` = b32(content_hash)[..26] is one
  origin per build. A static rendering can never plant storage an admitted one later reads
  (origin poisoning, protocol.md §1.2).
- **One parser.** `origin::split`/`parse` accept exactly `https://fe-portal.<label>.localhost…`
  (Windows document, navigation and IPC-sender spelling) and `fe-portal://<label>.localhost…`
  (protocol-handler spelling). It is byte-exact and lowercase-only: userinfo, ports, trailing dots,
  extra labels and uppercase all fail, because the authority must equal `<label>.localhost`.
  Every consumer goes through it; nobody compares origin strings by hand.
- **Storage keys on the https spelling** (`Label::https_origin`), never on `fe-portal://`.
- The raw policy (`fe_runtime::portal_url::is_url_allowed`) is untouched and still refuses every
  bundle spelling; `tests/security_test.rs::test_localhost_blocked` pins that.

## §mounts — mount.rs

- **One live mount** (`Table::live: Option<Arc<Mount>>`, A-7). Opening another bundle closes
  the first.
- **Readers snapshot, the main thread writes.** Protocol workers clone the `Arc<Mount>` under a
  read lock and release it before any I/O; install, unmount, leave and revoke happen only on the
  Bevy main thread (navigation callbacks run there too). Never hold the lock across a webview call.
- **Document generation lives in the table**, bumped by every admitted top-frame navigation and
  every unmount. It is not the fixture capture clock, which also bumps on `reposition` and
  would cut IPC on every resize (ipc-v1.md §1.4). S2 mints the per-generation document token.
- **Faults.** A member that fails its seal check flips the mount's `faulted` flag (so it serves
  nothing more, immediately) and queues its id; `BundleHost::poll` turns that into
  Blocked — integrity failure on the next frame. Workers never restructure the table.
- **Open runs off the main thread.** `BundleResolver::resolve` may block (the binary reads the
  node's `portal.bundle_asset` and the asset row through the DB channel), so `BundleHost::open`
  spawns one short-lived `fe-portal-open` thread. A request counter voids stale outcomes, so only
  the latest Open ever mounts.
- **Order inside `prepare`** follows admission.md §2: resolver-reported revocation (kill switch,
  hash-matched kill list) → bytes present → size cap → header/seal verification → validator →
  identity (`content_hash`) → governance verdict → label, CSP, entry URL.
  - Residual: a revocation that governance can only match by `(package_id, content_hash)` is
    decided after the header phase and the validator, not before. Nothing is rendered or served
    either way.
- **Admitted needs a seal by the admitting publisher.** The archive opens under
  `VerifyIfSealed`, which enforces any seal that is present; requiring
  `IntegrityStatus::Sealed { publisher_did }` to equal the verdict's publisher is the same
  guarantee as `RequireSealed { pin }` without opening twice. An unsealed build is always Static.
- **Validator sizes come from the central directory** (`zip::by_index_raw`), never from inflating
  members; actual reads stay under fe-format's per-entry caps.
- **Declared origins use the raw policy** (`is_url_allowed`), never the QA-widened
  `embedded_allowed`. fe-api's ingest uses `embedded_allowed`; R-P1 should reconcile the two.
- `FE_PORTAL_BUNDLE_MAX_BYTES` is read here and in fe-api under the same name (fe-webview cannot
  depend on fe-api).

## §lifecycle — BundleHost (mount.rs)

- **Close** (protocol.md §2.5): unmount (bumps the generation) → unload token →
  `load_url("about:blank")` → hide → `BundleState::Closed`. Bundles never keep running hidden;
  external sites are still only hidden.
- **Leave**: the gate unmounts on `AllowLeave` and queues a notice; `poll` reports Closed. The
  page is now an external website.
- **Host Navigate while mounted**: unmount without unloading, mint a navigation token for the
  normalized URL, and let that load replace the document.
  - Residual: if that load fails, the old document stays detached: it has no mount, no IPC, and
    cannot navigate itself.
- **Backends without bundles** keep `BundleWebview`'s defaults, and `WebViewBackendRes::bundles`
  is `None`, so every Open reports Unavailable. Non-Windows and pre-`ICoreWebView2_22` runtimes
  report Blocked — unsupported platform (OD-10, spike P-1).

## §protocol — protocol.rs

- Registered once in `TauriBackend::create` with `with_asynchronous_custom_protocol` and
  `with_https_scheme(true)`. A protocol cannot be added after the webview exists, which is why it
  resolves against the shared table.
- **Pool**: two `fe-portal-protocol-N` threads behind an mpsc queue. They exit when the webview
  (and with it the closure owning the sender) drops. With no thread alive, jobs run inline, so
  every request is answered (desk check F-5: an unanswered responder never completes).
- **Never percent-decode.** Any `%` or `\` is a 404; the README §3.1 member grammar
  (`fe_format::portal_bundle::is_member_path`) is the traversal defence. The URI is taken exactly
  as wry hands it over (F-3).
- **Any `Service-Worker` header is refused** (403), not only `script`: fail closed.
- 404 = no such label/member or a faulted mount on arrival; 410 = the mount went away (or
  faulted) while the request was in flight; 500 = integrity failure. Error bodies are empty and
  carry the mount's CSP, or `csp::REFUSAL_POLICY` when no mount matched.
- **Headers are pre-validated.** The CSP becomes an `http::HeaderValue` at mount time (a failure
  is Blocked — malformed manifest), and every other value is a constant. This is because wry
  silently drops a non-ASCII header value (F-1); the journey asserts `to_str()` on every response.
- **Range**: one `bytes=` range → 206; several or malformed → 200 full; out of bounds → 416.
- `/__fe/…` is the extension point for S2 (`portal-v1.js`, Admitted only) and S4 (`feeds.json`,
  Static only); S1 answers 404 for all of it.

## §csp — csp.rs

- **Typed input only** (`PortalCsp` plus mode plus feed sources). Each source is re-checked to be
  one lowercase source token (host, then an optional `[a-z0-9/_-]` path), so `;`, spaces and
  non-ASCII can never add a directive.
- Feed sources count only on Static mounts; an Admitted mount's data path is IPC.
- **Feeds are path-scoped** (R-P4 fix, `review-p4.md` P4-02): a kept feed adds
  `<origin>/api/v1/shared/` to `connect-src`, not the bare origin. CSP path matching then keeps
  an unsigned static page off every other route of this peer's loopback fe-api (simple
  cross-origin requests need no preflight, so CORS alone would not stop the send). This narrows
  admission.md §7.4's "exact origins".
- These deviate from spec §6.2, as protocol.md §5.4 records: `worker-src 'self'` (no `blob:`) and
  no `upgrade-insecure-requests`.

## §navigation — navigation.rs

- **One pure function** (`admits`), consulted first by both layers: the production wry callback
  (`backends/tauri.rs::decide_navigation`) and the captured-host native owner
  (`backends/qa_navigation.rs::enforce`). `DeferToRawPolicy` falls through to their unchanged
  logic, so external-site behaviour does not move.
- **`NavigationGate`** holds what `admits` reads (current top-frame document, one host token) and
  applies each decision's effects: it consumes the token, sets the current document, bumps the
  generation, and unmounts on leave.
- **Current states**: None, External, Bundle(label), Leaving(declared list), and Detached (a
  bundle document whose mount the host removed; it may only be replaced by a host-minted load).
  Detached is not in protocol.md §6; it closes the window between an unmount and the host's own
  replacement load.
- **One token slot**: Enter(url), Unload, Navigate(url). Each new mint voids the previous one, so
  a URL can never carry or forge a token.
- **Redirects.** Only the native owner sees `IsRedirected`. On the production callback row 10
  collapses into row 3 (same refusal, different reason), and a leave context ends at the next
  attempt (F-4). Spike P-3 would move production onto the native owner.
- **Frames.** WebView2 raises `NavigationStarting` for the top frame only (SP-2 desk check), so
  the frame rule is reachable only off Windows; CSP `frame-src` is the frame control.
- **Downloads** started from a bundle origin (`blob:` included), or while a bundle document is
  loaded, are refused (§6.5).

## §chrome

- `TRUST_BAR_JS` (`plugin.rs`) reads "Portal bundle" on a bundle origin, using the same label
  pattern as `origin::parse`. It is page content; the host-drawn toolbar badge is the only trust
  indicator.
- `BrowserEvent::BundleState { node_id, label, state }`: `label` is an addition to protocol.md
  §8, so the toolbar's hostname line can show the origin label. Reason suffixes come from
  `BlockedReason::suffix` / `RevokedReason::suffix` (admission.md §2 wording).

## §decisions-taken (recorded as contract changes)

- Unload uses `load_url("about:blank")` instead of `load_html`. It is P-4's alternative, taken
  ahead of SP-5 because its URI is fixed and the unload token matches one exact string.
- P-1 is adopted: bundles require the `ICoreWebView2_22` cast; anything else is Blocked —
  unsupported platform.
- The `Detached` state and the host `Navigate` token (see §navigation) are additions.
- `BundleState` events carry `label` (see §chrome).

## §residuals and deferred

- The mid-flight 410 row has no headless test: it needs a pause seam between read and recheck.
- SP-1..SP-9 have not been run (code-first swarm). P-2 (native `WebMessageReceived`) belongs to
  S2; P-3 is deferred.
- Per-origin storage clear (SP-6), the revoke listener and `/__fe/feeds.json` belong to S4.
- "Waiting → bytes arrive → mount" needs a re-Open from the chrome (S3) or S4's listener.
- The binary must insert `plugin::PortalBundleResolver`. Without it, every Open is Unavailable.
- **P-4's fallback is not built** (`review-p1.md` P1-05): if the unload navigation is refused or
  never commits (a page-initiated `about:blank` racing the host's can consume the one-shot
  token), the unmounted document keeps running hidden. It has no mount, so no bytes, no IPC and
  no storage clear; Stop + controller Close + recreate is the cure once SP-5 records the URI.
- **SP-3** decides whether an envelope posted just before a leave can reach the declared page.
  The send-time gate and the page API's token check ship either way (`review-p2.md` P2-03).
