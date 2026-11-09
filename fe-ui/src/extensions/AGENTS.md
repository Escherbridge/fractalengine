# fe-ui/src/extensions - extension UI slot host

## §slot-host (extension_platform_20261107 P1.8, R10)

Declarative and host-rendered (OD-E8): manifest `ui` entries become `fe_sdk::UiContribution`s in `plugin::UiExtensions`;
this module draws them and the guest draws no pixels.

- **Commands** (ToolbarAction, SidebarTool, ContextMenu): `draw_command_slot` / `draw_context_menu_items` render a button named
  `ext:<package>/<id>` (`UiContribution::accessible_name`) through `ResponseExt::button_named`, so the agent input surface finds it.
  A press only queues an `ExtensionCommandRequest` in `ExtensionSlotState`; the binary drains `take_commands()` and dispatches to the
  package. fe-ui has no fe-plugin dependency.
- **Context menu** entries honour `when` (property present, or equal to a value) against a `SlotContext` over the clicked node.
- **Property rows** (InspectorSection): read-only rows for the declared keys. The declared property scope is enforced by whoever
  supplies the `SlotContext` (the binary), not here; an absent key shows a dash.
- **Stream values** (DashboardWidget, ViewportOverlay): label plus the latest event of `(plugin_id, stream)`, narrowed by the
  `field` JSON Pointer. The binary feeds `ExtensionSlotState::set_stream_value` from the integration bus.
- **Details only**: contributions with no command, rows or stream (today's terrain labels) are returned by `details_only` for the
  Extensions details pane instead of drawing dead buttons, until R34 binds them.
- **Disable/remove**: call `UiExtensionRegistry::unregister_all(plugin_id)` and `ExtensionSlotState::forget_plugin(plugin_id)`; the
  contributions vanish on the next frame because every slot reads the registry each frame.

Slot call sites (owned elsewhere, one line each): `ui_shell::topbar` toolbar, `panels/sidebar.rs`, `panels/inspector.rs`,
`dialogs/context_menu.rs`, the viewport overlay pass, and the dashboard.

## §modules (G6, 2026-11-09; mounted by I-UI)

`mod.rs` declares every module: `slots` (P1.8), `bound_slots` (B10c), `bindings` + `install_offer` (B9), `panel` + `browse` +
`details` + `permissions` (P4, design in `docs/extensions/extensions-ui.md`), `consent` (P2.4) and `builtin` (P5.5). All are
compiled and reachable by the `extensions::` test filter.

## §host-handle (I-UI)

Surfaces (topbar, sidebar, inspector, context menu, egress card) are deep egui functions with long signatures, so the slot data
reaches them through the egui context instead of new parameters: `ExtensionSlotHost` (Arc<Mutex<HostInner>>) is stored in egui
temp data each pass. `plugin.rs` orders three systems around `gardener_ui_system` in `EguiPrimaryContextPass`:
`lend_slots_to_surfaces` swaps the `UiExtensions` registry, `ExtensionSlotState` and `BoundSlotsState` INTO the host (a `mem::swap`,
no clone, change detection bypassed), the surfaces draw, then `extension_overlay_system` / `extensions_window_system` draw,
and `reclaim_slots_from_surfaces` swaps everything back so the binary's Update systems drain `take_commands()` / `take_ops()`
unchanged. A surface that finds no handle draws nothing (tests that call a draw function directly are unaffected).

- **Bound vs unbound**: a package with a bound `ui.panel` at the active petal renders only inside that panel
  (`draw_bound_panels`); its toolbar/sidebar/overlay contributions are skipped elsewhere. Unbound packages (the built-in terrain
  extension) render in the plain slots.
- **Surfaces**: `draw_topbar_controls` + `topbar_toggle` (topbar.rs), `draw_sidebar_slot` (sidebar.rs, after Groups),
  `draw_inspector_slot` (inspector Properties tab, after the property table), `draw_context_slot` (node targets only),
  `bound_snippet_rows` (egress card), `extension_overlay_system` (stream slots, top-left of the free viewport, non-interactive).
- **Extensions window**: opened by the topbar "Extensions" toggle; `panel::draw` runs only while open (so the first
  `Refresh` op is queued on first open). `consent::render` runs every pass.
- **Limits**: the context menu `when` can read only node `id` and `name` (no per-node property cache in the menu); the inspector
  rows read the loaded `node_properties`, with declared property-scope filtering left to the binary (see section slot-host).
