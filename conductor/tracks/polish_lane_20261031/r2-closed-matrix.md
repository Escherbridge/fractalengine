---
type: Review
track: polish_lane_20261031
slice: UI5-06
head: 86ae3c2e
date: 2026-11-14
---

# R2 per-id closed/OUT evidence matrix (polish A8)

Source of ids: R2 research `.omc/research/polish-ux-20261031.md` (82 ids), coverage table in `plan.md` section 4. Each anchor was re-grepped at HEAD `86ae3c2e`; the plan's slice mapping was not trusted. Status: closed (the fixing code is present at the anchor), open (the symptom is still present or only partly fixed; partial evidence counts as open), OUT (excluded by spec). The commit column is the oldest commit that introduced the anchor text in that file (`git log -S`), a lower bound on when the line landed. No code was changed. Line numbers are working-tree lines at HEAD and may move.

OUT per spec (D-64 and the spec OUT list): peer blob fetch, T28, S-1, D-23, CANON-WS, CDL-NET, the INTENT items and the DW-04 share-link mint button. None of these is an R2 id except the DW-04 mint, so every R2 id below carries a closed or open status; DW-04 is closed for TTL validation and its mint half is OUT.

| id | status | evidence (file:line at HEAD) | commit / note |
|---|---|---|---|
| X-01 | closed | `fe-ui/src/actions/mod.rs:964` | 03d0acc; denial copy via notice.rs |
| X-02 | closed | `fe-ui/src/panels/widgets.rs:180` | 7ea181b; role_gated helper used by hexon_manager and shortcuts |
| X-03 | closed | `fe-ui/src/plugin.rs:1573` | 7ef208b; role cleared on scope change |
| X-04 | closed | `fe-ui/src/notice.rs:28` | 7ea181b; severity queue with 7 s errors |
| X-05 | closed | `fe-ui/src/plugin.rs:135` | 577c359; mesh budget is a persistent condition |
| X-06 | open | `fe-ui/src/panels/terrain_tools_panel.rs:316` | 7bdcfbe; proposal delete is single click while other sites use confirm_button |
| X-07 | closed | `fe-ui/src/node_manager/shortcuts.rs:74` | db64dc1; Delete, F and Ctrl+Z bound |
| X-08 | closed | `fe-ui/src/node_manager/shortcuts.rs:47` | db64dc1; command chords ignore tool keys |
| X-09 | closed | `fe-ui/src/ui_shell/topbar.rs:686` | 6f39d02; shortcut help overlay |
| X-10 | closed | `fe-ui/src/theme.rs:22` | 577c359; muted text raised |
| X-11 | closed | `fe-ui/src/panels/sidebar.rs:106` | 6f47326; minimum target size on icon buttons |
| X-12 | open | `fe-ui/src/panels/gis_panel.rs:369` | 2491704; BG_SAVE on a non-save action; BG_BUTTON_ACTIVE still the selected fill at inspector.rs:226 |
| X-13 | open | `fe-ui/src/dialogs/context_menu.rs:686` | e76bbb9; Debug output in menu headers; also terrain_tools_panel.rs:296 |
| X-14 | open | `fe-ui/src/dialogs/context_menu.rs:146` | 25d908c; tombstone jargon; also volume_report_panel.rs:840 integration tolerance |
| X-15 | open | `fe-ui/src/dialogs/context_menu.rs:168` | e76bbb9; track wording kept in labels |
| X-16 | open | `fe-ui/src/panels/asset_card.rs:92` | 2491704; unclipped path label |
| X-17 | open | `fe-ui/src/dialogs/entity_settings.rs:313` | f678dbe; bare send ok on ClaimOrphanedVerse |
| T-01 | closed | `fe-ui/src/ui_shell/topbar.rs:308` | f55ca21; frameless caption buttons |
| T-02 | closed | `fe-ui/src/ui_shell/right_sidebar.rs:1919` | 7141025; rail with tooltips plus topbar section buttons |
| T-03 | closed | `fe-ui/src/ui_shell/right_sidebar.rs:1849` | fa95556; bounded divider width |
| T-04 | open | `fe-ui/src/ui_shell/topbar.rs:847` | 9498869; red fill and no Esc hint remain |
| L-01 | closed | `fe-ui/src/panels/sidebar.rs:273` | 9ee86cc; toggle and navigate separated |
| L-02 | closed | `fe-ui/src/ui_shell/right_sidebar.rs:1050` | d0d7192; reset moved to Settings behind two-step confirm |
| L-03 | closed | `fe-ui/src/panels/sidebar.rs:952` | 0f1a6a0; explanatory tooltip |
| L-04 | closed | `fe-ui/src/panels/tree_entry.rs:151` | 9ee86cc; row context menu |
| L-05 | closed | `fe-ui/src/panels/sidebar.rs:261` | 9ee86cc; labelled add buttons |
| L-06 | closed | `fe-ui/src/dialogs/create_entity.rs:26` | 7ea181b; spawn position carried into create |
| L-07 | closed | `fe-ui/src/panels/sidebar.rs:121` | f678dbe; Space grid removed (no Space text left in sidebar.rs) |
| V-01 | closed | `fe-ui/src/viewport.rs:373` | 00189e1; ID crumb gone and name crumbs are buttons |
| V-02 | closed | `fe-ui/src/viewport.rs:310` | 9cdd313; truncating labels |
| V-03 | closed | `fe-ui/src/viewport.rs:214` | 9cdd313; scroll area plus plural handling at :308 |
| V-04 | closed | `fe-ui/src/viewport.rs:208` | 00189e1; honest next step |
| V-05 | closed | `fe-ui/src/viewport.rs:373` | 00189e1; clickable crumbs |
| V-06 | closed | `fe-ui/src/node_manager/shortcuts.rs:260` | db64dc1; Escape closes the menu |
| I-01 | closed | `fe-ui/src/panels/inspector.rs:736` | a8d9ed7; empty state replaces live controls |
| I-02 | open | `fe-ui/src/panels/inspector.rs:1617` | f678dbe; raw type names in the property type list |
| I-03 | closed | `fe-ui/src/panels/inspector.rs:2445` | 1dcbebf; delete disabled until the real id arrives |
| I-04 | closed | `fe-ui/src/panels/inspector.rs:1370` | 5aec1d8; copy toasts |
| I-05 | open | `fe-ui/src/panels/query_tab.rs:129` | f678dbe; no copy-for-BI in the Query tab |
| I-06 | open | `fe-ui/src/panels/annotation_card.rs:93` | a203bd7; Save stays enabled with an invalid hex colour |
| I-07 | open | `fe-ui/src/panels/asset_card.rs:45` | f678dbe; asset path unclipped and no open-folder |
| O-01 | closed | `fe-ui/src/node_manager/terrain_authoring.rs:408` | fbea2d7; region_draft writer exists |
| O-02 | open | `fe-ui/src/panels/tool_panel.rs:1094` | 0c8f311; shapes still append at the origin |
| O-03 | open | `fe-ui/src/panels/path_editor_card.rs:465` | e76bbb9; cross-referencing split remains |
| O-04 | open | `fe-ui/src/panels/terrain_tools_panel.rs:23` | e76bbb9; disclosure repeated at :64 :213 :244 |
| O-05 | closed | `fe-ui/src/panels/tool_options.rs:57` | e76bbb9; placeholders replaced by descriptor hints |
| O-06 | closed | `fe-ui/src/panels/tool_options.rs:511` | fbea2d7; link renamed |
| O-07 | closed | `fe-ui/src/panels/tool_panel.rs:685` | aa55ea9; single Apply and Make editable node |
| R-01 | closed | `fe-terrain/src/terrain_plugin.rs:2553` | 2b3c590; explicit reply (completeness-turn4 UI-01) |
| R-02 | open | `fe-ui/src/panels/terrain_tools_panel.rs:296` | 7bdcfbe; Debug op names in proposal rows |
| R-03 | open | `fe-ui/src/panels/proposal_report_panel.rs:122` | 7bdcfbe; slope and bearing shown for every footprint |
| R-04 | closed | `fe-ui/src/panels/volume_report_panel.rs:900` | 5bf9883; copy as TSV |
| S-01 | open | `fe-ui/src/ui_shell/right_sidebar.rs:897` | ff33d24; unit-less wu suffix remains |
| S-02 | closed | `fe-ui/src/ui_shell/right_sidebar.rs:1106` | 7ebb967; commit on edit finish |
| S-03 | open | `fe-ui/src/ui_shell/right_sidebar.rs:1223` | 7a74214; no Saved cue or Restart now button |
| G-01 | closed | `fe-ui/src/ui_shell/right_sidebar.rs:1725` | 7ebb967; Escape handled on rename |
| M-01 | open | `fe-ui/src/dialogs/hexon_manager.rs:612` | f678dbe; header and body still separate Grids |
| M-02 | open | `fe-ui/src/ui_shell/right_sidebar.rs:1495` | ff33d24; Maps still dialog-backed |
| M-03 | closed | `fe-ui/src/actions/hexon.rs:178` | 7ea181b; correlated set |
| M-04 | closed | `fe-ui/src/dialogs/hexon_manager.rs:64` | 7ea181b; two-step confirm |
| D-01 | closed | `fe-ui/src/viewport.rs:170` | 00189e1; card Options reachable for every role |
| D-02 | closed | `fe-ui/src/dialogs/entity_settings.rs:86` | 17e176d; stored details loaded |
| D-03 | closed | `fe-ui/src/dialogs/entity_access.rs:283` | f6ddf6b; access member list via EntityAccess |
| D-04 | open | `fe-ui/src/dialogs/entity_settings.rs:419` | f6ddf6b; only a copy toast; rename, description and default access give no feedback |
| D-05 | closed | `fe-ui/src/dialogs/join.rs:76` | 7ea181b; join outcome shown |
| D-06 | closed | `fe-ui/src/dialogs/gltf_import.rs:155` | 572da0b; failure kept in the dialog |
| D-07 | closed | `fe-ui/src/dialogs/create_entity.rs:4` | 7ea181b; blank-name guard |
| D-08 | closed | `fe-ui/src/dialogs/petal_manifest.rs:13` | a103cb3; dirty close confirm |
| D-09 | closed | `fe-ui/src/dialogs/node_options.rs:148` | 195f5cc; rename awaits the result |
| D-10 | closed | `fe-ui/src/dialogs/peer_debug.rs:5` | 720b7f1; real diagnostics block |
| D-11 | closed | `fe-ui/src/ui_shell/modal.rs:265` | a103cb3; pending count handled |
| D-12 | closed | `fe-ui/src/dialogs/context_menu.rs:857` | 59baf23; copy fixed |
| B-01 | closed | `fe-ui/src/role_chip.rs:25` | 577c359; role in the status bar |
| B-02 | closed | `fe-ui/src/ui_shell/modal.rs:20` | e2b0d95; per-panel retry |
| B-03 | closed | `fe-ui/src/panels/status_bar.rs:405` | 577c359; neutral dot |
| P-01 | closed | `fe-ui/src/actions/mod.rs:1105` | 7ea181b; inline error |
| P-02 | closed | `fe-ui/src/panels/portal_toolbar.rs:114` | 59baf23; reload, forward and spinner |
| P-03 | closed | `fe-ui/src/actions/portal.rs:225` | 59baf23; ack-based |
| DW-01 | open | `fe-ui/src/panels/gis_panel.rs:500` | 2491704; one message serves never-run and empty |
| DW-02 | open | `fe-ui/src/panels/gis_panel.rs:261` | 2491704; Bbox label, no Ctrl+Enter or scope chip |
| DW-03 | closed | `fe-ui/src/panels/path_editor_card.rs:151` | 0d51987; default name |
| DW-04 | closed | `fe-ui/src/panels/egress_card.rs:250` | 0d51987; TTL validated; mint OUT (D-64, EGR-MINT) |

