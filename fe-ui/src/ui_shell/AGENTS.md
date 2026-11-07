# fe-ui/src/ui_shell — area managers for the egui shell

## 2026-07-26 routing

Plural selection uses the same checked transform/property form in Inspector and
Move/Rotate/Scale/Select Options. Brush/Measure keep their own independent controls;
Pen explains its one-route requirement. Groups controls apply creation and membership
to every selected object through one checked batch. The legacy per-node membership
buttons are not used by the production Groups section. See `../editor_batch/AGENTS.md`.

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

**Guard granularity.** Each right-sidebar section owns a stable `right.<slug>`
quarantine key. The rail and a readable Retry surface remain available when a
section is disabled. Retry clears only that key; unrelated failures remain in
`disabled_panels` and the persistent error indicator. The portal toolbar has a
separate boundary. Other top-level render calls retain their own guards. A
Retry is an explicit user attempt, never an automatic loop. Debug fresh panics
still propagate and release `panic=abort` still cannot be recovered in process.

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
  does not mutate that mirror while editing. The scoped form draft described
  below keeps an interaction visible until its stored echo arrives.
- **Bundled, not a 17th param.** `apply_db_results` was already at Bevy's
  16-`SystemParam` ceiling; the state rides inside the `DescriptorCaches`
  bundle (see `fe-ui/src/verse_manager/AGENTS.md`).
- `MAX_REPLICATION_FACTOR = 16` is a UI bound (a slider needs a finite range);
  the fabric itself only clamps R to the reachable peers.

## Durable settings interaction commits

`setting_draft::SettingDraft` is a small form adapter shared by the durable
petal render distance and the verse's whole timeseries configuration. It owns
only UI drafts, keyed to the current target; it never writes hierarchy or
timeseries mirrors. Numeric widgets use native `update_while_editing(false)`
so partial text is not persisted. Pointer drag release, numeric focus loss or
Enter, and slider keyboard key release finalize one interaction. Mode and
bucket preset clicks finalize immediately. Session overrides remain immediate
and local. Ranges constrain edits, preserving existing values on render so
opening Settings cannot silently rewrite them. The old manifest precedence tooltip is replaced by the actual
shared-petal/session precedence.

Numeric interaction state is tracked by stable field purpose within the form,
using each field's actual native response. Only a field with value changes can
finalize its own interaction; an unrelated clean blur or key release creates
no readiness. The whole configuration waits while any numeric pointer/key/text
edit remains active or unfinished, regardless of renderer order. This prevents
the later bucket-width blur from publishing an earlier R drag. Partial numeric
text stays unfinished until its own Enter/blur. Explicit mode/preset clicks
can retry an unconfirmed draft; unchanged stored configurations still emit no
write. Failure/timeout consumes prior readiness, so clean focus/blur cannot retry.

Only one write per draft may await readback. A newer finalized edit remains
editable while the first write is pending and is submitted after that stored
value arrives. An echo cannot replace a newer local draft. Clean drafts adopt
external stored changes; target changes reset the form, including native
widget IDs. Dirty state records actual value edits separately from focus, so
leaving an unchanged focused field cannot overwrite a newer stored value.
Channel failure or 30 seconds without matching stored confirmation
keeps the payload with an inline unconfirmed/error message and requires another
edit to retry. This is value-readback confirmation through the existing GUI
contracts, not a new request-correlation or backend authorization guarantee.

## Groups mutation controls

The Groups renderer receives the active `LocalUserRole`. Every mutation is
disabled for unresolved/Viewer roles with an Editor explanation; backend
authorization remains authoritative. Selected-node membership controls receive
only a node belonging to the active petal. Group form/confirmation identities
include petal and group IDs. Rename commits on Enter, Escape discards, and an
unsubmitted focus loss also discards. Delete uses the shared two-step helper;
canceling it changes no registry or membership. Registry and membership caps
have disabled reasons, and a full registry has visible limit copy.

The existing residency counter test retains its original assertion and becomes
`sidebar_groups_and_durable_settings_journey`: native egui frames cover denied
roles, rename typing/Enter/Escape, Delete arm/cancel/confirm, the Group cap,
durable drag/channel counts, older readback plus newer drafts, slider keyboard
release, numeric Enter and unconfirmed timeout. Stored echoes are explicit model
fixtures, not DB persistence proof. No test function is added. Source parsing
is not execution or persistence evidence; integrated
compilation, the final test sweep, and live role/interaction/readback checks
remain required.

