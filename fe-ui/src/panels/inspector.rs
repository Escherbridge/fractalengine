//! Right inspector panel: entity/transform/URL/properties/schema/API-access
//! tabs for the selected node. This is a browser-integration seam — the
//! "Portal URLs" section here is where `UiAction::SaveUrl` and
//! `UiAction::OpenPortal` originate; see `fe-ui/src/AGENTS.md` §portal for
//! the full save/open chain and its known fragilities.

use bevy::camera::primitives::Aabb;
use bevy::prelude::{
    Children, Entity, GlobalTransform, Local, Query, Res, ResMut, Transform, Vec3,
};
use bevy_egui::egui;
use std::collections::BTreeMap;

use super::property_value::PropertyValueKind;

use crate::accessibility::ResponseExt;

use crate::actions::{UiAction, UiManager};
use crate::navigation_manager::NavigationManager;
use crate::node_manager::NodeManager;
use crate::panels::widgets::{meters_to_world, world_to_meters};
use crate::plugin::{InspectorFormState, InspectorTab, LocalUserRole, API_TOKEN_PAGE_SIZE};
use crate::terrain_map::PetalMapState;
use crate::theme;
use crate::verse_manager::db_results::tokens::mint_command;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

/// Inspector BODY. ui_semantics_unification_20260808 D10 removed this panel's
/// bespoke `SidePanel::right("inspector")` + `show_animated` self-collapse: the
/// right region has exactly one chrome now
/// (`ui_shell::right_sidebar::section_chrome`), and the Inspector opting out of
/// it was why the section rail — the sole entry point to Terrain Tools and the
/// Proposal Report — was invisible in the resting app (finding #3). The
/// section header/close affordance is the chrome's; the sections below all
/// carry their own empty states, so this renders fine with nothing selected.
pub(crate) fn right_inspector(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &mut crate::node_manager::NodeManager,
    hierarchy: &VerseManager,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
    asset_status: &crate::asset_ops::AssetDownloadStatus,
) {
    sync_property_editor_scope(inspector, node_mgr, ui.ctx().input(|input| input.time));
    if node_mgr.selected_entity().is_some() {
        crate::ui_shell::layout_policy::trailing_row(ui, |ui| {
            if ui
                .add(
                    egui::Button::new("\u{2715}")
                        .fill(egui::Color32::TRANSPARENT)
                        .min_size(egui::vec2(24.0, 24.0)),
                )
                .named(ui, "Deselect object")
                .on_hover_text("Deselect")
                .clicked()
            {
                node_mgr.deselect();
            }
        });
    } else {
        ui.label(
            egui::RichText::new("Nothing selected — pick an object in the viewport.")
                .small()
                .color(theme::TEXT_MUTED)
                .italics(),
        );
    }

    // Tab bar
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        for (tab, label) in [
            (InspectorTab::Properties, "Properties"),
            (InspectorTab::ApiAccess, "API Access"),
            (InspectorTab::Query, "Query"),
        ] {
            let active = inspector.active_tab == tab;
            let btn = egui::Button::new(egui::RichText::new(label).small().color(if active {
                theme::TEXT_BRIGHT
            } else {
                theme::TEXT_DIM
            }))
            .fill(if active {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            });
            if ui
                .add(btn)
                .named(ui, format_args!("Inspector tab: {label}"))
                .clicked()
            {
                let prev_tab = inspector.active_tab;
                inspector.active_tab = tab;

                // Clear sensitive state when leaving the API tab
                if prev_tab == InspectorTab::ApiAccess && tab != InspectorTab::ApiAccess {
                    inspector.generated_api_token = None;
                    inspector.generated_api_token_client = None;
                }

                // Auto-populate scope + fetch tokens when entering API tab
                if tab == InspectorTab::ApiAccess {
                    inspector.api_tokens_page = 0;
                    if inspector.api_token_scope_buf.is_empty() {
                        inspector.api_token_scope_buf = build_nav_scope(nav);
                    }
                    let scope = inspector.api_token_scope_buf.clone();
                    if !scope.is_empty() {
                        db_tx
                            .send(DbCommand::ListApiTokensByScope {
                                scope_prefix: scope,
                                offset: 0,
                                limit: API_TOKEN_PAGE_SIZE,
                            })
                            .ok();
                    }
                }
            }
        }
    });
    ui.separator();

    match inspector.active_tab {
        InspectorTab::Properties => {
            ui.add_space(4.0);
            inspector_entity_section(ui, node_mgr, ui_mgr);
            ui.add_space(2.0);
            crate::panels::asset_card::asset_card_section(
                ui,
                node_mgr,
                hierarchy,
                ui_mgr,
                asset_status,
            );
            ui.add_space(2.0);
            crate::panels::annotation_card::annotation_card_section(
                ui, inspector, node_mgr, ui_mgr,
            );
            ui.add_space(2.0);
            inspector_transform_section(ui, inspector, ui_mgr);
            ui.add_space(2.0);
            inspector_url_meta_section(ui, inspector, ui_mgr, local_role);
            ui.add_space(2.0);
            inspector_properties_section(ui, inspector, node_mgr, ui_mgr);
            ui.add_space(2.0);
            inspector_schema_section(ui, inspector, node_mgr, local_role, db_tx, nav);
        }
        InspectorTab::ApiAccess => {
            ui.add_space(4.0);
            inspector_api_access_section(ui, inspector, node_mgr, db_tx, local_role, ui_mgr);
        }
        InspectorTab::Query => {
            ui.add_space(4.0);
            crate::panels::query_tab::inspector_query_section(ui, inspector, nav, node_mgr, ui_mgr);
        }
    }
}

/// Build a scope string from the current navigation state.
pub(crate) fn build_nav_scope(nav: &NavigationManager) -> String {
    match (
        &nav.active_verse_id,
        &nav.active_fractal_id,
        &nav.active_petal_id,
    ) {
        (Some(v), Some(f), Some(p)) => fe_database::build_scope(v, Some(f), Some(p)),
        (Some(v), Some(f), None) => fe_database::build_scope(v, Some(f), None),
        (Some(v), None, _) => fe_database::build_scope(v, None, None),
        _ => String::new(),
    }
}

fn inspector_entity_section(
    ui: &mut egui::Ui,
    node_mgr: &crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Entity")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        // Rendered OUTSIDE a Grid (F1): a Grid cell ignores `set_max_width`,
        // which is the width-blowout bug `clipped_value*` exists to avoid.
        if let Some(sel) = &node_mgr.selected {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Node ID")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                crate::panels::widgets::clipped_value_copyable(
                    ui,
                    ui_mgr,
                    "Node ID",
                    &sel.node_id,
                    theme::TEXT_BRIGHT,
                    "Copied Node ID",
                );
            });
        }
        ui.add_space(4.0);
    })
    .header_response
    .observed(ui);
}

// ---------------------------------------------------------------------------
// Real-unit transform helpers (FR-2, inspector_units_width_20260716)
//
// The entity Transform is world units / radians; the inspector shows meters
// (position, size) and degrees (rotation). Conversions live here (pure,
// unit-tested). The panel edits meter buffers and live-writes the converted
// world / scale values into the EXISTING inspector.pos / inspector.scale
// buffers, so the Apply action path (actions::transform) is unchanged.
// See AGENTS.md §inspector-units.
// ---------------------------------------------------------------------------

/// Max characters shown for a custom-property value before eliding (FR-1); the
/// full value is still copyable via the box's copy button.
const PROP_VALUE_ELIDE_CHARS: usize = 240;

// Rotation is already stored degrees-side in `inspector.rot` (inspector_sync
// converts radians→degrees on fill; actions::transform converts back on Apply),
// so the panel needs no rotation conversion — these wrappers exist for symmetry
// and unit-test coverage of the radians↔degrees leg.
#[allow(dead_code)]
fn radians_to_degrees(rad: f32) -> f32 {
    rad.to_degrees()
}

#[allow(dead_code)]
fn degrees_to_radians(deg: f32) -> f32 {
    deg.to_radians()
}

/// Back-compute the scale multiplier yielding a target real size (`target_m`) on
/// one axis, given that axis' base extent at scale 1 (`base_extent`, world
/// units). `None` when the base extent is ~0 (no size↔scale ratio — leave scale
/// untouched).
fn size_to_scale(target_m: f32, base_extent: f32, world_scale: f64) -> Option<f32> {
    if base_extent.abs() < 1e-6 {
        return None;
    }
    Some(meters_to_world(target_m, world_scale) / base_extent)
}

