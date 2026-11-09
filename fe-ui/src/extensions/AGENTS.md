# fe-ui/src/extensions - extension UI slot host

## §slot-host (extension_platform_20261107 P1.8, R10)

Declarative and host-rendered (OD-E8): manifest `ui` entries become `fe_sdk::UiContribution`s in `plugin::UiExtensions`;
this module draws them and the guest draws no pixels.

- **Commands** (ToolbarAction, SidebarTool, ContextMenu): `draw_command_slot` / `draw_context_menu_items` render a button named
  `ext:<package>/<id>` (`UiContribution::accessible_name`) through `ResponseExt::button_named`, so the agent input surface finds it.
  A press only queues an `ExtensionCommandRequest` in `ExtensionSlotState`; the binary drains `take_commands()` and dispatches to the
  package. fe-ui has no fe-plugin dependency.
- **Context menu** entries honour `when` (property present, or equal to a value) against a `SlotContext` over the clicked node.
  `ExtensionCommandRequest` carries no args yet, so the manifest's `{"node_id"}` for a ContextMenu command cannot reach the
  package (handed off; see R-UI review).
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
- **Limits**: the context menu `when` can read only node `id` and `name` (no per-node property cache in the menu); the inspector
  rows read the loaded `node_properties`, with declared property-scope filtering left to the binary (see section slot-host).

## §bound-slots (B10c, R-UI)

- **Resolution cache**: the binary's feed calls `set_resolution(petal, panels, targets)` from the resolver, `clear()` when no petal
  is open (install-time `panel_uses` survive it), `set_panel_uses` per installed build, `forget_package` on disable, revoke and
  uninstall. With no `panel_uses` recorded a bound panel draws nothing (fail closed), never the whole package.
- **Snippet cache**: the card's arguments `{sql, query_url, export_urls}` key the cache. A row shows only for the arguments it was
  computed for, so changing the export source hides the old snippet at once and asks again without waiting for the 1 s TTL. One
  request per target per arguments per TTL, measured on the UI clock (`egui` input time); `store_snippet` ignores the feed's clock
  because the binary passes Bevy virtual time, which lags real time after a hitch. An answer is attributed to the latest request
  of its target, so a provider answer still in flight when the query changes can show for at most one provider deadline (1 s);
  carrying the request's arguments in the feed's answer would close that (handed off).
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