The same journey also focuses the clean bucket before an R press/drag and
asserts no press/held-frame command. Unconfirmed and closed-channel fixtures
exercise focus/key release/blur without editing and require no retry; the
closed-channel fixture counts actual failed sends through the shared adapter.

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
- **Restore from minimized is two frames (QA-0028).** Windows' `SW_RESTORE` on
  a minimized window returns it to its pre-minimize placement — maximized
  included — and bevy_winit applies `take_maximize_request` BEFORE
  `take_minimize_request` in one frame, so the old same-frame
  `set_minimized(false)` + `set_maximized(false)` ended maximized
  (maximize → minimize → restore left 1920×1032 `maximized:true`).
  `TopbarState::begin_restore` now only un-minimizes when the mirrored state
  is minimized and records `restore_after_unminimize`; `take_deferred_restore`
  un-maximizes on the first frame the OS reports un-minimized. Any newer
  chrome request clears the pending step.
- **Why a request slot, not a `UiAction`.** Widgets never touch `Window`: they
  write ONE typed `WindowChromeRequest` into the manager's own state
  (`TopbarState::try_request_chrome`), and `apply_window_chrome` is the single
  writer of `Window` for chrome (a command + single-executor pattern; the
  render fn stays a humble object, tested through real egui frames). It is not
  routed through `UiAction`/`UiManager` because `start_drag_move` /
  `start_drag_resize` must reach winit while the press that started them is
  still live — the request applies the SAME frame, after egui output and QA request dispatch.
  A pending request returns typed busy rather than being overwritten. UI-scale changes are separate and cannot
  cancel a queued window operation. `apply_window_chrome` reads the slot via
  `Deref` first so idle frames never flag `TopbarState` changed.
- **Close.** X, Alt+F4/OS, and QA requests share `WindowCloseRequested`. The editor disables Bevy's
  automatic close handler and `receive_close_requests` opens one confirmation. Cancel preserves the window;
  confirmation lets the single chrome executor despawn it, and `exit_on_all_closed` raises `AppExit`.
  No route calls `process::exit`. Close confirmation is exposed through the QA state probe.
- **Tests** (`topbar.rs`): real egui frames with synthetic pointer events —
  drag from free space moves; a drag that starts on the close X neither moves
  nor closes; click on X requests confirmation; double-click toggles maximize; a top-band press
  resizes (not while maximized); a table test covers every edge/corner.


## ?layout-policy (S6)

`layout_policy.rs` is the native-egui shell geometry boundary. The left and right panel limits reserve
45% of the window width (with a 360 point viewport floor when feasible); right width uses remaining
space after the left panel. Width ranges handle narrow min/max ordering. Each right section, including
Maps, uses `section_chrome`; its bounded scroll region prevents wide body content from inflating the
persisted panel. Scroll content receives a finite width and wrapping style. Overflow remains reachable
through scroll rather than consuming the viewport. The hierarchy reserves 86 points for its two-step
Reset footer. `trailing_row` bounds action rows; bare `with_layout` in a vertical body consumes its
remaining height. Native dialog windows share screen bounds and scrolling. Breadcrumbs reserve Back
before an ellipsis label; full names remain available on hover. `clipped_value` delegates path shortening
to Unicode-safe middle ellipsis. SQL uses a finite 38% shell-height budget with its own scroll region.

QA observations are opt-in context temp data reset before each primary pass. Scroll content IDs,
clip rectangles, offsets, content dimensions and layers are recorded while drawing and exported in
physical pixels by the existing guarded ui-tree bridge. Unknown hit/layer/text facts remain explicit;
AccessKit parent rectangles are not assumed to be clipping boundaries.

## ?query-results (S6)

`InspectorFormState.query_results` composes ONE `ResultsDockState` UI view/run model. SQL input remains
separate. `begin/complete/fail/invalidate` own pending correlation, provenance, timing and structured
rows. Existing GIS and Paths claims remain independent. A verse switch invalidates rows and pending
claims before rendering or routing DB replies. A stale/foreign correlation never claims the dock.
Captured submitted scope is provenance only: `RawQuery` still lacks W5 server-side scoped authorization.

