# fe-ui/src/ui_shell — area managers for the egui shell

## 2026-07-26 routing

Topbar **Options** toggles `RightSidebarSection::Options` (ALL non-Select tools request the
Options section via `ToolState::activate`, not just Brush). Right-row helpers resolve
active section, compute labels, and provide toggle/reveal operations.

`ui_shell_architecture_20260724` Phase 2 (FR-4/5/6). The top-level shell is
decomposed into per-area **managers**. Each manager module is: a small state
**Resource** + **pure** decision helpers (unit-tested) + a thin render fn that
takes `&egui::Context`/`&mut egui::Ui`. Managers are **called from**
`panels::gardener_console` in a fixed order — they are NEVER registered as their
own Bevy systems (NFR-4: exactly ONE `EguiPrimaryContextPass` entry,
`gardener_ui_system`).

## Topology

| Manager | File | State resource | Pure helpers | Render fn |
| :-- | :-- | :-- | :-- | :-- |
| Topbar (FR-4) | `topbar.rs` | `TopbarState { chrome_request, maximized, brand_icon }` | `title_bar_gesture`, `TitleBarLayout::is_free_space`, `resize_direction_at` | `render_topbar`, `render_window_resize_grips` (+ Bevy system `apply_window_chrome`, §window-chrome) |
| Left (FR-5) | `left_sidebar.rs` | `LeftSidebarState { policy, user_intent }` | `left_visibility` | `render_left_sidebar` |
| Right (FR-6) | `right_sidebar.rs` | `RightSidebarState { requested }` | `active_section`, `section_label`, `slug`/`from_slug`, `toggle`/`is_active`/`reveal`/`reveal_slug` | `render_right_sidebar` |
| Modal (FR-7) | `modal.rs` | `ModalManagerState { disabled_panels, last_error }` | `guarded`, `transient_order`, `resolve_exclusive` | transient layer, rendered last |

Resources are `init_resource`'d in `plugin.rs` and threaded into
`gardener_console` via the `UiShellParams` SystemParam bundle (all `ResMut`, so
downstream slices can mutate without re-touching `gardener_ui_system`).

## §topbar (FR-4)

`render_topbar` is the migrated body of the old `panels::toolbar::top_toolbar`.
The single-source data — `TOOL_DEFS`, `shortcut_hint_line`, `active_tool_hint`,
`stash_active_tool`, and `mode_button_fill` — STAYS in `panels::toolbar`; the
topbar only calls it. The tool-switcher, deselect, and Data/Settings/Maps
buttons are byte-for-byte the same behavior.

**Section-toggle wiring.** The **Options** button toggles
`RightSidebarSection::Options` via `right.toggle(Options)` — the SOLE reveal path
for tool options. **Settings** and **Maps** toggle their own sections the same
way (`right.toggle(Settings|Maps)`, D-A10). **Data** has no section variant: it
still flips the floating `GisPanelState.open` window.

**Tooltips (FR-8).** Each mode button's hover text comes from
`toolbar::tool_tooltip_text(def)`, which joins `TOOL_DEFS` (glyph/name/shortcut)
with `tool_inspector::panel_descriptor` (title/subtitle/Use guidance) into one
string — so button, shortcut, and description can't drift. The redundant
`ToolDef.tip` one-liner was removed (its single-source duty moved to
`panel_descriptor`).

## §left (FR-5)

The left sidebar's visibility is a **policy**, not a per-frame stomp. The pure
`left_visibility(policy, user_intent) -> bool` (`left_sidebar.rs`) has exactly
one policy, `LeftSidebarPolicy::Manual` → `user_intent`: the sidebar is
**user-sticky** (D-A11 / Q-3 — the old auto-collapse-on-selection policy was
removed outright, not defaulted off). It stays where the user last set it,
unaffected by selection, an open right section, or a petal switch
(session-scoped, Q-2). The enum survives only as the decision seam that keeps
the function pure and testable.

- **Toggle:** `LeftSidebarState.user_intent` is flipped by the topbar ☰ button
  and by Ctrl/Cmd+B, both in `topbar.rs::render_topbar` (bare B is the Brush
  tool shortcut — `panels/toolbar.rs`).