/// Current real size (meters) for a scale multiplier + base extent (inverse of
/// [`size_to_scale`]).
fn scale_to_size(scale: f32, base_extent: f32, world_scale: f64) -> f32 {
    world_to_meters(base_extent * scale, world_scale)
}

/// Combined AABB of `root` + its immediate children, expressed in `root`'s LOCAL
/// frame — extents at root scale/rotation = identity (a stable, rotation-
/// invariant "size at scale 1" basis). Mirrors node_manager's `pick_node_aabb`
/// child walk (glTF puts the mesh + Aabb on a child of the SceneRoot). `None`
/// when nothing pickable is spawned yet.
fn combined_local_extents(
    root: Entity,
    g_query: &Query<&GlobalTransform>,
    aabb_query: &Query<&Aabb>,
    children_query: &Query<&Children>,
) -> Option<Vec3> {
    let root_inv = g_query.get(root).ok()?.affine().inverse();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;
    let mut consider = |ent: Entity| {
        if let (Ok(g), Ok(aabb)) = (g_query.get(ent), aabb_query.get(ent)) {
            let rel = root_inv * g.affine(); // child-local → root-local
            let c = Vec3::from(aabb.center);
            let h = Vec3::from(aabb.half_extents);
            for sx in [-1.0f32, 1.0] {
                for sy in [-1.0f32, 1.0] {
                    for sz in [-1.0f32, 1.0] {
                        let corner = c + Vec3::new(sx * h.x, sy * h.y, sz * h.z);
                        let p = rel.transform_point3(corner);
                        min = min.min(p);
                        max = max.max(p);
                    }
                }
            }
            found = true;
        }
    };
    consider(root);
    if let Ok(children) = children_query.get(root) {
        for child in children.iter() {
            consider(*child);
        }
    }
    found.then(|| max - min)
}

/// Mirrors `node_manager::inspector_sync` for the real-unit display buffers: on
/// selection change / `Changed<Transform>` / world-scale change it refills
/// `inspector.pos_m` (meters), `inspector.size_m` (meters), `base_extents`
/// (scale-1 world extents) and `world_scale`. Read-only over Transform /
/// GlobalTransform / Aabb / Children (no mutation → no query conflicts). See
/// AGENTS.md §inspector-units.
pub(crate) fn sync_inspector_units(
    node_mgr: Res<NodeManager>,
    petal_map: Res<PetalMapState>,
    mut inspector: ResMut<InspectorFormState>,
    changed_query: Query<&Transform, bevy::prelude::Changed<Transform>>,
    all_query: Query<&Transform>,
    g_query: Query<&GlobalTransform>,
    aabb_query: Query<&Aabb>,
    children_query: Query<&Children>,
    mut last_selected: Local<Option<Entity>>,
    mut last_ws: Local<f64>,
) {
    inspector.world_scale = petal_map.world_scale;
    let Some(entity) = node_mgr.selected_entity() else {
        *last_selected = None;
        inspector.base_extents = None;
        return;
    };
    let just_selected = *last_selected != Some(entity);
    *last_selected = Some(entity);
    let ws_changed = (*last_ws - petal_map.world_scale).abs() > 1e-12;
    *last_ws = petal_map.world_scale;

    // Only refill on a real change (else leave buffers so in-flight edits stick).
    let t = if just_selected || ws_changed {
        match all_query.get(entity) {
            Ok(t) => t,
            Err(_) => return,
        }
    } else {
        match changed_query.get(entity) {
            Ok(t) => t,
            Err(_) => return,
        }
    };

    let base = combined_local_extents(entity, &g_query, &aabb_query, &children_query);
    inspector.base_extents = base.map(|e| e.to_array());
    let ws = petal_map.world_scale;
    inspector.pos_m = [
        format!("{:.3}", world_to_meters(t.translation.x, ws)),
        format!("{:.3}", world_to_meters(t.translation.y, ws)),
        format!("{:.3}", world_to_meters(t.translation.z, ws)),
    ];
    inspector.size_m = match base {
        Some(e) => [
            format!("{:.3}", scale_to_size(t.scale.x, e.x, ws)),
            format!("{:.3}", scale_to_size(t.scale.y, e.y, ws)),
            format!("{:.3}", scale_to_size(t.scale.z, e.z, ws)),
        ],
        None => [
            String::from("0.000"),
            String::from("0.000"),
            String::from("0.000"),
        ],
    };
}

/// One label + 3 axis inputs. `convert(axis, entered) -> Option<world>` maps the
/// edited display value into the world/scale buffer `dst_bufs` on change (None
/// leaves that axis' `dst` untouched). Returns true when Enter committed.
fn units_axis_row(
    ui: &mut egui::Ui,
    label: &str,
    src_bufs: &mut [String; 3],
    dst_bufs: &mut [String; 3],
    convert: impl Fn(usize, f32) -> Option<f32>,
) -> bool {
    let mut commit = false;
    ui.horizontal(|ui| {
        let input_w = axis_input_width(ui);
        ui.add_sized(
            [TRANSFORM_LABEL_W, 16.0],
            egui::Label::new(egui::RichText::new(label).small().color(theme::TEXT_DIM)),
        );
        for (i, (axis, (src, dst))) in ["X", "Y", "Z"]
            .iter()
            .zip(src_bufs.iter_mut().zip(dst_bufs.iter_mut()))
            .enumerate()
        {
            ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
            let resp = ui
                .add(
                    egui::TextEdit::singleline(src)
                        .desired_width(input_w)
                        .font(egui::TextStyle::Monospace),
                )
                .named(ui, format_args!("{label} {axis}"));
            if resp.changed() {
                if let Ok(entered) = src.trim().parse::<f32>() {
                    if let Some(v) = convert(i, entered) {
                        *dst = format!("{v:.4}");
                    }
                }
            }
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
        }
    });
    commit
}

/// One label + 3 axis inputs edited in place (no conversion). Used for the raw
/// rotation-degrees and scale-multiplier rows.
fn raw_axis_row(ui: &mut egui::Ui, label: &str, bufs: &mut [String; 3]) -> bool {
    let mut commit = false;
    ui.horizontal(|ui| {
        let input_w = axis_input_width(ui);
        ui.add_sized(
            [TRANSFORM_LABEL_W, 16.0],
            egui::Label::new(egui::RichText::new(label).small().color(theme::TEXT_DIM)),
        );
        for (axis, buf) in ["X", "Y", "Z"].iter().zip(bufs.iter_mut()) {
            ui.label(egui::RichText::new(*axis).small().color(theme::TEXT_AXIS));
            let resp = ui
                .add(
                    egui::TextEdit::singleline(buf)
                        .desired_width(input_w)
                        .font(egui::TextStyle::Monospace),
                )
                .named(ui, format_args!("{label} {axis}"));
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
        }
    });
    commit
}

/// Label column width for the transform rows (wide enough for "Position (m)").
const TRANSFORM_LABEL_W: f32 = 74.0;

/// Dynamic per-axis input width so the 3 fields always fit the current panel
/// width (same self-fitting math the transform section used before FR-2).
fn axis_input_width(ui: &egui::Ui) -> f32 {
    const AXIS_W: f32 = 10.0; // "X" / "Y" / "Z"
    let spacing = ui.spacing().item_spacing.x;
    ((ui.available_width() - TRANSFORM_LABEL_W - spacing * 7.0 - AXIS_W * 3.0) / 3.0).max(32.0)
}

fn inspector_transform_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    ui_mgr: &mut UiManager,
) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Transform")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);
        // Rows edit real-unit display buffers and live-write the converted
        // world/scale values into inspector.pos / inspector.scale; the Apply
        // action (actions::transform) still parses those world buffers, so its
        // path is unchanged (it receives world units / radians). Rotation is
        // already degrees-side in inspector.rot. See AGENTS.md §inspector-units.
        let mut commit = false;
        let ws = inspector.world_scale;
        let base = inspector.base_extents;

        // Position — meters → world into inspector.pos.
        commit |= units_axis_row(
            ui,
            "Position (m)",
            &mut inspector.pos_m,
            &mut inspector.pos,
            |_, m| Some(meters_to_world(m, ws)),
        );

        // Rotation — degrees, edited in place (Apply converts to radians).
        commit |= raw_axis_row(ui, "Rotation (\u{00B0})", &mut inspector.rot);

        // Size — real meters → scale multiplier into inspector.scale. Only when
        // a base AABB is available (else the raw Scale row below is the control).
        if let Some(base) = base {
            commit |= units_axis_row(
                ui,
                "Size (m)",
                &mut inspector.size_m,
                &mut inspector.scale,
                |i, m| size_to_scale(m, base[i], ws),
            );
        }

        // Scale — raw multiplier, edited in place (fallback when no AABB).
        commit |= raw_axis_row(ui, "Scale", &mut inspector.scale);

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui
                .button("Apply")
                .named(ui, "Apply node transform")
                .clicked()
            {
                commit = true;
            }
        });
        if commit {
            ui_mgr.push_action(UiAction::ApplyNodeTransform);
        }
        ui.add_space(4.0);
    })
    .header_response
    .observed(ui);
}