The dock renders after sidebars and before the CentralPanel. Native `show_rows` virtualizes rows;
sorting reorders indices, preserving source row identity. Formatting and scene node IDs are cached per
reply. Column handles resize bounded widths; cells and rows copy through egui's clipboard path (the QA
sink suppression still applies). Current-petal node rows queue `NodeManager::request_select_and_frame`,
which reuses the sidebar resolver and live-transform camera focus; projected non-node rows remain
copyable. Query-match scene markers are a separate visualization, never a second selection authority.


Shared `action`, `record_response` and `value_label` hooks retain actual drawing-time Response hit,
clip, sense, layer and modal-allowance facts, plus galley elision/full-hover facts. They use native
Response IDs and layer-to-global transforms; the binary applies physical scaling once. No hidden
unsafe ID reconstruction or post-EndPass read_response fallback is used. Each shell invocation resets
observations with its cumulative pass number so discarded/repeated egui passes do not mix geometry.
S2 can use these small helpers while naming the remaining controls. Unvisited controls remain explicit
coverage gaps. Narrow tool captions shorten to glyphs while accessible names retain full tool names.


Compact geometry keeps the requested section and manual left-sidebar intent unchanged. When both
120-point sidebar minima cannot coexist with the protected viewport, effective left visibility collapses;
it returns on widening. The viewport floor is 360 physical pixels divided by current pixels-per-point.
A still-infeasible right section also yields without changing its request. At narrow widths the topbar
uses native Tools/Panels menus, smaller caption cells and icon-only branding; section rails wrap.
Native dialog minimum and maximum widths use the same policy cap, so caller preferences cannot invert
resize constraints at the supported 640-pixel window / 2x UI scale edge.


The visual `ViewportRect` remains the actual CentralPanel rectangle. `CameraPointerRegion` separately
excludes the shared DividerMetrics radius on the actual left/right panel dividers and Results top divider,
plus native window grips. Camera input and NodeManager's single pointer arbiter share its physical-to-
egui conversion and foreground-area gate. Hovered gimbal picking reuses that arbiter. A fresh divider
press therefore belongs to resizing before egui has an actively held pointer, while interior picking
continues normally. This also avoids comparing logical window pixels directly with scaled egui points.


Scroll geometry version2 distinguishes three facts from pinned egui0.33.3:
`clip` is the actual content Ui painting clip captured inside the ScrollArea;
`viewport` is ScrollAreaOutput.inner_rect, used only for range computation;
`content_rect` is the final content Ui minimum rectangle. The output viewport
is captured before ScrollArea::end expands disabled axes, so a horizontal-only
area may paint tall content below that viewport while still inside its real
child/ancestor clip. Never derive paint clipping by intersecting inner_rect.
`content_id` and `parent_id` are actual Ui unique IDs, not spatial association.
All rectangles share one layer-to-global transform here; the host scales once
to physical screenshot pixels. Control observations retain their own actual
clip/hit geometry, including stricter local scissors within a scroll body.

Source basis: egui0.33.3 `containers/scroll_area.rs::{begin,show_viewport_dyn,end}`
in the pinned registry source. `begin` sets the actual child's clip per axis,
`show_viewport_dyn` saves the initial inner_rect, and `end` adjusts disabled-axis
size after content layout. The existing headless layout journey now retains
fully visible lower rows, parent-hidden rows and a stricter Cancel scissor as
separate telemetry cases; execution and fresh live checker proof are pending.


Dialog confirmation scopes in modal.rs are temporary UI identity, separate
from shell quarantine and domain selection. One per-dialog counter gives each
new target/lifetime a distinct confirmation salt. Explicit close and a missed
render frame disarm the prior lifetime. DirtyClose is the common decision for
requirements Close/X and explicit discard; it never writes or clears a draft.


## Settings receipts and restart boundary

SettingsSession compares the current settings with the last successful atomic
file write. The first startup observation seeds a baseline without displaying a
new save receipt. Success alone updates Saved; failure retains an inline error
and permits one explicit Retry. Reverting/editing the draft cannot invent a disk
receipt. Hosts without SettingsPersistence remain explicitly session-only.