- `render_left_sidebar` writes `sidebar.open = left_visibility(..)` **before**
  rendering. There is NO `!right_open` stomp any more — opening a right section
  never closes the left sidebar.
- `VisibilityState` (hierarchy eyes) is threaded via `gardener_console`.

## §right (FR-6) — precedence + the section-fn seam

`RightSidebarSection = { Inspector, Options, TerrainTools, ProposalReport, Settings, Maps, Groups }`
is the mutually-exclusive right region — **one section at a time** (RATIFIED
Q-2). `RightSidebarState.requested: Option<RightSidebarSection>` is the explicit
toggle (topbar / in-panel rail); `None` means selection-default. The rail has 7
glyphs sourced from `ALL_SECTIONS` (Inspector, Options, TerrainTools,
ProposalReport, Settings, Maps, Groups — PathTools retired; Options hosts all
tool-specific controls). Settings is `render_settings_section` (the former
floating Settings window, now the sole Settings surface); Maps is the former
Hexon Manager dialog; Groups (slug `"groups"`, VIS-PH5 / D-10) is the ONE
editor of the active petal's visibility Groups — the left sidebar only reads
+ toggles them and reaches this section through `RevealSection { slug:
"groups" }` (its "Manage Groups…" button and the node row's "Add selection to
group…").

**Section-toggle wiring.** `RevealSection { slug }` is the addressable idempotent
reveal (cross-links, context menu, future fe-api/MCP); topbar/rail stay toggles
(a reveal can never close a section).

**`active_section(state, selection_present, portal_open)` precedence —
portal > explicit toggle > selection-default:**

1. `portal_open` → `None`. A true **short-circuit**: the caller renders the
   portal toolbar and there is NO section rail underneath.
2. `state.requested == Some(sec)` → `Some(sec)` (explicit toggle wins over
   selection).
3. otherwise → `Some(Inspector)`. Inspector is the **never-blank** fallback: it
   self-collapses (`show_animated`) when nothing is selected, reproducing
   today's always-call-`right_inspector` behavior. `selection_present` is
   accepted for a future "welcome vs inspector" split but currently both cases
   resolve to Inspector.

`Option<_>` makes **never-double** structural (at most one section). `toggle`
also enforces it at the state level: requesting a new section replaces, never
stacks.

**The section-fn seam (CRITICAL — do not collapse).** There is exactly ONE
render fn per variant; all seven are now filled (the one-fn-per-section split is
what kept the P4 and P5 slices conflict-free):

| Section | Fn | Content | Landed by |
| :-- | :-- | :-- | :-- |
| Inspector | `render_inspector_section` | node inspector (`inspector::right_inspector`) | P2 |
| Options | `render_tool_options_section` | active-tool options dispatcher (Selection/Transform/Pen/Brush arms) | P5 |
| TerrainTools | `render_terrain_tools_section` | 8-mode palette + proposals (ex-"Terrain Tools" window) | P4 |
| ProposalReport | `render_proposal_report_section` | proposal report body (ex-"Proposal Report" window) | P4 |
| Settings | `render_settings_section` | app-settings knobs (render distance, mesh budget) + the per-verse Timeseries Fabric controls (§timeseries-settings) | — |
| Maps | — | map/hexon manager (ActiveDialog::HexonManager) | — |
| Groups | `render_groups_section` | Group create/rename/delete/eye/reorder (whole-registry `SetGroupRegistry`) + "Add selection to group" / "Remove from group" for the selected node (`SetNodeGroups`); empty Group shows "No members — select nodes and use Add to group" | VIS-PH5 |

Migrated tool sections keep their logic + tests verbatim and their authority
rules (path/stamp sections key ONLY on `PathEditorState.editing_track_id`, never
`NodeManager.selected` — NFR-1). Portal-open swaps the whole right region to
`portal_toolbar::right_portal_toolbar` (preserved). Empty states are calm hints,
never blank (`ui_ux.md §7`). `InputContext` (toolbar.rs owns the key; topbar is
the single stasher via `stash_input_context`) threads input-mode state.

## §modal (FR-7) — transient layer + panel panic guard

`modal.rs` owns the transient layer (dialogs, context menu, toast) and the
panel panic guard. It is CALLED FROM `gardener_console` (never a standalone
system — NFR-4).

**Panic guard.** `guarded(state, name, f)` runs each panel body inside
`catch_unwind(AssertUnwindSafe(f))`:
- **release:** a panic is caught → `tracing::error!` → the panel is marked
  disabled-for-session (`disabled_panels`) → `last_error` set → the frame
  completes. An already-disabled panel is skipped (not re-run).
- **debug (`cfg(debug_assertions)`):** the panic RE-PROPAGATES so developers
  crash loudly (RATIFIED Q-5).
- `AssertUnwindSafe` is sound by construction: `f` is a generic `FnOnce() -> R`,
  so no raw `&mut` egui borrow escapes; the guarded state is exactly what the
  guard quarantines.
- Surfaced UX (Q-5): a persistent status-bar Error-tier chip (`status_bar.rs`
  reads `last_error`), no auto-clear on petal switch.

**⚠ `panic = "abort"` — RATIFIED 2026-07-25: keep abort.** `Cargo.toml
[profile.release]` sets `panic = "abort"`, under which `catch_unwind` is INERT
— the shipped release binary will NOT quarantine a panicking panel. The user
ratified KEEPING `panic = "abort"` (retaining the tuned `lto=fat` /
`codegen-units=1` release profile): the guard is therefore intentionally
**debug/test-only scaffolding** (tests force `unwind`; debug re-propagates by
Q-5 design), and **FR-1 (root-causing the terrain crash) is the primary
remedy**. To make FR-7 shield in-app later, a future decision would flip the
release profile to `panic = "unwind"` (workspace-wide).

**Transient order.** `transient_order()` = Dialog → ContextMenu → Toast (toast
topmost); `TransientVisibility::resolve_exclusive()` keeps at most one dialog
family visible with toast independent. Rendered LAST in the pass so layering is
preserved.

**Guard granularity.** `gardener_console` guards each top-level render call
(topbar/left/right/viewport/each dialog/context-menu/toast/gis). The
right-sidebar guard is coarse — a panic in any one section quarantines all five
for the session (per-section granularity is a noted follow-up, would need the
guard threaded into `right_sidebar.rs`). `status_bar` is deliberately NOT
guarded (it hosts the error chip).

## §timeseries-settings (M2/F6 — the Settings section's fabric controls)

The Settings section hosts the per-verse Timeseries Fabric surface
(`render_timeseries_fabric_controls`, fed by
`crate::timeseries_settings::TimeseriesSettingsState`): mode radio
(mirror/sharded/balanced), the durability slider R (1..16 — clamped at
placement to the reachable peers), and the shard bucket width with presets.
It configures the ACTIVE verse (`nav.active_verse_id`; no active verse → a
calm hint, never blank — ui_ux §7).

Design decisions:

- **Settings live on the verse row, not `AppSettings`.** They replicate with
  the verse manifest and converge per-verse through the fabric; the app-level
  `AppSettings` knobs above them are process-local for a reason.
- **A dedicated state resource, not a `VerseEntry` field.** `VerseEntry` has
  13 field literals across the tree UI; churning all of them for one field
  (that the tree never shows) is the wrong trade. `TimeseriesSettingsState`
  mirrors per-verse settings (`HierarchyLoaded` folds
  `verse.timeseries` in), writes via
  `DbCommand::SetVerseTimeseriesSettings`, and displays only the
  authoritative `DbResult::VerseTimeseriesSettingsSet` echo — the surface
  never shows an optimistic guess (same read-back rule as the petal-map
  surface). The R slider is live-echo, not committed-buffer, so it tracks the
  echo exactly.
- **Bundled, not a 17th param.** `apply_db_results` was already at Bevy's
  16-`SystemParam` ceiling; the state rides inside the `DescriptorCaches`
  bundle (see `fe-ui/src/verse_manager/AGENTS.md`).
- `MAX_REPLICATION_FACTOR = 16` is a UI bound (a slider needs a finite range);
  the fabric itself only clamps R to the reachable peers.

## §window-chrome (frameless window, 2026-10-31)

The GUI's primary window has no OS decorations (`fractalengine/src/window_chrome.rs`,
fractalengine/src/AGENTS.md §window-chrome), so the topbar IS the title bar.
There is still exactly one top strip — `render_topbar` grew the chrome; nothing
adds a second `TopBottomPanel::top`.