## Tally

closed: 58; open: 24; total: 82

## Residuals for round 6

Exact anchor and one-line fix per open id (fix text from the R2 suggestion, trimmed). Not fixed here: these files belong to other owners.

- X-06 `fe-ui/src/panels/terrain_tools_panel.rs:316`: proposal delete is single click while other sites use confirm_button. Fix: Extract the inspector revoke pattern into `widgets::confirm_button(ui, id, label, confirm_label)` and use it at every site. Owner: `panels/widgets.rs` + call sites 
- X-12 `fe-ui/src/panels/gis_panel.rs:369`: BG_SAVE on a non-save action; BG_BUTTON_ACTIVE still the selected fill at inspector.rs:226. Fix: Use one `tab_fill(active)` = `mode_button_fill` everywhere. Make primary buttons neutral-bright. Add `STATUS_NORMAL` grey. Owner: `theme.rs`, `panels/toolbar.rs` 
- X-13 `fe-ui/src/dialogs/context_menu.rs:686`: Debug output in menu headers; also terrain_tools_panel.rs:296. Fix: Show names (find_petal_name etc.). Put ids behind a copyable "ID" affordance. Use 1-based indices, human dates ("expires in 6 d") and a humanized panel name table. Owner: listed files
- X-14 `fe-ui/src/dialogs/context_menu.rs:146`: tombstone jargon; also volume_report_panel.rs:840 integration tolerance. Fix: Rewrite in plain language ("Max mesh instances before spawning pauses", "Deleted for everyone (syncs to peers)"). Owner: listed files 
- X-15 `fe-ui/src/dialogs/context_menu.rs:168`: track wording kept in labels. Fix: Do a single terminology sweep: path, map, node, point, "glTF model". Owner: listed files 
- X-16 `fe-ui/src/panels/asset_card.rs:92`: unclipped path label. Fix: Sweep `widgets::clipped_value` (+ tooltip) onto every user-content label. Owner: listed files 
- X-17 `fe-ui/src/dialogs/entity_settings.rs:313`: bare send ok on ClaimOrphanedVerse. Fix: Add `send_or_toast(&db_tx, cmd, ui_mgr)`. Owner: `actions/mod.rs` 
- T-04 `fe-ui/src/ui_shell/topbar.rs:847`: red fill and no Esc hint remain. Fix: Tooltip "Deselect (Esc)", neutral fill. 
- I-02 `fe-ui/src/panels/inspector.rs:1617`: raw type names in the property type list. Fix: Validate inline, edit values in place, use friendly type names and widen the fields. 
- I-05 `fe-ui/src/panels/query_tab.rs:129`: no copy-for-BI in the Query tab. Fix: Reuse `egress_card` rows for the current SQL. 
- I-06 `fe-ui/src/panels/annotation_card.rs:93`: Save stays enabled with an invalid hex colour. Fix: Disable Save on invalid hex and toast on success. 
- I-07 `fe-ui/src/panels/asset_card.rs:45`: asset path unclipped and no open-folder. Fix: `clipped_value_copyable`, an "Open folder" button, and normal-tier color. 
- O-02 `fe-ui/src/panels/tool_panel.rs:1094`: shapes still append at the origin. Fix: Place at the last viewport click or the camera focus, or an "arm then click to place" gesture. Remove the dev promise. 
- O-03 `fe-ui/src/panels/path_editor_card.rs:465`: cross-referencing split remains. Fix: Make Options → Pen the single home (path picker + points), and keep the Data → Paths tab as a list/export view. 
- O-04 `fe-ui/src/panels/terrain_tools_panel.rs:23`: disclosure repeated at :64 :213 :244. Fix: Show only the field the selected op uses; add " m"; offer material presets (§8); show the disclosure once. 
- R-02 `fe-ui/src/panels/terrain_tools_panel.rs:296`: Debug op names in proposal rows. Fix: "#1 Raise · 314 m² · +2.0 m", plus click to frame the proposal. 
- R-03 `fe-ui/src/panels/proposal_report_panel.rs:122`: slope and bearing shown for every footprint. Fix: Read the record's material and hide metrics that don't apply. 
- S-01 `fe-ui/src/ui_shell/right_sidebar.rs:897`: unit-less wu suffix remains. Fix: Show meters. Use one per-petal home (the column) and remove it from the manifest dialog. Fix the tooltip. 
- S-03 `fe-ui/src/ui_shell/right_sidebar.rs:1223`: no Saved cue or Restart now button. Fix: Show a "Saved ✓" fade next to the heading and a "Restart now" button. 
- M-01 `fe-ui/src/dialogs/hexon_manager.rs:612`: header and body still separate Grids. Fix: Use one card per map (name + size line + actions row), or a horizontal ScrollArea plus a single Grid. **Verify visually.** 
- M-02 `fe-ui/src/ui_shell/right_sidebar.rs:1495`: Maps still dialog-backed. Fix: Render Maps through `section_chrome`. Name the blocking dialog, or render or remove InviteDialog. 
- D-04 `fe-ui/src/dialogs/entity_settings.rs:419`: only a copy toast; rename, description and default access give no feedback. Fix: Toast on DbResult, use `copy_row` everywhere, take the API base from config, and use one tab label. 
- DW-01 `fe-ui/src/panels/gis_panel.rs:500`: one message serves never-run and empty. Fix: Use separate result buffers, auto-load on first view, and an explicit "0 results" message. 
- DW-02 `fe-ui/src/panels/gis_panel.rs:261`: Bbox label, no Ctrl+Enter or scope chip. Fix: Add Ctrl+Enter, `build_nav_scope` chip, `copy_row` for SQL, "Area (bounding box)", and disable Center when nothing is selected. 