The relay startup baseline identifies changes that require restarting. Once
saved, Close to restart opens the same OS/caption/QA close confirmation. It does
not spawn a process or promise an automatic restart. The confirmation discloses
unsaved requirements and pending local settings. The user must reopen the app;
automatic cross-platform restart needs orderly DB/API/sync thread shutdown first.
The QA launcher remains the sole automated QA lifecycle owner.


## Development database reset gate

Danger zone is visible only in debug builds or when FE_DEV_TOOLS=1 was captured
at startup. The host permit also requires runtime quiescence. The current GUI
has live API, DB, plugin and sync writers with no coordinated stop/invalidate
barrier, so its permit is deliberately not runtime-ready. Controls explain that
isolated maintenance is required; typing RESET cannot bypass that gate. Neither
legacy ResetDatabase nor the correlated command can clear a live GUI database.

The future ready host uses exact RESET plus the shared two-click confirmation,
scoped to a continuous Settings lifetime and navigation context. It waits for the
exact request ID; after30 seconds an unknown result remains unconfirmed and
cannot silently retry. Late matching replies still resolve it. Clear success
plus default-seed failure is reported as a committed clear with a restoration
error, never as an untouched database or a successful reset. GUI clear replies
invalidate navigation and selection before reloading. They do not erase peers,
identity, preferences or blob files. Root must finish actual API EntityStore,
sync catalog and plugin/session invalidation before admitting a ready live host.

## Inspector tab lifecycle

The report's open_report_node_properties adapter preserves deferred NodeManager
selection and reveals Inspector through set_active_tab(Properties). This shared
transition clears API secrets and pending mint ownership immediately, including
when the report opens Properties for the already selected node in the same scope.
Never assign Inspector tabs directly; the form owns the private tab lifecycle.


## Chrome tool admission and shortcut help

Tool::requires_edit is shared by keyboard decoding and the topbar. Pointer
activation keeps the same ToolState::activate writer; edit tools are disabled
with an Editor explanation when the current role cannot edit. Select and
Measure remain available. This is advisory UI gating; mutation handlers retain
their stored target authorization.

The named question-mark button and unmodified question-mark text event open the
same egui memory popup. Tool names/keys come from TOOL_DEFS; the remaining copy
states the actual camera, selection, visibility and Undo shortcuts. The popup
blocks the existing scene keyboard decoder and hierarchy shortcut, preserves
text-input ownership and closes through Escape, outside click or its Close
button. Narrow windows use named Tools/Panels menus below 1248 points, keeping
full tool names in the menu. Section buttons retain their existing toggles.

Inspector's chrome body slot is unscrolled: right_inspector pins selection/tab
navigation above its one vertical body ScrollArea. Other sections retain the
shared chrome scroll. Keep the section title/rail outside both layouts; do not
wrap Inspector in another shell scroll or reset offsets to make tabs reachable.
Property-grid horizontal scrolling and bounded value editors remain local.

## Shared numeric interaction drafts

SettingDraft is crate-visible so the Paths point-height control uses the same finalized-text,
keyboard and pointer interaction policy as durable settings. The existing native settings
journey also exercises the actual point widget for Enter, blur, drag and focus-only frames.

## Form target policy

layout_policy::target_size is the shared 24-point/20-physical-pixel rule. Existing
object_data and action helpers delegate to it. text_input applies that minimum to
a native single-line TextEdit builder, leaving the caller's width, labels, enabled
state and interaction lifecycle intact. It does not resize read-only text or change
global icon/font density.

## Native floating-window header targets

Bounded windows finish with WindowTargets::show_with_targets. Pinned egui0.33.3
Window::show captures area_content_ui and allocates TitleBar before running the
scroll/body child; it later paints TitleBar against that captured area Ui.
TitleBar::close_button_ui uses spacing.icon_width for its real square interaction
rectangle, independently of interact_size. Window exposes no per-window style
setter, so the adapter temporarily installs a theme-specific header style before
native allocation. It restores/disarms that context scope before user contents
run and sets the body child back to the original style. Native Close/collapse,
dragging, IDs, open flags and pending guards remain egui's own operations.

The header's parent Ui retains its captured style while the body and nested
popups see the ordinary context. Deliberate style edits in the body persist;
the guard does not restore over them afterward. Closed/collapsed/no-body and
pre-body unwinding paths restore the exact original theme style through Drop.
This is a synchronous primary-UI adapter, not a general cross-thread style scope.
The existing import and shell layout journeys add real Close geometry/click,
pending suppression, nested Popup style and no-body restoration assertions.
They have not been run in the source-author lane.