/// The "Portal URLs" section: `Save` pushes `UiAction::SaveUrl` (commits
/// `external_url`/`config_url` for the selected node); `Open Portal` pushes
/// `UiAction::OpenPortal` with the current `external_url` text. See
/// `fe-ui/src/AGENTS.md` §portal for the full chain from here to `DbCommand`.
fn inspector_url_meta_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Portal URLs")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        ui.add_space(4.0);

        ui.label(
            egui::RichText::new("Portal URL")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add(
            egui::TextEdit::singleline(&mut inspector.external_url)
                .hint_text("https://\u{2026}")
                .desired_width(f32::INFINITY),
        )
        .named(ui, "Portal URL");

        if local_role.can_manage() {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("Config URL (admin)")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut inspector.config_url)
                    .hint_text("https://admin.\u{2026}")
                    .desired_width(f32::INFINITY),
            )
            .named(ui, "Config URL (admin)");
        }

        ui.add_space(6.0);
        if ui
            .add(
                egui::Button::new("\u{1F4BE} Save")
                    .fill(theme::BG_SAVE)
                    .min_size(egui::vec2(ui.available_width(), 28.0)),
            )
            .named(ui, "Save portal URLs")
            .clicked()
        {
            ui_mgr.push_action(UiAction::SaveUrl);
        }

        ui.add_space(4.0);

        let has_url = !inspector.external_url.trim().is_empty();
        let btn = egui::Button::new("\u{1F310} Open Portal")
            .fill(if has_url {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            })
            .min_size(egui::vec2(ui.available_width(), 28.0));
        let resp = ui.add_enabled(has_url, btn).observed(ui);
        if resp.clicked() {
            bevy::log::info!(
                "Portal: 'Open Portal' clicked — URL: {}",
                inspector.external_url
            );
            ui_mgr.push_action(UiAction::OpenPortal {
                url: inspector.external_url.clone(),
            });
        }
        if !has_url {
            resp.on_hover_text("Set a Portal URL above to open the webview");
        }

        ui.add_space(4.0);
    })
    .header_response
    .observed(ui);
}

