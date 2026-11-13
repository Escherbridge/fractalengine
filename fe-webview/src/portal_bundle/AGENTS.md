# fe-webview/src/portal_bundle — module notes

Signed web bundles rendered in the Petal Portal (track `portal_surface_20261107`, T1.3/T1.4).
The contracts are `docs/spec/portal-bundle/{README,protocol,admission,ipc-v1}.md`; this file
records why the code looks the way it does and where it departs from those drafts. Source files
carry one-line doc comments that point here.

Ownership: S1 wrote `origin`, `mount`, `protocol`, `csp`, `navigation`. S2 added `ipc.rs`,
`limits.rs` and `portal-v1.js` (serving `/__fe/portal-v1.js`); S4 added `admission.rs`,
`feeds.rs`, the revoke listener and `/__fe/feeds.json`. Sections §admission, §feeds and §ipc
below are the rationale for those files; §reviews-and-open lists the gate verdicts.

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
- **Targets** (`BundleTarget`, turn 2 P-WV for B10f). An Open names a repository node
  (`Node`, the original path) or a scope's bound bundle (`Bound { petal_id, package_id }`,
  bindings.md §6 `portal.open`). `BundleHost::open_target` and `prepare_target` take either;
  `open`/`prepare`/`refresh` keep their node signatures. `BundleResolver::resolve_bound` resolves
  the installed build the nearest-scope binding selects on this peer (Unavailable by default), and
  `prepare_target` refuses bytes of any other package (Blocked — integrity). Only a local Open
  calls it, so a replicated binding row never mounts anything (D-X5).
  - `Mount::node_id` is the target's **anchor**: the node id, or `binding:<petal_id>:<package_id>`,
    which no page can name (`ipc::is_node_id` refuses `:`). Chrome events carry it.
  - **`BrowserCommand::OpenBoundBundle { petal_id, package_id }`** (P-BOUND) is the by-name Open of a bound
    target: no URL, path or hash. `BrowserCommand::bundle_anchor` gives the chrome anchor both Open commands'
    events carry. It is written only behind fe-ui's `UiAction::OpenBundle` path, never by a replicated row or the
    petal lifecycle (R-POR-10); `dispatch_commands` mounts it with `BundleHost::open_target(BundleTarget::Bound ..)`.
  - A bound target reads no feeds and reports `repository_node_id: null` in `hello`; the broker
    derives the petal from `BindRequest::target`. One origin per (publisher, package), so a bundle
    bound at two scopes shares storage, like any admitted update.
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
  hash-matched kill list) → bytes present → size cap → archive digest against the list →
  header/seal verification → validator → identity (`content_hash`, member digests) → governance
  verdict → label, CSP, entry URL.
  - Residual: a list entry naming the `content_hash` or a member digest is decided after the
    header phase and the validator, not before. Nothing is rendered or served either way.
- **Member digests** are kill-list keys (admission.md §4.2). Sealed: the signed `members.json`
  values, which `read_member` enforces on every served byte. Unsealed: hashed here, one member at
  a time under the binary per-entry cap. They catch a killed bundle repackaged with the same files.
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
- `/__fe/…` answers 404 in `protocol::serve`, so no member ever shadows a host path. The
  wrappers answer their own path on that 404: `ipc::serve` (`portal-v1.js`, Admitted only) and
  `feeds::serve` (`feeds.json`, Static only), which is the handler the backend registers.

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
- SP-1..SP-9 have not been run (code-first swarm). P-2 (native `WebMessageReceived`) is built
  (§ipc "Native ingress"); P-3 is deferred.
- Per-origin storage clear (SP-6), the revoke listener and `/__fe/feeds.json` landed with S4 (`fc29e431`). The clear is unrun until SP-6 is spiked.
- "Waiting → bytes arrive → mount" needs a re-Open from the chrome (S3) or S4's listener.
- The binary inserts `plugin::PortalBundleResolver` and `PortalBrokerRes` (`fractalengine/src/portal_host`, turn 2).
  Without the resolver every Open is Unavailable; without the broker every call but `hello` is `unavailable`.
- **Shared-origin token race (SP-3, SP-5).** A protocol request carries no document identity. While a same-origin
  navigation is in flight, the outgoing document can fetch `portal-v1.js` and receive the incoming generation's token.
  This includes a reopen of the same `a-` label after a refresh. Both documents are builds of one (publisher, package).
  The worst case is a build revoked moments earlier, still alive until the unload commits, while the next build of
  the same package is already mounted. The spikes decide whether a per-document key is needed.
