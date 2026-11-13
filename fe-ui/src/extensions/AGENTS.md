# fe-ui/src/extensions - extension UI slot host

## §slot-host (extension_platform_20261107 P1.8, R10)

Declarative and host-rendered (OD-E8): manifest `ui` entries become `fe_sdk::UiContribution`s in `plugin::UiExtensions`;
this module draws them and the guest draws no pixels.

- **Commands** (ToolbarAction, SidebarTool, ContextMenu): `draw_command_slot` / `draw_context_menu_items` render a button named
  `ext:<package>/<id>` (`UiContribution::accessible_name`) through `ResponseExt::button_named`, so the agent input surface finds it.
  A press only queues an `ExtensionCommandRequest` in `ExtensionSlotState`; the binary drains `take_commands()` and dispatches to the
  package. fe-ui has no fe-plugin dependency.
- **Context menu** entries honour `when` (property present, or equal to a value) against a `SlotContext` over the clicked node:
  `id` and `name` first, then the node's cached property bag (`slots::with_cached_properties`). A press carries
  `{"node_id": <clicked>}` in `ExtensionCommandRequest::args` (R-UI-04); every other slot sends `Null`.
- **Property cache**: `mirror_inspected_node_properties` (PostUpdate, `ExtensionSlotsPlugin`) copies the inspector's loaded
  `node_properties` for the primary selection into `ExtensionSlotState::node_properties` (at most 256 nodes; the cache restarts
  past that). PostUpdate so the inspector sync of this frame has run: a bag is never filed under a newly selected node's id. Only
  nodes that were inspected at least once can match a property-keyed `when`; a node never selected shows only `id`/`name`
  entries. `draw_context_slot` pre-filters with the same layered node as the draw (R-UI turn 3: it used `id`/`name` only, which
  hid every property-keyed entry such as fleet-sim's `start-this`).
- **Property rows** (InspectorSection): read-only rows for the declared keys. The declared property scope is enforced by whoever
  supplies the `SlotContext` (the binary), not here; an absent key shows a dash.
- **Stream values** (DashboardWidget, ViewportOverlay): label plus the latest event of `(plugin_id, stream)`, narrowed by the
  `field` JSON Pointer. The binary feeds `ExtensionSlotState::set_stream_value` from the integration bus.
- **Details only**: contributions with no command, rows or stream (today's terrain labels) are returned by `details_only` for the
  Extensions details pane instead of drawing dead buttons, until R34 binds them.
- **Disable/remove**: the binary calls `UiExtensionRegistry::unregister_all(plugin_id)`, `ExtensionSlotState::forget_plugin` and
  `BoundSlotsState::forget_package`. The slot host also refuses to draw a package the last installed-list snapshot reports not
  Enabled, or that left the list (`ExtensionsPanelState::stopped_packages`), so a missed unregister fails closed.

Slot call sites (owned elsewhere, one line each): `ui_shell::topbar` toolbar (the Tools menu when compact), `panels/sidebar.rs`,
`panels/inspector.rs`, `dialogs/context_menu.rs`, `panels/egress_card.rs`, and the viewport overlay pass.

## §modules (G6, 2026-11-09; mounted by I-UI)

`mod.rs` declares every module: `slots` (P1.8), `bound_slots` (B10c), `bindings` (B9), `install_offer` + `consent` (B7, P2.4),
`panel` + `browse` + `details` + `permissions` (P4, design in `docs/extensions/extensions-ui.md`) and `builtin` (P5.5). All are
compiled and reachable by the `extensions::` test filter.

## §store-flows (I-UI)

Browse, store Details (licence, entitlement, counter-signature) and installed Details are tabs of `panel::draw`
(`PanelView::{Browse, StoreDetails, Details}`), mounted in the Extensions window by `extensions_window_system`; no extra mount
is needed. J9 (`extensions_panel_journey`) drives Browse and Details by accessible name and the bindings snapshot.
`feed_portal_offer_point` (Update, `ExtensionSlotsPlugin`) copies the winning `portal.open` row to
`UiManager::portal_binding_point` for the inspector offer (`fe-ui/src/AGENTS.md` section portal); it only runs when the
panel state changed.

States (R-UI turn 3): Browse reads `Loading extensions` until the first `Refresh` brings the sources (only then is "no sources"
true). A failed detail load is routed by `BrowseState::detail_requested`: the bridge answers `BrowseEvent::Failed` for both a
listing and a detail, so without that marker a refused detail document wiped the result list and Details showed a reason-less
line. Now Details shows `Details are unavailable: <reason>` and Back returns to the intact results. A licensed row in Browse
reads `Licence required`, because Details uses `Licensed` for a held licence.

## §gui-qa-path (R-UI turn 3, for the refine pass)

Run by hand or by the sim's `ext-install-gui` scenario; every step addresses a control by its accessible name (one Owner host,
Extensions window open via `Toggle Extensions panel`). Expected state follows each arrow.

1. **Install from file**: type the packed `.hexon` path into `Package file path`, press `Install from file` -> spinner
   `Verifying package...`, then the consent dialog. The dialog's `Enable <id>` stays disabled until the grant list was drawn;
   press it -> the row `Open <id>` lists `Disabled` (or `Enabled`).
2. **Install from the static index**: `Browse extensions` -> `Loading extensions`, then the source picker (`Extension source`)
   and rows. `Search extensions` + Enter filters; `Sort extensions` re-lists. `Details of <id>` -> publisher, versions,
   `Reviewed <id>` only with a verified counter-signature. `Install <id> <version>` (row or Details) -> consent -> `Enable <id>`.
   A kill-listed row reads `Unavailable: revoked by the registry` and has no control; a licensed one reads `Licence required`
   and installs from Details only after `Check licence for <id>` reports `Licensed`.
3. **Enable / disable**: `Open <id>` -> `Enable <binding> for <id>` -> `Enabled`; `Disable <binding> for <id>` -> `Disabled`.
   Every state-changing button is disabled while the spinner shows.
4. **Update with a delta**: with a newer build in the index, `Update <id> to <version>` -> consent dialog titled for the update
   with the `+ / -` delta; `Update <id>` -> `starting <version>, previous version still runs`, then `updated to <version>`.
   A start failure reads `update failed, previous version kept: <reason>` and the button is live again.
5. **Uninstall**: `Uninstall <id>` -> `Confirm uninstall <id>` / `Keep <id>`; confirm -> back on the list, the row is gone and
   none of its toolbar, sidebar or context entries draw.
6. **Disable all**: with one row `Enabled`, `Disable all extensions` -> `Confirm disable all extensions` / `Keep extensions
   running`; confirm -> every row `Disabled`, and `Enable all extensions` replaces the control (it answers `host-off` while
   `FE_EXT_DISABLE_ALL=1` was set at startup). Same `SetHostSwitch` path as the CLI's `disable-all`, audited `channel: gui`
   (C-EXT closed R-BIN-20; governance.md host-switch row names "the Extensions panel or `fe-ext admin`").
7. **Local admin channel leg**: `fe-ext admin --json disable <installation>` against the running app -> the row reads `Disabled`
   without any GUI click; an install verb over the channel still opens the consent dialog here and waits for its button.
8. **Failure states**: a tampered index shows its refusal verbatim in Browse with `Refresh extension list` live; a refused detail
   shows `Details are unavailable: <reason>` and `Back to extension results` returns to the intact list; `Dismiss extension error`
   clears the red line.
9. **Context menu**: inspect a fleet-sim node once (so its properties load), right-click it -> `ext:org.example.fleet-sim/start-this`
   shows; pressing it closes the menu and sends `{"node_id": <id>}`.

## §host-handle (I-UI, R-UI)

Surfaces (topbar, sidebar, inspector, context menu, egress card) are deep egui functions with long signatures, so the slot data
reaches them through the egui context instead of new parameters: `ExtensionSlotHost` (Arc<Mutex<HostInner>>) is stored in egui
temp data each pass. `plugin.rs` orders three systems around `gardener_ui_system` in `EguiPrimaryContextPass`:
`lend_slots_to_surfaces` swaps the `UiExtensions` registry, `ExtensionSlotState` and `BoundSlotsState` INTO the host (a `mem::swap`,
no clone, change detection bypassed), the surfaces draw, then `extension_overlay_system` / `extensions_window_system` draw,
and `reclaim_slots_from_surfaces` swaps everything back so the binary's Update systems drain `take_commands()` / `take_ops()`
unchanged. A surface that finds no handle draws nothing (tests that call a draw function directly are unaffected).

`lend_slots_to_surfaces` also (a) copies `stopped_packages` only when `rows_generation` moved (no per-frame clone), and (b) on an
active-petal change calls `ExtensionsPanelState::on_scope_changed`: installed rows (their binding id is the open petal) and the
bindings snapshot describe the petal they were read in, so the next draw asks again instead of acting on the old petal.

- **Plain vs panel content**: a contribution whose id is in its package's `ui.panel` `uses` (`BoundSlotsState::set_panel_uses`) is
  panel content. Commands and streams among it draw only inside the bound panel (`draw_bound_panels`) while the binding resolves
  at the active petal; inspector and context entries among it need a node, so they draw in place, but only while bound
  (`HostInner::in_node_slot`). Everything else draws in its declared slot (`in_plain_slot`). Unbound panel content draws nowhere:
  bindings.md section 6 "No extension UI at the scope".
- **Surfaces**: `draw_topbar_controls` + `topbar_toggle` (topbar.rs), `draw_sidebar_slot` (sidebar.rs, after Groups),
  `draw_inspector_slot` (inspector Properties tab, after the property table), `draw_context_slot` (node targets only, no separator
  when no entry matches), `bound_snippet_rows` (egress card), `extension_overlay_system` (stream slots, top-left of the free
  viewport, non-interactive).
- **Extensions window**: opened by the topbar "Extensions" toggle; `panel::draw` runs only while open (so the first
  `Refresh` op is queued on first open). `consent::render` runs every pass.
- **Limits**: the context menu `when` reads node `id`, `name` and the inspected-node cache (section slot-host); the inspector
  rows read the loaded `node_properties`, with declared property-scope filtering left to the binary.

## §bound-slots (B10c, R-UI)

- **Resolution cache**: the binary's feed calls `set_resolution(petal, panels, targets)` from the resolver, `clear()` when no petal
  is open (install-time `panel_uses` survive it), `set_panel_uses` per installed build, `forget_package` on disable, revoke and
  uninstall. With no `panel_uses` recorded a bound panel draws nothing (fail closed), never the whole package.
- **Snippet cache**: the card's arguments `{sql, query_url, export_urls}` key the cache. A row shows only for the arguments it was
  computed for, so changing the export source hides the old snippet at once and asks again without waiting for the 1 s TTL. One
  request per target per arguments per TTL, measured on the UI clock (`egui` input time); `store_snippet` ignores the feed's clock
  because the binary passes Bevy virtual time, which lags real time after a hitch. The binary answers through
  `store_snippet_answer(target, args, ..)` (R-UI-27), which is refused unless `args` are the target's latest request, so a slow
  answer never shows for another query; `store_snippet` (attribute to the latest request) remains for older callers.
- **Rows**: the egress card draws the compiled-in Copy-for-BI targets first, unconditionally (R33), then `bound_rows`. A bound
  row is labelled `<provider label> · <package id>` (first line, control characters stripped, 48 characters) and named
  `binding:egress.snippet:<key>`, so a provider can never pass its row off as a built-in target.

## §bindings-view (B9, R-UI)

- **Names** (bindings.md section 9): the state row is `binding:<hook>[:<key>]`; its one offer is `...:action` whatever the offer
  is (the button text says which). Manager+ controls extend the same name: `:edit`, and in the form `:scope:<petal|fractal|verse>`,
  `:mode:<extension|builtin>`, `:provider`, `:version`, `:field:<name>`, `:save`, `:cancel`, `:clear`.
- **Authority in the UI** (the DB plane still judges every write): the form opens only when `BindingsSnapshot::manages()`
  (Manager+ and at least one writable scope); the scope picker lists only `scopes`, the scopes the bridge says the local DID may
  write at; a form keeps its scope as a target, never an index, so a refreshed snapshot cannot move a write to another scope; Remove
  shows only when the winning row sits at a writable scope; an open form closes when a snapshot says the DID no longer manages.
  The Override offer shows only to the verse Owner (`local_is_owner`); Edit binding only to Manager+. Every other offer is a
  local-DID action (bindings.md section 5.2) and the bridge attaches it only when it can act (Install and Update need a source).
- **Layout**: one row per point (title, state), one detail line (scope, package, range), then at most the offer and Edit. Remove
  lives in the form, beside Save, with the scope in its label.
- **States**: loading (`Loading bindings`), failed or not loaded (a line plus `Refresh bindings`, never a dead end), empty
  (`No bindings at this petal`), busy (every write button disabled while the bridge works or consent is open).
- **Gap**: adding a binding for a point with no row yet is REST/MCP-only (deferred; see R-UI review).

## §consent (B7, P2.4)

`ConsentState` holds one pending `ConsentRequest`; `render` draws it every pass. Enable stays disabled until the full grant list
was drawn once and while a refusal or kill-list entry stands. A re-prompt shows the `+ / -` grant delta against the consented
version. A kill-listed build never enables from the dialog: the verse Owner types the exact package id to send
`ConsentDecision::KillListOverride`, and `panel::route_consent` refuses a mismatched confirmation again before any op is queued.
An install offer's approval carries the verse id (`accept_in_verse`), which records local acceptance.

## §install-offer (B7)

`install_offer.rs` decides the single action beside an unresolved binding (Install, Allow in this verse, Override) from the
host's facts (`policy_admits`, `registry_has_build`, `local_is_owner`), never auto-installs, and remembers local dismissals per
binding version. It builds the consent request with `ConsentOrigin::InstallOffer`.