fn inspector_api_access_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &crate::node_manager::NodeManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    local_role: &LocalUserRole,
    ui_mgr: &mut UiManager,
) {
    let node_id = match node_mgr.selected.as_ref() {
        Some(sel) => &sel.node_id,
        None => return,
    };

    // Padded container for the entire API Access tab
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());

            if !local_role.can_manage() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Manager role or higher required to manage API tokens.")
                        .small()
                        .color(theme::TEXT_MUTED)
                        .italics(),
                );
                return;
            }

            // --- Generate Token section ---
            egui::CollapsingHeader::new(
                egui::RichText::new("Generate Token")
                    .strong()
                    .color(theme::TEXT_SECTION),
            )
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(4.0);

                // Scope + Node ID — read-only, clipped to panel width (F1).
                ui.label(egui::RichText::new("Scope").small().color(theme::TEXT_DIM));
                egui::Frame::NONE
                    .fill(theme::BG_BUTTON)
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .corner_radius(3.0)
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width());
                        crate::panels::widgets::clipped_value_copyable(
                            ui,
                            ui_mgr,
                            "API token scope",
                            &inspector.api_token_scope_buf,
                            theme::TEXT_SECTION,
                            "Copied scope",
                        );
                    });

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Node ID").small().color(theme::TEXT_DIM));
                    crate::panels::widgets::clipped_value_copyable(
                        ui,
                        ui_mgr,
                        "Node ID",
                        node_id,
                        theme::TEXT_SECTION,
                        "Copied Node ID",
                    );
                });

                if inspector.api_token_scope_buf.is_empty() {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Navigate to a verse/fractal/petal to set the token scope.")
                            .small()
                            .color(theme::TEXT_MUTED)
                            .italics(),
                    );
                    return;
                }

                ui.add_space(6.0);

                // Role + Expiry on separate rows for narrow panels
                egui::Grid::new("api_token_opts")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Role").small().color(theme::TEXT_DIM));
                        egui::ComboBox::from_id_salt("inspector_api_role")
                            .selected_text(inspector.api_token_role_buf.as_str())
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut inspector.api_token_role_buf, "viewer".to_string(), "Viewer").observed(ui);
                                ui.selectable_value(&mut inspector.api_token_role_buf, "editor".to_string(), "Editor").observed(ui);
                                ui.selectable_value(&mut inspector.api_token_role_buf, "manager".to_string(), "Manager").observed(ui);
                            }).response.named(ui, "API token maximum role");
                        ui.end_row();

                        ui.label(egui::RichText::new("Expires").small().color(theme::TEXT_DIM));
                        egui::ComboBox::from_id_salt("inspector_api_expiry")
                            .selected_text(match inspector.api_token_expiry_buf {
                                1 => "1 hour",
                                24 => "24 hours",
                                168 => "7 days",
                                720 => "30 days",
                                _ => "30 days",
                            })
                            .width(100.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 1, "1 hour").observed(ui);
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 24, "24 hours").observed(ui);
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 168, "7 days").observed(ui);
                                ui.selectable_value(&mut inspector.api_token_expiry_buf, 720, "30 days").observed(ui);
                            }).response.named(ui, "API token expiration");
                        ui.end_row();
                    });

                // Client DID (W6-7b): blank mints a fresh client; the DB
                // door is the authority on `client_did` — this field only
                // gates the Generate button so a malformed DID never leaves
                // the GUI.
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Client DID (blank = new client)")
                        .small()
                        .color(theme::TEXT_DIM),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut inspector.api_token_client_buf)
                        .desired_width(ui.available_width())
                        .hint_text("did:key:z... (optional)")
                        .font(egui::TextStyle::Monospace),
                ).named(ui, "API token client DID");
                let client_trimmed = inspector.api_token_client_buf.trim();
                let client_valid =
                    client_trimmed.is_empty() || fe_identity::validate_client_did(client_trimmed).is_ok();
                if !client_valid {
                    ui.label(
                        egui::RichText::new("Not a valid ed25519 did:key")
                            .small()
                            .color(theme::STATUS_OFFLINE),
                    );
                }

                ui.add_space(6.0);
                if ui
                    .add_enabled(
                        client_valid,
                        egui::Button::new("Generate API Token")
                            .fill(theme::BG_SAVE)
                            .min_size(egui::vec2(ui.available_width(), 28.0)),
                    ).observed(ui)
                    .clicked()
                {
                    db_tx
                        .send(mint_command(
                            inspector.api_token_scope_buf.clone(),
                            inspector.api_token_role_buf.clone(),
                            inspector.api_token_expiry_buf,
                            Some(format!("node:{}", node_id)),
                            client_trimmed,
                        ))
                        .ok();
                }

                // Show generated token (selectable, with copy buttons)
                let mut dismiss_token = false;
                if let Some(ref token) = inspector.generated_api_token {
                    ui.add_space(6.0);
                    egui::Frame::NONE
                        .fill(theme::BG_BUTTON)
                        .inner_margin(egui::Margin::same(8))
                        .corner_radius(4.0)
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("Generated Token")
                                        .small()
                                        .strong()
                                        .color(theme::TEXT_SECTION),
                                );
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(egui::Button::new("\u{2715}").fill(egui::Color32::TRANSPARENT).small()).named(ui, "Dismiss generated API token").clicked() {
                                        dismiss_token = true;
                                    }
                                });
                            });

                            ui.add_space(2.0);
                            let mut display = token.clone();
                            ui.add(
                                egui::TextEdit::multiline(&mut display)
                                    .desired_width(ui.available_width())
                                    .desired_rows(3)
                                    .font(egui::TextStyle::Monospace),
                            ).named(ui, "Generated API token");

                            // Minted `client` claim (W6-7b): shown so the
                            // operator can hand it to the next mint for the
                            // same client, or record it for revocation.
                            if let Some(ref client) = inspector.generated_api_token_client {
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Client:")
                                            .small()
                                            .color(theme::TEXT_DIM),
                                    );
                                    crate::panels::widgets::clipped_value(
                                        ui,
                                        client,
                                        theme::TEXT_SECTION,
                                    );
                                    if ui
                                        .add(egui::Button::new("\u{1F4CB}").fill(egui::Color32::TRANSPARENT).small())
                                        .named(ui, "Copy generated token client DID").on_hover_text("Copy client DID")
                                        .clicked()
                                    {
                                        ui.ctx().copy_text(client.clone());
                                    }
                                });
                            }

                            ui.add_space(4.0);
                            let now = ui.ctx().input(|i| i.time);

                            // Copy Token button
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new("\u{1F4CB} Copy Token").fill(theme::BG_BUTTON_ACTIVE).small()).named(ui, "Copy generated API token").clicked() {
                                    ui.ctx().copy_text(token.clone());
                                    ui_mgr.show_toast("Copied token to clipboard", now);
                                }
                            });

                            // curl command — pre-built with the actual token
                            ui.add_space(4.0);
                            let curl_cmd = format!(
                                "curl -s http://localhost:8765/health -H \"Authorization: Bearer {}\"",
                                token,
                            );
                            egui::Frame::NONE
                                .fill(egui::Color32::from_rgb(20, 20, 30))
                                .inner_margin(egui::Margin::symmetric(6, 4))
                                .corner_radius(3.0)
                                .show(ui, |ui| {
                                    ui.set_max_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("curl")
                                                .small()
                                                .color(theme::TEXT_MUTED),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.add(egui::Button::new("\u{1F4CB}").fill(egui::Color32::TRANSPARENT).small())
                                                .named(ui, "Copy API health check curl command").on_hover_text("Copy curl command")
                                                .clicked()
                                            {
                                                ui.ctx().copy_text(curl_cmd.clone());
                                                ui_mgr.show_toast("Copied curl command", now);
                                            }
                                        });
                                    });
                                    crate::panels::widgets::clipped_value(
                                        ui,
                                        &curl_cmd,
                                        theme::TEXT_DIM,
                                    );
                                });
                        });
                }
                if dismiss_token {
                    inspector.generated_api_token = None;
                    inspector.generated_api_token_client = None;
                }

                ui.add_space(4.0);
            }).header_response.observed(ui);

            ui.add_space(2.0);

            // --- Active Tokens section ---
            egui::CollapsingHeader::new(
                egui::RichText::new("Active Tokens")
                    .strong()
                    .color(theme::TEXT_SECTION),
            )
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(4.0);

                if inspector.api_tokens_loading {
                    ui.label(
                        egui::RichText::new("Loading tokens...")
                            .color(theme::TEXT_MUTED)
                            .italics(),
                    );
                } else if inspector.api_tokens.is_empty() {
                    ui.label(
                        egui::RichText::new("No active API tokens.")
                            .color(theme::TEXT_MUTED)
                            .italics(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(format!(
                            "Showing {} of {}",
                            inspector.api_tokens.len(),
                            inspector.api_tokens_total,
                        ))
                        .small()
                        .color(theme::TEXT_MUTED),
                    );
                    ui.add_space(2.0);

                    let mut revoke_jti: Option<(String, String)> = None;
                    for (i, tok) in inspector.api_tokens.iter().enumerate() {
                        let row_bg = if i % 2 == 0 {
                            theme::BG_PEER_ROW_EVEN
                        } else {
                            theme::BG_PEER_ROW_ODD
                        };
                        egui::Frame::NONE
                            .fill(row_bg)
                            .inner_margin(egui::Margin::symmetric(6, 4))
                            .corner_radius(2.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(&tok.max_role)
                                            .small()
                                            .strong()
                                            .color(theme::TEXT_SECTION),
                                    );
                                    if let Some(ref lbl) = tok.label {
                                        crate::panels::widgets::clipped_value(
                                            ui,
                                            lbl,
                                            theme::TEXT_MUTED,
                                        );
                                    }
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            // Two-step confirm (destructive-action convention).
                                            let pending_id =
                                                egui::Id::new("inspector_token_revoke_pending");
                                            let pending: Option<String> =
                                                ui.ctx().data(|d| d.get_temp(pending_id));
                                            if pending.as_deref() == Some(tok.jti.as_str()) {
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            egui::RichText::new("Cancel").small(),
                                                        )
                                                        .fill(theme::BG_BUTTON),
                                                    ).named(ui, format_args!("Cancel API token revocation for scope {}", tok.scope))
                                                    .clicked()
                                                {
                                                    ui.ctx().data_mut(|d| {
                                                        d.remove::<String>(pending_id)
                                                    });
                                                }
                                                if ui
                                                    .add(
                                                        egui::Button::new(
                                                            egui::RichText::new("Confirm Revoke")
                                                                .small(),
                                                        )
                                                        .fill(theme::BG_DANGER),
                                                    ).named(ui, format_args!("Confirm API token revocation for scope {}", tok.scope))
                                                    .clicked()
                                                {
                                                    revoke_jti =
                                                        Some((tok.jti.clone(), tok.sub.clone()));
                                                    ui.ctx().data_mut(|d| {
                                                        d.remove::<String>(pending_id)
                                                    });
                                                }
                                            } else if ui
                                                .add(
                                                    egui::Button::new(
                                                        egui::RichText::new("Revoke").small(),
                                                    )
                                                    .fill(theme::BG_DANGER),
                                                ).named(ui, format_args!("Revoke API token for scope {}", tok.scope))
                                                .clicked()
                                            {
                                                ui.ctx().data_mut(|d| {
                                                    d.insert_temp(pending_id, tok.jti.clone())
                                                });
                                            }
                                        },
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(format!("exp: {}", tok.expires_at))
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                );
                                if let Some(ref client) = tok.client {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new("client:")
                                                .small()
                                                .color(theme::TEXT_DIM),
                                        );
                                        crate::panels::widgets::clipped_value(
                                            ui,
                                            client,
                                            theme::TEXT_DIM,
                                        );
                                    });
                                }
                            });
                        ui.add_space(1.0);
                    }
                    if let Some((jti, sub)) = revoke_jti.take() {
                        db_tx.send(DbCommand::RevokeApiToken { jti, sub }).ok();
                    }
                }

                // Pagination + refresh row
                ui.add_space(4.0);
                let total_pages = ((inspector.api_tokens_total as u32).max(1) - 1) / API_TOKEN_PAGE_SIZE + 1;
                let current_page = inspector.api_tokens_page;
                ui.horizontal(|ui| {
                    let can_prev = current_page > 0;
                    if ui.add_enabled(can_prev, egui::Button::new("\u{25C0}").fill(theme::BG_BUTTON).small()).named(ui, "Previous API token page").clicked() {
                        inspector.api_tokens_page = current_page.saturating_sub(1);
                        send_scoped_token_list(db_tx, &inspector.api_token_scope_buf, inspector.api_tokens_page);
                    }
                    ui.label(
                        egui::RichText::new(format!("{}/{}", current_page + 1, total_pages))
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    let can_next = current_page + 1 < total_pages;
                    if ui.add_enabled(can_next, egui::Button::new("\u{25B6}").fill(theme::BG_BUTTON).small()).named(ui, "Next API token page").clicked() {
                        inspector.api_tokens_page = current_page + 1;
                        send_scoped_token_list(db_tx, &inspector.api_token_scope_buf, inspector.api_tokens_page);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("\u{21BB} Refresh").fill(theme::BG_BUTTON).small()).observed(ui).clicked() {
                            send_scoped_token_list(db_tx, &inspector.api_token_scope_buf, inspector.api_tokens_page);
                        }
                    });
                });

                ui.add_space(4.0);
            }).header_response.observed(ui);
        });
}

/// Send a scoped, paginated token list request.
fn send_scoped_token_list(db_tx: &crossbeam::channel::Sender<DbCommand>, scope: &str, page: u32) {
    if scope.is_empty() {
        db_tx
            .send(DbCommand::ListApiTokens {
                offset: page * API_TOKEN_PAGE_SIZE,
                limit: API_TOKEN_PAGE_SIZE,
            })
            .ok();
    } else {
        db_tx
            .send(DbCommand::ListApiTokensByScope {
                scope_prefix: scope.to_string(),
                offset: page * API_TOKEN_PAGE_SIZE,
                limit: API_TOKEN_PAGE_SIZE,
            })
            .ok();
    }
}

