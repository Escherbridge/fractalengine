---
type: FeatureSourceInventory
date: 2026-11-01
source_commit: 214045aab523259c56c701c8fe9741117e38a7d0
execution_status: NOT_RUN
---

# Source-to-feature reconciliation

This preserves the preparation scan's complete 70-action, 343-rendered-control and 44-MCP-tool inventories.
Family owners below identify which journey must cover the producer; a many-family helper is not itself proof
that every caller or variant ran. Sources predate S2 naming and later polish; regenerate/reconcile after changes.
Native popup children, color-picker children, scrollbars and resize grips are also covered by the live tree walk
in J06; a mechanical source scan alone cannot establish that a control was generated or reachable.

`RenameNode` has no external constructor in this snapshot; real rename routes are covered through Node Options
and Entity Settings. The Atlas tag helper has no production opener and remains U/F127. InviteDialog's enum
opener has no transient renderer and remains U/F128. These exclusions must be rechecked after later wiring.

## UiAction producers

| Action | Family owner(s) | Actual producer references |
|---|---|---|
| `OpenPortal` | F024, F025 | `fe-ui/src/actions/portal.rs:24`, `fe-ui/src/panels/inspector.rs:4`, `fe-ui/src/panels/inspector.rs:497`, `fe-ui/src/panels/inspector.rs:567` |
| `ClosePortal` | F025 | `fe-ui/src/panels/portal_toolbar.rs:68` |
| `PortalGoBack` | F025 | `fe-ui/src/panels/portal_toolbar.rs:38` |
| `SaveUrl` | F024 | `fe-ui/src/actions/portal.rs:57`, `fe-ui/src/actions/portal.rs:70`, `fe-ui/src/panels/inspector.rs:3`, `fe-ui/src/panels/inspector.rs:495`, `fe-ui/src/panels/inspector.rs:548` |
| `ApplyNodeTransform` | F018 | `fe-ui/src/actions/transform.rs:53`, `fe-ui/src/panels/inspector.rs:489` |
| `CreateNodeAt` | F020 | `fe-ui/src/dialogs/context_menu.rs:655` |
| `SubmitQuery` | F033, F034 | `fe-ui/src/panels/query_tab.rs:184` |
| `LoadNodeProperties` | F021, F030 | `fe-ui/src/dialogs/context_menu.rs:225` |
| `SetNodeProperty` | F030, F032 | `fe-ui/src/panels/annotation_card.rs:164`, `fe-ui/src/panels/inspector.rs:1234` |
| `DeleteNodeProperty` | F030, F032 | `fe-ui/src/panels/annotation_card.rs:159`, `fe-ui/src/panels/inspector.rs:1167` |
| `HexonInstallFromFile` | F120 | `fe-ui/src/dialogs/hexon_manager.rs:112` |
| `HexonRemoveTileset` | F123 | `fe-ui/src/dialogs/hexon_manager.rs:400` |
| `HexonToggleSeeding` | F123 | `fe-ui/src/dialogs/hexon_manager.rs:315` |
| `HexonStartDownload` | F122 | `fe-ui/src/dialogs/hexon_manager.rs:596` |
| `HexonCancelDownload` | F122 | `fe-ui/src/dialogs/hexon_manager.rs:639` |
| `HexonRefreshList` | F119 | `fe-ui/src/dialogs/hexon_manager.rs:100`, `fe-ui/src/ui_shell/right_sidebar.rs:1085` |
| `HexonOpenStorageDir` | F123 | `fe-ui/src/dialogs/hexon_manager.rs:222` |
| `HexonInstallFromRegistry` | F121 | `fe-ui/src/dialogs/hexon_manager.rs:142` |
| `PetalSetMap` | F047 | `fe-ui/src/dialogs/hexon_manager.rs:357`, `fe-ui/src/dialogs/hexon_manager.rs:373` |
| `PetalSetMapScale` | F048 | `fe-ui/src/dialogs/hexon_manager.rs:486`, `fe-ui/src/dialogs/hexon_manager.rs:507` |
| `PetalManifestSave` | F124 | `fe-ui/src/dialogs/petal_manifest.rs:210` |
| `PetalSetRenderOverride` | F061 | `fe-ui/src/actions/hexon.rs:284` |
| `PetalManifestOpen` | F124 | `fe-ui/src/panels/sidebar.rs:466` |
| `DownloadNodeAsset` | F026 | `fe-ui/src/panels/asset_card.rs:2`, `fe-ui/src/panels/asset_card.rs:75` |
| `GisQueryAnnotated` | F040, F042 | `fe-ui/src/panels/gis_panel.rs:394` |
| `GisQueryPropertyFilter` | F040 | `fe-ui/src/panels/gis_panel.rs:408` |
| `GisSetLayer` | F049 | `fe-ui/src/gis/layers.rs:121`, `fe-ui/src/panels/layer_manager_card.rs:135` |
| `GisSetViewMode` | F049 | `fe-ui/src/panels/layer_manager_card.rs:87` |
| `GpxImportFile` | F064 | `fe-ui/src/panels/gpx_import_card.rs:2`, `fe-ui/src/panels/gpx_import_card.rs:42` |
| `PathQueryTracks` | F063 | `fe-ui/src/node_manager/pointer/mod.rs:168`, `fe-ui/src/node_manager/pointer/mod.rs:198`, `fe-ui/src/node_manager/pointer/mod.rs:211`, `fe-ui/src/panels/gis_panel.rs:122`, `fe-ui/src/panels/path_editor_card.rs:163`, `fe-ui/src/panels/path_editor_card.rs:288` |
| `PathSelectTrack` | F063, F077 | `fe-ui/src/dialogs/context_menu.rs:770`, `fe-ui/src/dialogs/context_menu.rs:783`, `fe-ui/src/gis/mod.rs:411`, `fe-ui/src/node_manager/pointer/mod.rs:4`, `fe-ui/src/node_manager/pointer/mod.rs:79`, `fe-ui/src/panels/path_editor_card.rs:262` |
| `PathCreateTrack` | F063, F065 | `fe-ui/src/node_manager/path_point_interaction.rs:516`, `fe-ui/src/panels/path_editor_card.rs:132` |
| `PathDeleteTrack` | F063, F077 | `fe-ui/src/panels/path_editor_card.rs:268` |
| `PathAppendPoint` | F065 | `fe-ui/src/node_manager/path_point_interaction.rs:487`, `fe-ui/src/verse_manager/db_results/nodes.rs:148` |
| `PathAppendSmoothPoint` | F065 | `fe-ui/src/node_manager/path_point_interaction.rs:477`, `fe-ui/src/verse_manager/db_results/nodes.rs:139` |
| `PathSetAnchorHandles` | F067, F069 | `fe-ui/src/node_manager/path_handle_interaction.rs:353`, `fe-ui/src/panels/tool_options.rs:225` |
| `PathSetAnchorCorner` | F067, F077 | `fe-ui/src/dialogs/context_menu.rs:267`, `fe-ui/src/node_manager/path_handle_interaction.rs:362`, `fe-ui/src/panels/tool_options.rs:218` |
| `PathRemovePoint` | F069, F077 | `fe-ui/src/dialogs/context_menu.rs:272`, `fe-ui/src/panels/path_editor_card.rs:500` |
| `PathMovePoint` | F069 | `fe-ui/src/node_manager/path_gimbal_drag.rs:7`, `fe-ui/src/node_manager/path_point_interaction.rs:744`, `fe-ui/src/panels/path_editor_card.rs:436`, `fe-ui/src/panels/path_editor_card.rs:509` |
| `PathTransformPoints` | F072 | `fe-ui/src/node_manager/transform_broadcast.rs:51` |
| `PathAnnotatePoint` | F070 | `fe-ui/src/panels/path_editor_card.rs:1027` |
| `PathExportGpx` | F078 | `fe-ui/src/panels/path_editor_card.rs:316` |
| `PathSetStyle` | F071 | `fe-ui/src/panels/path_editor_card.rs:328` |
| `PathAssetApply` | F074 | `fe-ui/src/panels/tool_panel.rs:7`, `fe-ui/src/panels/tool_panel.rs:366` |
| `PathSmoothCurrent` | F066 | `fe-ui/src/panels/tool_panel.rs:771` |
| `PathAppendShape` | F068 | `fe-ui/src/panels/tool_panel.rs:828`, `fe-ui/src/panels/tool_panel.rs:840`, `fe-ui/src/panels/tool_panel.rs:852` |
| `TerrainProposalAdd` | F050, F051 | `fe-ui/src/panels/terrain_tools_panel.rs:7`, `fe-ui/src/panels/terrain_tools_panel.rs:126`, `fe-ui/src/panels/terrain_tools_panel.rs:233`, `fe-ui/src/ui_shell/right_sidebar.rs:534` |
| `TerrainProposalDelete` | F051 | `fe-ui/src/dialogs/context_menu.rs:306`, `fe-ui/src/panels/terrain_tools_panel.rs:316` |
| `TerrainProposalSelect` | F051 | `fe-ui/src/dialogs/context_menu.rs:289` |
| `SelectStamp` | F075 | `fe-ui/src/panels/tool_panel.rs:434` |
| `SetStampScale` | F075 | `fe-ui/src/panels/tool_panel.rs:462` |
| `SetStampRotation` | F075 | `fe-ui/src/panels/tool_panel.rs:484` |
| `SlideStampAlongPath` | F075 | `fe-ui/src/panels/tool_panel.rs:509` |
| `SculptBrushStroke` | F052 | `fe-ui/src/node_manager/brush_interaction.rs:145` |
| `SculptShapeRegion` | F053, F054 | `fe-ui/src/panels/tool_options.rs:11`, `fe-ui/src/panels/tool_options.rs:78`, `fe-ui/src/panels/tool_options.rs:308`, `fe-ui/src/panels/tool_options.rs:375` |
| `SculptDeleteRegion` | F057 | `fe-ui/src/dialogs/context_menu.rs:308` |
| `SaveMeasureAsCallout` | F059 | `fe-ui/src/actions/dimension_callout.rs:38`, `fe-ui/src/panels/tool_options.rs:429`, `fe-ui/src/panels/tool_options.rs:514` |
| `DeleteNode` | F012, F022, F076, F077 | `fe-ui/src/dialogs/context_menu.rs:217`, `fe-ui/src/dialogs/context_menu.rs:901`, `fe-ui/src/dialogs/node_options.rs:223` |
| `DuplicateNode` | F021 | `fe-ui/src/dialogs/context_menu.rs:221` |
| `RenameNode` | F012, F021, F023 | **No external constructor found**; check internal action generation before exclusion |
| `PromoteStamp` | F075, F076 | `fe-ui/src/actions/asset.rs:82`, `fe-ui/src/dialogs/context_menu.rs:756`, `fe-ui/src/panels/tool_panel.rs:444` |
| `ClearNodeProperties` | F022, F023 | `fe-ui/src/dialogs/context_menu.rs:222` |
| `CopyApiString` | F027 | `fe-ui/src/dialogs/context_menu.rs:223` |
| `ReportObject` | F027 | `fe-ui/src/dialogs/context_menu.rs:224` |
| `RevealSection` | F082 | `fe-ui/src/dialogs/context_menu.rs:290`, `fe-ui/src/panels/sidebar.rs:552`, `fe-ui/src/panels/sidebar.rs:788`, `fe-ui/src/panels/terrain_tools_panel.rs:5`, `fe-ui/src/panels/tool_options.rs:403`, `fe-ui/src/panels/tool_options.rs:528`, `fe-ui/src/plugin.rs:191`, `fe-ui/src/ui_shell/right_sidebar.rs:12`, `fe-ui/src/ui_shell/right_sidebar.rs:86`, `fe-ui/src/ui_shell/right_sidebar.rs:174`, `fe-ui/src/ui_shell/right_sidebar.rs:1387`, `fe-ui/src/ui_shell/topbar.rs:594` |
| `SetNodeView` | F008, F009 | `fe-ui/src/node_manager/shortcuts.rs:46`, `fe-ui/src/node_manager/shortcuts.rs:66`, `fe-ui/src/panels/sidebar.rs:9`, `fe-ui/src/panels/sidebar.rs:527`, `fe-ui/src/panels/sidebar.rs:686`, `fe-ui/src/visibility/cache.rs:144`, `fe-ui/src/visibility/mod.rs:463`, `fe-ui/src/visibility/mod.rs:479` |
| `SetGroupRegistry` | F010 | `fe-ui/src/gis/layers.rs:127`, `fe-ui/src/panels/sidebar.rs:772`, `fe-ui/src/ui_shell/right_sidebar.rs:1154`, `fe-ui/src/ui_shell/right_sidebar.rs:1177`, `fe-ui/src/visibility/groups.rs:140` |
| `SetScopeHidden` | F008 | `fe-ui/src/panels/sidebar.rs:509`, `fe-ui/src/visibility/groups.rs:179`, `fe-ui/src/visibility/mod.rs:450` |
| `SetNodeGroups` | F011 | `fe-ui/src/ui_shell/right_sidebar.rs:1156`, `fe-ui/src/ui_shell/right_sidebar.rs:1307`, `fe-ui/src/visibility/groups.rs:228` |
| `SetAppOverlay` | F060 | `fe-ui/src/gis/layers.rs:137` |

