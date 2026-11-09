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