- **P-4's fallback is not built** (`review-p1.md` P1-05): if the unload navigation is refused or
  never commits (a page-initiated `about:blank` racing the host's can consume the one-shot
  token), the unmounted document keeps running hidden. It has no mount, so no bytes, no IPC and
  no storage clear; Stop + controller Close + recreate is the cure once SP-5 records the URI.
- **SP-3** decides whether an envelope posted just before a leave can reach the declared page.
  The send-time gate and the page API's token check ship either way (`review-p2.md` P2-03).

## §admission — admission.rs (admission.md §2-§6)

- **Verdict ladder.** `decide` returns the first match, in this order:
  1. host-wide disable, or this installation locally disabled, gives `Revoked(KillSwitch)`;
  2. kill-listed under any id, or governance's record is `revoked`, gives `Revoked(RevocationList)`. An audited verse-Owner
     override for this build in this verse spares the listing and a list-caused (`revocation-list`) record, never another cause;
  3. record `disabled` or `policy_denied` gives `Revoked(AdmissionWithdrawn)`;
  4. verse-policy ceiling gives `Blocked(VersePolicy)`;
  5. an `Admitted` record for the exact `(package_id, content_hash)` pair, sealed by the consented publisher and carrying the build's capability digest, gives `Admitted`;
  6. otherwise `Unadmitted { can_admit }`, where `can_admit` requires a seal. An unsealed build is never admitted (D-X5).
- **Kill list matches by digest, never by id.** `GovernanceView::kill_listed(&[digests])` mirrors the sibling's
  `RevocationSet::is_revoked_any` and takes no package id. The portal passes the `content_hash`, the archive digest and every
  member digest (`BundleIdentity::digests`); an entry naming any of them matches. The entry's `package_id` is informational and
  never narrows the match (orchestrator steer 2026-11-08 20:45). The residual is genuine repackaging: new bytes in every member
  under a new signature are caught only by publisher-key revocation.
- **Override is per build and verse, never per digest** (governance.md §kill-list-override: key `(verse_id, content_hash)` on
  this host). `GovernanceView::kill_list_override(target, content_hash)`; the target names the verse. This closes P4-05 (an
  override on one digest clearing a listing on another). `revocation_for(.., overridden)` lets a list event pass a build the
  Owner overrode here, per governance.md "Arrival" (closes P4-04); Disable all and withdrawals still unload it. Overrides are
  local and never withdrawn (E10 default), so a lagging view can only miss a fresh one, which fails closed.
  - Residual: the pre-byte check keys the override on the blob hash, which is the `content_hash` only when unsealed. A sealed
    build whose *archive digest* is listed is refused before bytes even under an override. Lists key on `content_hash`
    (store-client v1), so this bites only a list naming a sealed archive.
- **Re-consent is digest-bound.** A record admits exactly one capability digest. The digest covers CSP, navigation,
  subscriptions, commands, hooks, streams and intents, so a capability increase cannot slip through as a same-id update.
- **Open question (OD-7, not settled in code).** Records are keyed on the exact pair, so a same-publisher update with no
  capability increase has a new `content_hash` and no record yet. It stays `Unadmitted` until governance writes one. Governance
  decides whether that is the intended re-consent path.
- **Signals.** A `host/admission` delta (`{package_id, content_hash, state, reason}`, A-16) and a kill-list acceptance reach
  `plugin::apply_portal_admission` in the frame they arrive. `Rejudge` then does one of three things: unload now (`Unload`),
  promote a static mount to admitted on a fresh local Admit (`Promote`; a `restored` record never remounts), or refresh the
  chrome only (`Refresh`).
- **Layers.** Host-wide disable unloads any mount, static included. Kill-list matching by hash applies at open (archive digest
  before any byte is parsed) and in `revocation_for` for live mounts.

P4-04 and P4-05 (review-p4.md) are closed by the per-build override above (turn 2, P-WV).

## §feeds — feeds.rs (admission.md §7)

- **Static mounts only.** Feeds (`properties.portal.feeds`, a name to share URL map) reach a page only through
  `/__fe/feeds.json` on a Static mount. An Admitted mount reads data through IPC, never through feeds.