// ---------------------------------------------------------------------------
// Property type options for ComboBox dropdowns
// ---------------------------------------------------------------------------

const PROPERTY_TYPES: &[&str] = &[
    "string",
    "number",
    "bool",
    "datetime",
    "geometry_point",
    "geometry_polygon",
    "blob_ref",
    "hexon_ref",
    "address",
    "array",
    "object",
];

// ---------------------------------------------------------------------------
// Custom Properties section (key-value editor)
// ---------------------------------------------------------------------------

const PROPERTY_CONFIRM_SECONDS: f64 = 30.0;

#[derive(Default)]
pub(crate) struct PropertyEditorState {
    node_id: Option<String>,
    drafts: BTreeMap<String, PropertyDraft>,
    add_error: Option<String>,
    pending_add: Option<(String, PropertyWrite)>,
}

struct PropertyWrite {
    value: serde_json::Value,
    submitted_at: f64,
}

struct PropertyDraft {
    kind: PropertyValueKind,
    source: serde_json::Value,
    raw: String,
    original_raw: String,
    error: Option<String>,
    pending: Option<PropertyWrite>,
}

impl PropertyDraft {
    fn new(value: &serde_json::Value) -> Self {
        let raw = PropertyValueKind::format(value);
        Self {
            kind: PropertyValueKind::of(value),
            source: value.clone(),
            original_raw: raw.clone(),
            raw,
            error: None,
            pending: None,
        }
    }

    fn dirty(&self) -> bool {
        self.raw != self.original_raw
    }

    fn prepare_save(&mut self, stored: &serde_json::Value, now: f64) -> Option<serde_json::Value> {
        if self.pending.is_some() {
            return None;
        }
        if stored != &self.source {
            self.error = Some(
                "The stored value changed. Cancel and reopen this editor to use the latest value."
                    .into(),
            );
            return None;
        }
        match self.kind.parse(&self.raw) {
            Err(error) => {
                self.error = Some(error);
                None
            }
            Ok(value) if &value == stored => {
                self.original_raw = self.raw.clone();
                self.error = None;
                None
            }
            Ok(value) => {
                self.pending = Some(PropertyWrite {
                    value: value.clone(),
                    submitted_at: now,
                });
                self.error = None;
                Some(value)
            }
        }
    }

    fn reconcile(&mut self, stored: Option<&serde_json::Value>, now: f64) {
        if let Some(pending) = &self.pending {
            if stored == Some(&pending.value) {
                self.source = pending.value.clone();
                self.original_raw = self.raw.clone();
                self.pending = None;
                self.error = None;
            } else if now - pending.submitted_at >= PROPERTY_CONFIRM_SECONDS {
                self.pending = None;
                self.error = Some("Save has not been confirmed. Your draft is kept; refresh stored values before retrying.".into());
            }
            return;
        }
        if let Some(stored) = stored {
            if stored != &self.source {
                if self.dirty() {
                    self.error = Some("The stored value changed. Cancel and reopen this editor to use the latest value.".into());
                } else {
                    *self = Self::new(stored);
                }
            }
        }
    }
}

impl PropertyEditorState {
    fn set_scope(&mut self, node_id: Option<&str>) -> bool {
        if self.node_id.as_deref() == node_id {
            return false;
        }
        *self = Self {
            node_id: node_id.map(str::to_owned),
            ..Default::default()
        };
        true
    }

    fn reconcile(&mut self, properties: Option<&serde_json::Value>, now: f64) -> bool {
        let object = properties.and_then(serde_json::Value::as_object);
        if let Some(object) = object {
            self.drafts.retain(|key, _| object.contains_key(key));
        }
        for (key, draft) in &mut self.drafts {
            draft.reconcile(object.and_then(|object| object.get(key)), now);
        }
        if let Some((key, pending)) = &self.pending_add {
            if object.and_then(|object| object.get(key)) == Some(&pending.value) {
                self.pending_add = None;
                self.add_error = None;
                return true;
            }
            if now - pending.submitted_at >= PROPERTY_CONFIRM_SECONDS {
                self.pending_add = None;
                self.add_error = Some("Add has not been confirmed. Your draft is kept; refresh stored values before retrying.".into());
            }
        }
        false
    }
}

fn sync_property_editor_scope(
    inspector: &mut InspectorFormState,
    node_mgr: &NodeManager,
    now: f64,
) {
    let selected = node_mgr
        .selected
        .as_ref()
        .map(|selection| selection.node_id.as_str());
    if inspector.property_editor.set_scope(selected) {
        inspector.prop_add_key_buf.clear();
        inspector.prop_add_value_buf.clear();
        inspector.prop_add_type_buf = "string".into();
    }
    let stored = (!inspector.node_properties_loading).then_some(&inspector.node_properties);
    if inspector.property_editor.reconcile(stored, now) {
        inspector.prop_add_key_buf.clear();
        inspector.prop_add_value_buf.clear();
    }
}

fn submit_new_property(
    inspector: &mut InspectorFormState,
    node_id: &str,
    ui_mgr: &mut UiManager,
    now: f64,
) {
    if inspector.node_properties_loading
        || inspector.property_editor.pending_add.is_some()
        || inspector.property_editor.node_id.as_deref() != Some(node_id)
    {
        return;
    }
    let key = inspector.prop_add_key_buf.trim();
    let result = if key.is_empty() {
        Err("Enter a property key.".to_owned())
    } else if inspector.node_properties.get(key).is_some() {
        Err(
            "This property already exists. Use Edit on its card to preserve its stored type."
                .to_owned(),
        )
    } else {
        PropertyValueKind::from_selection(&inspector.prop_add_type_buf)
            .ok_or_else(|| "Choose a supported property type.".to_owned())
            .and_then(|kind| kind.parse(&inspector.prop_add_value_buf))
    };
    match result {
        Err(error) => inspector.property_editor.add_error = Some(error),
        Ok(value) => {
            inspector.property_editor.pending_add = Some((
                key.to_owned(),
                PropertyWrite {
                    value: value.clone(),
                    submitted_at: now,
                },
            ));
            inspector.property_editor.add_error = None;
            ui_mgr.push_action(UiAction::SetNodeProperty {
                node_id: node_id.to_owned(),
                key: key.to_owned(),
                value,
            });
        }
    }
}

fn submit_property_edit(
    inspector: &mut InspectorFormState,
    node_id: &str,
    key: &str,
    ui_mgr: &mut UiManager,
    now: f64,
) {
    if inspector.node_properties_loading
        || inspector.property_editor.node_id.as_deref() != Some(node_id)
    {
        return;
    }
    let Some(stored) = inspector.node_properties.get(key) else {
        return;
    };
    let Some(draft) = inspector.property_editor.drafts.get_mut(key) else {
        return;
    };
    if let Some(value) = draft.prepare_save(stored, now) {
        ui_mgr.push_action(UiAction::SetNodeProperty {
            node_id: node_id.to_owned(),
            key: key.to_owned(),
            value,
        });
    }
}

fn property_value_editor(
    ui: &mut egui::Ui,
    raw: &mut String,
    kind: PropertyValueKind,
    name: impl std::fmt::Display,
) -> egui::Response {
    let height = crate::ui_shell::layout_policy::editor_height(ui.ctx()).min(180.0);
    crate::ui_shell::layout_policy::scroll(
        ui,
        "Property value editor",
        [false, true],
        egui::ScrollArea::vertical()
            .id_salt("property-value-scroll")
            .max_height(height)
            .auto_shrink([false, true]),
        |ui| {
            ui.add(
                egui::TextEdit::multiline(raw)
                    .id_salt("property-value")
                    .desired_rows(
                        if matches!(kind, PropertyValueKind::Object | PropertyValueKind::Array) {
                            4
                        } else {
                            2
                        },
                    )
                    .desired_width(ui.available_width())
                    .font(egui::TextStyle::Monospace)
                    .hint_text(kind.hint()),
            )
            .named(ui, name)
        },
    )
    .inner
}