- **Layout.** Left: brand mark (the app icon, uploaded once as an egui texture
  from `TopbarState.brand_icon`, which the binary inserts via
  `TopbarState::with_brand_icon`) + "FractalEngine" (non-selectable label), then
  the existing tool cluster. Right (right-to-left): close X, maximize/restore,
  minimize, then the existing Data/Options/Settings/Maps buttons. Caption
  buttons are painted vector glyphs (46x28, independent of font coverage); the
  close X hovers `theme::BG_WINDOW_CLOSE_HOVER` red (ui_ux.md §1 destructive
  chrome).
- **Drag decision.** A full-bar `click_and_drag` surface is registered BEFORE
  the widgets, so widgets win clicks. But egui hands a DRAG to a lower
  drag-sensing widget when the top one senses only clicks — egui's hit test
  alone would let a drag that began on a button move the window. The bar
  therefore records its two widget clusters' x-spans (`TitleBarLayout`), and
  `title_bar_gesture` admits only a press whose ORIGIN is on free space and
  outside the resize band: drag → `StartMove`, double-click → `ToggleMaximize`.
  The brand counts as free space (it drags, like a native caption).
- **Resize grips.** `render_window_resize_grips` runs after every panel (end of
  `gardener_ui_system`) so its cursor wins: a 5 px band at each edge/corner
  (`resize_direction_at`, corners first) sets the egui resize cursor — which
  bevy_egui forwards to the OS, so nothing fights over `CursorIcon` — and a
  primary press there requests `StartResize(CompassOctant)`. Inert while
  maximized. The grips read raw input (they claim nothing in egui), and the
  Bevy click router (`node_manager::router::resolve_pointer_frame`) runs in
  `Update` BEFORE the egui pass sees the press — so `gardener_ui_system` writes
  `ViewportRect` through `viewport_outside_resize_bands` (viewport ∩
  screen.shrink(band), unless maximized). Without it, a resize from an edge the
  viewport touches also picked / deselected / dropped a pen anchor (critic
  pass, 2026-10-31).