- **Exact URLs.** A feed is kept only when it is exactly `<origin>/api/v1/shared/<token>` on this peer's loopback fe-api
  (`http://<loopback IP>:<port>`, nothing after it) or on an https origin the raw policy allows. Names match
  `[a-z][a-z0-9_]{0,47}` (README §9.3). Tokens and names have length bounds; over-long sources are left out rather than
  blocking the whole mount.
- **Path-scoped CSP (R-P4 fix P4-02).** A kept feed adds `<origin>/api/v1/shared/` to `connect-src`, never the bare origin.
  Simple cross-origin requests need no preflight, so CORS alone would not stop an unsigned static page from sending to every
  other route of this peer's loopback API. This narrows admission.md §7.4's "exact origins".
- **Syntax only.** The share-token shape (`<payload>.<signature>`, base64url) is checked here. The `exp` value is shown as a
  display hint, and the share route decides validity.
- **No date or base64 crate.** Base64url (RFC 4648 §5) and the UNIX-to-RFC 3339 conversion are small local helpers, so
  fe-webview gains no dependency for them.

## §ipc — ipc.rs and limits.rs (ipc-v1.md; OD-8)

**Ingress order.** `BridgeState::ingress` runs checks 1-3 on raw strings. Serde runs only later, in `process_inbox`, with
`deny_unknown_fields`:

1. **Sender.** `Source` is read only within the 8 192-byte cap (P-2), then parsed by `origin::parse`. The mount must be live,
   Admitted and unfaulted.
2. **Current.** The portal is visible, and the body starts with this document generation's token (compared in constant time).
3. **Budget.** The envelope `id` is digits, the body is within size, the per-mount message bucket has a token, and the
   pending-reply cap is not reached.

Senders that fail checks 1-2 get no reply. They are dropped and counted, so an external site or a stale document learns nothing.

**Native ingress (SP-9, P-2).** wry's `with_ipc_handler` is never registered: wry builds its request with
`Request::builder().uri(source)...unwrap()`, and a `Source` over 65 534 bytes panics inside a COM callback, which aborts the
app. `backends/tauri.rs::install_ipc_handler` adds its own `ICoreWebView2::add_WebMessageReceived` handler (wry's handler
stays inert without an IPC closure) and calls `PortalBridge::receive`:

- `Source` and the body are copied by `take_bounded`, which scans for the terminator only up to the cap (8 192 and 65 536
  UTF-16 units; a unit is at least one UTF-8 byte) and frees the CoTaskMem buffer on every path. Longer, non-UTF-16 or
  unreadable parts become `None` and are never converted. A non-string `postMessage` fails `TryGetWebMessageAsString`.
- **Sender first.** `receive` runs check 1 on the mount table alone and only then calls the body reader, so no unauthorised
  page's body is ever read. Neither the reader nor any webview call runs under the bridge lock.
- A `None` part is dropped as `dropped_origin` without a reply, like a foreign sender. The page API rejects an over-cap
  envelope itself before posting. Check 3 still answers `invalid` for a body whose UTF-8 bytes exceed the cap.
- **No panic paths.** Nothing on the path unwraps or indexes page data: the prefix and id slices end on verified ASCII
  bytes, locks recover from poisoning, and `origin::split` slices at found char boundaries. `panic = "abort"` makes any panic
  here fatal, so keep it that way.
- Lock rule the handler relies on: no bridge-locked or mount-locked section calls into WebView2. A re-entrant event would
  otherwise find the std `Mutex` held by its own thread.

**Role ceiling** (steer 2026-11-08 23:40). `BindRequest::viewer_ceiling` carries the viewer token's `max_role` (`Owner` for the
in-app local viewer, set with `PortalBridge::set_viewer_ceiling`). The broker opens the bus client at the installing identity's
role at the target's scope, capped there, and reports it as `BrokerClient::role_ceiling`; P-BROKER passes it to
`Bus::open_portal_client`.

- A reported ceiling above the viewer cap is a broker fault: the client is closed unused, and every call answers
  `unavailable` (fail closed).
- A changed viewer ceiling rebinds on the next pump. Live streams get `ended` with `reason: "role-changed"`, a waiting
  subscribe answers `unavailable`, and a waiting call answers `unknown_outcome`.
- `act` below Editor answers `role_denied` before the write path, audited through `audit_refusal`. The DB still judges every
  write at use.

**Document tokens.** A token is 128 bits of BLAKE3 keyed by a per-process secret, minted once per (mount, generation).
`portal-v1.js` is served only for Admitted mounts, with `Cache-Control: no-store` and CORP same-origin, so other origins
cannot read it. The generation lives in the mount table (§mounts). It is not the fixture capture clock, which also bumps on
resize; a bump there would cut IPC on every reposition.

