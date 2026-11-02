---
type: FeatureSourceInventory
date: 2026-11-02
source_commit: 44ad28da5ac33120c988bd26a8e8a79362dd15d4
execution_status: NOT_RUN
---

# Source-to-feature reconciliation

This is an audited source census, not execution evidence. The final staged fingerprint remains pending main integration. Stable checkpoint IDs and retirement reasons live in feature-checkpoints.json. Reproduce with the command in tools/qa/AGENTS.md.

Lexical producer candidates, excluding literal cfg(test) items while preserving line numbers. Helpers/named wrappers can repeat one control; conditional production openers require manual reconciliation. Not a Rust parser, live-tree census or proof of execution.

Counts: 84 UiAction variants; 678 lexical control/naming candidates; 49 MCP tools; 103 REST method/path cases. Full candidate lines and normalized-source hashes are retained in catalog-source-census.json.

Candidates map to owning capability families; this is routing responsibility, not an automatic assertion that every branch is covered. A shared helper is not an extra control. Every current mandatory checkpoint still requires current-pass evidence and independent assertions.

## UiAction producer ownership

| Action | Families | Current render/gesture producers | Note |
|---|---|---|---|
| `OpenPortal` | F024, F025 | `fe-ui/src/panels/inspector.rs:717` |  |
| `ClosePortal` | F025 | `fe-ui/src/panels/portal_toolbar.rs:59` |  |
| `PortalGoBack` | F025 | `fe-ui/src/panels/portal_toolbar.rs:27` |  |
| `PortalGoForward` | F025 | `fe-ui/src/panels/portal_toolbar.rs:28` |  |
| `PortalReload` | F025 | `fe-ui/src/panels/portal_toolbar.rs:29` |  |
| `PortalOpenExternal` | F025 | `fe-ui/src/panels/portal_toolbar.rs:52` |  |
| `SaveNodeUrl` | F024 | `fe-ui/src/dialogs/node_options.rs:139`, `fe-ui/src/panels/inspector.rs:701` |  |
| `ApplyNodeTransform` | F018 | `fe-ui/src/panels/inspector.rs:602` |  |
| `CreateNodeAt` | F020 | `fe-ui/src/dialogs/context_menu.rs:726` |  |
| `SubmitQuery` | F033, F034 | `fe-ui/src/panels/query_tab.rs:184` |  |
| `LoadNodeProperties` | F021, F030 | `fe-ui/src/dialogs/context_menu.rs:231`, `fe-ui/src/panels/inspector.rs:1986` |  |
| `SetNodeProperty` | F030, F032 | `fe-ui/src/panels/annotation_card.rs:173`, `fe-ui/src/panels/inspector.rs:1609`, `fe-ui/src/panels/inspector.rs:1637` |  |
| `DeleteNodeProperty` | F030, F032 | `fe-ui/src/panels/annotation_card.rs:168`, `fe-ui/src/panels/inspector.rs:2019` |  |
| `HexonInstallFromFile` | F120 | `fe-ui/src/dialogs/hexon_manager.rs:115` |  |
| `HexonRemoveTileset` | F123 | `fe-ui/src/dialogs/hexon_manager.rs:409` |  |
| `HexonToggleSeeding` | F123 | `fe-ui/src/dialogs/hexon_manager.rs:320` |  |
| `HexonStartDownload` | F122 | `fe-ui/src/dialogs/hexon_manager.rs:612` |  |
| `HexonCancelDownload` | F122 | `fe-ui/src/dialogs/hexon_manager.rs:662` |  |
| `HexonRefreshList` | F119 | `fe-ui/src/dialogs/hexon_manager.rs:103`, `fe-ui/src/ui_shell/right_sidebar.rs:1308` |  |
| `HexonOpenStorageDir` | F123 | `fe-ui/src/dialogs/hexon_manager.rs:226` |  |
| `HexonInstallFromRegistry` | F121 | `fe-ui/src/dialogs/hexon_manager.rs:146` |  |
| `PetalSetMap` | F047 | `fe-ui/src/dialogs/hexon_manager.rs:364`, `fe-ui/src/dialogs/hexon_manager.rs:381` |  |
| `PetalSetMapScale` | F048 | `fe-ui/src/dialogs/hexon_manager.rs:499`, `fe-ui/src/dialogs/hexon_manager.rs:522` |  |
| `PetalManifestSave` | F124 | `fe-ui/src/dialogs/petal_manifest.rs:281` | Legacy intent is refused; canonical requirements controller uses correlated typed DB commands and F124 owns its rendered Save. |
| `PetalSetRenderOverride` | F061 | No render/gesture reference in census | Legacy action enum has no current render/gesture producer; the Settings session-override surface owns F061 through its current resource path. No separate GUI action is claimed. |
| `PetalManifestOpen` | F124 | `fe-ui/src/panels/sidebar.rs:532` |  |
| `DownloadNodeAsset` | F026 | `fe-ui/src/panels/asset_card.rs:77` |  |
| `GisQueryAnnotated` | F040, F042 | `fe-ui/src/panels/gis_panel.rs:409` |  |
| `GisRefreshAnnotations` | F039 | `fe-ui/src/panels/gis_panel.rs:211` |  |
| `GisQueryPropertyFilter` | F040 | `fe-ui/src/panels/gis_panel.rs:423` |  |
| `GisSetLayer` | F049 | `fe-ui/src/gis/layers.rs:121`, `fe-ui/src/panels/layer_manager_card.rs:154` |  |
| `GisSetViewMode` | F049 | `fe-ui/src/panels/layer_manager_card.rs:100` |  |
| `GpxImportFile` | F064 | `fe-ui/src/panels/gpx_import_card.rs:45` |  |
| `PathQueryTracks` | F063 | `fe-ui/src/node_manager/pointer/mod.rs:211`, `fe-ui/src/panels/gis_panel.rs:129`, `fe-ui/src/panels/path_editor_card.rs:193`, `fe-ui/src/panels/path_editor_card.rs:335`, `fe-ui/src/panels/tool_options.rs:175` |  |
| `PathSelectTrack` | F063, F077 | `fe-ui/src/dialogs/context_menu.rs:853`, `fe-ui/src/dialogs/context_menu.rs:866`, `fe-ui/src/node_manager/pointer/mod.rs:79`, `fe-ui/src/panels/path_editor_card.rs:306`, `fe-ui/src/panels/tool_options.rs:200` |  |
| `PathCreateTrack` | F063, F065 | `fe-ui/src/node_manager/path_point_interaction.rs:518`, `fe-ui/src/panels/path_editor_card.rs:159` |  |
| `PathDeleteTrack` | F063, F077 | `fe-ui/src/panels/path_editor_card.rs:312` |  |
| `PathAppendPoint` | F065 | `fe-ui/src/node_manager/path_point_interaction.rs:489`, `fe-ui/src/verse_manager/db_results/nodes.rs:114` |  |
| `PathAppendSmoothPoint` | F065 | `fe-ui/src/node_manager/path_point_interaction.rs:479`, `fe-ui/src/verse_manager/db_results/nodes.rs:105` |  |
| `PathSetAnchorHandles` | F067, F069 | `fe-ui/src/node_manager/path_handle_interaction.rs:353` |  |
| `PathSetAnchorMetadata` | F139 | `fe-ui/src/panels/tool_options.rs:325` |  |
| `PathSetAnchorCorner` | F067, F077 | `fe-ui/src/dialogs/context_menu.rs:273`, `fe-ui/src/dialogs/context_menu.rs:906`, `fe-ui/src/node_manager/path_handle_interaction.rs:362` |  |
| `ContextSetAnchorCorner` | F139 | `fe-ui/src/dialogs/context_menu.rs:907` |  |
| `PathRemovePoint` | F069, F077 | `fe-ui/src/dialogs/context_menu.rs:899`, `fe-ui/src/panels/path_editor_card.rs:602` |  |
| `PathMovePointFromGesture` | F139 | `fe-ui/src/node_manager/path_gimbal_drag.rs:154`, `fe-ui/src/node_manager/path_point_interaction.rs:749` |  |
| `PathMovePointUntracked` | F139 | `fe-ui/src/node_manager/path_gimbal_drag.rs:161` | Internal legacy adapter, not a new standalone GUI control. |
| `PathMovePoint` | F069 | `fe-ui/src/panels/path_editor_card.rs:613` |  |
| `PathTransformPoints` | F072 | `fe-ui/src/node_manager/transform_broadcast.rs:56` |  |
| `PathAnnotatePoint` | F070 | `fe-ui/src/panels/path_editor_card.rs:1231` |  |
| `PathExportGpx` | F078 | `fe-ui/src/panels/path_editor_card.rs:364` |  |
| `PathSetStyle` | F071 | `fe-ui/src/panels/path_editor_card.rs:380` |  |
| `PathAssetApply` | F074 | `fe-ui/src/panels/tool_panel.rs:472` |  |
| `PathSmoothCurrent` | F066 | `fe-ui/src/panels/tool_panel.rs:1005` |  |
| `PathAppendShape` | F068 | `fe-ui/src/panels/tool_panel.rs:1066`, `fe-ui/src/panels/tool_panel.rs:1079`, `fe-ui/src/panels/tool_panel.rs:1092` |  |
| `TerrainProposalAdd` | F050, F051 | `fe-ui/src/node_manager/terrain_authoring.rs:96`, `fe-ui/src/node_manager/terrain_authoring.rs:354` |  |
| `CommitTerrainAuthoring` | F053 | `fe-ui/src/node_manager/brush_interaction.rs:193`, `fe-ui/src/node_manager/brush_interaction.rs:214`, `fe-ui/src/node_manager/brush_interaction.rs:312` |  |
| `TerrainProposalDelete` | F051 | `fe-ui/src/dialogs/context_menu.rs:351`, `fe-ui/src/panels/terrain_tools_panel.rs:315` |  |
| `TerrainProposalSelect` | F051 | `fe-ui/src/dialogs/context_menu.rs:292` |  |
| `OpenTerrainReport` | F055 | `fe-ui/src/dialogs/context_menu.rs:313`, `fe-ui/src/panels/volume_report_panel.rs:237` |  |
| `SelectStamp` | F075, F138 | `fe-ui/src/panels/tool_panel.rs:629` | Options one-based Select emits a private live scene/source origin; lower-tier viewport and fresh context picks enter the same NodeManager controller without inventing UiAction references. F10 path pick priority remains. No unchecked current-index GUI mutation. |
| `InspectStampNode` | F075, F138 | `fe-ui/src/panels/tool_panel.rs:698` |  |
| `CheckStampSave` | F066, F067, F068, F073, F074, F075, F138 | `fe-ui/src/panels/tool_panel.rs:521` |  |
| `DownloadStampProperties` | F138 | `fe-ui/src/panels/tool_panel.rs:692` |  |
| `ApplyStampOverrides` | F138 | `fe-ui/src/panels/tool_panel.rs:688` |  |
| `SetStampScale` | F075 | No render/gesture reference in census | Legacy per-field adapter has no current render constructor; actual selected-stamp scale uses atomic ApplyStampOverrides (F138). |
| `SetStampRotation` | F075 | No render/gesture reference in census | Legacy per-field adapter has no current render constructor; actual yaw/quaternion-preserving editor uses atomic ApplyStampOverrides (F138). |
| `SlideStampAlongPath` | F075 | No render/gesture reference in census | Legacy per-field adapter has no current render constructor; current metric slide uses atomic ApplyStampOverrides (F138). |
| `SculptBrushStroke` | F052 | `fe-ui/src/node_manager/brush_interaction.rs:145`, `fe-ui/src/node_manager/terrain_authoring.rs:114` |  |
| `SculptShapeRegion` | F053, F054 | `fe-ui/src/node_manager/terrain_authoring.rs:102`, `fe-ui/src/node_manager/terrain_authoring.rs:374`, `fe-ui/src/node_manager/terrain_authoring.rs:432` |  |
| `SculptDeleteRegion` | F057 | `fe-ui/src/dialogs/context_menu.rs:353` |  |
| `SaveMeasureAsCallout` | F059 | `fe-ui/src/panels/tool_options.rs:608` |  |
| `DeleteNode` | F012, F022, F076, F077 | `fe-ui/src/dialogs/context_menu.rs:223`, `fe-ui/src/dialogs/context_menu.rs:998`, `fe-ui/src/dialogs/node_options.rs:316` |  |
| `DuplicateNode` | F021 | `fe-ui/src/dialogs/context_menu.rs:227` |  |
| `RenameNode` | F012, F021, F023 | No render/gesture reference in census | Semantic rename also routes directly through Node Options/Entity Details; absence of a UiAction constructor does not retire rename. |
| `PromoteStamp` | F075, F076 | `fe-ui/src/dialogs/context_menu.rs:841` | Actual context Promote carries the selected private scene/source origin into the same checked promotion controller. Public legacy PromoteInstance API remains a separate caller-current-index contract. |
| `ClearNodeProperties` | F022, F023 | `fe-ui/src/dialogs/context_menu.rs:228` |  |
| `CopyApiString` | F027 | `fe-ui/src/dialogs/context_menu.rs:229` |  |
| `ReportObject` | F027 | `fe-ui/src/dialogs/context_menu.rs:230` |  |
| `RevealSection` | F082 | `fe-ui/src/dialogs/context_menu.rs:293`, `fe-ui/src/panels/sidebar.rs:621`, `fe-ui/src/panels/sidebar.rs:886`, `fe-ui/src/panels/tool_options.rs:622` |  |
| `SetNodeView` | F008, F009 | `fe-ui/src/node_manager/shortcuts.rs:205`, `fe-ui/src/panels/sidebar.rs:767`, `fe-ui/src/visibility/mod.rs:463`, `fe-ui/src/visibility/mod.rs:479` |  |
| `SetGroupRegistry` | F010 | `fe-ui/src/gis/layers.rs:127`, `fe-ui/src/panels/sidebar.rs:869`, `fe-ui/src/ui_shell/right_sidebar.rs:1428` |  |
| `SetScopeHidden` | F008 | `fe-ui/src/panels/sidebar.rs:578`, `fe-ui/src/visibility/mod.rs:450` |  |
| `SetNodeGroups` | F011 | `fe-ui/src/ui_shell/right_sidebar.rs:1602` |  |
| `SetAppOverlay` | F060 | `fe-ui/src/gis/layers.rs:137` |  |