- **Applying requests.** `apply_window_chrome` (a Bevy system in
  `EguiPrimaryContextPass` after `gardener_ui_system` — a window side effect,
  not a second render pass) takes `TopbarState.chrome_request` and calls
  `Window::{set_minimized, set_maximized, start_drag_move, start_drag_resize}`.
  It is main-thread only (`NonSendMarker`) because it reads `WINIT_WINDOWS` to
  mirror `is_maximized()` into `TopbarState.maximized` (restore glyph, grips off).
- **Why a request slot, not a `UiAction`.** Widgets never touch `Window`: they
  write ONE typed `WindowChromeRequest` into the manager's own state
  (`TopbarState.chrome_request`), and `apply_window_chrome` is the single
  writer of `Window` for chrome (a command + single-executor pattern; the
  render fn stays a humble object, tested through real egui frames). It is not
  routed through `UiAction`/`UiManager` because `start_drag_move` /
  `start_drag_resize` must reach winit while the press that started them is
  still live — the request applies the SAME frame, right after
  `gardener_ui_system` — and window chrome is shell state, not a domain action
  an API/MCP caller should address. `apply_window_chrome` reads the slot via
  `Deref` first so idle frames never flag `TopbarState` changed.
- **Close.** The X writes `WindowCloseRequested { window }` — the same message
  winit raises for Alt+F4 / the OS close. Bevy's `close_when_requested` then
  despawns the window and `exit_on_all_closed` raises `AppExit`, so shutdown is
  exactly the OS-close path (no `process::exit`). Alt+F4 keeps working natively:
  winit keeps `WS_SYSMENU` on undecorated Windows windows.
- **Tests** (`topbar.rs`): real egui frames with synthetic pointer events —
  drag from free space moves; a drag that starts on the close X neither moves
  nor closes; click on X closes; double-click toggles maximize; a top-band press
  resizes (not while maximized); a table test covers every edge/corner.