## Confirmed Group registry editing

Registry controls wait for the active Petal registry, then queue a checked proposal without optimistic display changes. Pending edits disable competing registry and batch Group controls while keeping Check stored Groups available. The matching current readback releases ownership; a missing read can be checked again after its bounded wait. Actual Delete routes through a complete server-derived Group transaction and one proven chronological Undo; registry CAS still refuses ID removal. See `../visibility/AGENTS.md`.

Rename drafts survive queue/admission refusal and clear when the stored name matches or Escape discards them. Pending batch ownership disables registry mutation controls across the right panel, left Groups eyes and layer manager. The cached loaded-member count is advisory; deletion always checks actual stored membership, and protected tombstoned references produce a counted whole-operation refusal.


## Responsive Group registry rows

Group names and the two fixed-size reorder actions share a flexible header row.
The destructive confirmation receives a separate full-width row, so arming Delete
cannot squeeze Cancel into the leftover name/reorder gutter. Existing confirmation
IDs, accessible action names, four-second expiry, outside-click/Cancel behavior and
Editor/pending/registry gates are unchanged. New-Group input/actions wrap as native
widgets, while the Add label keeps its unwrapped intrinsic width. The existing
Groups/settings rendering journey adds armed rows at narrow and normal widths and
two scales, checking complete target bounds, readable one-line Confirm/Cancel,
editable name space and actual Cancel dispatch without a mutation. These checks
remain source-authored until the parent integrated sweep.


## Host interaction-state observation

ModalManagerState is publicly re-exported at ui_shell for the native accessibility
host's read-only Resource borrow. The modal implementation module stays crate-private;
the host observes quarantine eligibility through the existing UI guard.


## Panel width intent and owned dividers (VIS-003)

LeftSidebarState and RightSidebarState each own a PanelWidthState from layout_policy.
Its private preferred width is session-only (defaults220/320 logical points).
Automatic viewport caps affect effective width and never write the preference.
The native SidePanels keep their explicit IDs and layout/animation but disable
SDK resizing and use exact effective width. The right frame has no layout-cost
stroke; the native separator draws its edge without adding four points to the
stored outer geometry. No PanelState deletion/reverse lookup/private SDK resize
ID or general context-layout reset is used.

prepare reads only the previous actual app-owned divider Response under the
public fe-hierarchy-panel-divider / fe-right-panel-divider IDs. register records
a new semantic vertical Splitter after panel contents, on the normal Background
layer. Its actual strip uses the displayed panel edge, vertical extent and
shared DividerMetrics radius, matching existing strict camera/selection exclusions.
No foreground Area or global-pointer resize authority is added. Foreground
windows retain egui hit-test priority; popups, modal state, lost focus, Escape,
close/unavailable sections and quarantines refuse/cancel this owned gesture.

The drag captures actual displayed outer width and pointer origin, so a panel
preferred at220 but clamped to160 responds immediately to an inward10-point drag.
Preference stays unchanged during preview. A stationary press, or motion beyond
a cap that never changes the displayed divider, keeps the prior preference.
Primary release commits only an actual width change beyond half a physical pixel.
Cancellation restores the original preference; it never releases another widget's
capture. Scale, viewport, available anchor/budget, actual Context or pass-continuity
changes cancel and suppress the still-held pointer until release. Geometry is
captured before SidePanel allocation and retained across registration, rather than
comparing pre-panel geometry with the already-consumed available rectangle.

Left show_animated registers a divider only for the public fully expanded Some
response. Collapsing, animation and reopening preserve preference. Right section
switches share one preference. The hierarchy retains both-axis scrolling and its
default-outer-width-derived content floor, so narrow row actions remain scrollable rather than
removed. Its outside-scroll header reserves a truncating full-name/hover row and
a separate explicit Join/Create row to avoid squeezing those actions into the name.
Actual 2x horizontal-scroll-and-invoke and responsive screenshots remain live gates.

The existing left visibility/layout journey is extended via the shared helper,
without new test declarations. It authors actual public Response drags, default
and custom preference restoration across narrow/high-scale cycles, stationary and
cap-only motion, constrained drag baseline, modal/scale cancellation, collapse and
reopen, actual right outer budget/no-stroke drift, and unchanged camera exclusion
geometry. These assertions are authored source coverage, not execution evidence.