## Current control source owners

| Production source | Families | Lexical candidates |
|---|---|---|
| `fe-ui/src/atlas/tag_panel.rs` | F127 | 4 |
| `fe-ui/src/atlas/visibility_control.rs` | F094 | 3 |
| `fe-ui/src/dialogs/context_menu.rs` | F020, F021, F022, F027, F076, F077, F139 | 7 |
| `fe-ui/src/dialogs/create_entity.rs` | F005, F089 | 2 |
| `fe-ui/src/dialogs/entity_access.rs` | F096, F097, F098, F102, F103 | 39 |
| `fe-ui/src/dialogs/entity_settings.rs` | F012, F089, F093, F094, F095, F098, F102, F103 | 21 |
| `fe-ui/src/dialogs/gltf_import.rs` | F006, F019, F089 | 5 |
| `fe-ui/src/dialogs/hexon_manager.rs` | F047, F048, F089, F119, F120, F121, F122, F123 | 27 |
| `fe-ui/src/dialogs/join.rs` | F099 | 5 |
| `fe-ui/src/dialogs/node_options.rs` | F012, F013, F021, F022, F023, F024, F089 | 11 |
| `fe-ui/src/dialogs/peer_debug.rs` | F104 | 1 |
| `fe-ui/src/dialogs/petal_manifest.rs` | F089, F124 | 24 |
| `fe-ui/src/object_data/history.rs` | F134 | 12 |
| `fe-ui/src/object_data/measurements.rs` | F135 | 7 |
| `fe-ui/src/object_data/mod.rs` | F134, F135, F136 | 6 |
| `fe-ui/src/object_data/schema.rs` | F136 | 14 |
| `fe-ui/src/panels/annotation_card.rs` | F032 | 7 |
| `fe-ui/src/panels/asset_card.rs` | F026 | 3 |
| `fe-ui/src/panels/egress_card.rs` | F027, F043, F044, F045 | 10 |
| `fe-ui/src/panels/gis_panel.rs` | F039, F040, F041, F042 | 23 |
| `fe-ui/src/panels/gpx_import_card.rs` | F064, F125 | 2 |
| `fe-ui/src/panels/inspector.rs` | F018, F024, F029, F030, F031, F100, F101, F134, F135, F136, F137 | 89 |
| `fe-ui/src/panels/layer_manager_card.rs` | F049 | 4 |
| `fe-ui/src/panels/mod.rs` | F087, F089 | 3 |
| `fe-ui/src/panels/path_editor_card.rs` | F063, F069, F070, F071, F072, F078, F139 | 48 |
| `fe-ui/src/panels/portal_toolbar.rs` | F025 | 7 |
| `fe-ui/src/panels/query_tab.rs` | F033, F034 | 5 |
| `fe-ui/src/panels/sidebar.rs` | F001, F002, F003, F004, F005, F006, F007, F008, F015 | 20 |
| `fe-ui/src/panels/status_bar.rs` | F087, F104 | 4 |
| `fe-ui/src/panels/terrain_tools_panel.rs` | F050, F051, F052, F057 | 28 |
| `fe-ui/src/panels/tool_options.rs` | F053, F054, F058, F059, F065, F139 | 25 |
| `fe-ui/src/panels/tool_panel.rs` | F066, F067, F068, F073, F074, F075, F138 | 59 |
| `fe-ui/src/panels/transform_options.rs` | F028 | 16 |
| `fe-ui/src/panels/tree_appearance.rs` | F133 | 18 |
| `fe-ui/src/panels/tree_entry.rs` | F001, F002, F003, F007, F008, F013, F133 | 13 |
| `fe-ui/src/panels/volume_report_panel.rs` | F054, F055, F056 | 7 |
| `fe-ui/src/panels/widgets.rs` | F089 | 11 |
| `fe-ui/src/role_chip.rs` | F087 | 1 |
| `fe-ui/src/ui_shell/layout_policy.rs` | F089 | 2 |
| `fe-ui/src/ui_shell/results_dock.rs` | F035, F036, F037, F038 | 12 |
| `fe-ui/src/ui_shell/right_sidebar.rs` | F010, F011, F060, F061, F082, F083, F084, F085, F086 | 43 |
| `fe-ui/src/ui_shell/topbar.rs` | F015, F079, F080, F081, F082, F090, F092 | 21 |
| `fe-ui/src/viewport.rs` | F001, F005, F089, F093 | 8 |
| `fractalengine/src/terminal_reset/presentation.rs` | F086, F089 | 1 |

