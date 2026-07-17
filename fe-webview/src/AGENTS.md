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
- Two activation tests (`selecting_model_with_allowed_url_sets_active_portal`,
  `selecting_model_with_no_url_sets_active_portal_but_no_navigate`) are
  `#[ignore = "needs full Bevy app (validation phase)"]` stubs:
  `MessageWriter`-driven activation needs a full Bevy app, so the behavior is
  exercised at the validation phase rather than in unit tests.

## §security — security.rs

All navigation funnels through `is_url_allowed`: http/https only, loopback and
private/link-local ranges blocked. Enforced in **both** layers on purpose —
at command dispatch (`dispatch_commands`) and again in wry's
`navigation_handler` (catches redirects and in-page navigation the command
layer never sees). The trust bar (`TRUST_BAR_JS`) is injected as an init
script on every page; do not remove.

## §tauri-commands — tauri_commands.rs

Scaffolding for a future full Tauri app shell (`#[tauri::command]` handlers for
node data / assets). Nothing registers them yet — there is no `tauri::Builder`
in the binary; wry is used directly. `resolve_asset` canonicalizes both base
and target to keep symlinks inside the petal asset dir.

**VerseManager wiring gap**: the node/petal commands (`get_node_data`,
`list_nodes_for_petal`, `notify_interaction`, `update_node_transform`,
`update_node_url`, `list_petals`) are stubs — no VerseManager query/event path
reaches this seam yet. Each logs via `tracing` and returns placeholder data
(hardcoded node, empty list, or bare `Ok`) until that wiring lands;
`resolve_asset`/`get_asset_base_url` are the only fully-functional commands.

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