The left renderer receives current TopbarState.close_pending through the narrow
shell call before prepare, and its production eligibility predicate combines
that flag with the existing UiManager dialog/portal checks. Closing can become
pending before the first egui modal is painted, so ctx modal state alone is not
sufficient. The existing width journey uses this same production predicate with
an actual TopbarState: newly pending close on release cancels instead of saving;
removing close-pending while still held cannot revive the canceled drag. Right
eligibility retains its existing close-pending check. No close action is executed
by the resize owner, and no other widget's input capture is released.

## Divider accessibility (VIS-003 v3)

DividerMetrics is one finite value object shared by both app-owned side handles,
the SDK-owned Results top divider, scene input and interactive overlay clearances.
Its stable product baseline is five logical points; radius is max(5,10/scale),
giving at least twenty physical pixels across a complete strip. The inner gutter
is ceil(radius+1/scale), checked for finite positive scale and representable i8
before conversion. Invalid metrics suspend owned gestures, disable native Results
resizing and make scene/overlay routing unavailable; no stale radius is retained.
The captured panel geometry includes these metrics, so changing them cancels a
preview with the existing held-input suppression rather than saving a preference.

apply_style refreshes the current pass/scale metrics and clears the prior Results
edge BEFORE its one-time typography/style early return. Other callers may derive
the same metrics directly when no current-pass cache exists. Results alone uses
the short native_results RAII style scope: it changes only the public SDK radius,
then restores it on normal return or unwind. Source-audited Results content has
table/header widgets and native popups, with no nested resizable panel/window.
No other SDK owner inherits this policy: portal_toolbar's native SidePanel,
GIS/other bounded Windows and modal windows retain their original radii/margins.
This deliberate scope avoids expanding their existing control overlap; their
permission/modal/portal admission remains unchanged. Camera routing reads the
explicit metrics, never the SDK radius after restoration. No private resize IDs,
checker exemption, new native Focus/numeric action or keyboard resize is added.

Zero-stroke inner margins reserve the left panel's right gutter and right panel's
left gutter without altering default outer budgets220/320. Their scrollbars and
headers stay inside that content rect. Hierarchy's minimum content width is220
minus the actual gutter AND the shared scroll helper's existing12-point reserve.
The Add Node/requirements row wraps rather than intrinsically widening the normal
panel. Narrow/deep content can still overflow through the existing both-axis scroll.
Results preserves side_top_panel styling, with its top margin raised from2 to
at least the checked gutter. Its side margins are max(original8,gutter) only
where actual pre-Results available geometry borders a side divider. With no
adjacent owner the original8-point margin remains. It remains the native
TopBottomPanel resize owner.

Results records its actual outer rect and cumulative pass after rendering. The
shared policy admits its bottom exclusion only when that same-pass top edge
matches CentralPanel.bottom, including the current frame after its Close button.
Overlay content receives a clipped child rect inset on actual side-divider edges
and that actual Results edge. Browse all/Back/browser cards stay outside the
inclusive strips with a physical-pixel gap. CentralPanel's parent response and
visual ViewportRect, camera projection and width budget remain unchanged.
CameraPointerRegion and NodeManager keep their shared strict endpoint comparisons.

The existing width journey now authors actual-margin/no-outer-drift checks,
complete physical strip widths, panel/overlay clearances, actual native Results
header/control protection and SDK-radius restoration, scale-policy refresh and
strict camera endpoints with physical one-pixel interior neighbors. Prior width,
pending-close, modal and held-input journeys remain. These assertions have not
been executed in the source-author lane; actual measured targets, neighbor edge
clicks, high-scale horizontal scroll/invoke and screenshots remain root acceptance.

V4 closes V3-D1's Results-side overlap. Frame inner margins alone do not clip
their content UI; pinned ScrollArea admits its normal three-point clip margin
until the parent clip. Both side-panel bodies and Results therefore intersect
their inherited clip with the actual inner content rect before widgets render.
This preserves native overflow/scroll reachability while preventing scrolled
targets from reclaiming the reserved gutter. Popups retain their own native
layers and clips. No painted border or outer-size change provides this clearance.