fn inspector_properties_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &crate::node_manager::NodeManager,
    ui_mgr: &mut UiManager,
) {
    egui::CollapsingHeader::new(
        egui::RichText::new("Custom Properties")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(true)
    .show(ui, |ui| {
        let Some(selection) = node_mgr.selected.as_ref() else {
            ui.label("No node selected");
            return;
        };
        let node_id = &selection.node_id;
        let now = ui.ctx().input(|input| input.time);
        if ui
            .button("Refresh stored values")
            .named(ui, "Refresh selected node properties")
            .clicked()
        {
            ui_mgr.push_action(UiAction::LoadNodeProperties {
                node_id: node_id.clone(),
            });
        }
        let loading = inspector.node_properties_loading;
        if loading {
            ui.label(
                egui::RichText::new("Refreshing properties...")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            let has_draft = !inspector.prop_add_key_buf.is_empty()
                || !inspector.prop_add_value_buf.is_empty()
                || !inspector.property_editor.drafts.is_empty()
                || inspector.property_editor.add_error.is_some()
                || inspector.property_editor.pending_add.is_some();
            if !has_draft
                && inspector
                    .node_properties
                    .as_object()
                    .is_none_or(|object| object.is_empty())
            {
                return;
            }
        }
        let mut delete_key = None;
        let mut save_key = None;
        let mut cancel_key = None;
        if let Some(object) = inspector.node_properties.as_object() {
            if object.is_empty() {
                ui.label("No custom properties");
            }
            for (key, value) in object {
                ui.push_id((node_id, key), |ui| {
                    egui::Frame::NONE
                        .fill(theme::BG_BUTTON)
                        .inner_margin(egui::Margin::symmetric(6, 6))
                        .corner_radius(3.0)
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(key).strong().color(theme::TEXT_SECTION),
                                )
                                .wrap(),
                            );
                            ui.label(
                                egui::RichText::new(PropertyValueKind::of(value).label())
                                    .small()
                                    .color(theme::TEXT_DIM),
                            );
                            let editing = inspector.property_editor.drafts.contains_key(key);
                            let pending = inspector
                                .property_editor
                                .drafts
                                .get(key)
                                .is_some_and(|draft| draft.pending.is_some());
                            if !editing {
                                let display = PropertyValueKind::format(value);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(crate::panels::widgets::elide(
                                            &display,
                                            PROP_VALUE_ELIDE_CHARS,
                                        ))
                                        .monospace()
                                        .small(),
                                    )
                                    .wrap(),
                                );
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui
                                    .small_button("Copy")
                                    .named(ui, format_args!("Copy property {key}"))
                                    .clicked()
                                {
                                    ui.ctx().copy_text(match value {
                                        serde_json::Value::String(value) => value.clone(),
                                        value => value.to_string(),
                                    });
                                    ui_mgr.show_toast(format!("Copied {key}"), now);
                                }
                                if !editing
                                    && ui
                                        .add_enabled(!loading, egui::Button::new("Edit").small())
                                        .named(ui, format_args!("Edit property {key}"))
                                        .clicked()
                                {
                                    inspector
                                        .property_editor
                                        .drafts
                                        .insert(key.clone(), PropertyDraft::new(value));
                                }
                                if ui
                                    .add_enabled(
                                        !loading && !pending,
                                        egui::Button::new("Delete").small(),
                                    )
                                    .named(ui, format_args!("Delete property {key}"))
                                    .clicked()
                                {
                                    delete_key = Some(key.clone());
                                }
                            });
                            if let Some(draft) = inspector.property_editor.drafts.get_mut(key) {
                                ui.add_enabled_ui(draft.pending.is_none(), |ui| {
                                    if property_value_editor(
                                        ui,
                                        &mut draft.raw,
                                        draft.kind,
                                        format_args!("Value of property {key}"),
                                    )
                                    .changed()
                                    {
                                        draft.error = None;
                                    }
                                });
                                if let Some(error) = &draft.error {
                                    ui.colored_label(theme::STATUS_OFFLINE, error);
                                }
                                if draft.pending.is_some() {
                                    ui.label("Waiting for saved value...");
                                } else if draft.dirty() {
                                    ui.label(
                                        egui::RichText::new("Unsaved changes")
                                            .small()
                                            .color(theme::TEXT_DIM),
                                    );
                                } else {
                                    ui.label(
                                        egui::RichText::new("Matches stored value")
                                            .small()
                                            .color(theme::TEXT_DIM),
                                    );
                                }
                                ui.horizontal_wrapped(|ui| {
                                    let can_save = !loading
                                        && draft.pending.is_none()
                                        && draft.dirty()
                                        && value == &draft.source;
                                    if ui
                                        .add_enabled(
                                            can_save,
                                            egui::Button::new("Save").fill(theme::BG_SAVE),
                                        )
                                        .named(ui, format_args!("Save property {key}"))
                                        .clicked()
                                    {
                                        save_key = Some(key.clone());
                                    }
                                    if ui
                                        .add_enabled(
                                            draft.pending.is_none(),
                                            egui::Button::new("Cancel"),
                                        )
                                        .named(ui, format_args!("Cancel editing property {key}"))
                                        .clicked()
                                    {
                                        cancel_key = Some(key.clone());
                                    }
                                });
                            }
                        });
                });
                ui.add_space(4.0);
            }
        }
        if let Some(key) = cancel_key {
            inspector.property_editor.drafts.remove(&key);
        }
        if let Some(key) = save_key {
            submit_property_edit(inspector, node_id, &key, ui_mgr, now);
        }
        if let Some(key) = delete_key {
            ui_mgr.push_action(UiAction::DeleteNodeProperty {
                node_id: node_id.clone(),
                key,
            });
        }
        ui.separator();
        ui.label(
            egui::RichText::new("Add Property")
                .strong()
                .color(theme::TEXT_SECTION),
        );
        let pending = inspector.property_editor.pending_add.is_some();
        ui.add_enabled_ui(!pending, |ui| {
            let key_label = ui.label("New property key");
            let mut changed = ui
                .add(
                    egui::TextEdit::singleline(&mut inspector.prop_add_key_buf)
                        .id_salt("new-property-key")
                        .desired_width(ui.available_width()),
                )
                .labelled(ui, &key_label)
                .changed();
            ui.label("Type");
            let mut type_changed = false;
            egui::ComboBox::from_id_salt("prop_type_combo")
                .selected_text(&inspector.prop_add_type_buf)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for &kind in PROPERTY_TYPES {
                        type_changed |= ui
                            .selectable_value(
                                &mut inspector.prop_add_type_buf,
                                kind.to_owned(),
                                kind,
                            )
                            .observed(ui)
                            .changed();
                    }
                })
                .response
                .named(ui, "New property value type");
            changed |= type_changed;
            let kind = PropertyValueKind::from_selection(&inspector.prop_add_type_buf)
                .unwrap_or(PropertyValueKind::String);
            ui.label("Value");
            changed |= property_value_editor(
                ui,
                &mut inspector.prop_add_value_buf,
                kind,
                "New property value",
            )
            .changed();
            if changed {
                inspector.property_editor.add_error = None;
            }
        });
        if let Some(error) = &inspector.property_editor.add_error {
            ui.colored_label(theme::STATUS_OFFLINE, error);
        }
        if pending {
            ui.label("Waiting for added property...");
        }
        let can_add = !loading && !pending && !inspector.prop_add_key_buf.trim().is_empty();
        if ui
            .add_enabled(can_add, egui::Button::new("Add").fill(theme::BG_SAVE))
            .named(ui, "Add property")
            .clicked()
        {
            submit_new_property(inspector, node_id, ui_mgr, now);
        }
    })
    .header_response
    .observed(ui);
}

// ---------------------------------------------------------------------------
// Property Schema section (field definition management)
// ---------------------------------------------------------------------------