## MCP cases (J09/F116)

Every available tool requires actual invocation, independent readback, role/scope denial and REST parity where applicable. Feature flags/operator opt-ins are prerequisites; tools/list presence is not PASS.

| Tool | Role | Scope rule | Checkpoint | Source |
|---|---|---|---|---|
| `get_hierarchy` | Viewer | ` ScopeRule::None` | `F116.mcp-get-hierarchy` | `fe-api/src/mcp/mod.rs:213` |
| `create_verse` | Manager | ` Global` | `F116.mcp-create-verse` | `fe-api/src/mcp/mod.rs:215` |
| `create_fractal` | Editor | ` HierarchyArgs(Verse)` | `F116.mcp-create-fractal` | `fe-api/src/mcp/mod.rs:217` |
| `create_petal` | Editor | ` HierarchyArgs(Fractal)` | `F116.mcp-create-petal` | `fe-api/src/mcp/mod.rs:219` |
| `create_node` | Editor | ` HierarchyArgs(Petal)` | `F116.mcp-create-node` | `fe-api/src/mcp/mod.rs:221` |
| `update_transform` | Editor | ` NodeArg("node_id")` | `F116.mcp-update-transform` | `fe-api/src/mcp/mod.rs:223` |
| `read_node` | Viewer | ` NodeArg("node_id")` | `F116.mcp-read-node` | `fe-api/src/mcp/mod.rs:225` |
| `get_node_history` | Viewer | ` HistoryNodeArg("node_id")` | `F116.mcp-get-node-history` | `fe-api/src/mcp/mod.rs:227` |
| `read_node_at_version` | Viewer | ` HistoryNodeArg("node_id")` | `F116.mcp-read-node-at-version` | `fe-api/src/mcp/mod.rs:229` |
| `commit_node_mutation` | Editor | ` NodeArg("node_id")` | `F116.mcp-commit-node-mutation` | `fe-api/src/mcp/mod.rs:231` |
| `node_address` | Viewer | ` NodeArg("node_id")` | `F116.mcp-node-address` | `fe-api/src/mcp/mod.rs:233` |
| `delete_node` | Editor | ` NodeArg("node_id")` | `F116.mcp-delete-node` | `fe-api/src/mcp/mod.rs:235` |
| `promote_instance` | Editor | ` PetalArg("petal_id")` | `F116.mcp-promote-instance` | `fe-api/src/mcp/mod.rs:237` |
| `query_timeseries` | Viewer | ` PetalArg("petal_id")` | `F116.mcp-query-timeseries` | `fe-api/src/mcp/mod.rs:239` |
| `sim_start` | Owner | ` Global, requires SimControl` | `F116.mcp-sim-start` | `fe-api/src/mcp/mod.rs:241` |
| `sim_stop` | Owner | ` Global, requires SimControl` | `F116.mcp-sim-stop` | `fe-api/src/mcp/mod.rs:243` |
| `sim_status` | Owner | ` Global, requires SimControl` | `F116.mcp-sim-status` | `fe-api/src/mcp/mod.rs:245` |
| `sim_step` | Owner | ` Global, requires SimControl` | `F116.mcp-sim-step` | `fe-api/src/mcp/mod.rs:247` |
| `sim_inject_fault` | Owner | ` Global, requires SimControl` | `F116.mcp-sim-inject-fault` | `fe-api/src/mcp/mod.rs:249` |
| `upload_asset` | Editor | ` PetalArg("petal_id")` | `F116.mcp-upload-asset` | `fe-api/src/mcp/mod.rs:252` |
| `place_asset` | Editor | ` PetalArg("petal_id")` | `F116.mcp-place-asset` | `fe-api/src/mcp/mod.rs:254` |
| `set_property` | Editor | ` NodeArg("node_id")` | `F116.mcp-set-property` | `fe-api/src/mcp/mod.rs:256` |
| `get_properties` | Viewer | ` NodeArg("node_id")` | `F116.mcp-get-properties` | `fe-api/src/mcp/mod.rs:258` |
| `get_field_schema` | Viewer | ` FieldDefArg("field_def_id")` | `F116.mcp-get-field-schema` | `fe-api/src/mcp/mod.rs:260` |
| `set_field_schema` | Manager | ` FieldDefArg("field_def_id")` | `F116.mcp-set-field-schema` | `fe-api/src/mcp/mod.rs:262` |
| `delete_property` | Editor | ` NodeArg("node_id")` | `F116.mcp-delete-property` | `fe-api/src/mcp/mod.rs:264` |
| `create_waypoint` | Editor | ` PetalArg("petal_id")` | `F116.mcp-create-waypoint` | `fe-api/src/mcp/mod.rs:266` |
| `move_waypoint` | Editor | ` NodeArg("node_id")` | `F116.mcp-move-waypoint` | `fe-api/src/mcp/mod.rs:268` |
| `import_gpx` | Editor | ` PetalArg("petal_id")` | `F116.mcp-import-gpx` | `fe-api/src/mcp/mod.rs:270` |
| `set_petal_terrain` | Editor | ` PetalArg("petal_id")` | `F116.mcp-set-petal-terrain` | `fe-api/src/mcp/mod.rs:272` |
| `set_petal_render_distance` | Editor | ` PetalArg("petal_id")` | `F116.mcp-set-petal-render-distance` | `fe-api/src/mcp/mod.rs:274` |
| `list_visibility_groups` | Viewer | ` PetalArg("petal_id")` | `F116.mcp-list-visibility-groups` | `fe-api/src/mcp/mod.rs:276` |
| `set_visibility_group` | Editor | ` PetalArg("petal_id")` | `F116.mcp-set-visibility-group` | `fe-api/src/mcp/mod.rs:278` |
| `list_tilesets` | Viewer | ` PetalArg("petal_id")` | `F116.mcp-list-tilesets` | `fe-api/src/mcp/mod.rs:280` |
| `install_tileset` | Editor | ` PetalArg("petal_id")` | `F116.mcp-install-tileset` | `fe-api/src/mcp/mod.rs:282` |
| `get_gis_nodes` | Viewer | ` PetalArg("petal_id")` | `F116.mcp-get-gis-nodes` | `fe-api/src/mcp/mod.rs:284` |
| `query` | Viewer | ` ScopeRule::None` | `F116.mcp-query` | `fe-api/src/mcp/mod.rs:286` |
| `register_device_format` | Manager | ` VerseArg("verse_id")` | `F116.mcp-register-device-format` | `fe-api/src/mcp/mod.rs:289` |
| `list_device_formats` | Viewer | ` VerseArg("verse_id")` | `F116.mcp-list-device-formats` | `fe-api/src/mcp/mod.rs:291` |
| `ingest_device_payload` | Editor | ` PetalArg("petal_id")` | `F116.mcp-ingest-device-payload` | `fe-api/src/mcp/mod.rs:293` |
| `list_device_payloads` | Viewer | ` PetalArg("petal_id")` | `F116.mcp-list-device-payloads` | `fe-api/src/mcp/mod.rs:295` |
| `screenshot` | Owner | ` Global, requires Screenshot, content` | `F116.mcp-screenshot` | `fe-api/src/mcp/mod.rs:298` |
| `get_camera` | Owner | ` Global, requires Screenshot` | `F116.mcp-get-camera` | `fe-api/src/mcp/mod.rs:300` |
| `ui_tree` | Owner | ` Global, requires QaDebug, "Read the primary window accessibility tree in physical screenshot pixels. Scoped to the active verse; FE_API_INPUT_ALLOW=1 required. Requires owner role.")` | `F116.mcp-ui-tree` | `fe-api/src/mcp/mod.rs:305` |
| `dialog_answers` | Owner | ` Global, requires QaDebug, "Queue bounded open/save/folder answers for QA file dialogs in the active verse. Requires input opt-in, an actual loopback listener and an isolated QA root; paths stay inside that run. Requires owner role.")` | `F116.mcp-dialog-answers` | `fe-api/src/mcp/mod.rs:306` |
| `window` | Owner | ` Global, requires QaDebug, "Request QA window size in physical pixels, app UI scale, maximize/restore/minimize, or close confirmation. Requires input opt-in, loopback listener and isolated QA root. Close never bypasses confirmation. Requires owner role.")` | `F116.mcp-window` | `fe-api/src/mcp/mod.rs:307` |
| `ui_state` | Owner | ` Global, requires QaDebug, "Read authoritative navigation, selection, tool, viewport, notices and pending state. FE_API_INPUT_ALLOW=1 required. Requires owner role.")` | `F116.mcp-ui-state` | `fe-api/src/mcp/mod.rs:308` |
| `ui_logs` | Owner | ` Global, requires QaDebug, "Read a bounded, redacted log tail for the current verse generation. FE_API_INPUT_ALLOW=1 required. Requires owner role.")` | `F116.mcp-ui-logs` | `fe-api/src/mcp/mod.rs:309` |
| `inject_input` | Owner | ` Global, requires Input` | `F116.mcp-inject-input` | `fe-api/src/mcp/mod.rs:310` |