The same existing journey now populates64rows/24columns, requires actual overflow
on both axes and checks visible control hit-and-clip intersections on both X and
Y boundaries. Both/left/right/no-side profiles run at 0.75/1/1.25/2x, retaining a
separate same-Context scale-restoration sequence. It asserts first-column header
and cell presence plus the original8-point margin on edges without a divider.
A cfg(test)-only accessor retains actual public ScrollAreaOutput viewport,
content size and offset. Hover probes obtain actual native scrollbar IDs from
the public Context interaction_snapshot; read_response measures the real strips,
and a physical pointer drag must change the corresponding table offset. No
private SDK ID is derived. These checks remain authored, unexecuted source.

## Physical control targets (QA0039)

target_size is the shared24-point control density with a20-physical-pixel
backstop, rounded UP to public emath::GUI_ROUNDING before native allocation.
At0.75x the old26.6667-point request rounded down to26.65625/19.9921875px;
the new26.6875-point minimum gives20.015625px. At1/1.25/2x the normal24-point
density remains. The finite helper rejects invalid/unrepresentable scale math;
target_size keeps a finite normal UI fallback for an invalid context without
claiming a valid physical measurement. Supported context scales remain enabled.

apply_style refreshes ordinary interaction height every pass BEFORE the existing
one-time typography/padding return, from the stable baseline rather than the
prior enlarged style. Ordinary native buttons/checkboxes inherit this height.
Their fonts and painted icons are unchanged. A small button bypasses native
height inheritance, and native Button does not inherit a universal width floor:
small_button, action and control_size therefore reserve actual square/explicit
space. control_size preserves larger requested dimensions. Do not grow hit rects
over neighbors or change checker measurements to meet this minimum.

Native single-line TextEdit does not inherit interact_size.y. All existing
single-line builder sites delegate to text_input, preserving IDs, password/
draft/pending/auth behavior and further builder options. Both input axes have
the shared minimum. Local tree/grid/axis/rail/footer/copy formulas delegate to
target_size; object_data and editor_batch continue through their existing seam.

SectionHeader is a narrow native-view adapter used by Inspector sections. Pinned
CollapsingHeader always lays out with Extend even when its Ui has wrapping style.
A pre-truncated galley bounds that title to available width; its original full
job text retains the native collapse ID and accessible title. The adapter returns
the actual native response and adds full-title hover. It owns no selection or
collapse state. Compact field-schema inputs stack, property type text truncates,
and the outer Inspector native scroll minimum/maximum use the actual remaining
visible body budget rather than the default64-point minimum under a short clip.
Nested property editors retain their own stable content layout. Existing bottom
wheel evidence confirms Add Property fits the old clip at maximum offset; this
change corrects the viewport mismatch without asserting it was unreachable.

Results column resizing reserves a target_size-wide slot within each existing
column width, subtracting that slot from its sort header allocation. Its line
still paints at one point. The60-point minimum column leaves usable header space
at the supported0.75/1/1.25/2 scales. No strip overlays an adjacent Sort/cell target.

The existing journeys add real native rounded targets across all four scales,
same-context scale restoration, bounded section title/native identity, populated
column target dimensions/non-overlap, and a real compact Inspector shell with
populated properties checking X extent and native viewport versus actual clip.
No new test declaration is added; source-author execution is prohibited. Actual
screenshots, neighbor clicks, small-scale checker results, typed drafts, panel/
popup/menu coverage and scrollbar reachability remain root live acceptance.
The ruler's whole-window anchoring remains a separate terrain/viewport boundary
task; no ruler producer or camera/selection pointer routing is edited here.

## Residual layout correction (QA41)

DividerMetrics rounds the required radius upward to GUI_ROUNDING and reserves
one extra physical pixel on each side of the twenty-pixel target. Actual side
strip endpoints round outward on the physical grid; final finite clipped spans
must still meet twenty pixels. Two further physical pixels conservatively bound
endpoint conversion/rounding in the shared camera clearance. The interior gutter
adds another physical pixel beyond that clearance. Native Results uses the same
radius under the existing restoring scope, while overlay and scene routing use
the corresponding gutter/clearance. Invalid geometry suspends the owner, retaining
its existing held-input/release rules. Visual panel widths are unchanged.