fn inspector_schema_section(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &crate::node_manager::NodeManager,
    local_role: &LocalUserRole,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
) {
    if !local_role.can_manage() {
        return;
    }

    egui::CollapsingHeader::new(
        egui::RichText::new("Property Schema")
            .strong()
            .color(theme::TEXT_SECTION),
    )
    .default_open(false)
    .show(ui, |ui| {
        ui.add_space(4.0);

        if node_mgr.selected.is_none() {
            ui.label(
                egui::RichText::new("No node selected")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
            return;
        }

        if inspector.field_defs_loading {
            ui.label(
                egui::RichText::new("Loading...")
                    .small()
                    .color(theme::TEXT_DIM),
            );
            return;
        }

        // Display existing field definitions
        let mut delete_idx: Option<usize> = None;
        if inspector.field_defs.is_empty() {
            ui.label(
                egui::RichText::new("No field definitions")
                    .small()
                    .color(theme::TEXT_MUTED),
            );
        } else {
            egui::Grid::new("field_defs_grid")
                .num_columns(4)
                .spacing([6.0, 3.0])
                .show(ui, |ui| {
                    // Header
                    ui.label(
                        egui::RichText::new("Key")
                            .small()
                            .strong()
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        egui::RichText::new("Type")
                            .small()
                            .strong()
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(
                        egui::RichText::new("Req")
                            .small()
                            .strong()
                            .color(theme::TEXT_DIM),
                    );
                    ui.label(egui::RichText::new("").small());
                    ui.end_row();

                    for (idx, fd) in inspector.field_defs.iter().enumerate() {
                        ui.label(
                            egui::RichText::new(&fd.key)
                                .small()
                                .color(theme::TEXT_BRIGHT),
                        );
                        ui.label(
                            egui::RichText::new(&fd.value_type)
                                .small()
                                .monospace()
                                .color(theme::TEXT_AXIS),
                        );
                        let req_label = if fd.required { "\u{2713}" } else { "\u{2014}" };
                        ui.label(
                            egui::RichText::new(req_label)
                                .small()
                                .color(theme::TEXT_DIM),
                        );
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("\u{2715}").small())
                                    .fill(theme::BG_DANGER)
                                    .small(),
                            )
                            .named(ui, format_args!("Delete field definition {}", fd.key))
                            .on_hover_text(format!("Delete field def '{}'", fd.key))
                            .clicked()
                        {
                            delete_idx = Some(idx);
                        }
                        ui.end_row();
                    }
                });
        }

        if let Some(idx) = delete_idx {
            let fd = inspector.field_defs.remove(idx);
            db_tx
                .send(DbCommand::DeleteFieldDef {
                    field_def_id: fd.field_def_id,
                })
                .ok();
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        // Add field definition row
        ui.label(
            egui::RichText::new("Add Field Definition")
                .small()
                .color(theme::TEXT_DIM),
        );
        ui.add_space(2.0);

        // Key + type only — `DbCommand::CreateFieldDef` cannot persist
        // description/required yet.
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut inspector.field_def_add_key_buf)
                    .hint_text("key")
                    .desired_width(60.0),
            )
            .named(ui, "New field definition key");
            egui::ComboBox::from_id_salt("field_def_type_combo")
                .selected_text(&inspector.field_def_add_type_buf)
                .width(80.0)
                .show_ui(ui, |ui| {
                    for &t in PROPERTY_TYPES {
                        ui.selectable_value(
                            &mut inspector.field_def_add_type_buf,
                            t.to_string(),
                            t,
                        )
                        .observed(ui);
                    }
                })
                .response
                .named(ui, "New field definition type");
        });

        ui.add_space(4.0);

        let can_add = !inspector.field_def_add_key_buf.trim().is_empty();
        let btn = egui::Button::new("Add Field")
            .fill(if can_add {
                theme::BG_SAVE
            } else {
                theme::BG_BUTTON
            })
            .min_size(egui::vec2(ui.available_width(), 24.0));

        if ui.add_enabled(can_add, btn).observed(ui).clicked() {
            let key = inspector.field_def_add_key_buf.trim().to_string();
            let value_type = inspector.field_def_add_type_buf.clone();
            let scope = build_nav_scope(nav);

            // Send DbCommand to persist
            db_tx
                .send(DbCommand::CreateFieldDef {
                    scope: scope.clone(),
                    entity_type: "node".to_string(),
                    key: key.clone(),
                    value_type: value_type.clone(),
                    default_val: None,
                })
                .ok();

            // Add locally for immediate feedback
            let entry = crate::plugin::FieldDefEntry {
                field_def_id: format!("pending-{}", inspector.field_defs.len()),
                key,
                value_type,
                description: String::new(),
                required: false,
                default_val: None,
            };
            inspector.field_defs.push(entry);
            inspector.field_def_add_key_buf.clear();
        }

        ui.add_space(4.0);
    })
    .header_response
    .observed(ui);
}