**Host-minted binding.** The broker binding is built from the `Mount` alone. Capabilities are the manifest's declared set
intersected with what the broker grants. Page data never names the node, petal, package or identity.

- `BindRequest` carries every kill-list key of the build (`content_hash`, `archive_digest`, `member_digests`) and its
  `capability_digest` (R-POR-IPC). The broker's bind-time re-check therefore matches the list by hash under any id, as
  `prepare` does, and admits only the capability set consent covered. A rebind (a late broker, or a viewer-ceiling
  change) can never bind a build listed by an archive or member digest in the up-to-500 ms before the re-judge tick
  unloads it.

**One audit entry per refusal** (ipc-v1.md §9). The bridge audits only its own check-5 refusals (undeclared or not
granted). A refusal the broker returns (`broker_refusal`, which sets `broker_refused`) was already audited by the bus
(`client_subscribe` denials, `note_refusal` for invoke, `admit_write` for act), so the bridge never audits it a second
time. A portal is a local caller, so the bus answers a declared `invoke` target that is not exposed, or is below its role
floor, with `capability_denied` (ipc-v1.md §6.2, bus.md §4). The broker passes that code through unchanged.

**Broker seam (A-1).** The bridge calls the integration bus through a non-blocking client made on the main thread, once per frame
(`pump_portal_ipc`). The sibling's `BusRefusal` maps onto `IpcError` codes: not-found, unavailable, capability-denied,
rate-limited, invalid-input and limit-reached. Writes (`set_property`, `invoke`, `intent`) are judged as the installing identity
at write time. A portal action is a one-op commit on the extension write path, with host-stamped provenance
`{package_id, content_hash}` (A-13).

**Budgets (OD-8, `limits.rs`).**

| Limit | Value | Where |
|---|---|---|
| Page messages | 50 per second per mount | `limits.rs` |
| Live subscriptions | 16 per document | `limits.rs` |
| Delta delivery | one per subscription per 250 ms (4 Hz), last state wins | `Pending` coalescing |
| `set_property` | 2 per second, burst 5 | `limits.rs` |
| `invoke`, `intent` | 5 per second (burst 5); 2 per second (burst 4) | `limits.rs` |
| Declared stream rate | capped at 10 Hz | `MAX_EMIT_RATE_HZ` |

Buckets are per mount and survive reloads, so a page cannot reset its write budget by navigating.

**Delivery.** `take_outbox` re-checks visibility, the live Admitted mount and the generation at send time, so anything that
went stale is discarded. Pending scene changes collapse per `(op, node, key)`, with last state winning; a resync supersedes
what is pending and counts the drop. A `node` subscription waits for the broker's petal-scoped snapshot to prove the node
belongs to the petal; a missing node gets the same `not_found` as REST. Host to page is `PostWebMessageAsJson` of serde
JSON. There is no variable `eval`; `evaluate_script` appears only for fixed `history.back()`, `history.forward()` and
`location.reload()`.

**Lock order.** Protocol workers and the native handler never hold the mount `RwLock` while taking the bridge mutex.
`ipc::serve` reads the generation before `document_token`.

**Open.** fe-webview builds no bus `Caller`; the ceiling crosses the seam as above, and P-BROKER must open every portal
client through `Bus::open_portal_client(installing_did, role_ceiling, ..)`. Residuals: SP-3 (an envelope queued just before
a leave may still reach the new page) and the OD-3 cadence question for the sibling.

## §reviews-and-open — where the verdicts live

The per-gate verdicts are in the track directory, one file per gate, with one verdict line per finding:
`review-p1.md` (protocol, PASS-WITH-FIXES), `review-p2.md` (IPC, PASS), `review-p3.md` (UX, PASS-WITH-FIXES, blocked on
P3-01..P3-03), `review-p4.md` (admission, PASS-WITH-FIXES). The consolidated W6 verdict is PASS-WITH-FIXES, merge-blocked
on NEEDS P1-02, P3-01..P3-03 and P4-06 (`review-p4.md` §W6). Turn 2's security gate over this module, the backend and the
binary's portal host is `review-turn2/R-POR-IPC.md`.

Desk review only. Nothing in this module has been built or run. SP-1..SP-9 are desk-checked against pinned wry and
WebView2 sources (protocol.md §12) but not executed on hardware.
