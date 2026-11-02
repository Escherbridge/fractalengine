# fe-webview — module notes

Design rationale for the browser overlay ("Petal Portal") and its webview
backends. Terse one-line doc comments in source point here.

## Architecture: the update seam

Bevy (main thread) ⇄ webview backend, connected by two Bevy message types
(`ipc.rs`):

- `BrowserCommand` (UI → backend): `Navigate`, `GoBack`, `Close`, `SwitchTab`.
  Written by `fe-ui` (`process_ui_actions`) and `petal_portal` systems; consumed
  once per frame by `plugin::dispatch_commands`.
- `BrowserEvent` (backend → UI): `UrlChanged`, `LoadComplete`, `Error`,
  `TabChanged`. Produced by `plugin::drain_backend_events` from
  `BackendEvent`s the backend queued.

The backend itself is behind the `WebViewBackend` trait (`backend.rs`) and is
`!Send` (native window handles are thread-bound), so it lives in the
`WebViewBackendRes` NonSend resource. `backends/mod.rs` selects the concrete
type at compile time: Servo > Tauri > Stub (default feature = `backend-tauri`).

Geometry flows one way: `fe-ui` writes `PortalPanelRect` (logical px, panel
coordinates) every egui frame → `plugin::sync_portal_position` (PostUpdate)
converts to screen-space physical pixels via the winit window and calls
`reposition`. Repositioning is skipped when the geometry is unchanged from the
last applied one — `SetWindowPos` + `set_bounds` every frame is wasted work and
caused z-order churn.

## §tauri — TauriBackend (wry / WebView2)

On Windows the webview cannot be a child of the Bevy window directly: wgpu owns
the swap chain and a WebView2 child HWND gets painted over. Instead we create a
borderless `WS_POPUP` **owned** by the Bevy window (`win32_popup.rs`) and build
the wry webview as a child of that popup. Ownership keeps the popup above its
owner without being topmost globally.

- **Create-visible dance**: WebView2 refuses to initialize its render pipeline
  under a hidden HWND, so the popup is created `WS_VISIBLE`, the webview is
  built, then the popup is hidden until the first `navigate()`. A brief flash
  at startup is the accepted cost.
- **URL dedup** (`current_url`): `navigate()` skips a `load_url` when we are
  already at that URL. Two invariants keep it honest: it is recorded only
  *after* `load_url` succeeds (a failed load stays retryable), and
  `drain_events` refreshes it from `UrlChanged` events so in-page navigation
  (user clicks a link) doesn't leave the dedup pointing at a stale URL.
  `hide()` clears it so re-opening the same node re-navigates.
- **Z-order**: only `show()` raises the popup (`HWND_TOP`, no activate).
  `move_window` passes `SWP_NOZORDER` — it is called on geometry changes and
  must not raise the popup over other applications' windows.
- **External close** (`WindowClosed`): `drain_events` detects the popup HWND
  dying (e.g. Alt+F4). `plugin::drain_backend_events` then drops the backend
  and sets `WebViewBackendRes::lost`, which makes `init_backend` recreate it
  on the next frame. Create-*failure* is deliberately terminal (no retry loop).

## §portal-lifecycle — petal_portal.rs

Selection-driven overlay lifecycle: `Added<Selected>` on an entity with
`ModelUrlMeta` opens the portal; `SelectionCleared`/ESC/despawn closes it.
The old guard/flush command re-write pipeline was removed — it echoed
`Navigate` commands every frame; `SwitchTab(Config)` role-gating now happens
inline in `dispatch_commands`.

## §petal-portal-deferred — validation-phase work

- `position_overlay_system` is a change-detection shell today. Sprint-4 plan:
  project the selected model's AABB to screen space and drive
  `surface.position()`; the projection math waits on wry surface integration.
- Activation is covered headlessly: the two activation tests spawn a
  `(ModelUrlMeta, Selected)` entity in a `MinimalPlugins` app and drain
  `Messages<BrowserCommand>` after one frame (no backend system consumes them
  in that app), asserting `Navigate` + `SwitchTab{ExternalUrl}` for an allowed
  URL and neither for absent/empty/policy-blocked URLs.

## §security — security.rs