## Rendered-control candidates

| Source | Family owner(s) | Actual control expression |
|---|---|---|
| `fe-ui/src/atlas/tag_panel.rs:42` | F127 | `.small_button("×")` |
| `fe-ui/src/atlas/tag_panel.rs:54` | F127 | `.text_edit_singleline(input)` |
| `fe-ui/src/atlas/tag_panel.rs:55` | F127 | `.button("Add")` |
| `fe-ui/src/atlas/visibility_control.rs:22` | F094 | `.selectable_value(current, Visibility::Private, "Private")` |
| `fe-ui/src/atlas/visibility_control.rs:23` | F094 | `.selectable_value(current, Visibility::Public, "Public")` |
| `fe-ui/src/atlas/visibility_control.rs:24` | F094 | `.selectable_value(current, Visibility::Unlisted, "Unlisted")` |
| `fe-ui/src/dialogs/context_menu.rs:337` | F020, F021, F022, F027, F051, F057, F076, F077 | `Button::new(egui::RichText::new(label).color(color))` |
| `fe-ui/src/dialogs/context_menu.rs:891` | F020, F021, F022, F027, F051, F057, F076, F077 | `Button::new( egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE), )` |
| `fe-ui/src/dialogs/context_menu.rs:912` | F020, F021, F022, F027, F051, F057, F076, F077 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/create_entity.rs:62` | F005, F006 | `TextEdit::singleline(name_buf)` |
| `fe-ui/src/dialogs/create_entity.rs:79` | F005, F006 | `Button::new("Create")` |
| `fe-ui/src/dialogs/create_entity.rs:92` | F005, F006 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/entity_settings.rs:107` | F093, F094, F095, F096, F098, F102, F103 | `Button::new(egui::RichText::new("General").color(theme::TEXT_BRIGHT))` |
| `fe-ui/src/dialogs/entity_settings.rs:122` | F093, F094, F095, F096, F098, F102, F103 | `Button::new(egui::RichText::new("Access").color(theme::TEXT_BRIGHT))` |
| `fe-ui/src/dialogs/entity_settings.rs:137` | F093, F094, F095, F096, F098, F102, F103 | `Button::new(egui::RichText::new("API").color(theme::TEXT_BRIGHT))` |
| `fe-ui/src/dialogs/entity_settings.rs:164` | F093, F094, F095, F096, F098, F102, F103 | `TextEdit::singleline(name_buf)` |
| `fe-ui/src/dialogs/entity_settings.rs:165` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Rename")` |
| `fe-ui/src/dialogs/entity_settings.rs:231` | F093, F094, F095, F096, F098, F102, F103 | `TextEdit::multiline(desc)` |
| `fe-ui/src/dialogs/entity_settings.rs:236` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Save Description")` |
| `fe-ui/src/dialogs/entity_settings.rs:281` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Claim Ownership")` |
| `fe-ui/src/dialogs/entity_settings.rs:296` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Confirm Claim")` |
| `fe-ui/src/dialogs/entity_settings.rs:307` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/entity_settings.rs:322` | F093, F094, F095, F096, F098, F102, F103 | `Button::new(egui::RichText::new("Delete").color(egui::Color32::WHITE))` |
| `fe-ui/src/dialogs/entity_settings.rs:337` | F093, F094, F095, F096, F098, F102, F103 | `Button::new( egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE), )` |
| `fe-ui/src/dialogs/entity_settings.rs:357` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/entity_settings.rs:458` | F093, F094, F095, F096, F098, F102, F103 | `ComboBox::from_id_salt("invite_role")` |
| `fe-ui/src/dialogs/entity_settings.rs:462` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(invite_role_buf, "viewer".to_string(), "Viewer")` |
| `fe-ui/src/dialogs/entity_settings.rs:463` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(invite_role_buf, "editor".to_string(), "Editor")` |
| `fe-ui/src/dialogs/entity_settings.rs:464` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(invite_role_buf, "manager".to_string(), "Manager")` |
| `fe-ui/src/dialogs/entity_settings.rs:468` | F093, F094, F095, F096, F098, F102, F103 | `ComboBox::from_id_salt("invite_expiry")` |
| `fe-ui/src/dialogs/entity_settings.rs:478` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(invite_expiry_buf, 1, "1 hour")` |
| `fe-ui/src/dialogs/entity_settings.rs:479` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(invite_expiry_buf, 24, "24 hours")` |
| `fe-ui/src/dialogs/entity_settings.rs:480` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(invite_expiry_buf, 168, "7 days")` |
| `fe-ui/src/dialogs/entity_settings.rs:481` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(invite_expiry_buf, 720, "30 days")` |
| `fe-ui/src/dialogs/entity_settings.rs:484` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Generate")` |
| `fe-ui/src/dialogs/entity_settings.rs:512` | F093, F094, F095, F096, F098, F102, F103 | `TextEdit::multiline(&mut link_display)` |
| `fe-ui/src/dialogs/entity_settings.rs:517` | F093, F094, F095, F096, F098, F102, F103 | `.button("Copy to Clipboard")` |
| `fe-ui/src/dialogs/entity_settings.rs:549` | F093, F094, F095, F096, F098, F102, F103 | `.small_button("Copy")` |
| `fe-ui/src/dialogs/entity_settings.rs:607` | F093, F094, F095, F096, F098, F102, F103 | `TextEdit::singleline(api_token_scope_buf)` |
| `fe-ui/src/dialogs/entity_settings.rs:615` | F093, F094, F095, F096, F098, F102, F103 | `ComboBox::from_id_salt("api_token_role")` |
| `fe-ui/src/dialogs/entity_settings.rs:619` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(api_token_role_buf, "viewer".to_string(), "Viewer")` |
| `fe-ui/src/dialogs/entity_settings.rs:620` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(api_token_role_buf, "editor".to_string(), "Editor")` |
| `fe-ui/src/dialogs/entity_settings.rs:621` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(api_token_role_buf, "manager".to_string(), "Manager")` |
| `fe-ui/src/dialogs/entity_settings.rs:625` | F093, F094, F095, F096, F098, F102, F103 | `ComboBox::from_id_salt("api_token_expiry")` |
| `fe-ui/src/dialogs/entity_settings.rs:635` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(api_token_expiry_buf, 1, "1 hour")` |
| `fe-ui/src/dialogs/entity_settings.rs:636` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(api_token_expiry_buf, 24, "24 hours")` |
| `fe-ui/src/dialogs/entity_settings.rs:637` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(api_token_expiry_buf, 168, "7 days")` |
| `fe-ui/src/dialogs/entity_settings.rs:638` | F093, F094, F095, F096, F098, F102, F103 | `.selectable_value(api_token_expiry_buf, 720, "30 days")` |
| `fe-ui/src/dialogs/entity_settings.rs:649` | F093, F094, F095, F096, F098, F102, F103 | `TextEdit::singleline(api_token_client_buf)` |
| `fe-ui/src/dialogs/entity_settings.rs:670` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Generate API Token")` |
| `fe-ui/src/dialogs/entity_settings.rs:701` | F093, F094, F095, F096, F098, F102, F103 | `TextEdit::multiline(&mut token_display)` |
| `fe-ui/src/dialogs/entity_settings.rs:707` | F093, F094, F095, F096, F098, F102, F103 | `.button("Copy Token")` |
| `fe-ui/src/dialogs/entity_settings.rs:710` | F093, F094, F095, F096, F098, F102, F103 | `.button("Copy curl")` |
| `fe-ui/src/dialogs/entity_settings.rs:733` | F093, F094, F095, F096, F098, F102, F103 | `.small_button("Copy")` |
| `fe-ui/src/dialogs/entity_settings.rs:760` | F093, F094, F095, F096, F098, F102, F103 | `TextEdit::multiline(&mut curl_display)` |
| `fe-ui/src/dialogs/entity_settings.rs:804` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Revoke")` |
| `fe-ui/src/dialogs/entity_settings.rs:900` | F093, F094, F095, F096, F098, F102, F103 | `Button::new("Revoke")` |
| `fe-ui/src/dialogs/gltf_import.rs:92` | F019 | `TextEdit::singleline(file_path_buf)` |
| `fe-ui/src/dialogs/gltf_import.rs:97` | F019 | `Button::new("Browse...")` |
| `fe-ui/src/dialogs/gltf_import.rs:128` | F019 | `TextEdit::singleline(name_buf)` |
| `fe-ui/src/dialogs/gltf_import.rs:170` | F019 | `Button::new("Import")` |
| `fe-ui/src/dialogs/gltf_import.rs:199` | F019 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/hexon_manager.rs:84` | F047, F048, F119, F120, F121, F122, F123 | `TextEdit::singleline(filter_text)` |
| `fe-ui/src/dialogs/hexon_manager.rs:91` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Close")` |
| `fe-ui/src/dialogs/hexon_manager.rs:97` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Refresh")` |
| `fe-ui/src/dialogs/hexon_manager.rs:103` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Install from file...")` |
| `fe-ui/src/dialogs/hexon_manager.rs:130` | F047, F048, F119, F120, F121, F122, F123 | `TextEdit::singleline(registry_uri_buf)` |
| `fe-ui/src/dialogs/hexon_manager.rs:138` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Install from registry")` |
| `fe-ui/src/dialogs/hexon_manager.rs:173` | F047, F048, F119, F120, F121, F122, F123 | `Button::new(egui::RichText::new(label).color(theme::TEXT_BRIGHT))` |
| `fe-ui/src/dialogs/hexon_manager.rs:222` | F047, F048, F119, F120, F121, F122, F123 | `.button("Open folder")` |
| `fe-ui/src/dialogs/hexon_manager.rs:310` | F047, F048, F119, F120, F121, F122, F123 | `.checkbox(&mut seeding, "Seed")` |
| `fe-ui/src/dialogs/hexon_manager.rs:341` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Applying\u{2026}")` |
| `fe-ui/src/dialogs/hexon_manager.rs:349` | F047, F048, F119, F120, F121, F122, F123 | `Button::new( egui::RichText::new("Active map \u{2713}").color(theme::TEXT_BRIGHT), )` |
| `fe-ui/src/dialogs/hexon_manager.rs:369` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Set petal map")` |
| `fe-ui/src/dialogs/hexon_manager.rs:484` | F047, F048, F119, F120, F121, F122, F123 | `Button::new(*label)` |
| `fe-ui/src/dialogs/hexon_manager.rs:498` | F047, F048, F119, F120, F121, F122, F123 | `Slider::new(&mut scale, slider_min..=slider_max)` |
| `fe-ui/src/dialogs/hexon_manager.rs:593` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Download")` |
| `fe-ui/src/dialogs/hexon_manager.rs:636` | F047, F048, F119, F120, F121, F122, F123 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/join.rs:69` | F099 | `TextEdit::multiline(invite_buf)` |
| `fe-ui/src/dialogs/join.rs:85` | F099 | `Button::new("Join")` |
| `fe-ui/src/dialogs/join.rs:95` | F099 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/node_options.rs:65` | F012, F022, F023, F024 | `TextEdit::singleline(node_name_buf)` |
| `fe-ui/src/dialogs/node_options.rs:74` | F012, F022, F023, F024 | `TextEdit::singleline(webpage_url_buf)` |
| `fe-ui/src/dialogs/node_options.rs:90` | F012, F022, F023, F024 | `Button::new("Save")` |
| `fe-ui/src/dialogs/node_options.rs:117` | F012, F022, F023, F024 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/node_options.rs:179` | F012, F022, F023, F024 | `Button::new( egui::RichText::new("Delete Node").color(egui::Color32::WHITE), )` |
| `fe-ui/src/dialogs/node_options.rs:214` | F012, F022, F023, F024 | `Button::new( egui::RichText::new("Confirm Delete").color(egui::Color32::WHITE), )` |
| `fe-ui/src/dialogs/node_options.rs:230` | F012, F022, F023, F024 | `Button::new("Cancel")` |
| `fe-ui/src/dialogs/peer_debug.rs:48` | F104 | `.button("Close")` |
| `fe-ui/src/dialogs/petal_manifest.rs:55` | F124 | `TextEdit::singleline(render_distance_buf)` |
| `fe-ui/src/dialogs/petal_manifest.rs:99` | F124 | `.checkbox(&mut entry.required, "")` |
| `fe-ui/src/dialogs/petal_manifest.rs:116` | F124 | `Button::new("X")` |
| `fe-ui/src/dialogs/petal_manifest.rs:142` | F124 | `TextEdit::singleline(add_hexon_id_buf)` |
| `fe-ui/src/dialogs/petal_manifest.rs:147` | F124 | `ComboBox::from_id_salt("hexon_type_combo")` |
| `fe-ui/src/dialogs/petal_manifest.rs:156` | F124 | `.selectable_value(add_hexon_type_buf, t.to_string(), *t)` |
| `fe-ui/src/dialogs/petal_manifest.rs:162` | F124 | `Button::new("Add")` |
| `fe-ui/src/dialogs/petal_manifest.rs:188` | F124 | `CollapsingHeader::new( egui::RichText::new("JSON Schema Preview").small().color(theme::TEXT_DIM), )` |
| `fe-ui/src/dialogs/petal_manifest.rs:195` | F124 | `TextEdit::multiline(&mut json.as_str())` |
| `fe-ui/src/dialogs/petal_manifest.rs:207` | F124 | `Button::new(save_label)` |
| `fe-ui/src/dialogs/petal_manifest.rs:216` | F124 | `Button::new("Close")` |
| `fe-ui/src/panels/annotation_card.rs:25` | F032 | `CollapsingHeader::new( egui::RichText::new("Annotation") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/annotation_card.rs:36` | F032 | `TextEdit::singleline(&mut inspector.annotation_title_buf)` |
| `fe-ui/src/panels/annotation_card.rs:44` | F032 | `TextEdit::multiline(&mut inspector.annotation_body_buf)` |
| `fe-ui/src/panels/annotation_card.rs:53` | F032 | `TextEdit::singleline(&mut inspector.annotation_color_buf)` |
| `fe-ui/src/panels/annotation_card.rs:81` | F032 | `Button::new("\u{1F4BE} Save Annotation")` |
| `fe-ui/src/panels/asset_card.rs:32` | F026 | `CollapsingHeader::new( egui::RichText::new("Asset") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/asset_card.rs:66` | F026 | `Button::new("\u{2B07} Download asset")` |
| `fe-ui/src/panels/egress_card.rs:43` | F043, F044, F045 | `TextEdit::singleline(&mut gis_state.egress.base_url_buf)` |
| `fe-ui/src/panels/egress_card.rs:89` | F043, F044, F045 | `Button::new(label)` |
| `fe-ui/src/panels/egress_card.rs:107` | F043, F044, F045 | `TextEdit::singleline(&mut gis_state.egress.node_id_buf)` |
| `fe-ui/src/panels/egress_card.rs:111` | F043, F044, F045 | `.small_button("Use viewport selection")` |
| `fe-ui/src/panels/egress_card.rs:181` | F043, F044, F045 | `TextEdit::singleline(&mut gis_state.egress.ttl_buf)` |
| `fe-ui/src/panels/gis_panel.rs:163` | F039, F040, F041, F042 | `Button::new(egui::RichText::new(label).small().color(if active { theme::TEXT_BRIGHT } else { theme::TEXT_DIM }))` |
| `fe-ui/src/panels/gis_panel.rs:209` | F039, F040, F041, F042 | `Button::new(label)` |
| `fe-ui/src/panels/gis_panel.rs:258` | F039, F040, F041, F042 | `Button::new(label)` |
| `fe-ui/src/panels/gis_panel.rs:281` | F039, F040, F041, F042 | `TextEdit::singleline(&mut gis_state.filter_key_buf)` |
| `fe-ui/src/panels/gis_panel.rs:286` | F039, F040, F041, F042 | `TextEdit::singleline(&mut gis_state.filter_value_buf)` |
| `fe-ui/src/panels/gis_panel.rs:290` | F039, F040, F041, F042 | `ComboBox::from_id_salt("gis_filter_value_type")` |
| `fe-ui/src/panels/gis_panel.rs:295` | F039, F040, F041, F042 | `.selectable_value( &mut gis_state.filter_value_type_buf, t.to_string(), t, )` |
| `fe-ui/src/panels/gis_panel.rs:311` | F039, F040, F041, F042 | `TextEdit::singleline(&mut gis_state.bbox_min[0])` |
| `fe-ui/src/panels/gis_panel.rs:312` | F039, F040, F041, F042 | `TextEdit::singleline(&mut gis_state.bbox_min[1])` |
| `fe-ui/src/panels/gis_panel.rs:320` | F039, F040, F041, F042 | `TextEdit::singleline(&mut gis_state.bbox_max[0])` |
| `fe-ui/src/panels/gis_panel.rs:321` | F039, F040, F041, F042 | `TextEdit::singleline(&mut gis_state.bbox_max[1])` |
| `fe-ui/src/panels/gis_panel.rs:324` | F039, F040, F041, F042 | `.small_button("Center on Selection")` |
| `fe-ui/src/panels/gis_panel.rs:338` | F039, F040, F041, F042 | `.small_button("Center on Origin")` |
| `fe-ui/src/panels/gis_panel.rs:353` | F039, F040, F041, F042 | `Button::new(run_label)` |
| `fe-ui/src/panels/gis_panel.rs:484` | F039, F040, F041, F042 | `.allocate_exact_size( egui::vec2(12.0, 12.0), egui::Sense::hover(), )` |
| `fe-ui/src/panels/gpx_import_card.rs:30` | F064 | `Button::new("\u{1F4C1} Import GPX...")` |
| `fe-ui/src/panels/inspector.rs:47` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{2715}")` |
| `fe-ui/src/panels/inspector.rs:75` | F018, F024, F029, F030, F031, F100, F101 | `Button::new(egui::RichText::new(label).small().color(if active { theme::TEXT_BRIGHT } else { theme::TEXT_DIM }))` |
| `fe-ui/src/panels/inspector.rs:172` | F018, F024, F029, F030, F031, F100, F101 | `CollapsingHeader::new( egui::RichText::new("Entity") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/inspector.rs:378` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(src)` |
| `fe-ui/src/panels/inspector.rs:410` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(buf)` |
| `fe-ui/src/panels/inspector.rs:438` | F018, F024, F029, F030, F031, F100, F101 | `CollapsingHeader::new( egui::RichText::new("Transform") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/inspector.rs:484` | F018, F024, F029, F030, F031, F100, F101 | `.button("Apply")` |
| `fe-ui/src/panels/inspector.rs:505` | F018, F024, F029, F030, F031, F100, F101 | `CollapsingHeader::new( egui::RichText::new("Portal URLs") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/inspector.rs:520` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(&mut inspector.external_url)` |
| `fe-ui/src/panels/inspector.rs:533` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(&mut inspector.config_url)` |
| `fe-ui/src/panels/inspector.rs:542` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{1F4BE} Save")` |
| `fe-ui/src/panels/inspector.rs:554` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{1F310} Open Portal")` |
| `fe-ui/src/panels/inspector.rs:610` | F018, F024, F029, F030, F031, F100, F101 | `CollapsingHeader::new( egui::RichText::new("Generate Token") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/inspector.rs:667` | F018, F024, F029, F030, F031, F100, F101 | `ComboBox::from_id_salt("inspector_api_role")` |
| `fe-ui/src/panels/inspector.rs:671` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.api_token_role_buf, "viewer".to_string(), "Viewer")` |
| `fe-ui/src/panels/inspector.rs:672` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.api_token_role_buf, "editor".to_string(), "Editor")` |
| `fe-ui/src/panels/inspector.rs:673` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.api_token_role_buf, "manager".to_string(), "Manager")` |
| `fe-ui/src/panels/inspector.rs:678` | F018, F024, F029, F030, F031, F100, F101 | `ComboBox::from_id_salt("inspector_api_expiry")` |
| `fe-ui/src/panels/inspector.rs:688` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.api_token_expiry_buf, 1, "1 hour")` |
| `fe-ui/src/panels/inspector.rs:689` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.api_token_expiry_buf, 24, "24 hours")` |
| `fe-ui/src/panels/inspector.rs:690` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.api_token_expiry_buf, 168, "7 days")` |
| `fe-ui/src/panels/inspector.rs:691` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.api_token_expiry_buf, 720, "30 days")` |
| `fe-ui/src/panels/inspector.rs:707` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(&mut inspector.api_token_client_buf)` |
| `fe-ui/src/panels/inspector.rs:727` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("Generate API Token")` |
| `fe-ui/src/panels/inspector.rs:762` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{2715}")` |
| `fe-ui/src/panels/inspector.rs:771` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::multiline(&mut display)` |
| `fe-ui/src/panels/inspector.rs:794` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{1F4CB}")` |
| `fe-ui/src/panels/inspector.rs:808` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{1F4CB} Copy Token")` |
| `fe-ui/src/panels/inspector.rs:833` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{1F4CB}")` |
| `fe-ui/src/panels/inspector.rs:861` | F018, F024, F029, F030, F031, F100, F101 | `CollapsingHeader::new( egui::RichText::new("Active Tokens") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/inspector.rs:931` | F018, F024, F029, F030, F031, F100, F101 | `Button::new( egui::RichText::new("Cancel").small(), )` |
| `fe-ui/src/panels/inspector.rs:944` | F018, F024, F029, F030, F031, F100, F101 | `Button::new( egui::RichText::new("Confirm Revoke") .small(), )` |
| `fe-ui/src/panels/inspector.rs:960` | F018, F024, F029, F030, F031, F100, F101 | `Button::new( egui::RichText::new("Revoke").small(), )` |
| `fe-ui/src/panels/inspector.rs:1007` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{25C0}")` |
| `fe-ui/src/panels/inspector.rs:1017` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{25B6}")` |
| `fe-ui/src/panels/inspector.rs:1022` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("\u{21BB} Refresh")` |
| `fe-ui/src/panels/inspector.rs:1081` | F018, F024, F029, F030, F031, F100, F101 | `CollapsingHeader::new( egui::RichText::new("Custom Properties") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/inspector.rs:1148` | F018, F024, F029, F030, F031, F100, F101 | `Button::new(egui::RichText::new("\u{2715}").small())` |
| `fe-ui/src/panels/inspector.rs:1187` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(&mut inspector.prop_add_key_buf)` |
| `fe-ui/src/panels/inspector.rs:1192` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(&mut inspector.prop_add_value_buf)` |
| `fe-ui/src/panels/inspector.rs:1196` | F018, F024, F029, F030, F031, F100, F101 | `ComboBox::from_id_salt("prop_type_combo")` |
| `fe-ui/src/panels/inspector.rs:1201` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value(&mut inspector.prop_add_type_buf, t.to_string(), t)` |
| `fe-ui/src/panels/inspector.rs:1211` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("Add")` |
| `fe-ui/src/panels/inspector.rs:1263` | F018, F024, F029, F030, F031, F100, F101 | `CollapsingHeader::new( egui::RichText::new("Property Schema") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/inspector.rs:1345` | F018, F024, F029, F030, F031, F100, F101 | `Button::new(egui::RichText::new("\u{2715}").small())` |
| `fe-ui/src/panels/inspector.rs:1384` | F018, F024, F029, F030, F031, F100, F101 | `TextEdit::singleline(&mut inspector.field_def_add_key_buf)` |
| `fe-ui/src/panels/inspector.rs:1388` | F018, F024, F029, F030, F031, F100, F101 | `ComboBox::from_id_salt("field_def_type_combo")` |
| `fe-ui/src/panels/inspector.rs:1393` | F018, F024, F029, F030, F031, F100, F101 | `.selectable_value( &mut inspector.field_def_add_type_buf, t.to_string(), t, )` |
| `fe-ui/src/panels/inspector.rs:1405` | F018, F024, F029, F030, F031, F100, F101 | `Button::new("Add Field")` |
| `fe-ui/src/panels/layer_manager_card.rs:78` | F049 | `Button::new(label)` |
| `fe-ui/src/panels/layer_manager_card.rs:109` | F049 | `.checkbox(&mut visible, row.name.as_str())` |
| `fe-ui/src/panels/layer_manager_card.rs:127` | F049 | `Slider::new(&mut opacity, 0.0..=1.0)` |
| `fe-ui/src/panels/path_editor_card.rs:121` | F063, F066, F067, F069, F070, F071, F072, F078 | `TextEdit::singleline(&mut path_state.new_track_name_buf)` |
| `fe-ui/src/panels/path_editor_card.rs:126` | F063, F066, F067, F069, F070, F071, F072, F078 | `Button::new("New Path")` |
| `fe-ui/src/panels/path_editor_card.rs:159` | F063, F066, F067, F069, F070, F071, F072, F078 | `Button::new(label)` |
| `fe-ui/src/panels/path_editor_card.rs:227` | F063, F066, F067, F069, F070, F071, F072, F078 | `.small_button("Cancel")` |
| `fe-ui/src/panels/path_editor_card.rs:232` | F063, F066, F067, F069, F070, F071, F072, F078 | `Button::new( egui::RichText::new("Confirm Delete") .small() .color(egui::Color32::WHITE), )` |
| `fe-ui/src/panels/path_editor_card.rs:243` | F063, F066, F067, F069, F070, F071, F072, F078 | `.small_button("Delete")` |
| `fe-ui/src/panels/path_editor_card.rs:286` | F063, F066, F067, F069, F070, F071, F072, F078 | `.small_button("\u{2190} Back")` |
| `fe-ui/src/panels/path_editor_card.rs:313` | F063, F066, F067, F069, F070, F071, F072, F078 | `Button::new("Export GPX")` |
| `fe-ui/src/panels/path_editor_card.rs:474` | F063, F066, F067, F069, F070, F071, F072, F078 | `.small_button("Remove")` |
| `fe-ui/src/panels/path_editor_card.rs:477` | F063, F066, F067, F069, F070, F071, F072, F078 | `.small_button("Annotate")` |
| `fe-ui/src/panels/path_editor_card.rs:483` | F063, F066, F067, F069, F070, F071, F072, F078 | `DragValue::new(&mut new_y)` |
| `fe-ui/src/panels/path_editor_card.rs:579` | F063, F066, F067, F069, F070, F071, F072, F078 | `color_edit_button_srgba( ui, &mut rgba, egui::color_picker::Alpha::Opaque, )` |
| `fe-ui/src/panels/path_editor_card.rs:611` | F063, F066, F067, F069, F070, F071, F072, F078 | `Slider::new(&mut w, 0.1..=20.0)` |
| `fe-ui/src/panels/path_editor_card.rs:627` | F063, F066, F067, F069, F070, F071, F072, F078 | `Checkbox::new(&mut v, "Visible")` |
| `fe-ui/src/panels/path_editor_card.rs:636` | F063, F066, F067, F069, F070, F071, F072, F078 | `Checkbox::new(&mut closed, "Closed zone")` |
| `fe-ui/src/panels/path_editor_card.rs:652` | F063, F066, F067, F069, F070, F071, F072, F078 | `color_edit_button_srgba( ui, &mut rgba, egui::color_picker::Alpha::OnlyBlend, )` |
| `fe-ui/src/panels/path_editor_card.rs:695` | F063, F066, F067, F069, F070, F071, F072, F078 | `.checkbox(&mut snap.enabled, "Enable snapping")` |
| `fe-ui/src/panels/path_editor_card.rs:700` | F063, F066, F067, F069, F070, F071, F072, F078 | `DragValue::new(&mut snap.grid_step_m)` |
| `fe-ui/src/panels/path_editor_card.rs:708` | F063, F066, F067, F069, F070, F071, F072, F078 | `DragValue::new(&mut snap.anchor_radius_m)` |
| `fe-ui/src/panels/path_editor_card.rs:873` | F063, F066, F067, F069, F070, F071, F072, F078 | `.selectable_label(row.corner == kind, kind.label())` |
| `fe-ui/src/panels/path_editor_card.rs:904` | F063, F066, F067, F069, F070, F071, F072, F078 | `Slider::new(&mut s, 0.0..=1.0)` |
| `fe-ui/src/panels/path_editor_card.rs:952` | F063, F066, F067, F069, F070, F071, F072, F078 | `Slider::new(&mut s, 0.0..=1.0)` |
| `fe-ui/src/panels/path_editor_card.rs:1002` | F063, F066, F067, F069, F070, F071, F072, F078 | `TextEdit::singleline(&mut path_state.annotate_title_buf)` |
| `fe-ui/src/panels/path_editor_card.rs:1009` | F063, F066, F067, F069, F070, F071, F072, F078 | `TextEdit::singleline(&mut path_state.annotate_body_buf)` |
| `fe-ui/src/panels/path_editor_card.rs:1016` | F063, F066, F067, F069, F070, F071, F072, F078 | `TextEdit::singleline(&mut path_state.annotate_color_buf)` |
| `fe-ui/src/panels/path_editor_card.rs:1024` | F063, F066, F067, F069, F070, F071, F072, F078 | `Button::new("Save")` |
| `fe-ui/src/panels/path_editor_card.rs:1037` | F063, F066, F067, F069, F070, F071, F072, F078 | `Button::new("Cancel")` |
| `fe-ui/src/panels/portal_toolbar.rs:29` | F025 | `Button::new( egui::RichText::new("\u{2190}").size(16.0), // ← )` |
| `fe-ui/src/panels/portal_toolbar.rs:59` | F025 | `Button::new( egui::RichText::new("\u{2715}").size(14.0), // ✕ )` |
| `fe-ui/src/panels/query_tab.rs:114` | F033, F034 | `Button::new(egui::RichText::new(label).small())` |
| `fe-ui/src/panels/query_tab.rs:145` | F033, F034 | `TextEdit::multiline(&mut inspector.query_sql_buf)` |
| `fe-ui/src/panels/query_tab.rs:167` | F033, F034 | `Button::new(if inspector.query_results.pending() { "Running..." } else { "Run Query" })` |
| `fe-ui/src/panels/sidebar.rs:83` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new( egui::RichText::new("Reset Database") .small() .color(theme::TEXT_DIM), )` |
| `fe-ui/src/panels/sidebar.rs:102` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new( egui::RichText::new("Confirm Reset") .color(egui::Color32::WHITE), )` |
| `fe-ui/src/panels/sidebar.rs:115` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new("Cancel")` |
| `fe-ui/src/panels/sidebar.rs:158` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new("+")` |
| `fe-ui/src/panels/sidebar.rs:170` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new("Join")` |
| `fe-ui/src/panels/sidebar.rs:187` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new( egui::RichText::new(glyph) .small() .color(theme::TREE_NODE_ICON), )` |
| `fe-ui/src/panels/sidebar.rs:459` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new(egui::RichText::new("Manifest").small())` |
| `fe-ui/src/panels/sidebar.rs:638` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `.button(label)` |
| `fe-ui/src/panels/sidebar.rs:711` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new("+")` |
| `fe-ui/src/panels/sidebar.rs:735` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `CollapsingHeader::new( egui::RichText::new("Groups") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/sidebar.rs:785` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `Button::new("Manage Groups…")` |
| `fe-ui/src/panels/sidebar.rs:796` | F001, F002, F003, F004, F005, F006, F007, F008, F010, F086, F124 | `CollapsingHeader::new( egui::RichText::new("Space") .strong() .color(theme::TEXT_SECTION), )` |
| `fe-ui/src/panels/status_bar.rs:206` | F087, F104 | `.small_button("Copy")` |
| `fe-ui/src/panels/status_bar.rs:211` | F087, F104 | `.small_button("Dismiss")` |
| `fe-ui/src/panels/terrain_tools_panel.rs:64` | F050, F051, F052, F057 | `Button::new(mode.label())` |
| `fe-ui/src/panels/terrain_tools_panel.rs:168` | F050, F051, F052, F057 | `DragValue::new(&mut radius_display)` |
| `fe-ui/src/panels/terrain_tools_panel.rs:187` | F050, F051, F052, F057 | `DragValue::new(&mut height_display)` |
| `fe-ui/src/panels/terrain_tools_panel.rs:202` | F050, F051, F052, F057 | `DragValue::new(&mut delta_display)` |
| `fe-ui/src/panels/terrain_tools_panel.rs:215` | F050, F051, F052, F057 | `.button(format!( "Add {} proposal", tool_panel.terrain_tool_mode.label() ))` |
| `fe-ui/src/panels/terrain_tools_panel.rs:298` | F050, F051, F052, F057 | `.selectable_label(selected, label.as_str())` |
| `fe-ui/src/panels/terrain_tools_panel.rs:302` | F050, F051, F052, F057 | `.small_button("\u{2715}")` |
| `fe-ui/src/panels/terrain_tools_panel.rs:392` | F050, F051, F052, F057 | `DragValue::new(&mut sculpt.radius)` |
| `fe-ui/src/panels/terrain_tools_panel.rs:404` | F050, F051, F052, F057 | `Slider::new(&mut sculpt.strength, 0.0..=1.0)` |
| `fe-ui/src/panels/terrain_tools_panel.rs:413` | F050, F051, F052, F057 | `.selectable_value(&mut sculpt.op, op, op.label())` |
| `fe-ui/src/panels/terrain_tools_panel.rs:425` | F050, F051, F052, F057 | `DragValue::new(&mut sculpt.target_height)` |
| `fe-ui/src/panels/terrain_tools_panel.rs:435` | F050, F051, F052, F057 | `DragValue::new(&mut sculpt.delta)` |
| `fe-ui/src/panels/terrain_tools_panel.rs:455` | F050, F051, F052, F057 | `TextEdit::singleline(&mut sculpt.material)` |
| `fe-ui/src/panels/tool_options.rs:321` | F052, F053, F054, F058, F059, F065, F067 | `.selectable_value(&mut sculpt.shape_mode, mode, mode.label())` |
| `fe-ui/src/panels/tool_options.rs:348` | F052, F053, F054, F058, F059, F065, F067 | `.small_button("Clear")` |
| `fe-ui/src/panels/tool_options.rs:368` | F052, F053, F054, F058, F059, F065, F067 | `Button::new(format!("Apply {} region", sculpt.shape_mode.label()))` |
| `fe-ui/src/panels/tool_options.rs:408` | F052, F053, F054, F058, F059, F065, F067 | `.button("Terrain Tools \u{2192}")` |
| `fe-ui/src/panels/tool_options.rs:415` | F052, F053, F054, F058, F059, F065, F067 | `.button("Earthwork report \u{2192}")` |
| `fe-ui/src/panels/tool_options.rs:445` | F052, F053, F054, F058, F059, F065, F067 | `.selectable_label(measure.mode == mode, mode.label())` |
| `fe-ui/src/panels/tool_options.rs:483` | F052, F053, F054, F058, F059, F065, F067 | `Button::new("Undo")` |
| `fe-ui/src/panels/tool_options.rs:489` | F052, F053, F054, F058, F059, F065, F067 | `Button::new("Clear")` |
| `fe-ui/src/panels/tool_options.rs:497` | F052, F053, F054, F058, F059, F065, F067 | `Button::new("Close polygon")` |
| `fe-ui/src/panels/tool_options.rs:508` | F052, F053, F054, F058, F059, F065, F067 | `Button::new("Save as callout")` |
| `fe-ui/src/panels/tool_panel.rs:302` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.spacing_mode, SpacingMode::FixedSpacing, "Fixed spacing", )` |
| `fe-ui/src/panels/tool_panel.rs:307` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.spacing_mode, SpacingMode::FixedCount, "Fixed count", )` |
| `fe-ui/src/panels/tool_panel.rs:326` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.spacing_value)` |
| `fe-ui/src/panels/tool_panel.rs:337` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.count_value)` |
| `fe-ui/src/panels/tool_panel.rs:343` | F066, F067, F068, F073, F074, F075 | `.checkbox(&mut state.tangent_align, "Align to path tangent")` |
| `fe-ui/src/panels/tool_panel.rs:362` | F066, F067, F068, F073, F074, F075 | `Button::new("Stamp along path")` |
| `fe-ui/src/panels/tool_panel.rs:427` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.stamp_edit_index)` |
| `fe-ui/src/panels/tool_panel.rs:430` | F066, F067, F068, F073, F074, F075 | `.button("Select")` |
| `fe-ui/src/panels/tool_panel.rs:440` | F066, F067, F068, F073, F074, F075 | `.button("Promote")` |
| `fe-ui/src/panels/tool_panel.rs:456` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.stamp_scale)` |
| `fe-ui/src/panels/tool_panel.rs:460` | F066, F067, F068, F073, F074, F075 | `.button("Apply")` |
| `fe-ui/src/panels/tool_panel.rs:478` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.stamp_yaw_deg)` |
| `fe-ui/src/panels/tool_panel.rs:482` | F066, F067, F068, F073, F074, F075 | `.button("Apply")` |
| `fe-ui/src/panels/tool_panel.rs:500` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.stamp_arc_m)` |
| `fe-ui/src/panels/tool_panel.rs:508` | F066, F067, F068, F073, F074, F075 | `.button("Apply")` |
| `fe-ui/src/panels/tool_panel.rs:558` | F066, F067, F068, F073, F074, F075 | `.small_button("\u{2715}")` |
| `fe-ui/src/panels/tool_panel.rs:593` | F066, F067, F068, F073, F074, F075 | `TextEdit::singleline(&mut state.asset_filter)` |
| `fe-ui/src/panels/tool_panel.rs:616` | F066, F067, F068, F073, F074, F075 | `.selectable_label(selected, egui::RichText::new(&row.name).small())` |
| `fe-ui/src/panels/tool_panel.rs:628` | F066, F067, F068, F073, F074, F075 | `CollapsingHeader::new( egui::RichText::new("Or paste a blob:// path") .small() .color(theme::TEXT_DIM), )` |
| `fe-ui/src/panels/tool_panel.rs:637` | F066, F067, F068, F073, F074, F075 | `TextEdit::singleline(&mut asset_buf)` |
| `fe-ui/src/panels/tool_panel.rs:693` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.pen_mode, PenMode::Polyline, PenMode::Polyline.label(), )` |
| `fe-ui/src/panels/tool_panel.rs:698` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.pen_mode, PenMode::CatmullRom, PenMode::CatmullRom.label(), )` |
| `fe-ui/src/panels/tool_panel.rs:703` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.pen_mode, PenMode::Bezier, PenMode::Bezier.label(), )` |
| `fe-ui/src/panels/tool_panel.rs:719` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.pen_new_anchor_kind, CornerKind::Corner, CornerKind::Corner.label(), )` |
| `fe-ui/src/panels/tool_panel.rs:724` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.pen_new_anchor_kind, CornerKind::Smooth, CornerKind::Smooth.label(), )` |
| `fe-ui/src/panels/tool_panel.rs:729` | F066, F067, F068, F073, F074, F075 | `.selectable_value( &mut state.pen_new_anchor_kind, CornerKind::Symmetric, CornerKind::Symmetric.label(), )` |
| `fe-ui/src/panels/tool_panel.rs:744` | F066, F067, F068, F073, F074, F075 | `Slider::new(&mut state.pen_tension, 0.0..=1.0)` |
| `fe-ui/src/panels/tool_panel.rs:755` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut samples)` |
| `fe-ui/src/panels/tool_panel.rs:766` | F066, F067, F068, F073, F074, F075 | `Button::new("Smooth path")` |
| `fe-ui/src/panels/tool_panel.rs:787` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.shape_radius)` |
| `fe-ui/src/panels/tool_panel.rs:797` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut state.shape_radius_z)` |
| `fe-ui/src/panels/tool_panel.rs:810` | F066, F067, F068, F073, F074, F075 | `DragValue::new(&mut segs)` |
| `fe-ui/src/panels/tool_panel.rs:818` | F066, F067, F068, F073, F074, F075 | `.button("Add Ellipse")` |
| `fe-ui/src/panels/tool_panel.rs:831` | F066, F067, F068, F073, F074, F075 | `.button("Add Circle")` |
| `fe-ui/src/panels/tool_panel.rs:843` | F066, F067, F068, F073, F074, F075 | `.button("Add Rectangle")` |
| `fe-ui/src/panels/volume_report_panel.rs:486` | F055, F056 | `.button("Copy as TSV")` |
| `fe-ui/src/panels/widgets.rs:43` | F027, F037, F044, F059, F088 | `Button::new("\u{1F4CB}")` |
| `fe-ui/src/panels/widgets.rs:217` | F027, F037, F044, F059, F088 | `Button::new(egui::RichText::new(confirm_label).small())` |
| `fe-ui/src/panels/widgets.rs:227` | F027, F037, F044, F059, F088 | `Button::new(egui::RichText::new("Cancel").small())` |
| `fe-ui/src/panels/widgets.rs:244` | F027, F037, F044, F059, F088 | `Button::new(egui::RichText::new(label).small())` |
| `fe-ui/src/ui_shell/results_dock.rs:222` | F035, F036, F037, F038 | `.button("Copy row")` |
| `fe-ui/src/ui_shell/results_dock.rs:227` | F035, F036, F037, F038 | `.button("Close Results")` |
| `fe-ui/src/ui_shell/results_dock.rs:278` | F035, F036, F037, F038 | `Button::new(label)` |
| `fe-ui/src/ui_shell/results_dock.rs:286` | F035, F036, F037, F038 | `.allocate_exact_size( egui::vec2(6.0, row_height), egui::Sense::drag(), )` |
| `fe-ui/src/ui_shell/results_dock.rs:334` | F035, F036, F037, F038 | `.button("Copy cell")` |
| `fe-ui/src/ui_shell/results_dock.rs:338` | F035, F036, F037, F038 | `.button("Copy row")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:615` | F010, F011, F060, F061, F082, F083, F084, F085 | `DragValue::new(&mut app_settings.render_distance)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:669` | F010, F011, F060, F061, F082, F083, F084, F085 | `.checkbox(&mut app_settings.show_scale_bar, "Scale bar")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:671` | F010, F011, F060, F061, F082, F083, F084, F085 | `.checkbox(&mut app_settings.show_world_grid, "Metric grid")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:678` | F010, F011, F060, F061, F082, F083, F084, F085 | `.checkbox(&mut offline, "Offline tiles (no network)")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:748` | F010, F011, F060, F061, F082, F083, F084, F085 | `DragValue::new(&mut value)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:781` | F010, F011, F060, F061, F082, F083, F084, F085 | `DragValue::new(&mut value)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:822` | F010, F011, F060, F061, F082, F083, F084, F085 | `DragValue::new(&mut wide)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:832` | F010, F011, F060, F061, F082, F083, F084, F085 | `DragValue::new(value)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:847` | F010, F011, F060, F061, F082, F083, F084, F085 | `TextEdit::singleline(&mut buffer)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:886` | F010, F011, F060, F061, F082, F083, F084, F085 | `TextEdit::singleline(&mut buffer)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:954` | F010, F011, F060, F061, F082, F083, F084, F085 | `.radio(selected, tss::mode_label(mode))` |
| `fe-ui/src/ui_shell/right_sidebar.rs:978` | F010, F011, F060, F061, F082, F083, F084, F085 | `Slider::new(&mut r, 1..=tss::MAX_REPLICATION_FACTOR)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1010` | F010, F011, F060, F061, F082, F083, F084, F085 | `DragValue::new(&mut buf_u64)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1029` | F010, F011, F060, F061, F082, F083, F084, F085 | `.selectable_label(selected, label)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1188` | F010, F011, F060, F061, F082, F083, F084, F085 | `TextEdit::singleline(&mut new_name)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1197` | F010, F011, F060, F061, F082, F083, F084, F085 | `Button::new("Add group")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1226` | F010, F011, F060, F061, F082, F083, F084, F085 | `.checkbox(&mut visible, "")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1241` | F010, F011, F060, F061, F082, F083, F084, F085 | `TextEdit::singleline(&mut name)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1258` | F010, F011, F060, F061, F082, F083, F084, F085 | `Button::new("\u{25B2}")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1267` | F010, F011, F060, F061, F082, F083, F084, F085 | `Button::new("\u{25BC}")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1275` | F010, F011, F060, F061, F082, F083, F084, F085 | `Button::new("Delete")` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1303` | F010, F011, F060, F061, F082, F083, F084, F085 | `Button::new(label)` |
| `fe-ui/src/ui_shell/right_sidebar.rs:1396` | F010, F011, F060, F061, F082, F083, F084, F085 | `Button::new(glyph)` |
| `fe-ui/src/ui_shell/topbar.rs:282` | F079, F080, F081, F082, F089, F090 | `.allocate_exact_size(size, egui::Sense::click())` |
| `fe-ui/src/ui_shell/topbar.rs:488` | F079, F080, F081, F082, F089, F090 | `.button("Cancel close")` |
| `fe-ui/src/ui_shell/topbar.rs:491` | F079, F080, F081, F082, F089, F090 | `.button("Close FractalEngine")` |
| `fe-ui/src/ui_shell/topbar.rs:509` | F079, F080, F081, F082, F089, F090 | `Button::new(caption)` |
| `fe-ui/src/ui_shell/topbar.rs:527` | F079, F080, F081, F082, F089, F090 | `Button::new("\u{1F5FA} Data")` |
| `fe-ui/src/ui_shell/topbar.rs:543` | F079, F080, F081, F082, F089, F090 | `Button::new("\u{1F527} Options")` |
| `fe-ui/src/ui_shell/topbar.rs:560` | F079, F080, F081, F082, F089, F090 | `Button::new("\u{2699} Settings")` |
| `fe-ui/src/ui_shell/topbar.rs:576` | F079, F080, F081, F082, F089, F090 | `Button::new("\u{1F4E6} Maps")` |
| `fe-ui/src/ui_shell/topbar.rs:642` | F079, F080, F081, F082, F089, F090 | `Button::new("\u{2630}")` |
| `fe-ui/src/ui_shell/topbar.rs:652` | F079, F080, F081, F082, F089, F090 | `.menu_button("Tools", &#124;ui&#124; { tool_buttons(ui, tool, right); if node_mgr.selected_entity().is_some() && ui.button("Deselect").clicked() { node_mgr.deselect(); } })` |
| `fe-ui/src/ui_shell/topbar.rs:654` | F079, F080, F081, F082, F089, F090 | `.button("Deselect")` |
| `fe-ui/src/ui_shell/topbar.rs:676` | F079, F080, F081, F082, F089, F090 | `Button::new("\u{2715} Deselect")` |
| `fe-ui/src/ui_shell/topbar.rs:689` | F079, F080, F081, F082, F089, F090 | `.menu_button("Panels", &#124;ui&#124; section_buttons(ui, gis_panel, right))` |
| `fe-ui/src/viewport.rs:90` | F001, F002, F003, F093 | `Button::new("\u{25C0} Back")` |
| `fe-ui/src/viewport.rs:222` | F001, F002, F003, F093 | `Button::new( egui::RichText::new(&verse.name) .strong() .size(13.0) .color(theme::TEXT_BRIGHT), )` |
| `fe-ui/src/viewport.rs:246` | F001, F002, F003, F093 | `Button::new( egui::RichText::new("\u{2699}") .size(14.0) .color(theme::ICON_GEAR), )` |
| `fe-ui/src/viewport.rs:312` | F001, F002, F003, F093 | `Button::new( egui::RichText::new("+ New Verse") .size(13.0) .color(theme::TEXT_DIM), )` |
| `fe-ui/src/viewport.rs:416` | F001, F002, F003, F093 | `Button::new(egui::RichText::new("\u{25C0} Verses").small())` |
| `fe-ui/src/viewport.rs:472` | F001, F002, F003, F093 | `Button::new( egui::RichText::new(&fractal.name) .strong() .size(13.0) .color(theme::TEXT_BRIGHT), )` |
| `fe-ui/src/viewport.rs:496` | F001, F002, F003, F093 | `Button::new( egui::RichText::new("\u{2699}") .size(14.0) .color(theme::ICON_GEAR), )` |
| `fe-ui/src/viewport.rs:568` | F001, F002, F003, F093 | `Button::new( egui::RichText::new("+ New Fractal") .size(13.0) .color(theme::TEXT_DIM), )` |
| `fe-ui/src/viewport.rs:619` | F001, F002, F003, F093 | `Button::new(egui::RichText::new("\u{25C0} Fractals").small())` |
| `fe-ui/src/viewport.rs:697` | F001, F002, F003, F093 | `Button::new( egui::RichText::new(&petal.name) .strong() .size(13.0) .color(theme::TEXT_BRIGHT), )` |
| `fe-ui/src/viewport.rs:721` | F001, F002, F003, F093 | `Button::new( egui::RichText::new("\u{2699}") .size(14.0) .color(theme::ICON_GEAR), )` |
| `fe-ui/src/viewport.rs:795` | F001, F002, F003, F093 | `Button::new( egui::RichText::new("+ New Petal") .size(13.0) .color(theme::TEXT_DIM), )` |

## MCP inventory and acceptance ownership

J09 owns every tool below, including success, role/scope denial and REST parity where a REST twin exists.
GUI counterparts must still be driven through their GUI journey; a tools/call is API evidence only.
Each tool has an explicit F116.mcp-<tool-name> checkpoint, alongside F062/F118/F126 specialized cases.

| Tool | Minimum role | Declared scope/seam (source excerpt) | Source |
|---|---|---|---|
| `get_hierarchy` | Viewer | `ScopeRule::None,` | `fe-api/src/mcp/mod.rs:209` |
| `create_verse` | Manager | `Global,` | `fe-api/src/mcp/mod.rs:211` |
| `create_fractal` | Editor | `HierarchyArgs(Verse),` | `fe-api/src/mcp/mod.rs:213` |
| `create_petal` | Editor | `HierarchyArgs(Fractal),` | `fe-api/src/mcp/mod.rs:215` |
| `create_node` | Editor | `HierarchyArgs(Petal),` | `fe-api/src/mcp/mod.rs:217` |
| `update_transform` | Editor | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:219` |
| `read_node` | Viewer | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:221` |
| `node_address` | Viewer | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:223` |
| `delete_node` | Editor | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:225` |
| `promote_instance` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:227` |
| `query_timeseries` | Viewer | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:229` |
| `sim_start` | Owner | `Global, requires SimControl,` | `fe-api/src/mcp/mod.rs:231` |
| `sim_stop` | Owner | `Global, requires SimControl,` | `fe-api/src/mcp/mod.rs:233` |
| `sim_status` | Owner | `Global, requires SimControl,` | `fe-api/src/mcp/mod.rs:235` |
| `sim_step` | Owner | `Global, requires SimControl,` | `fe-api/src/mcp/mod.rs:237` |
| `sim_inject_fault` | Owner | `Global, requires SimControl,` | `fe-api/src/mcp/mod.rs:239` |
| `upload_asset` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:242` |
| `place_asset` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:244` |
| `set_property` | Editor | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:246` |
| `get_properties` | Viewer | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:248` |
| `delete_property` | Editor | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:250` |
| `create_waypoint` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:252` |
| `move_waypoint` | Editor | `NodeArg("node_id"),` | `fe-api/src/mcp/mod.rs:254` |
| `import_gpx` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:256` |
| `set_petal_terrain` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:258` |
| `set_petal_render_distance` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:260` |
| `list_visibility_groups` | Viewer | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:262` |
| `set_visibility_group` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:264` |
| `list_tilesets` | Viewer | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:266` |
| `install_tileset` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:268` |
| `get_gis_nodes` | Viewer | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:270` |
| `query` | Viewer | `ScopeRule::None,` | `fe-api/src/mcp/mod.rs:272` |
| `register_device_format` | Manager | `VerseArg("verse_id"),` | `fe-api/src/mcp/mod.rs:275` |
| `list_device_formats` | Viewer | `VerseArg("verse_id"),` | `fe-api/src/mcp/mod.rs:277` |
| `ingest_device_payload` | Editor | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:279` |
| `list_device_payloads` | Viewer | `PetalArg("petal_id"),` | `fe-api/src/mcp/mod.rs:281` |
| `screenshot` | Owner | `Global, requires Screenshot, content,` | `fe-api/src/mcp/mod.rs:284` |
| `get_camera` | Owner | `Global, requires Screenshot,` | `fe-api/src/mcp/mod.rs:286` |
| `ui_tree` | Owner | `Global, requires QaDebug, "Read the primary window accessibility tree in physical screenshot pixels. Scoped to the active verse; FE_API_INPUT_ALLOW=1 required. Requires owner role."),` | `fe-api/src/mcp/mod.rs:291` |
| `dialog_answers` | Owner | `Global, requires QaDebug, "Queue bounded open/save/folder answers for QA file dialogs in the active verse. Requires input opt-in, an actual loopback listener and an isolated QA root; paths stay inside that run. Requires owner role."),` | `fe-api/src/mcp/mod.rs:292` |
| `window` | Owner | `Global, requires QaDebug, "Request QA window size in physical pixels, app UI scale, maximize/restore/minimize, or close confirmation. Requires input opt-in, loopback listener and isolated QA root. Close never bypasses confirmation. Requires owner role."),` | `fe-api/src/mcp/mod.rs:293` |
| `ui_state` | Owner | `Global, requires QaDebug, "Read authoritative navigation, selection, tool, viewport, notices and pending state. FE_API_INPUT_ALLOW=1 required. Requires owner role."),` | `fe-api/src/mcp/mod.rs:294` |
| `ui_logs` | Owner | `Global, requires QaDebug, "Read a bounded, redacted log tail for the current verse generation. FE_API_INPUT_ALLOW=1 required. Requires owner role."),` | `fe-api/src/mcp/mod.rs:295` |
| `inject_input` | Owner | `Global, requires Input,` | `fe-api/src/mcp/mod.rs:296` |


## Registered REST method/path cases

Every row has its own F116.rest-<method>-<sanitized-path> checkpoint in the JSON catalog.
Resolve braces from the owned fixture; create disposable resources for destructive calls. Public or WebSocket
routes retain their real auth/handshake semantics. An unavailable opt-in backend is BLOCKED, never PASS.

| Method | Path | Source | Checkpoint |
|---|---|---|---|
| GET | `/api/v1/health` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-health` |
| GET | `/ready` | `fe-api/src/server.rs::build_router` | `F116.rest-get-ready` |
| GET | `/ws` | `fe-api/src/server.rs::build_router` | `F116.rest-get-ws` |
| GET | `/api/v1/shared/{token}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-shared-token` |
| GET | `/api/v1/hierarchy` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-hierarchy` |
| POST | `/api/v1/verses` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-verses` |
| POST | `/api/v1/verses/{verse_id}/fractals` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-verses-verse-id-fractals` |
| POST | `/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-verses-verse-id-fractals-fractal-id-petals` |
| POST | `/api/v1/verses/{verse_id}/fractals/{fractal_id}/petals/{petal_id}/nodes` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-verses-verse-id-fractals-fractal-id-petals-petal-id-nodes` |
| DELETE | `/api/v1/nodes/{node_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-nodes-node-id` |
| GET | `/api/v1/nodes/{node_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-node-id` |
| GET | `/api/v1/nodes/{node_id}/address` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-node-id-address` |
| GET | `/api/v1/address` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-address` |
| GET | `/api/v1/petals/{petal_id}/nodes` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-nodes` |
| GET | `/api/v1/petals/{petal_id}/earthwork/summary` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-earthwork-summary` |
| POST | `/api/v1/petals/{petal_id}/paths/{path_id}/instances/{instance_index}/promote` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-paths-path-id-instances-instance-index-promote` |
| GET | `/api/v1/nodes/{node_id}/transform` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-node-id-transform` |
| PATCH | `/api/v1/nodes/{node_id}/transform` | `fe-api/src/server.rs::build_router` | `F116.rest-patch-api-v1-nodes-node-id-transform` |
| GET | `/api/v1/nodes/{node_id}/properties` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-node-id-properties` |
| PATCH | `/api/v1/nodes/{node_id}/properties` | `fe-api/src/server.rs::build_router` | `F116.rest-patch-api-v1-nodes-node-id-properties` |
| DELETE | `/api/v1/nodes/{node_id}/properties/{key}` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-nodes-node-id-properties-key` |
| GET | `/api/v1/assets/{content_hash}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-assets-content-hash` |
| GET | `/api/v1/assets/by-id/{asset_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-assets-by-id-asset-id` |
| POST | `/api/v1/petals/{petal_id}/assets` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-assets` |
| GET | `/api/v1/nodes/{node_id}/asset` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-node-id-asset` |
| GET | `/api/v1/petals/{petal_id}/export` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-export` |
| POST | `/api/v1/petals/{petal_id}/import` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-import` |
| POST | `/api/v1/petals/{petal_id}/import/gpx` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-import-gpx` |
| GET | `/api/v1/petals/{petal_id}/export/gpx` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-export-gpx` |
| DELETE | `/api/v1/petals/{petal_id}/terrain` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-petals-petal-id-terrain` |
| GET | `/api/v1/petals/{petal_id}/terrain` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-terrain` |
| PUT | `/api/v1/petals/{petal_id}/terrain` | `fe-api/src/server.rs::build_router` | `F116.rest-put-api-v1-petals-petal-id-terrain` |
| DELETE | `/api/v1/petals/{petal_id}/render-distance` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-petals-petal-id-render-distance` |
| GET | `/api/v1/petals/{petal_id}/render-distance` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-render-distance` |
| PUT | `/api/v1/petals/{petal_id}/render-distance` | `fe-api/src/server.rs::build_router` | `F116.rest-put-api-v1-petals-petal-id-render-distance` |
| GET | `/api/v1/petals/{petal_id}/visibility-groups` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-visibility-groups` |
| PUT | `/api/v1/petals/{petal_id}/visibility-groups` | `fe-api/src/server.rs::build_router` | `F116.rest-put-api-v1-petals-petal-id-visibility-groups` |
| DELETE | `/api/v1/petals/{petal_id}/visibility-groups/{group_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-petals-petal-id-visibility-groups-group-id` |
| PATCH | `/api/v1/petals/{petal_id}/visibility-groups/{group_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-patch-api-v1-petals-petal-id-visibility-groups-group-id` |
| GET | `/api/v1/tiles/elevation/{tileset_id}/{z}/{x}/{y_png}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-tiles-elevation-tileset-id-z-x-y-png` |
| GET | `/api/v1/tiles/satellite/{tileset_id}/{z}/{x}/{y_jpg}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-tiles-satellite-tileset-id-z-x-y-jpg` |
| GET | `/api/v1/tilesets` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-tilesets` |
| GET | `/api/v1/tilesets/{tileset_id}/meta` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-tilesets-tileset-id-meta` |
| GET | `/api/v1/petals/{petal_id}/gis/nodes` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-gis-nodes` |
| GET | `/api/v1/petals/{petal_id}/gis/tracks` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-gis-tracks` |
| POST | `/api/v1/petals/{petal_id}/waypoints` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-waypoints` |
| PATCH | `/api/v1/nodes/{waypoint_id}/move` | `fe-api/src/server.rs::build_router` | `F116.rest-patch-api-v1-nodes-waypoint-id-move` |
| GET | `/api/v1/nodes/{track_id}/elevation-profile` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-track-id-elevation-profile` |
| GET | `/api/v1/nodes/{track_id}/stats` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-track-id-stats` |
| POST | `/api/v1/field-defs` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-field-defs` |
| GET | `/api/v1/field-defs/{scope}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-field-defs-scope` |
| DELETE | `/api/v1/field-defs/by-id/{field_def_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-field-defs-by-id-field-def-id` |
| PATCH | `/api/v1/field-defs/by-id/{field_def_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-patch-api-v1-field-defs-by-id-field-def-id` |
| POST | `/api/v1/nodes` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-nodes` |
| GET | `/api/v1/debug/screenshot` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-debug-screenshot` |
| POST | `/api/v1/debug/screenshot` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-debug-screenshot` |
| GET | `/api/v1/debug/camera` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-debug-camera` |
| POST | `/api/v1/debug/input` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-debug-input` |
| GET | `/api/v1/debug/ui-tree` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-debug-ui-tree` |
| GET | `/api/v1/debug/ui-state` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-debug-ui-state` |
| GET | `/api/v1/debug/logs` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-debug-logs` |
| POST | `/api/v1/debug/dialog-answers` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-debug-dialog-answers` |
| POST | `/api/v1/debug/window` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-debug-window` |
| POST | `/api/v1/query` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-query` |
| POST | `/api/v1/query/elevated` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-query-elevated` |
| GET | `/api/v1/petals/{petal_id}/export.parquet` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-export-parquet` |
| GET | `/api/v1/petals/{petal_id}/export.csv` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-export-csv` |
| POST | `/api/v1/petals/{petal_id}/iot/readings` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-iot-readings` |
| GET | `/api/v1/verses/{verse_id}/device-formats` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-verses-verse-id-device-formats` |
| POST | `/api/v1/verses/{verse_id}/device-formats` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-verses-verse-id-device-formats` |
| GET | `/api/v1/petals/{petal_id}/iot/payloads` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-petals-petal-id-iot-payloads` |
| POST | `/api/v1/petals/{petal_id}/iot/payloads` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-iot-payloads` |
| POST | `/api/v1/query/share` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-query-share` |
| POST | `/api/v1/analytics/query` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-analytics-query` |
| POST | `/api/v1/hexons/tilesets/install` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-hexons-tilesets-install` |
| DELETE | `/api/v1/hexons/tilesets/{hexon_id}` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-hexons-tilesets-hexon-id` |
| PATCH | `/api/v1/hexons/tilesets/{hexon_id}/seeding` | `fe-api/src/server.rs::build_router` | `F116.rest-patch-api-v1-hexons-tilesets-hexon-id-seeding` |
| GET | `/api/v1/hexons/tilesets` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-hexons-tilesets` |
| GET | `/api/v1/hexons/storage` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-hexons-storage` |
| POST | `/api/v1/crates/publish` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-crates-publish` |
| POST | `/api/v1/crates/{hexon_uri}/install` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-crates-hexon-uri-install` |
| DELETE | `/api/v1/crates/{hexon_uri}/uninstall` | `fe-api/src/server.rs::build_router` | `F116.rest-delete-api-v1-crates-hexon-uri-uninstall` |
| GET | `/api/v1/crates/search` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-crates-search` |
| GET | `/api/v1/crates/installed` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-crates-installed` |
| GET | `/api/v1/crates/{hexon_uri}` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-crates-hexon-uri` |
| GET | `/api/v1/crates/{hexon_uri}/entries` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-crates-hexon-uri-entries` |
| GET | `/api/v1/crates/{hexon_uri}/entries/{entry_id}/asset` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-crates-hexon-uri-entries-entry-id-asset` |
| GET | `/api/v1/crates/available` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-crates-available` |
| POST | `/api/v1/sim/start` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-sim-start` |
| POST | `/api/v1/sim/stop` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-sim-stop` |
| GET | `/api/v1/sim/status` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-sim-status` |
| POST | `/api/v1/sim/step` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-sim-step` |
| POST | `/api/v1/sim/inject-fault` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-sim-inject-fault` |
| POST | `/mcp` | `fe-api/src/server.rs::build_router` | `F116.rest-post-mcp` |


## Rich-data API follow-up (source branch `qa/rich-data-core`, frozen public contracts `1fe217f`)

These six REST cases and five MCP tools extend the preparation inventory. Read-only source reconciliation is not execution evidence. Main must reconcile the integrated source fingerprint and new GUI producers before final acceptance. The existing API authorization/denial obligations remain mandatory in J09.

| Method | Registered path | Producer | Checkpoint |
|---|---|---|---|
| POST | `/api/v1/petals/{petal_id}/objects` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-petals-petal-id-objects` |
| POST | `/api/v1/nodes/{node_id}/mutations` | `fe-api/src/server.rs::build_router` | `F116.rest-post-api-v1-nodes-node-id-mutations` |
| GET | `/api/v1/nodes/{node_id}/history` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-node-id-history` |
| GET | `/api/v1/nodes/{node_id}/state` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-nodes-node-id-state` |
| GET | `/api/v1/field-defs/by-id/{field_def_id}/schema` | `fe-api/src/server.rs::build_router` | `F116.rest-get-api-v1-field-defs-by-id-field-def-id-schema` |
| PUT | `/api/v1/field-defs/by-id/{field_def_id}/schema` | `fe-api/src/server.rs::build_router` | `F116.rest-put-api-v1-field-defs-by-id-field-def-id-schema` |

| MCP tool | Required role | Scope resolver | Checkpoint |
|---|---|---|---|
| `get_node_history` | Viewer | `HistoryNodeArg("node_id")` | `F116.mcp-get-node-history` |
| `read_node_at_version` | Viewer | `HistoryNodeArg("node_id")` | `F116.mcp-read-node-at-version` |
| `commit_node_mutation` | Editor | `NodeArg("node_id")` | `F116.mcp-commit-node-mutation` |
| `get_field_schema` | Viewer | `FieldDefArg("field_def_id")` | `F116.mcp-get-field-schema` |
| `set_field_schema` | Manager | `FieldDefArg("field_def_id")` | `F116.mcp-set-field-schema` |

`GET state` requires `at_version`; no current-version default. New-object creation has a committed baseline v1. History is ordered by each object's committed row version, with honest coverage metadata, not sensor event time or a global timestamp. `F129`–`F132` cover the distinct rich behavior; endpoint inventory does not substitute for those assertions.