compact_navigation evaluates the local visible height and width against a
three-row/96-point body target and ordinary navigation cost. Compact section
and tab selectors are native named Button menus with real selected options;
popups retain the native surface marker and ordinary keyboard operation. Section
choices call the existing toggle method. Wide rails keep their prior behavior.
Inspector limits its compact navigation scroll to the genuine remaining budget,
leaving body room when the window permits it, rather than inflating the native
body scroll minimum through the footer.

The existing width journey now checks the actual side-strip physical spans
without a threshold epsilon and rejects actual divider endpoint overlap with
the scene rectangle across the retained scale/restoration profiles. Integrated
execution and live camera-neighbor clicks remain parent verification.

Options Waiting renders the same checked-selection status/refresh UI as Inspector
through editor_batch::ui::render_read_status. Existing capture/capability gates
still disable transforms; terminal errors and pure-read retry are visible without
introducing a second selection owner or resetting an unknown accepted write.


## Painted core control icons (QA59)

icons.rs is the font-independent presentation adapter for core toolbar tools,
section navigation, Close, stored-value disclosure and tree visibility states.
Icon is a closed semantic enum; domain actions and state remain with the existing
callers. It reuses TreeIcon circle/ring/square paint and the caption-button vector
idiom. User data, arbitrary text, and the tree appearance palette are unchanged.

IconButton builds one native egui Button using a fixed-size Atom::custom and,
optionally, native text. Button::atom_ui returns the actual icon rectangle after
native alignment/wrapping/truncation. Painting consumes that rectangle, capped at
14 logical points and the resolved button font size, with thin 1.25-point strokes.
Its clip intersects the native control and caller clip. Disabled opacity comes
from the caller's existing painter; ordinary hover/focus text color comes from
native interaction visuals. The caller retains fill/active-mode conventions,
explicit accessible names, tooltips, permission gates and the click handler.
There is no new response, child UI, texture/font dependency or input owner.
The custom atom id is local to each Button's returned layout, not a widget id.

The same native target minimum remains independent of icon paint size. Existing
section rail mode fills stay with their original owner; no new selection writer
or semantic selected flag is introduced. ToolDef carries Icon instead of a Unicode
glyph; topbar tests locate unchanged textual tool names. The existing distinct-rail
journey now also authors the adapter's vector bounds/clip, text-free icon paint,
native accessible Button names/roles, stable IDs and disabled-click checks at
0.75/1/2x. These checks are authored but unexecuted in this isolated candidate.
Actual screenshots must still establish icon recognition, contrast and toolbar
fit, including responsive breakpoint cases, before visual acceptance.


### Glyph coverage (QA-0075 reopen, 2026-11-07)

egui's proportional family is Ubuntu-Light + NotoEmoji + emoji-icon-font; Hack is
monospace-only, so arrows/triangles/dots that only Hack covers (→ ← ↑ ↓ ▲ ▼ ● ◆)
painted missing-glyph squares in ordinary labels. `layout_policy::font_definitions`
appends Hack to the proportional fallback once (`apply_style`). Glyphs no bundled
font covers (✓ ✕ ✗ ⦿ ⁿ) are not used: icon-only buttons use `Icon::Close`, status
text uses ✔/✖. Scan new literals against the proportional set before adding one.

## Divider eligibility observations (QA59)

The existing width owner captures a fixed eligibility value at each existing
prepare/register boundary. Prepare still checks interaction before consuming the
prior Response; register still rechecks after section contents, since a popup or
modal may open during those contents. Its one decision disables the native
surface and supplies the read-only observation. Caller gates retain app, dialog,
close-pending and quarantine facts without exporting dialog contents. The old
boolean setter remains a generic app gate for hierarchy and existing callers.

Opt-in LayoutObservations retain at most two registered divider records per pass.
The record includes the exact egui input focus, Escape/popup/modal flags, primary
state and release latch; it never changes focus, pointer capture or preference.
The closed first blocker explains priority while all flags expose simultaneous
blocks. response_enabled is the actual Response flag, distinct from layer
allowed and keyboard widget focus. An absent record does not diagnose an absent
panel or invalid geometry. No historical cancel log or extra resize writer exists.
The existing width journey checks actual disabled Responses, dialog/quarantine,
close-pending, Escape, focus loss, held-release rearm, same-pass identity and the
two-record bound.