All navigation funnels through `is_url_allowed`: http/https only, loopback and
private/link-local ranges blocked. Enforced in **both** layers on purpose —
at command dispatch (`dispatch_commands`) and again in wry's
`navigation_handler` (catches redirects and in-page navigation the command
layer never sees). The trust bar (`TRUST_BAR_JS`) is injected as an init
script on every page; do not remove.

## §tauri-commands — tauri_commands.rs

Only the asset helpers remain: `resolve_asset` and `get_asset_base_url`.
Nothing registers them yet: there is no `tauri::Builder` in the binary; the
embedded backend uses wry directly. `resolve_asset` retains lexical path checks
and canonicalizes both base and target to confine symlinks to the petal asset dir.

The six unregistered node/petal commands and their unused PetalInfo DTO were
removed. They returned fabricated data, empty lists or successful no-op writes;
there is no webview-JS object mutation/query bridge at this seam.

## fallback-backend

Stub and unfinished Servo report embedded browsing unsupported and never auto-launch a page.
Their navigation/history/window operations cannot report success. The existing stub journey
asserts refusal/no embedded events without launching any application.

OpenExternal is handled independently of backend initialization, requires the same URL policy
and explicitly invokes explorer.exe/open/xdg-open with one URL argument. A launch request
produces ExternalLaunchRequested, never UrlChanged or LoadComplete. Tauri Forward and Reload
use the existing pinned wry evaluate_script API with history.forward()/location.reload(). The
page-finished callback supplies its URL as PageLoaded; old unbound completion remains compatible
but cannot confirm the UI's current loading state. Missing/init-failed backends emit visible
errors rather than leaving the UI silently waiting.

## §tests

`tests/tauri_cutover_test.rs` shells out to nested `cargo check`. Path rule:
`CARGO_MANIFEST_DIR` in an integration test is the **crate** root
(`<workspace>/fe-webview`); the workspace root is one `.parent()` up. The
nested `--workspace` check and `--list` tests are `#[ignore]`d (log-only,
expensive); run them explicitly with `cargo test -p fe-webview -- --ignored`.

## §tab-policy (auth_policy_pattern_20260710)

`petal_portal.rs::TabVisibilityFilter::can_view_config()` no longer compares
roles locally: the local `Role` enum maps onto `fe_policy::RoleLevel`
(Admin→Owner, Editor→Editor, Viewer→Viewer) and the Config tab is evaluated
as `Action::Manage` on scope `UI#config-tab` against the shared
`RoleLevelPolicy::standard()` engine. Same observable behavior (only Admin
sees Config), but the decision + its log come from the engine. The `Role`
enum itself stays until the session layer (`fe_database::session_cache` /
`fe-policy`) exposes the canonical session role.

## Canonical loopback policy correction

The existing loopback intent uses typed url::Host addresses: every127/8 IPv4
address and IPv6::1 is denied after URL canonicalization, including compact,
numeric and bracketed spellings. IPv4-mapped and compatible IPv6 addresses pass
through the same IPv4 checks, closing bypasses of loopback,0.0.0.0 and the
configured private/link-local CIDRs. Those CIDRs remain10/8,172.16/12,192.168/16,
169.254/16 andfc00::/7. No other address range is added by this correction.

Domain checks deny localhost, its reserved subdomains and trailing-dot forms,
plus explicit conventional localhost.localdomain,localhost6,localhost6.localdomain6,
ip6-localhost andip6-loopback aliases. Ordinary public domains/IPs remain allowed
under HTTP(S). This is literal URL validation, without DNS resolution or a DNS
rebinding firewall claim. No QA private-origin exception is introduced. Existing
policy tables extend canonical loopback, aliases, mapped private addresses and
public boundary cases; source authoring is not execution evidence.

## QA web navigation

The shared raw admission functions live in `fe-runtime::portal_url`; production `is_url_allowed` remains the unchanged canonical literal-address policy. `BrowserCommand::Navigate` retains the original URL string through deserialization and dispatch so URL normalization cannot turn an alias into an admitted QA origin. UI previews do no process/listener work; persistence and actual command/backend navigation revalidate the same captured capability.

A fixture session latches when a permitted fixture URL enters the backend, including history or in-page navigation. It stays latched while hidden and refuses all in-page destinations outside the two exact fixture pages. Only another explicit admitted Navigate command switches that session back to normal production browsing. Captured Windows hosts install exactly one native WebView2 NavigationStarting policy owner and omit the Wry navigation callback. That owner cancels first, reads raw URI and IsRedirected, checks production policy or the live exact fixture helper, then reallows only an admitted destination. UrlChanged is emitted only after the final native setter succeeds. Hosts without a captured capability retain the Wry callback and existing production decisions. New windows are explicitly denied for captured hosts; external launching always retains production policy.