// ---------------------------------------------------------------------------
// Tests — pure real-unit conversion helpers (FR-2). No Bevy App / egui context.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = 1e-4;

    #[test]
    fn inspector_transform_and_typed_property_editing_journey() {
        assert!((radians_to_degrees(std::f32::consts::FRAC_PI_2) - 90.0).abs() < EPS);
        assert!((degrees_to_radians(180.0) - std::f32::consts::PI).abs() < EPS);
        let r = 1.234_f32;
        assert!((degrees_to_radians(radians_to_degrees(r)) - r).abs() < EPS);

        use serde_json::json;
        let mut nodes = NodeManager::default();
        nodes.select(Entity::from_bits(1), "node-1");
        let mut inspector = InspectorFormState::default();
        let mut ui_mgr = UiManager::default();
        sync_property_editor_scope(&mut inspector, &nodes, 0.0);
        let (tx, rx) = crossbeam::channel::unbounded();
        let db_sender = fe_runtime::app::DbCommandSender(tx);
        let nested = json!({"labels": ["north", "south"], "sensor": {"active": true, "samples": [1, null, {"value": 2.5}]}});
        for (key, selected_type, raw, expected) in [
            (
                "note",
                "string",
                "  exact text\nwith whitespace  ",
                json!("  exact text\nwith whitespace  "),
            ),
            ("empty-note", "string", "", json!("")),
            ("amount", "number", "  -1.5e2  ", json!(-150.0)),
            ("counter", "number", "18446744073709551615", json!(u64::MAX)),
            ("flag", "bool", " false ", json!(false)),
            (
                "config",
                "object",
                r#"{"labels":["north","south"],"sensor":{"active":true,"samples":[1,null,{"value":2.5}]}}"#,
                nested.clone(),
            ),
            (
                "series",
                "array",
                r#"[1,{"nested":[true,null,"text"]}]"#,
                json!([1,{"nested":[true,null,"text"]}]),
            ),
            (
                "address-label",
                "address",
                " exact address ",
                json!(" exact address "),
            ),
        ] {
            inspector.prop_add_key_buf = format!("  {key}  ");
            inspector.prop_add_type_buf = selected_type.into();
            inspector.prop_add_value_buf = raw.into();
            let stored_before = inspector.node_properties.clone();
            submit_new_property(&mut inspector, "node-1", &mut ui_mgr, 1.0);
            assert_eq!(
                inspector.node_properties, stored_before,
                "queueing must not mutate stored data"
            );
            assert_eq!(
                inspector.prop_add_value_buf, raw,
                "retain the submitted draft until readback"
            );
            let actions = ui_mgr.drain_actions();
            let [UiAction::SetNodeProperty {
                node_id,
                key: action_key,
                value,
            }] = actions.as_slice()
            else {
                panic!("a valid property must queue exactly one typed write")
            };
            assert_eq!(node_id, "node-1");
            assert_eq!(action_key, key);
            assert_eq!(value, &expected);
            crate::actions::node_props::set(
                &db_sender,
                node_id.clone(),
                action_key.clone(),
                value.clone(),
            );
            let DbCommand::SetNodeProperty {
                node_id,
                key: command_key,
                value: command_value,
                correlation_id,
            } = rx.try_recv().unwrap()
            else {
                panic!("the existing action handler must dispatch the property write")
            };
            assert_eq!(node_id, "node-1");
            assert_eq!(command_key, key);
            assert_eq!(command_value, expected);
            assert!(
                correlation_id.is_none(),
                "GUI writes retain the None correlation contract"
            );
            sync_property_editor_scope(&mut inspector, &nodes, 2.0);
            assert!(
                inspector.property_editor.pending_add.is_some(),
                "old cache is not a save confirmation"
            );
            inspector
                .node_properties
                .as_object_mut()
                .unwrap()
                .insert(key.into(), expected);
            sync_property_editor_scope(&mut inspector, &nodes, 3.0);
            assert!(inspector.property_editor.pending_add.is_none());
            assert!(inspector.prop_add_key_buf.is_empty());
            assert!(inspector.prop_add_value_buf.is_empty());
        }
        assert_eq!(
            inspector.node_properties["counter"].as_u64(),
            Some(u64::MAX)
        );
        let stored = inspector.node_properties.clone();
        for (kind, invalid) in [
            ("number", "NaN"),
            ("number", "Inf"),
            ("number", "-Infinity"),
            ("number", "1e999"),
            ("number", "01"),
            ("number", r#""12""#),
            ("bool", "yes"),
            ("bool", "1"),
            ("bool", "TRUE"),
            ("object", "[]"),
            ("object", "{\"unfinished\":"),
            ("array", "{}"),
            ("array", "[1,"),
            ("unknown", "value"),
        ] {
            inspector.prop_add_key_buf = "invalid".into();
            inspector.prop_add_type_buf = kind.into();
            inspector.prop_add_value_buf = invalid.into();
            submit_new_property(&mut inspector, "node-1", &mut ui_mgr, 4.0);
            assert!(
                ui_mgr.drain_actions().is_empty(),
                "invalid input cannot queue a mutation: {kind} {invalid}"
            );
            assert!(rx.try_recv().is_err());
            assert_eq!(inspector.node_properties, stored);
            assert_eq!(inspector.prop_add_value_buf, invalid);
            assert!(inspector.property_editor.add_error.is_some());
            assert_eq!(nodes.selected.as_ref().unwrap().node_id, "node-1");
        }
        inspector.prop_add_key_buf = "config".into();
        inspector.prop_add_type_buf = "string".into();
        inspector.prop_add_value_buf = "do not replace the object".into();
        submit_new_property(&mut inspector, "node-1", &mut ui_mgr, 5.0);
        assert!(
            ui_mgr.drain_actions().is_empty(),
            "Add must not silently replace an existing typed property"
        );
        assert_eq!(inspector.node_properties, stored);

        inspector
            .property_editor
            .drafts
            .insert("config".into(), PropertyDraft::new(&nested));
        let draft = inspector.property_editor.drafts.get_mut("config").unwrap();
        assert_eq!(draft.kind, PropertyValueKind::Object);
        assert!(!draft.dirty());
        draft.raw = "[]".into();
        submit_property_edit(&mut inspector, "node-1", "config", &mut ui_mgr, 10.0);
        assert!(ui_mgr.drain_actions().is_empty());
        let draft = &inspector.property_editor.drafts["config"];
        assert_eq!(draft.raw, "[]");
        assert!(draft.dirty() && draft.error.is_some());
        assert_eq!(inspector.node_properties, stored);
        let edited =
            json!({"labels": ["west"], "sensor": {"active": false, "samples": [4, {"value": 8}]}});
        let edited_raw = format!("  {}  ", edited);
        inspector
            .property_editor
            .drafts
            .get_mut("config")
            .unwrap()
            .raw = edited_raw.clone();
        submit_property_edit(&mut inspector, "node-1", "config", &mut ui_mgr, 11.0);
        let actions = ui_mgr.drain_actions();
        assert!(
            matches!(actions.as_slice(), [UiAction::SetNodeProperty { node_id, key, value }] if node_id == "node-1" && key == "config" && value == &edited)
        );
        assert_eq!(inspector.node_properties, stored);
        submit_property_edit(&mut inspector, "node-1", "config", &mut ui_mgr, 11.1);
        assert!(
            ui_mgr.drain_actions().is_empty(),
            "an awaiting save must not double-submit"
        );
        inspector.node_properties_loading = true;
        sync_property_editor_scope(&mut inspector, &nodes, 12.0);
        assert!(inspector.property_editor.drafts["config"].pending.is_some());
        inspector.node_properties["config"] = edited.clone();
        inspector.node_properties_loading = false;
        sync_property_editor_scope(&mut inspector, &nodes, 13.0);
        let draft = &inspector.property_editor.drafts["config"];
        assert_eq!(draft.kind, PropertyValueKind::Object);
        assert_eq!(draft.raw, edited_raw);
        assert!(draft.pending.is_none() && !draft.dirty() && draft.error.is_none());

        inspector
            .property_editor
            .drafts
            .insert("flag".into(), PropertyDraft::new(&json!(false)));
        inspector
            .property_editor
            .drafts
            .get_mut("flag")
            .unwrap()
            .raw = "true".into();
        submit_property_edit(&mut inspector, "node-1", "flag", &mut ui_mgr, 20.0);
        assert_eq!(ui_mgr.drain_actions().len(), 1);
        inspector.node_properties_loading = true;
        sync_property_editor_scope(&mut inspector, &nodes, 51.0);
        let timed_out = &inspector.property_editor.drafts["flag"];
        assert!(timed_out.pending.is_none() && timed_out.dirty() && timed_out.error.is_some());
        assert_eq!(inspector.node_properties["flag"], json!(false));
        inspector
            .property_editor
            .drafts
            .get_mut("flag")
            .unwrap()
            .raw = "false ".into();
        submit_property_edit(&mut inspector, "node-1", "flag", &mut ui_mgr, 51.1);
        assert!(
            ui_mgr.drain_actions().is_empty(),
            "an editable local draft cannot bypass a pending stored-value refresh"
        );
        inspector.node_properties_loading = false;

        inspector
            .property_editor
            .drafts
            .get_mut("config")
            .unwrap()
            .raw = r#"{"local":"kept"}"#.into();
        inspector.node_properties["config"] = json!({"remote": "changed"});
        sync_property_editor_scope(&mut inspector, &nodes, 52.0);
        submit_property_edit(&mut inspector, "node-1", "config", &mut ui_mgr, 52.1);
        assert!(
            ui_mgr.drain_actions().is_empty(),
            "a changed stored value cannot be silently overwritten"
        );
        assert_eq!(
            inspector.property_editor.drafts["config"].raw,
            r#"{"local":"kept"}"#
        );
        assert!(inspector.property_editor.drafts["config"].error.is_some());
        inspector.property_editor.drafts.remove("config");
        assert_eq!(
            inspector.node_properties["config"],
            json!({"remote":"changed"})
        );
        inspector
            .node_properties
            .as_object_mut()
            .unwrap()
            .remove("flag");
        sync_property_editor_scope(&mut inspector, &nodes, 53.0);
        assert!(
            !inspector.property_editor.drafts.contains_key("flag"),
            "a deleted key must release its draft scope"
        );
        assert_eq!(
            PropertyValueKind::Null.parse(" null ").unwrap(),
            json!(null)
        );
        assert!(PropertyValueKind::Null.parse("false").is_err());

        inspector.prop_add_key_buf = "scope-local".into();
        inspector.prop_add_value_buf = "8".into();
        inspector.prop_add_type_buf = "number".into();
        submit_new_property(&mut inspector, "node-1", &mut ui_mgr, 54.0);
        assert_eq!(ui_mgr.drain_actions().len(), 1);
        nodes.select(Entity::from_bits(2), "node-2");
        inspector.node_properties = json!({});
        inspector.node_properties_loading = true;
        sync_property_editor_scope(&mut inspector, &nodes, 55.0);
        assert_eq!(nodes.selected.as_ref().unwrap().node_id, "node-2");
        assert!(inspector.prop_add_key_buf.is_empty() && inspector.prop_add_value_buf.is_empty());
        assert_eq!(inspector.prop_add_type_buf, "string");
        assert!(inspector.property_editor.drafts.is_empty());
        assert!(
            inspector.property_editor.pending_add.is_none()
                && inspector.property_editor.add_error.is_none()
        );
        nodes.deselect();
        sync_property_editor_scope(&mut inspector, &nodes, 56.0);
        assert!(inspector.property_editor.node_id.is_none());
    }

    #[test]
    fn size_to_scale_back_computes_multiplier() {
        // base extent 2 wu at scale 1; world_scale 1.0. Want 6 m → scale 3.
        assert!((size_to_scale(6.0, 2.0, 1.0).unwrap() - 3.0).abs() < EPS);
        // With world_scale 0.001: 6 m = 0.006 wu; /2 wu → scale 0.003.
        assert!((size_to_scale(6.0, 2.0, 0.001).unwrap() - 0.003).abs() < EPS);
    }

    #[test]
    fn size_to_scale_guards_zero_extent() {
        // A zero-extent axis has no ratio → None (leave scale untouched).
        assert!(size_to_scale(5.0, 0.0, 1.0).is_none());
        assert!(size_to_scale(5.0, 1e-9, 1.0).is_none());
    }

    #[test]
    fn size_scale_are_inverses() {
        // scale_to_size ∘ size_to_scale == identity on the target size.
        let base = 1.5_f32;
        let ws = 0.01_f64;
        let scale = size_to_scale(12.0, base, ws).unwrap();
        assert!((scale_to_size(scale, base, ws) - 12.0).abs() < 1e-3);
    }
}