## Registered REST method/path cases (J09/F116)

| Method | Path | Checkpoint | Source |
|---|---|---|---|
| GET | `/api/v1/health` | `F116.rest-get-api-v1-health` | `fe-api/src/server.rs:121` |
| GET | `/ready` | `F116.rest-get-ready` | `fe-api/src/server.rs:125` |
| GET | `/ws` | `F116.rest-get-ws` | `fe-api/src/server.rs:127` |
| GET | `/api/v1/shared/{token}` | `F116.rest-get-api-v1-shared-token` | `fe-api/src/server.rs:130` |
| GET | `/api/v1/hierarchy` | `F116.rest-get-api-v1-hierarchy` | `fe-api/src/server.rs:138` |
| POST | `/api/v1/verses` | `F116.rest-post-api-v1-verses` | `fe-api/src/server.rs:139` |
| POST | `/api/v1/verses/{verse_id}/fractals` | `F116.rest-post-api-v1-verses-verse-id-fractals` | `fe-api/src/server.rs:141` |
| POST | `/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals` | `F116.rest-post-api-v1-verses-verse-id-fractals-fractal-id-petals` | `fe-api/src/server.rs:146` |
| POST | `/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals/{petal_id}/nodes` | `F116.rest-post-api-v1-verses-verse-id-fractals-fractal-id-petals-petal-id-nodes` | `fe-api/src/server.rs:151` |
| GET | `/api/v1/nodes/{node_id}` | `F116.rest-get-api-v1-nodes-node-id` | `fe-api/src/server.rs:157` |
| DELETE | `/api/v1/nodes/{node_id}` | `F116.rest-delete-api-v1-nodes-node-id` | `fe-api/src/server.rs:157` |
| GET | `/api/v1/nodes/{node_id}/address` | `F116.rest-get-api-v1-nodes-node-id-address` | `fe-api/src/server.rs:162` |
| GET | `/api/v1/address` | `F116.rest-get-api-v1-address` | `fe-api/src/server.rs:167` |
| GET | `/api/v1/petals/{petal_id}/nodes` | `F116.rest-get-api-v1-petals-petal-id-nodes` | `fe-api/src/server.rs:169` |
| GET | `/api/v1/petals/{petal_id}/earthwork/summary` | `F116.rest-get-api-v1-petals-petal-id-earthwork-summary` | `fe-api/src/server.rs:174` |
| POST | `/api/v1/petals/{petal_id}/paths/{path_id}/instances/{instance_index}/promote` | `F116.rest-post-api-v1-petals-petal-id-paths-path-id-instances-instance-index-promote` | `fe-api/src/server.rs:179` |
| POST | `/api/v1/petals/{petal_id}/objects` | `F116.rest-post-api-v1-petals-petal-id-objects` | `fe-api/src/server.rs:184` |
| GET | `/api/v1/nodes/{node_id}/history` | `F116.rest-get-api-v1-nodes-node-id-history` | `fe-api/src/server.rs:188` |
| GET | `/api/v1/nodes/{node_id}/state` | `F116.rest-get-api-v1-nodes-node-id-state` | `fe-api/src/server.rs:192` |
| POST | `/api/v1/nodes/{node_id}/mutations` | `F116.rest-post-api-v1-nodes-node-id-mutations` | `fe-api/src/server.rs:196` |
| PATCH | `/api/v1/nodes/{node_id}/transform` | `F116.rest-patch-api-v1-nodes-node-id-transform` | `fe-api/src/server.rs:200` |
| GET | `/api/v1/nodes/{node_id}/transform` | `F116.rest-get-api-v1-nodes-node-id-transform` | `fe-api/src/server.rs:200` |
| GET | `/api/v1/field-defs/by-id/{field_def_id}/schema` | `F116.rest-get-api-v1-field-defs-by-id-field-def-id-schema` | `fe-api/src/server.rs:205` |
| PUT | `/api/v1/field-defs/by-id/{field_def_id}/schema` | `F116.rest-put-api-v1-field-defs-by-id-field-def-id-schema` | `fe-api/src/server.rs:205` |
| PATCH | `/api/v1/nodes/{node_id}/properties` | `F116.rest-patch-api-v1-nodes-node-id-properties` | `fe-api/src/server.rs:209` |
| GET | `/api/v1/nodes/{node_id}/properties` | `F116.rest-get-api-v1-nodes-node-id-properties` | `fe-api/src/server.rs:209` |
| DELETE | `/api/v1/nodes/{node_id}/properties/{key}` | `F116.rest-delete-api-v1-nodes-node-id-properties-key` | `fe-api/src/server.rs:213` |
| GET | `/api/v1/assets/{content_hash}` | `F116.rest-get-api-v1-assets-content-hash` | `fe-api/src/server.rs:218` |
| GET | `/api/v1/assets/by-id/{asset_id}` | `F116.rest-get-api-v1-assets-by-id-asset-id` | `fe-api/src/server.rs:223` |
| POST | `/api/v1/petals/{petal_id}/assets` | `F116.rest-post-api-v1-petals-petal-id-assets` | `fe-api/src/server.rs:228` |
| GET | `/api/v1/nodes/{node_id}/asset` | `F116.rest-get-api-v1-nodes-node-id-asset` | `fe-api/src/server.rs:234` |
| GET | `/api/v1/petals/{petal_id}/export` | `F116.rest-get-api-v1-petals-petal-id-export` | `fe-api/src/server.rs:239` |
| POST | `/api/v1/petals/{petal_id}/import` | `F116.rest-post-api-v1-petals-petal-id-import` | `fe-api/src/server.rs:243` |
| POST | `/api/v1/petals/{petal_id}/import/gpx` | `F116.rest-post-api-v1-petals-petal-id-import-gpx` | `fe-api/src/server.rs:248` |
| GET | `/api/v1/petals/{petal_id}/export/gpx` | `F116.rest-get-api-v1-petals-petal-id-export-gpx` | `fe-api/src/server.rs:252` |
| GET | `/api/v1/petals/{petal_id}/terrain` | `F116.rest-get-api-v1-petals-petal-id-terrain` | `fe-api/src/server.rs:257` |
| PUT | `/api/v1/petals/{petal_id}/terrain` | `F116.rest-put-api-v1-petals-petal-id-terrain` | `fe-api/src/server.rs:257` |
| DELETE | `/api/v1/petals/{petal_id}/terrain` | `F116.rest-delete-api-v1-petals-petal-id-terrain` | `fe-api/src/server.rs:257` |
| GET | `/api/v1/petals/{petal_id}/render-distance` | `F116.rest-get-api-v1-petals-petal-id-render-distance` | `fe-api/src/server.rs:264` |
| PUT | `/api/v1/petals/{petal_id}/render-distance` | `F116.rest-put-api-v1-petals-petal-id-render-distance` | `fe-api/src/server.rs:264` |
| DELETE | `/api/v1/petals/{petal_id}/render-distance` | `F116.rest-delete-api-v1-petals-petal-id-render-distance` | `fe-api/src/server.rs:264` |
| GET | `/api/v1/petals/{petal_id}/visibility-groups` | `F116.rest-get-api-v1-petals-petal-id-visibility-groups` | `fe-api/src/server.rs:271` |
| PUT | `/api/v1/petals/{petal_id}/visibility-groups` | `F116.rest-put-api-v1-petals-petal-id-visibility-groups` | `fe-api/src/server.rs:271` |
| PATCH | `/api/v1/petals/{petal_id}/visibility-groups/{group_id}` | `F116.rest-patch-api-v1-petals-petal-id-visibility-groups-group-id` | `fe-api/src/server.rs:276` |
| DELETE | `/api/v1/petals/{petal_id}/visibility-groups/{group_id}` | `F116.rest-delete-api-v1-petals-petal-id-visibility-groups-group-id` | `fe-api/src/server.rs:276` |
| GET | `/api/v1/tiles/elevation/{tileset_id}/{z}/{x}/{y_png}` | `F116.rest-get-api-v1-tiles-elevation-tileset-id-z-x-y-png` | `fe-api/src/server.rs:282` |
| GET | `/api/v1/tiles/satellite/{tileset_id}/{z}/{x}/{y_jpg}` | `F116.rest-get-api-v1-tiles-satellite-tileset-id-z-x-y-jpg` | `fe-api/src/server.rs:286` |
| GET | `/api/v1/tilesets` | `F116.rest-get-api-v1-tilesets` | `fe-api/src/server.rs:290` |
| GET | `/api/v1/tilesets/{tileset_id}/meta` | `F116.rest-get-api-v1-tilesets-tileset-id-meta` | `fe-api/src/server.rs:294` |
| GET | `/api/v1/petals/{petal_id}/gis/nodes` | `F116.rest-get-api-v1-petals-petal-id-gis-nodes` | `fe-api/src/server.rs:299` |
| GET | `/api/v1/petals/{petal_id}/gis/tracks` | `F116.rest-get-api-v1-petals-petal-id-gis-tracks` | `fe-api/src/server.rs:303` |
| POST | `/api/v1/petals/{petal_id}/waypoints` | `F116.rest-post-api-v1-petals-petal-id-waypoints` | `fe-api/src/server.rs:308` |
| PATCH | `/api/v1/nodes/{waypoint_id}/move` | `F116.rest-patch-api-v1-nodes-waypoint-id-move` | `fe-api/src/server.rs:312` |
| GET | `/api/v1/nodes/{track_id}/elevation-profile` | `F116.rest-get-api-v1-nodes-track-id-elevation-profile` | `fe-api/src/server.rs:317` |
| GET | `/api/v1/nodes/{track_id}/stats` | `F116.rest-get-api-v1-nodes-track-id-stats` | `fe-api/src/server.rs:321` |
| POST | `/api/v1/field-defs` | `F116.rest-post-api-v1-field-defs` | `fe-api/src/server.rs:326` |
| GET | `/api/v1/field-defs/{scope}` | `F116.rest-get-api-v1-field-defs-scope` | `fe-api/src/server.rs:327` |
| PATCH | `/api/v1/field-defs/by-id/{field_def_id}` | `F116.rest-patch-api-v1-field-defs-by-id-field-def-id` | `fe-api/src/server.rs:331` |
| DELETE | `/api/v1/field-defs/by-id/{field_def_id}` | `F116.rest-delete-api-v1-field-defs-by-id-field-def-id` | `fe-api/src/server.rs:331` |
| POST | `/api/v1/nodes` | `F116.rest-post-api-v1-nodes` | `fe-api/src/server.rs:336` |
| GET | `/api/v1/debug/screenshot` | `F116.rest-get-api-v1-debug-screenshot` | `fe-api/src/server.rs:340` |
| POST | `/api/v1/debug/screenshot` | `F116.rest-post-api-v1-debug-screenshot` | `fe-api/src/server.rs:340` |
| GET | `/api/v1/debug/camera` | `F116.rest-get-api-v1-debug-camera` | `fe-api/src/server.rs:344` |
| POST | `/api/v1/debug/input` | `F116.rest-post-api-v1-debug-input` | `fe-api/src/server.rs:348` |
| GET | `/api/v1/debug/ui-tree` | `F116.rest-get-api-v1-debug-ui-tree` | `fe-api/src/server.rs:349` |
| GET | `/api/v1/debug/ui-state` | `F116.rest-get-api-v1-debug-ui-state` | `fe-api/src/server.rs:350` |
| GET | `/api/v1/debug/portal-fixture/observe` | `F116.rest-get-api-v1-debug-portal-fixture-observe` | `fe-api/src/server.rs:351` |
| POST | `/api/v1/debug/portal-fixture/action` | `F116.rest-post-api-v1-debug-portal-fixture-action` | `fe-api/src/server.rs:352` |
| POST | `/api/v1/debug/portal-fixture/capture` | `F116.rest-post-api-v1-debug-portal-fixture-capture` | `fe-api/src/server.rs:353` |
| GET | `/api/v1/debug/logs` | `F116.rest-get-api-v1-debug-logs` | `fe-api/src/server.rs:354` |
| POST | `/api/v1/debug/dialog-answers` | `F116.rest-post-api-v1-debug-dialog-answers` | `fe-api/src/server.rs:355` |
| POST | `/api/v1/debug/window` | `F116.rest-post-api-v1-debug-window` | `fe-api/src/server.rs:360` |
| POST | `/api/v1/query` | `F116.rest-post-api-v1-query` | `fe-api/src/server.rs:365` |
| POST | `/api/v1/query/elevated` | `F116.rest-post-api-v1-query-elevated` | `fe-api/src/server.rs:366` |
| GET | `/api/v1/petals/{petal_id}/export.parquet` | `F116.rest-get-api-v1-petals-petal-id-export-parquet` | `fe-api/src/server.rs:371` |
| GET | `/api/v1/petals/{petal_id}/export.csv` | `F116.rest-get-api-v1-petals-petal-id-export-csv` | `fe-api/src/server.rs:375` |
| POST | `/api/v1/petals/{petal_id}/iot/readings` | `F116.rest-post-api-v1-petals-petal-id-iot-readings` | `fe-api/src/server.rs:380` |
| POST | `/api/v1/verses/{verse_id}/device-formats` | `F116.rest-post-api-v1-verses-verse-id-device-formats` | `fe-api/src/server.rs:385` |
| GET | `/api/v1/verses/{verse_id}/device-formats` | `F116.rest-get-api-v1-verses-verse-id-device-formats` | `fe-api/src/server.rs:385` |
| POST | `/api/v1/petals/{petal_id}/iot/payloads` | `F116.rest-post-api-v1-petals-petal-id-iot-payloads` | `fe-api/src/server.rs:389` |
| GET | `/api/v1/petals/{petal_id}/iot/payloads` | `F116.rest-get-api-v1-petals-petal-id-iot-payloads` | `fe-api/src/server.rs:389` |
| POST | `/api/v1/query/share` | `F116.rest-post-api-v1-query-share` | `fe-api/src/server.rs:398` |
| POST | `/api/v1/analytics/query` | `F116.rest-post-api-v1-analytics-query` | `fe-api/src/server.rs:400` |
| POST | `/api/v1/hexons/tilesets/install` | `F116.rest-post-api-v1-hexons-tilesets-install` | `fe-api/src/server.rs:405` |
| DELETE | `/api/v1/hexons/tilesets/{hexon_id}` | `F116.rest-delete-api-v1-hexons-tilesets-hexon-id` | `fe-api/src/server.rs:411` |
| PATCH | `/api/v1/hexons/tilesets/{hexon_id}/seeding` | `F116.rest-patch-api-v1-hexons-tilesets-hexon-id-seeding` | `fe-api/src/server.rs:415` |
| GET | `/api/v1/hexons/tilesets` | `F116.rest-get-api-v1-hexons-tilesets` | `fe-api/src/server.rs:419` |
| GET | `/api/v1/hexons/storage` | `F116.rest-get-api-v1-hexons-storage` | `fe-api/src/server.rs:423` |
| POST | `/api/v1/crates/publish` | `F116.rest-post-api-v1-crates-publish` | `fe-api/src/server.rs:428` |
| POST | `/api/v1/crates/{hexon_uri}/install` | `F116.rest-post-api-v1-crates-hexon-uri-install` | `fe-api/src/server.rs:429` |
| DELETE | `/api/v1/crates/{hexon_uri}/uninstall` | `F116.rest-delete-api-v1-crates-hexon-uri-uninstall` | `fe-api/src/server.rs:433` |
| GET | `/api/v1/crates/search` | `F116.rest-get-api-v1-crates-search` | `fe-api/src/server.rs:437` |
| GET | `/api/v1/crates/installed` | `F116.rest-get-api-v1-crates-installed` | `fe-api/src/server.rs:438` |
| GET | `/api/v1/crates/{hexon_uri}` | `F116.rest-get-api-v1-crates-hexon-uri` | `fe-api/src/server.rs:442` |
| GET | `/api/v1/crates/{hexon_uri}/entries` | `F116.rest-get-api-v1-crates-hexon-uri-entries` | `fe-api/src/server.rs:446` |
| GET | `/api/v1/crates/{hexon_uri}/entries/{entry_id}/asset` | `F116.rest-get-api-v1-crates-hexon-uri-entries-entry-id-asset` | `fe-api/src/server.rs:450` |
| GET | `/api/v1/crates/available` | `F116.rest-get-api-v1-crates-available` | `fe-api/src/server.rs:454` |
| POST | `/api/v1/sim/start` | `F116.rest-post-api-v1-sim-start` | `fe-api/src/server.rs:459` |
| POST | `/api/v1/sim/stop` | `F116.rest-post-api-v1-sim-stop` | `fe-api/src/server.rs:460` |
| GET | `/api/v1/sim/status` | `F116.rest-get-api-v1-sim-status` | `fe-api/src/server.rs:461` |
| POST | `/api/v1/sim/step` | `F116.rest-post-api-v1-sim-step` | `fe-api/src/server.rs:462` |
| POST | `/api/v1/sim/inject-fault` | `F116.rest-post-api-v1-sim-inject-fault` | `fe-api/src/server.rs:463` |
| POST | `/mcp` | `F116.rest-post-mcp` | `fe-api/src/server.rs:469` |