Missing event arguments, metadata failure and either native cancellation-setter failure synchronously attempt both Stop and controller Close. They permanently disable backend methods, suppress pending URL/load events, hide the popup and deliver a static terminal error without scheduling automatic backend recreation. Registration failure also attempts both abort operations and refuses backend creation. The receipt distinguishes a confirmed initial cancellation or successful abort from the case where cancellation, Stop and Close all fail. In the latter case navigation cancellation is explicitly unconfirmed; the unusable flag alone is not evidence of transport abortion. This residual requires security review before claiming the exception is safe under every COM failure. The existing Windows journey injects adapter failures for metadata, cancel, allow, Stop and Close without launching a browser.

The optional Windows direct webview2-com dependency matches the already-locked transitive0.38.2 version. Root must resolve the manifest-only dependency graph before a locked build. No browser execution or native callback verification is claimed by this source change.

## Terminal maintenance

The desktop destroys its native browser on the main thread when terminal maintenance starts. Ordinary Update command/event processing is removed with the editor schedule; PostUpdate geometry/overlay helpers additionally require editor_running. They cannot recreate or display a portal from stale navigation after reset. Hosts without TerminalMode retain their previous behavior.

## QA Portal evidence

`backends/fixture_surface` owns fixed DOM scripts and native CapturePreview work for a captured fixture host. Scripts embed the admitted exact source URL and check it again inside the document, then validate only the known title, single heading, ready state and two literal links. Link operations invoke the actual fixed DOM anchor; page two requires a fresh observed target document, while the third-link oracle requires the native guard refusal counter and exact refused destination with the original page unchanged. No caller supplies JavaScript, selectors or URLs. Script output is size capped and strictly deserialized before exposure.

Each job binds the owned popup lifetime cookie, controller, visible state, page generation and exact live permit. The cookie disappears when its HWND dies, rejecting reused handles. Native navigation, history/reload, close and geometry changes invalidate pending captures. Completion keeps bytes private until the backend and host context checks pass. Timeouts/stale pending callbacks use the reviewed Stop/Close terminal contract, including truthful unconfirmed cancellation if both abort calls fail. Stub and non-Windows backends return unavailable.

`fixture_stream` implements a bounded eight-MiB COM IStream sink rather than an unbounded HGlobal stream. CapturePreview supplies the actual owned native webview pixels. The result checks PNG signature/IHDR, bounded complete chunk framing through IDAT/IEND, actual physical dimensions and SHA-256; it does not claim a full PNG decoder or visual page judgment. The client and independent reviewer must inspect saved native PNGs. The sole navigation handler remains authoritative; the fixture surface installs no second policy handler. Direct already-locked serde_json, sha2, Windows0.61.3 COM/Foundation and windows-core0.61.2 dependencies require root-owned lock resolution.

Fixed scripts also embed the admitted exact source URL inside the document, preventing a delayed script from operating on another origin/page. A bounded loading sentinel permits retry without interpreting incomplete DOM facts. Before releasing a completed receipt the backend rechecks its owned popup/session/page generation, exact source, helper and capture dimensions again. Request expiry and changed-page results terminate pending native work; completed document/PNG diagnostic refusals do not claim a page loaded. The existing journeys extend actual COM stream cap enforcement, page-generation ABA/refused foreign destinations and strict PNG framing refusal. Runtime image and native callback execution remain root-owned verification.

The page clock records the raw admitted destination inside the existing native policy owner, after the final allow setter succeeds. NavigationStarting may precede the Source change and the initiating script callback. Only a second-link job beginning on page one may wait through that intermediate old-Source state, with exactly one generation increment admitting the exact same-origin page two and no intervening refusal. Completion still requires Source to become page two and a new fixed observation of its ready document. Starting that operation on page two is refused, so a self-link cannot mistake the old ready document for a fresh navigation. Generic geometry/history/close invalidations clear the admitted destination; extra transitions and foreign destinations remain terminal stale work. The extended existing native boundary journey covers this ordering and invalidation contract; source checks do not establish native execution acceptance.