## Reconciliation limits and explicit gaps

Unmapped producer summary: `{"actions": [], "controls": []}`.

Missing endpoint checkpoint summary: `{"routes": [], "tools": []}`.

Developer Settings now owns terminal Reset (F086); only a disposable owned store may exercise it. Config URL has an explicit unsupported Inspector explanation and no editor/save route. The old InviteDialog enum/opener is retired. Petal Requirements is declaration-only. PluginHost has no plugin-management GUI. Native Portal fixture evidence (F141) is separate from GUI browsing (F025). Durable REST/legacy WebSocket auth (F142) requires actual lifecycle outcomes, not schema presence.

Pending source reconciliation: `["P2P QA-0050 repair remains isolated and is not part of this audited source. Actual two-root invite/join/role/replica and P2P transfer acceptance remains required; no completed collaboration is inferred."]`.

Additional live acceptance obligations: `["F143.watch-nonzero-exit-refused and F143.watch-writer-drain-refused require actual root-owned native refusal experiments. Synthetic F143 validator cases provide zero application coverage and cannot replace those outcomes or actual F080.confirm-close GUI/lifecycle/native-exit proof.", "Stage20 reconciliation adds native creation/baseline/materialization, request-owner/unknown/fullqueue, startup-hydration, native map fences, native form/header and expected page navigation variants. Every new checkpoint remains NOT_RUN; source assertions and QA18/19 do not execute these repairs.", "Y cases require actual current-stage GUI actions/state/images/readback. Controlled production channel/store/COM journeys may cover their A cases only; synthetic schema/receipt packets cannot substitute for native GUI or browser results."]`.
