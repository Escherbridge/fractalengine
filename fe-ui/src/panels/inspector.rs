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
use crate::ui_shell::layout_policy;
use crate::verse_manager::db_results::tokens::mint_command;
use crate::verse_manager::VerseManager;
use fe_runtime::messages::DbCommand;

/// Inspector navigation and scrolling tab body; see panels/AGENTS.md.
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
    synchronize_token_target(
        inspector,
        node_mgr
            .selected
            .as_ref()
            .map(|selection| selection.node_id.as_str()),
        &build_nav_scope(nav),
        local_role.can_manage(),
    );
    crate::object_data::synchronize(
        &mut inspector.object_data,
        node_mgr,
        hierarchy,
        nav,
        local_role,
    );
    egui::Frame::NONE
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            inspector_navigation(ui, inspector, node_mgr, db_tx, nav)
        });
    crate::ui_shell::layout_policy::scroll(
        ui,
        "Inspector",
        [false, true],
        egui::ScrollArea::vertical()
            .id_salt("right_section_scroll_Inspector")
            .auto_shrink([false, false]),
        |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    inspector_tab_body(
                        ui,
                        inspector,
                        node_mgr,
                        hierarchy,
                        ui_mgr,
                        local_role,
                        db_tx,
                        nav,
                        asset_status,
                    );
                });
        },
    );
}

fn inspector_navigation(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &mut NodeManager,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
) {
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
    ui.horizontal_wrapped(|ui| {
        for (tab, label) in [
            (InspectorTab::Properties, "Properties"),
            (InspectorTab::ApiAccess, "API Access"),
            (InspectorTab::Query, "Query"),
            (InspectorTab::Data, "Data"),
        ] {
            let active = inspector.active_tab() == tab;
            let btn = egui::Button::new(egui::RichText::new(label).small().color(if active {
                theme::TEXT_BRIGHT
            } else {
                theme::TEXT_DIM
            }))
            .fill(if active {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            })
            .min_size(egui::Vec2::splat(crate::object_data::target_size(ui)));
            if ui
                .add(btn)
                .named(ui, format_args!("Inspector tab: {label}"))
                .clicked()
            {
                inspector.set_active_tab(tab);

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
}

fn inspector_tab_body(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_mgr: &NodeManager,
    hierarchy: &VerseManager,
    ui_mgr: &mut UiManager,
    local_role: &LocalUserRole,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    nav: &NavigationManager,
    asset_status: &AssetDownloadStatus,
) {
    match inspector.active_tab() {
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
            inspector_url_meta_section(ui, inspector, node_mgr, ui_mgr, local_role);
            ui.add_space(2.0);
            inspector_properties_section(ui, inspector, node_mgr, ui_mgr);
            ui.add_space(2.0);
            inspector_schema_section(ui, inspector, node_mgr, local_role, db_tx, nav, ui_mgr);
        }
        InspectorTab::ApiAccess => {
            ui.add_space(4.0);
            inspector_api_access_section(ui, inspector, node_mgr, db_tx, local_role, ui_mgr);
        }
        InspectorTab::Query => {
            ui.add_space(4.0);
            crate::panels::query_tab::inspector_query_section(
                ui, inspector, nav, node_mgr, hierarchy, ui_mgr,
            );
        }
        InspectorTab::Data => {
            crate::object_data::render(ui, &mut inspector.object_data, local_role, db_tx, ui_mgr);
        }
    }
}

/// Build a scope string from the current navigation state.
pub(crate) fn build_nav_scope(nav: &NavigationManager) -> String {
    nav.active_scope().unwrap_or_default()
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
    node_mgr: &NodeManager,
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
        let validation = crate::actions::portal::compute_save_url(node_mgr, inspector);
        if let crate::actions::portal::SaveUrlOutcome::Blocked { reason } = &validation {
            ui.add(
                egui::Label::new(egui::RichText::new(reason).color(theme::STATUS_OFFLINE)).wrap(),
            );
        }
        let valid = matches!(
            validation,
            crate::actions::portal::SaveUrlOutcome::Persist { .. }
        );
        let editable = local_role.can_edit();
        if ui
            .add_enabled(
                valid && editable,
                egui::Button::new("\u{1F4BE} Save")
                    .fill(theme::BG_SAVE)
                    .min_size(egui::vec2(ui.available_width(), 28.0)),
            )
            .named(ui, "Save portal URLs")
            .on_disabled_hover_text(if node_mgr.selected.is_none() {
                "Select a node to save its Portal URL."
            } else if !editable {
                "Editor access is required to save this Portal URL."
            } else {
                "Correct the Portal URL before saving."
            })
            .clicked()
        {
            ui_mgr.push_action(UiAction::SaveUrl);
        }

        ui.add_space(4.0);

        let has_url = valid && !inspector.external_url.trim().is_empty();
        let btn = egui::Button::new("\u{1F310} Open Portal")
            .fill(if has_url {
                theme::BG_BUTTON_ACTIVE
            } else {
                theme::BG_BUTTON
            })
            .min_size(egui::vec2(ui.available_width(), 28.0));
        let resp = ui.add_enabled(has_url, btn).observed(ui);
        if resp.clicked() {
            ui_mgr.push_action(UiAction::OpenPortal {
                url: inspector.external_url.clone(),
            });
        }
        if !has_url {
            resp.on_disabled_hover_text(
                "Select a node and enter an allowed Portal URL to open it.",
            );
        }

        ui.add_space(4.0);
    })
    .header_response
    .observed(ui);
}

const INSPECTOR_MINT_PREFIX: &str = "ui-inspector-mint:";

#[derive(Default)]
pub(crate) struct InspectorTokenRequest {
    node_id: Option<String>,
    scope: String,
    pending: Option<PendingTokenMint>,
    error: Option<&'static str>,
    revealed: bool,
}

struct PendingTokenMint {
    nonce: String,
    scope: String,
    role: String,
    client: Option<String>,
    started: std::time::Instant,
}

impl InspectorTokenRequest {
    pub(crate) fn owns(id: &str) -> bool {
        id.starts_with(INSPECTOR_MINT_PREFIX)
    }

    pub(crate) fn cancel(&mut self) {
        self.pending = None;
        self.error = None;
        self.revealed = false;
    }

    fn expire(&mut self, now: std::time::Instant) {
        if self
            .pending
            .as_ref()
            .is_some_and(|request| now.saturating_duration_since(request.started).as_secs() >= 30)
        {
            self.pending = None;
            self.error = Some("Generation was not confirmed. A token may exist; refresh the list before retrying.");
        }
    }

    pub(crate) fn accept(
        &mut self,
        id: &str,
        scope: &str,
        role: &str,
        client: &str,
        now: std::time::Instant,
    ) -> bool {
        self.expire(now);
        let Some(request) = self.pending.as_ref().filter(|request| request.nonce == id) else {
            return false;
        };
        let matches = request.scope == scope
            && request.role == role
            && request
                .client
                .as_deref()
                .is_none_or(|expected| expected == client);
        self.pending = None;
        self.revealed = false;
        self.error = (!matches).then_some(
            "The token reply did not match this request. Refresh the list before retrying.",
        );
        matches
    }

    pub(crate) fn fail(&mut self, id: &str, now: std::time::Instant) {
        self.expire(now);
        if self
            .pending
            .as_ref()
            .is_some_and(|request| request.nonce == id)
        {
            self.pending = None;
            self.error =
                Some("Token generation failed. Check your access and client DID, then retry.");
        }
    }
}

pub(crate) fn synchronize_token_target(
    inspector: &mut InspectorFormState,
    selected: Option<&str>,
    scope: &str,
    can_manage: bool,
) {
    let changed = inspector.api_token_request.node_id.as_deref() != selected
        || inspector.api_token_request.scope != scope;
    if changed {
        inspector.api_token_request = InspectorTokenRequest {
            node_id: selected.map(str::to_owned),
            scope: scope.to_owned(),
            ..Default::default()
        };
        inspector.api_token_scope_buf = scope.to_owned();
        inspector.api_token_client_buf.clear();
        inspector.api_tokens.clear();
        inspector.api_tokens_total = 0;
        inspector.api_tokens_page = 0;
        inspector.api_tokens_loading = false;
    }
    if changed
        || !can_manage
        || selected.is_none()
        || inspector.active_tab() != InspectorTab::ApiAccess
    {
        inspector.generated_api_token = None;
        inspector.generated_api_token_client = None;
        inspector.api_token_request.cancel();
    }
}

pub(crate) fn submit_inspector_token(
    inspector: &mut InspectorFormState,
    node_id: &str,
    can_manage: bool,
    db_tx: &crossbeam::channel::Sender<DbCommand>,
    now: std::time::Instant,
) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(1);
    if inspector.active_tab() != InspectorTab::ApiAccess
        || !can_manage
        || inspector.api_token_request.pending.is_some()
        || inspector.api_token_request.node_id.as_deref() != Some(node_id)
        || inspector.api_token_scope_buf.is_empty()
        || inspector.api_token_scope_buf != inspector.api_token_request.scope
    {
        return;
    }
    let client = inspector.api_token_client_buf.trim();
    if !client.is_empty() && fe_identity::validate_client_did(client).is_err() {
        inspector.api_token_request.error =
            Some("Enter a valid ed25519 client DID or leave it blank.");
        return;
    }
    let nonce = format!(
        "{INSPECTOR_MINT_PREFIX}{}",
        NONCE.fetch_add(1, Ordering::Relaxed)
    );
    let request = PendingTokenMint {
        nonce: nonce.clone(),
        scope: inspector.api_token_scope_buf.clone(),
        role: inspector.api_token_role_buf.clone(),
        client: (!client.is_empty()).then(|| client.to_owned()),
        started: now,
    };
    let command = mint_command(
        request.scope.clone(),
        request.role.clone(),
        inspector.api_token_expiry_buf,
        Some(format!("node:{node_id}")),
        client,
        Some(nonce),
    );
    inspector.generated_api_token = None;
    inspector.generated_api_token_client = None;
    inspector.api_token_request.revealed = false;
    inspector.api_token_request.error = None;
    if db_tx.send(command).is_ok() {
        inspector.api_token_request.pending = Some(request);
    } else {
        bevy::log::warn!("DB command channel closed during Inspector token generation");
        inspector.api_token_request.error =
            Some("Could not send the token request. Your inputs are kept.");
    }
}

fn node_properties_curl(
    address: std::net::SocketAddr,
    node_id: &str,
    token: &str,
) -> Option<String> {
    let mut url = url::Url::parse(&format!("http://{address}")).ok()?;
    url.path_segments_mut()
        .ok()?
        .extend(["api", "v1", "nodes", node_id, "properties"]);
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    Some(format!(
        "curl -sS {} -H {}",
        quote(url.as_str()),
        quote(&format!("Authorization: Bearer {token}"))
    ))
}

fn token_expiry_label(raw: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(raw) else {
        return "Expiry unavailable".into();
    };
    let seconds = expiry.signed_duration_since(now).num_seconds();
    if seconds <= 0 {
        return "Expired".into();
    }
    if seconds < 60 {
        return "Expires in under a minute".into();
    }
    let (count, unit) = if seconds < 3600 {
        ((seconds + 59) / 60, "minute")
    } else if seconds < 86400 {
        ((seconds + 3599) / 3600, "hour")
    } else {
        ((seconds + 86399) / 86400, "day")
    };
    format!(
        "Expires in {count} {unit}{}",
        if count == 1 { "" } else { "s" }
    )
}

fn copy_inspector_text(ui: &egui::Ui, ui_mgr: &mut UiManager, value: &str, notice: &str) {
    ui.ctx().copy_text(value.to_owned());
    ui_mgr.show_toast(notice, ui.input(|input| input.time));
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
        None => {
            ui.label("Select a node to manage API access and create a scoped token.");
            return;
        }
    };
    inspector
        .api_token_request
        .expire(std::time::Instant::now());

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

            if let Some(address) = inspector.api_endpoint {
                crate::panels::widgets::clipped_value_copyable(ui, ui_mgr,
                    "Local API address", &format!("http://{address}"), theme::TEXT_DIM, "Copied API address");
            } else {
                ui.label("The local API address is not available yet.");
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
                let pending = inspector.api_token_request.pending.is_some();

                // Role + Expiry on separate rows for narrow panels
                ui.add_enabled_ui(!pending, |ui| {
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
                ui.add_enabled(!pending,
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
                if pending { ui.label("Creating API token…"); }
                if let Some(error) = inspector.api_token_request.error {
                    ui.add(egui::Label::new(egui::RichText::new(error).color(theme::STATUS_OFFLINE)).wrap());
                }
                if ui
                    .add_enabled(
                        client_valid && !pending && !inspector.api_token_scope_buf.is_empty(),
                        egui::Button::new("Generate API Token")
                            .fill(theme::BG_SAVE)
                            .min_size(egui::vec2(ui.available_width(), 28.0)),
                    ).observed(ui)
                    .clicked()
                {
                    submit_inspector_token(inspector, node_id, local_role.can_manage(), db_tx, std::time::Instant::now());
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
                                    if layout_policy::action(ui, egui::Button::new("\u{2715}").fill(egui::Color32::TRANSPARENT), "Dismiss generated API token").clicked() {
                                        dismiss_token = true;
                                    }
                                });
                            });

                            ui.add_space(2.0);
                            let mut display = token.clone();
                            let reveal_label = if inspector.api_token_request.revealed { "Hide token" } else { "Reveal token" };
                            let reveal_name = if inspector.api_token_request.revealed { "Hide token: generated API token" } else { "Reveal token: generated API token" };
                            if layout_policy::action(ui, egui::Button::new(reveal_label), reveal_name).clicked() {
                                inspector.api_token_request.revealed = !inspector.api_token_request.revealed;
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut display)
                                    .password(!inspector.api_token_request.revealed)
                                    .interactive(false)
                                    .desired_width(ui.available_width())
                                    .clip_text(true)
                                    .font(egui::TextStyle::Monospace),
                            ).named(ui, "Generated API token");

                            // Minted `client` claim (W6-7b): shown so the
                            // operator can hand it to the next mint for the
                            // same client, or record it for revocation.
                            if let Some(ref client) = inspector.generated_api_token_client {
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    if layout_policy::action(ui,
                                        egui::Button::new("Copy client").fill(egui::Color32::TRANSPARENT),
                                        "Copy generated token client DID").on_hover_text("Copy client DID")
                                        .clicked()
                                    {
                                        copy_inspector_text(ui, ui_mgr, client, "Copied token client DID");
                                    }
                                    crate::panels::widgets::clipped_value(
                                        ui,
                                        client,
                                        theme::TEXT_SECTION,
                                    );
                                });
                            }

                            ui.add_space(4.0);
                            // Copy Token button
                            ui.horizontal(|ui| {
                                if layout_policy::action(ui, egui::Button::new("\u{1F4CB} Copy Token").fill(theme::BG_BUTTON_ACTIVE), "Copy generated API token").clicked() {
                                    copy_inspector_text(ui, ui_mgr, token, "Copied token to clipboard");
                                }
                            });

                            // curl command — pre-built with the actual token
                            ui.add_space(4.0);
                            if let Some(address) = inspector.api_endpoint {
                            let preview_token = if inspector.api_token_request.revealed { token.as_str() } else { "<TOKEN>" };
                            let curl_cmd = node_properties_curl(address, node_id, preview_token).unwrap_or_default();
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
                                            if layout_policy::action(ui, egui::Button::new("\u{1F4CB}").fill(egui::Color32::TRANSPARENT),
                                                "Copy selected node properties curl command").on_hover_text("Copy authenticated GET properties command")
                                                .clicked()
                                            {
                                                if let Some(command) = node_properties_curl(address, node_id, token) {
                                                    copy_inspector_text(ui, ui_mgr, &command, "Copied curl command");
                                                }
                                            }
                                        });
                                    });
                                    crate::panels::widgets::clipped_value(
                                        ui,
                                        &curl_cmd,
                                        theme::TEXT_DIM,
                                    );
                                });
                            } else {
                                ui.label("Curl is unavailable until the local API reports its bound address.");
                            }
                        });
                }
                if dismiss_token {
                    inspector.generated_api_token = None;
                    inspector.generated_api_token_client = None;
                    inspector.api_token_request.revealed = false;
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
                                    egui::RichText::new(token_expiry_label(&tok.expires_at, chrono::Utc::now()))
                                        .small()
                                        .color(theme::TEXT_MUTED),
                                ).on_hover_text(&tok.expires_at);
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
            "This property already exists. Use Edit on its row to preserve its stored type."
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

#[derive(Default)]
struct PropertyTableActions {
    save: Option<String>,
    cancel: Option<String>,
    delete: Option<String>,
}

fn render_property_table(
    ui: &mut egui::Ui,
    inspector: &mut InspectorFormState,
    node_id: &str,
    ui_mgr: &mut UiManager,
    now: f64,
) -> PropertyTableActions {
    use super::property_grid::Columns;

    let mut actions = PropertyTableActions::default();
    let Some(object) = inspector
        .node_properties
        .as_object()
        .filter(|o| !o.is_empty())
    else {
        ui.label("No custom properties");
        return actions;
    };
    let columns = Columns::new(ui, ui.available_width());
    if columns.width() > ui.available_width() {
        ui.label(egui::RichText::new("Scroll horizontally for row actions").small());
    }
    crate::ui_shell::layout_policy::scroll(
        ui,
        "Custom property table",
        [true, false],
        egui::ScrollArea::horizontal()
            .id_salt(("property-table", node_id))
            .auto_shrink([false, true]),
        |ui| {
            ui.set_width(columns.width());
            columns.row(ui, |ui| {
                for (column, heading) in ["Key", "Type", "Value", "Actions"].into_iter().enumerate()
                {
                    columns.cell(ui, column, |ui| {
                        ui.add(egui::Label::new(egui::RichText::new(heading).strong()).truncate());
                    });
                }
            });
            ui.separator();
            for (key, value) in object {
                ui.push_id((node_id, key), |ui| {
                    render_property_row(
                        ui,
                        &columns,
                        node_id,
                        key,
                        value,
                        &mut inspector.property_editor,
                        inspector.node_properties_loading,
                        ui_mgr,
                        now,
                        &mut actions,
                    );
                });
                ui.separator();
            }
        },
    );
    actions
}

#[allow(clippy::too_many_arguments)]
fn render_property_row(
    ui: &mut egui::Ui,
    columns: &super::property_grid::Columns,
    node_id: &str,
    key: &str,
    value: &serde_json::Value,
    editor: &mut PropertyEditorState,
    loading: bool,
    ui_mgr: &mut UiManager,
    now: f64,
    actions: &mut PropertyTableActions,
) {
    let editing = editor.drafts.contains_key(key);
    let pending = editor
        .drafts
        .get(key)
        .is_some_and(|draft| draft.pending.is_some());
    let detail_id = ui.make_persistent_id("stored-property-detail");
    let mut expanded = ui
        .ctx()
        .data(|data| data.get_temp::<bool>(detail_id).unwrap_or(false));
    columns.row(ui, |ui| {
        columns.cell(ui, 0, |ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(key).strong().color(theme::TEXT_SECTION))
                    .truncate(),
            )
            .on_hover_text(key);
        });
        columns.cell(ui, 1, |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(PropertyValueKind::of(value).label())
                        .small()
                        .color(theme::TEXT_DIM),
                )
                .truncate(),
            );
        });
        columns.cell(ui, 2, |ui| {
            let preview = super::property_grid::preview(value);
            if value.is_object() || value.is_array() {
                let marker = if expanded { "▾" } else { "▸" };
                if ui
                    .add_enabled(
                        !editing,
                        egui::Button::new(format!("{marker} {preview}"))
                            .truncate()
                            .min_size(egui::vec2(ui.available_width(), columns.height)),
                    )
                    .named(
                        ui,
                        format_args!(
                            "{} stored property {key}",
                            if expanded { "Collapse" } else { "Expand" }
                        ),
                    )
                    .clicked()
                {
                    expanded = !expanded;
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(detail_id, expanded));
                }
            } else {
                ui.add(
                    egui::Label::new(egui::RichText::new(preview).monospace().small()).truncate(),
                )
                .on_hover_ui(|ui| {
                    let text = match value {
                        serde_json::Value::String(text) => super::widgets::elide(text, 800),
                        other => other.to_string(),
                    };
                    ui.add(egui::Label::new(egui::RichText::new(text).monospace()).wrap());
                });
            }
        });
        columns.cell(ui, 3, |ui| {
            columns.action(ui, 0, |ui| {
                if ui
                    .add_enabled(
                        !loading && !editing,
                        egui::Button::new("Edit").min_size(egui::Vec2::splat(columns.height)),
                    )
                    .named(ui, format_args!("Edit property {key}"))
                    .clicked()
                {
                    editor
                        .drafts
                        .insert(key.to_owned(), PropertyDraft::new(value));
                }
            });
            columns.action(ui, 1, |ui| {
                if ui
                    .add(egui::Button::new("Copy").min_size(egui::Vec2::splat(columns.height)))
                    .named(ui, format_args!("Copy property {key}"))
                    .clicked()
                {
                    ui.ctx()
                        .copy_text(super::property_grid::clipboard_value(value));
                    ui_mgr.show_toast(format!("Copied {key}"), now);
                }
            });
            columns.action(ui, 2, |ui| {
                ui.add_enabled_ui(!loading && !pending, |ui| {
                    if super::widgets::confirm_button(
                        ui,
                        ("delete-property", node_id, key),
                        "Delete",
                        "Confirm",
                        format_args!("Delete property {key}"),
                    ) {
                        actions.delete = Some(key.to_owned());
                    }
                });
            });
        });
    });
    if let Some(draft) = editor.drafts.get_mut(key) {
        columns.expanded(ui, |ui| {
            render_property_draft(ui, key, value, draft, loading, columns.height, actions);
        });
    } else if expanded && (value.is_object() || value.is_array()) {
        columns.expanded(ui, |ui| {
            ui.label(
                egui::RichText::new(format!("Stored JSON for {key} (read only)"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            let mut display = PropertyValueKind::format(value);
            ui.add_enabled_ui(false, |ui| {
                property_value_editor(
                    ui,
                    &mut display,
                    PropertyValueKind::of(value),
                    format_args!("Stored value of property {key}"),
                );
            });
        });
    }
}

fn render_property_draft(
    ui: &mut egui::Ui,
    key: &str,
    value: &serde_json::Value,
    draft: &mut PropertyDraft,
    loading: bool,
    target: f32,
    actions: &mut PropertyTableActions,
) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("Editing {key} ({})", draft.kind.label()))
                .small()
                .color(theme::TEXT_DIM),
        )
        .wrap(),
    )
    .observed(ui);
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
        ui.add(egui::Label::new(egui::RichText::new(error).color(theme::STATUS_OFFLINE)).wrap());
    }
    let status = if draft.pending.is_some() {
        "Waiting for saved value..."
    } else if draft.dirty() {
        "Unsaved changes"
    } else {
        "Matches stored value"
    };
    ui.label(egui::RichText::new(status).small().color(theme::TEXT_DIM))
        .observed(ui);
    ui.horizontal_wrapped(|ui| {
        let can_save =
            !loading && draft.pending.is_none() && draft.dirty() && value == &draft.source;
        if ui
            .add_enabled(
                can_save,
                egui::Button::new("Save")
                    .fill(theme::BG_SAVE)
                    .min_size(egui::Vec2::splat(target)),
            )
            .named(ui, format_args!("Save property {key}"))
            .clicked()
        {
            actions.save = Some(key.to_owned());
        }
        if ui
            .add_enabled(
                draft.pending.is_none(),
                egui::Button::new("Cancel").min_size(egui::Vec2::splat(target)),
            )
            .named(ui, format_args!("Cancel editing property {key}"))
            .clicked()
        {
            actions.cancel = Some(key.to_owned());
        }
    });
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
        let actions = render_property_table(ui, inspector, node_id, ui_mgr, now);
        if let Some(key) = actions.cancel {
            inspector.property_editor.drafts.remove(&key);
        }
        if let Some(key) = actions.save {
            submit_property_edit(inspector, node_id, &key, ui_mgr, now);
        }
        if let Some(key) = actions.delete {
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
    ui_mgr: &mut UiManager,
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
                        ui.add_enabled_ui(!fd.field_def_id.starts_with("pending-"), |ui| {
                            if super::widgets::confirm_button(
                                ui,
                                ("delete-field-definition", &fd.field_def_id),
                                "Delete",
                                "Confirm",
                                format_args!("Delete field definition {}", fd.key),
                            ) {
                                delete_idx = Some(idx);
                            }
                        })
                        .response
                        .on_disabled_hover_text("Waiting for the stored definition ID");
                        ui.end_row();
                    }
                });
        }

        if let Some(idx) = delete_idx {
            let fd = &inspector.field_defs[idx];
            crate::actions::send_or_toast(
                db_tx,
                DbCommand::DeleteFieldDef {
                    field_def_id: fd.field_def_id.clone(),
                },
                ui_mgr,
                ui.input(|input| input.time),
                "delete field definition",
            );
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
            if scope.is_empty() {
                ui_mgr.show_error(
                    "Choose a valid Verse, Fractal and Petal before creating a definition",
                    ui.input(|input| input.time),
                );
                return;
            }

            // Send DbCommand to persist
            if !crate::actions::send_or_toast(
                db_tx,
                DbCommand::CreateFieldDef {
                    scope: scope.clone(),
                    entity_type: "node".to_string(),
                    key: key.clone(),
                    value_type: value_type.clone(),
                    default_val: None,
                },
                ui_mgr,
                ui.input(|input| input.time),
                "create field definition",
            ) {
                return;
            }

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
            (
                "counter",
                "number",
                "9007199254740993",
                json!(9007199254740993_u64),
            ),
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
            Some(9007199254740993_u64)
        );
        let stored = inspector.node_properties.clone();
        for (kind, invalid) in [
            ("number", "NaN"),
            ("number", "Inf"),
            ("number", "-Infinity"),
            ("number", "1e999"),
            ("number", "01"),
            ("number", "18446744073709551615"),
            ("object", r#"{"nested":18446744073709551615}"#),
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

        use super::super::property_grid::{clipboard_value, preview, Columns};
        let exact = json!("  first\nsecond\t🦀  ");
        assert_eq!(clipboard_value(&exact), "  first\nsecond\t🦀  ");
        assert_eq!(preview(&exact), "  first\\nsecond\\t🦀  ");
        assert_eq!(preview(&json!("")), "(empty string)");
        assert_eq!(preview(&json!({"a": [1, 2]})), "{1 fields}");
        assert_eq!(preview(&json!([1, {"nested": true}])), "[2 items]");
        assert!(preview(&json!("🦀".repeat(121))).ends_with('…'));
        assert_eq!(clipboard_value(&nested), nested.to_string());

        nodes.select(Entity::from_bits(1), "node-1");
        sync_property_editor_scope(&mut inspector, &nodes, 57.0);
        inspector.node_properties = json!({"note": exact});
        inspector.node_properties_loading = false;
        let stored = inspector.node_properties.clone();
        let ctx = egui::Context::default();
        let mut time = 57.0;
        let right_edge = std::cell::Cell::new(false);
        let frame = |width: f32,
                     events: Vec<egui::Event>,
                     time: &mut f64,
                     inspector: &mut InspectorFormState,
                     ui_mgr: &mut UiManager| {
            *time += 0.05;
            crate::ui_shell::layout_policy::begin_frame(&ctx, true);
            let mut actions = PropertyTableActions::default();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 600.0),
                    )),
                    time: Some(*time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let columns = Columns::new(ui, ui.available_width());
                        if right_edge.replace(false) {
                            let id = ui.make_persistent_id(("property-table", "node-1"));
                            let mut state =
                                egui::scroll_area::State::load(ctx, id).unwrap_or_default();
                            state.offset.x = (columns.width() - ui.available_width()).max(0.0);
                            state.store(ctx, id);
                        }
                        assert!(columns.widths[2] >= 119.9);
                        assert!(columns.height * ctx.pixels_per_point() >= 20.0);
                        actions = render_property_table(ui, inspector, "node-1", ui_mgr, *time);
                    });
                },
            );
            (
                actions,
                crate::ui_shell::layout_policy::observations(&ctx),
                output,
            )
        };
        let click = |width: f32,
                     point: egui::Pos2,
                     time: &mut f64,
                     inspector: &mut InspectorFormState,
                     ui_mgr: &mut UiManager| {
            let mut result = frame(
                width,
                vec![egui::Event::PointerMoved(point)],
                time,
                inspector,
                ui_mgr,
            );
            for pressed in [true, false] {
                result = frame(
                    width,
                    vec![egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    time,
                    inspector,
                    ui_mgr,
                );
            }
            result
        };
        for width in [260.0, 320.0, 640.0] {
            right_edge.set(true);
            frame(width, vec![], &mut time, &mut inspector, &mut ui_mgr);
            let (_, before, _) = frame(width, vec![], &mut time, &mut inspector, &mut ui_mgr);
            let table_before = before
                .scrolls
                .iter()
                .find(|scroll| scroll.label == "Custom property table")
                .unwrap();
            assert_eq!(before.controls.len(), 3);
            for control in &before.controls {
                let visible = control
                    .rect
                    .intersect(control.hit)
                    .intersect(control.clip)
                    .intersect(table_before.clip);
                assert!(
                    (visible.width() - control.rect.width()).abs() < 0.5,
                    "the whole normal action must be visible at the right scroll edge"
                );
                assert!((visible.height() - control.rect.height()).abs() < 0.5);
                assert!(visible.width() >= 20.0 && visible.height() >= 20.0);
            }
            let (actions, _, _) = click(
                width,
                before.controls[2].rect.center(),
                &mut time,
                &mut inspector,
                &mut ui_mgr,
            );
            assert!(actions.delete.is_none());
            let (_, armed, _) = frame(width, vec![], &mut time, &mut inspector, &mut ui_mgr);
            let table_armed = armed
                .scrolls
                .iter()
                .find(|scroll| scroll.label == "Custom property table")
                .unwrap();
            assert_eq!(armed.controls.len(), 4);
            assert!(
                (table_armed.content_size.x - table_before.content_size.x).abs() < 0.5,
                "arming cannot widen the table"
            );
            assert!(
                (table_armed.offset.x - table_before.offset.x).abs() < 0.5,
                "arming cannot move horizontal scroll"
            );
            for index in 0..2 {
                assert!(
                    (armed.controls[index].rect.left() - before.controls[index].rect.left()).abs()
                        < 0.5,
                    "Edit/Copy column positions must remain stable"
                );
                assert!(
                    (armed.controls[index].rect.top() - before.controls[index].rect.top()).abs()
                        < 0.5
                );
            }
            for control in &armed.controls {
                let visible = control
                    .rect
                    .intersect(control.hit)
                    .intersect(control.clip)
                    .intersect(table_armed.clip);
                assert!(
                    (visible.width() - control.rect.width()).abs() < 0.5,
                    "the complete Confirm/Cancel labels and targets must fit"
                );
                assert!((visible.height() - control.rect.height()).abs() < 0.5);
                assert!(visible.width() >= 20.0 && visible.height() >= 20.0);
            }
            let (actions, _, _) = click(
                width,
                armed.controls[3].rect.center(),
                &mut time,
                &mut inspector,
                &mut ui_mgr,
            );
            assert!(actions.delete.is_none());
            assert_eq!(inspector.node_properties, stored);
            let (_, canceled, _) = frame(width, vec![], &mut time, &mut inspector, &mut ui_mgr);
            let table_canceled = canceled
                .scrolls
                .iter()
                .find(|scroll| scroll.label == "Custom property table")
                .unwrap();
            assert!((table_canceled.content_size.x - table_before.content_size.x).abs() < 0.5);
            assert!((table_canceled.offset.x - table_before.offset.x).abs() < 0.5);

            click(
                width,
                canceled.controls[0].rect.center(),
                &mut time,
                &mut inspector,
                &mut ui_mgr,
            );
            inspector
                .property_editor
                .drafts
                .get_mut("note")
                .unwrap()
                .raw = "unsaved replacement at the right scroll edge".into();
            let (_, editing, _) = frame(width, vec![], &mut time, &mut inspector, &mut ui_mgr);
            let table_editing = editing
                .scrolls
                .iter()
                .find(|scroll| scroll.label == "Custom property table")
                .unwrap();
            assert_eq!(
                editing.controls.len(),
                8,
                "row actions, Editing header, input, status and Save/Cancel"
            );
            assert!((table_editing.content_size.x - table_before.content_size.x).abs() < 0.5);
            assert!((table_editing.offset.x - table_before.offset.x).abs() < 0.5);
            assert!(
                table_editing.content_size.y > table_before.content_size.y,
                "the editor must retain real vertical layout space"
            );
            for control in &editing.controls[3..] {
                let visible = control
                    .rect
                    .intersect(control.hit)
                    .intersect(control.clip)
                    .intersect(table_editing.clip);
                assert!((visible.width() - control.rect.width()).abs() < 0.5, "the complete editor header, input, status and footer must fit the visible viewport");
                assert!((visible.height() - control.rect.height()).abs() < 0.5);
            }
            for control in &editing.controls[6..] {
                assert!(control.rect.width() >= 20.0 && control.rect.height() >= 20.0);
            }
            let (actions, _, _) = click(
                width,
                editing.controls[7].rect.center(),
                &mut time,
                &mut inspector,
                &mut ui_mgr,
            );
            assert_eq!(actions.cancel.as_deref(), Some("note"));
            assert!(actions.save.is_none() && actions.delete.is_none());
            inspector.property_editor.drafts.remove("note");
            assert_eq!(inspector.node_properties, stored);
            assert!(
                ui_mgr.drain_actions().is_empty(),
                "canceling a visible dirty editor must not write"
            );
            let (_, closed, _) = frame(width, vec![], &mut time, &mut inspector, &mut ui_mgr);
            let table_closed = closed
                .scrolls
                .iter()
                .find(|scroll| scroll.label == "Custom property table")
                .unwrap();
            assert!((table_closed.content_size.x - table_before.content_size.x).abs() < 0.5);
            assert!((table_closed.offset.x - table_before.offset.x).abs() < 0.5);
        }
        let (_, observed, _) = frame(1000.0, vec![], &mut time, &mut inspector, &mut ui_mgr);
        let copy_at = observed.controls[1].rect.center();
        let delete_at = observed.controls[2].rect.center();
        let (_, _, output) = click(1000.0, copy_at, &mut time, &mut inspector, &mut ui_mgr);
        assert!(output.platform_output.commands.iter().any(|command| matches!(command, egui::OutputCommand::CopyText(text) if text == "  first\nsecond\t🦀  ")));
        let (actions, _, _) = click(1000.0, delete_at, &mut time, &mut inspector, &mut ui_mgr);
        assert!(actions.delete.is_none(), "first Delete click only arms");
        let (_, armed, _) = frame(1000.0, vec![], &mut time, &mut inspector, &mut ui_mgr);
        assert_eq!(armed.controls.len(), 4, "Confirm and Cancel replace Delete");
        let (actions, _, _) = click(
            1000.0,
            armed.controls[3].rect.center(),
            &mut time,
            &mut inspector,
            &mut ui_mgr,
        );
        assert!(actions.delete.is_none(), "Cancel must never queue deletion");
        assert_eq!(inspector.node_properties, stored);
        assert!(ui_mgr.drain_actions().is_empty());
        let (_, disarmed, _) = frame(1000.0, vec![], &mut time, &mut inspector, &mut ui_mgr);
        assert_eq!(disarmed.controls.len(), 3);
        click(
            1000.0,
            disarmed.controls[2].rect.center(),
            &mut time,
            &mut inspector,
            &mut ui_mgr,
        );
        let (_, armed, _) = frame(1000.0, vec![], &mut time, &mut inspector, &mut ui_mgr);
        let (actions, _, _) = click(
            1000.0,
            armed.controls[2].rect.center(),
            &mut time,
            &mut inspector,
            &mut ui_mgr,
        );
        assert_eq!(actions.delete.as_deref(), Some("note"));
        assert_eq!(
            inspector.node_properties, stored,
            "confirmation only requests deletion; readback remains authoritative"
        );
        let (_, idle, _) = frame(1000.0, vec![], &mut time, &mut inspector, &mut ui_mgr);
        click(
            1000.0,
            idle.controls[0].rect.center(),
            &mut time,
            &mut inspector,
            &mut ui_mgr,
        );
        assert!(inspector.property_editor.drafts.contains_key("note"));
        inspector
            .property_editor
            .drafts
            .get_mut("note")
            .unwrap()
            .raw = "unsaved replacement".into();
        let (_, editing, _) = frame(1000.0, vec![], &mut time, &mut inspector, &mut ui_mgr);
        assert_eq!(
            editing.controls.len(),
            8,
            "row actions remain beside the editor header, input, status and Save/Cancel"
        );
        let (actions, _, _) = click(
            1000.0,
            editing.controls[7].rect.center(),
            &mut time,
            &mut inspector,
            &mut ui_mgr,
        );
        assert_eq!(actions.cancel.as_deref(), Some("note"));
        inspector
            .property_editor
            .drafts
            .remove(actions.cancel.as_deref().unwrap());
        assert_eq!(inspector.node_properties, stored);
        assert!(ui_mgr.drain_actions().is_empty());

        use crate::object_data::ObjectDataSection;
        use crate::verse_manager::{FractalEntry, NodeEntry, PetalEntry, VerseEntry};
        use fe_runtime::messages::DbResult;
        use fe_runtime::node_history::{NodeHistoryCoverage, NodeHistoryPage};
        use fe_runtime::node_measurements::{
            MeasurementPoint, MeasurementSeries, NodeMeasurements,
        };
        while rx.try_recv().is_ok() {}
        let tree = VerseManager::from_verses(vec![VerseEntry {
            id: "v".into(),
            name: "Verse".into(),
            fractals: vec![FractalEntry {
                id: "f".into(),
                name: "Fractal".into(),
                petals: vec![PetalEntry {
                    id: "p".into(),
                    name: "Petal".into(),
                    nodes: vec![
                        NodeEntry {
                            id: "node-1".into(),
                            position: [6.0, 0.0, 8.0],
                            ..Default::default()
                        },
                        NodeEntry {
                            id: "node-2".into(),
                            position: [10.0, 2.0, 6.0],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }]);
        let mut nav = NavigationManager::default();
        nav.navigate_to_petal_in_fractal("v", "Verse", "f", "Fractal", "p");
        let owner = LocalUserRole {
            role: Some(fe_database::RoleLevel::Owner),
        };
        let data_ctx = egui::Context::default();
        let mut data_time = 100.0;
        inspector.set_active_tab(InspectorTab::Data);
        let native_frame = |width: f32,
                            nodes: &mut NodeManager,
                            inspector: &mut InspectorFormState,
                            notices: &mut UiManager,
                            role: &LocalUserRole,
                            time: &mut f64| {
            *time += 0.05;
            crate::ui_shell::layout_policy::begin_frame(&data_ctx, true);
            let output = data_ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 1100.0),
                    )),
                    time: Some(*time),
                    ..Default::default()
                },
                |context| {
                    crate::ui_shell::layout_policy::begin_ui_pass(context);
                    egui::CentralPanel::default().show(context, |ui| {
                        right_inspector(
                            ui,
                            inspector,
                            nodes,
                            &tree,
                            notices,
                            role,
                            &db_sender.0,
                            &nav,
                            &crate::asset_ops::AssetDownloadStatus::default(),
                        );
                    });
                },
            );
            (
                output,
                crate::ui_shell::layout_policy::observations(&data_ctx),
            )
        };
        native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &owner,
            &mut data_time,
        );
        let DbCommand::ReadNodeHistory {
            node_id,
            after_version,
            limit,
            at_version,
            grant,
            correlation_id: Some(history_id),
        } = rx.try_recv().unwrap()
        else {
            panic!("Data opens the typed history reader");
        };
        assert_eq!(node_id, "node-1");
        assert_eq!(after_version, 0);
        assert_eq!(limit, 50);
        assert!(at_version.is_none());
        assert!(matches!(
            grant,
            fe_runtime::node_history::NodeAccessGrant::Local
        ));
        let page = NodeHistoryPage {
            coverage: NodeHistoryCoverage {
                node_id: "node-1".into(),
                journal_version: 1,
                current_version: 5,
                coverage_start_version: Some(1),
                coverage_started_at: Some("2026-11-01T00:00:00Z".into()),
                supported_operations: vec!["property_set".into(), "full_transform".into()],
                order: "commit_version".into(),
                event_time_ordering: false,
                current_state_verified: true,
            },
            entries: vec![],
            next_after_version: None,
        };
        assert!(!crate::object_data::route(
            &mut inspector.object_data,
            &DbResult::NodeHistoryLoaded {
                page: page.clone(),
                correlation_id: Some("stale-request".into())
            }
        ));
        assert!(crate::object_data::route(
            &mut inspector.object_data,
            &DbResult::NodeHistoryLoaded {
                page: page.clone(),
                correlation_id: Some(history_id.clone())
            }
        ));
        assert_eq!(
            tree.all_nodes()
                .find(|node| node.id == "node-1")
                .unwrap()
                .position,
            [6.0, 0.0, 8.0],
            "browsing history must not rewind the live tree"
        );
        assert_eq!(
            inspector.node_properties, stored,
            "history replies must not replace current property buffers"
        );
        inspector
            .object_data
            .activate(ObjectDataSection::Measurements);
        native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &owner,
            &mut data_time,
        );
        let DbCommand::ReadNodeMeasurements {
            node_id,
            petal_id,
            correlation_id,
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("Measurements uses its scoped typed reader");
        };
        assert_eq!((node_id.as_str(), petal_id.as_str()), ("node-1", "p"));
        let series = MeasurementSeries {
            metric: "calibrated".into(),
            units: "m".into(),
        };
        let measurements = NodeMeasurements {
            node_id: "node-1".into(),
            petal_id: "p".into(),
            series: Some(series.clone()),
            available_series: vec![series],
            series_truncated: false,
            points: [21.0, 23.0, 25.0]
                .into_iter()
                .enumerate()
                .map(|(index, value)| MeasurementPoint {
                    reading_id: format!("r{index}"),
                    recorded_at: format!("2026-11-01T00:0{index}:00Z"),
                    recorded_at_ms: 1793491200000 + index as i64 * 60000,
                    value,
                })
                .collect(),
            truncated: false,
            limit: 1024,
        };
        assert!(crate::object_data::route(
            &mut inspector.object_data,
            &DbResult::NodeMeasurementsLoaded {
                measurements,
                correlation_id
            }
        ));
        for width in [260.0, 640.0, 1000.0] {
            let (_, observed) = native_frame(
                width,
                &mut nodes,
                &mut inspector,
                &mut ui_mgr,
                &owner,
                &mut data_time,
            );
            assert!(
                observed
                    .controls
                    .iter()
                    .any(|control| (control.rect.height() - 170.0).abs() < 0.1),
                "the native chart must have measured geometry"
            );
            assert!(
                rx.try_recv().is_err(),
                "cached measurements must not query every frame"
            );
        }
        inspector.object_data.activate(ObjectDataSection::Schema);
        native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &owner,
            &mut data_time,
        );
        assert!(
            matches!(rx.try_recv().unwrap(), DbCommand::ListFieldDefs { scope } if scope == "VERSE#v-FRACTAL#f-PETAL#p")
        );
        let definition_scope = "VERSE#v-FRACTAL#f-PETAL#p";
        crate::object_data::route(
            &mut inspector.object_data,
            &DbResult::FieldDefsListed {
                scope: definition_scope.into(),
                field_defs: vec![fe_runtime::messages::FieldDefInfo {
                    field_def_id: "calibration-def".into(),
                    scope: definition_scope.into(),
                    entity_type: "node".into(),
                    key: "calibration".into(),
                    value_type: "object".into(),
                    default_val: None,
                    created_by: "qa-owner".into(),
                    created_at: "2026-11-01T00:00:00Z".into(),
                }],
            },
        );
        let schema_v1 =
            json!({"version":1,"schema":{"type":"object","properties":{"gain":{"type":"number"}}}});
        let schema_v2 = json!({"version":2,"schema":{"type":"object","properties":{"gain":{"type":"number"},"offset":{"type":"number"}}}});
        let schema_v3 = json!({"version":3,"schema":{"type":"object","properties":{"gain":{"type":"number"},"offset":{"type":"number"},"label":{"type":"string"}}}});
        let submitted_text = serde_json::to_string_pretty(&schema_v2).unwrap();
        for newer_text in [
            Some(schema_v3.to_string()),
            Some("{\"version\":3,\"schema\":{".into()),
            Some(format!("{submitted_text}\n  ")),
            None,
        ] {
            inspector.object_data.schema.edit_definition(
                "calibration-def",
                definition_scope,
                &db_sender.0,
            );
            let DbCommand::GetFieldSchema {
                correlation_id: Some(read_id),
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("read the stored descriptor before editing");
            };
            assert!(crate::object_data::route(
                &mut inspector.object_data,
                &DbResult::QueryResult {
                    data: vec![
                        json!({"field_def_id":"calibration-def","scope":definition_scope,"enforced_schema":schema_v1})
                    ],
                    correlation_id: Some(read_id)
                }
            ));
            inspector
                .object_data
                .schema
                .replace_draft(submitted_text.clone());
            inspector.object_data.schema.save(&db_sender.0);
            let DbCommand::SetFieldSchema {
                descriptor,
                correlation_id: Some(save_id),
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("valid next revision must queue one save");
            };
            assert_eq!(descriptor, schema_v2);
            inspector
                .object_data
                .schema
                .expire_requests(std::time::Instant::now() + std::time::Duration::from_secs(31));
            if let Some(newer) = &newer_text {
                inspector.object_data.schema.replace_draft(newer.clone());
            }
            inspector.object_data.schema.save(&db_sender.0);
            assert!(
                rx.try_recv().is_err(),
                "an ambiguous timed-out write requires readback before resubmission"
            );
            inspector.object_data.schema.read(&db_sender.0);
            let DbCommand::GetFieldSchema {
                correlation_id: Some(refresh_id),
                ..
            } = rx.try_recv().unwrap()
            else {
                panic!("refresh gets a fresh read request");
            };
            assert_ne!(refresh_id, save_id);
            let refreshed = DbResult::QueryResult {
                data: vec![
                    json!({"field_def_id":"calibration-def","scope":definition_scope,"enforced_schema":schema_v2}),
                ],
                correlation_id: Some(refresh_id),
            };
            assert!(crate::object_data::route(
                &mut inspector.object_data,
                &refreshed
            ));
            if let Some(newer) = newer_text {
                assert_eq!(
                    inspector.object_data.schema.draft(),
                    newer,
                    "confirming an older submitted revision must preserve exact newer edits"
                );
                inspector.object_data.schema.save(&db_sender.0);
                if newer == schema_v3.to_string() {
                    assert!(
                        matches!(rx.try_recv().unwrap(), DbCommand::SetFieldSchema { descriptor,.. } if descriptor == schema_v3)
                    );
                } else {
                    assert!(
                        rx.try_recv().is_err(),
                        "partial JSON or stale revision cannot queue another write"
                    );
                }
            } else {
                let next: serde_json::Value =
                    serde_json::from_str(inspector.object_data.schema.draft()).unwrap();
                assert_eq!(
                    next["version"], 3,
                    "an untouched submitted draft advances to the next editable revision"
                );
                assert_eq!(next["schema"], schema_v2["schema"]);
            }
        }
        inspector
            .object_data
            .activate(ObjectDataSection::Measurements);
        native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &owner,
            &mut data_time,
        );
        assert!(
            rx.try_recv().is_err(),
            "the authorized target still has cached measurements"
        );
        inspector.object_data.schema.read(&db_sender.0);
        let DbCommand::GetFieldSchema {
            correlation_id: Some(revoked_read),
            ..
        } = rx.try_recv().unwrap()
        else {
            panic!("capture an in-flight read before revocation");
        };
        let revoked = LocalUserRole {
            role: Some(fe_database::RoleLevel::None),
        };
        let (_, revoked_observations) = native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &revoked,
            &mut data_time,
        );
        assert!(
            !revoked_observations
                .controls
                .iter()
                .any(|control| (control.rect.height() - 170.0).abs() < 0.1),
            "revocation removes the cached chart"
        );
        assert!(
            rx.try_recv().is_err(),
            "Some(RoleLevel::None) must not admit another read"
        );
        assert!(
            !crate::object_data::route(
                &mut inspector.object_data,
                &DbResult::QueryResult {
                    data: vec![
                        json!({"field_def_id":"calibration-def","scope":definition_scope,"enforced_schema":schema_v2})
                    ],
                    correlation_id: Some(revoked_read)
                }
            ),
            "a late pre-revocation reply must not repopulate the cleared state"
        );
        let viewer = LocalUserRole {
            role: Some(fe_database::RoleLevel::Viewer),
        };
        native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &viewer,
            &mut data_time,
        );
        assert!(
            matches!(rx.try_recv().unwrap(), DbCommand::ReadNodeMeasurements {node_id,..} if node_id == "node-1"),
            "restoring Viewer must perform a fresh read instead of reusing revoked data"
        );
        nodes.select(Entity::from_bits(2), "node-2");
        native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &owner,
            &mut data_time,
        );
        while rx.try_recv().is_ok() {}
        assert!(
            !crate::object_data::route(
                &mut inspector.object_data,
                &DbResult::NodeHistoryLoaded {
                    page,
                    correlation_id: Some(history_id)
                }
            ),
            "a previous selection's reply must not claim the new target"
        );
        native_frame(
            640.0,
            &mut nodes,
            &mut inspector,
            &mut ui_mgr,
            &LocalUserRole::default(),
            &mut data_time,
        );
        assert!(
            rx.try_recv().is_err(),
            "unresolved access must not start a data read"
        );

        let panel_ctx = egui::Context::default();
        let mut panel_time = 400.0;
        let mut polish = InspectorFormState::default();
        let mut polish_nodes = NodeManager::default();
        let mut notices = UiManager::default();
        let manager = LocalUserRole {
            role: Some(fe_database::RoleLevel::Manager),
        };
        let (polish_tx, polish_rx) = crossbeam::channel::unbounded();
        let surface = |api: bool,
                       events: Vec<egui::Event>,
                       time: &mut f64,
                       form: &mut InspectorFormState,
                       nodes: &NodeManager,
                       notices: &mut UiManager| {
            *time += 0.05;
            crate::ui_shell::layout_policy::begin_frame(&panel_ctx, true);
            let output = panel_ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 1600.0),
                    )),
                    time: Some(*time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    crate::ui_shell::layout_policy::begin_ui_pass(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        if api {
                            inspector_api_access_section(
                                ui, form, nodes, &polish_tx, &manager, notices,
                            );
                        } else {
                            inspector_url_meta_section(ui, form, nodes, notices, &manager);
                        }
                    });
                },
            );
            (
                crate::ui_shell::layout_policy::observations(&panel_ctx),
                output,
            )
        };
        let painted = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        Some(text.galley.job.text.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let click_surface = |api,
                             point: egui::Pos2,
                             time: &mut f64,
                             form: &mut InspectorFormState,
                             nodes: &NodeManager,
                             notices: &mut UiManager| {
            let mut result = surface(
                api,
                vec![egui::Event::PointerMoved(point)],
                time,
                form,
                nodes,
                notices,
            );
            for pressed in [true, false] {
                result = surface(
                    api,
                    vec![egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    time,
                    form,
                    nodes,
                    notices,
                );
            }
            result
        };
        let (_, empty) = surface(
            true,
            vec![],
            &mut panel_time,
            &mut polish,
            &polish_nodes,
            &mut notices,
        );
        assert!(painted(&empty).contains("Select a node"));
        polish_nodes.select(Entity::from_bits(20), "polish-node");
        for invalid in [
            "not a URL",
            "http://127.0.0.1/private",
            "javascript:alert(1)",
        ] {
            polish.external_url = invalid.into();
            let (observed, _) = surface(
                false,
                vec![],
                &mut panel_time,
                &mut polish,
                &polish_nodes,
                &mut notices,
            );
            assert!(matches!(
                crate::actions::portal::compute_save_url(&polish_nodes, &polish),
                crate::actions::portal::SaveUrlOutcome::Blocked { .. }
            ));
            click_surface(
                false,
                observed.controls[2].rect.center(),
                &mut panel_time,
                &mut polish,
                &polish_nodes,
                &mut notices,
            );
            assert!(notices.drain_actions().is_empty());
            assert_eq!(polish.external_url, invalid);
            assert_eq!(
                polish_nodes.selected.as_ref().unwrap().node_id,
                "polish-node"
            );
        }
        polish.external_url.clear();
        let (observed, _) = surface(
            false,
            vec![],
            &mut panel_time,
            &mut polish,
            &polish_nodes,
            &mut notices,
        );
        click_surface(
            false,
            observed.controls[2].rect.center(),
            &mut panel_time,
            &mut polish,
            &polish_nodes,
            &mut notices,
        );
        assert!(matches!(&notices.drain_actions()[..], [UiAction::SaveUrl]));
        let scope = "VERSE#v#FRACTAL#f#PETAL#p";
        polish.set_active_tab(InspectorTab::ApiAccess);
        synchronize_token_target(&mut polish, Some("polish-node"), scope, true);
        let endpoint = fe_runtime::app::RuntimeApiEndpoint(Some("127.0.0.1:8882".parse().unwrap()));
        polish.api_endpoint = endpoint.0;
        let (observed, _) = surface(
            true,
            vec![],
            &mut panel_time,
            &mut polish,
            &polish_nodes,
            &mut notices,
        );
        click_surface(
            true,
            observed.controls[6].rect.center(),
            &mut panel_time,
            &mut polish,
            &polish_nodes,
            &mut notices,
        );
        let DbCommand::MintApiToken {
            correlation_id: Some(nonce),
            scope: sent_scope,
            label,
            ..
        } = polish_rx.try_recv().unwrap()
        else {
            panic!("Inspector generation must send one correlated mint")
        };
        assert_eq!(sent_scope, scope);
        assert_eq!(label.as_deref(), Some("node:polish-node"));
        assert!(InspectorTokenRequest::owns(&nonce));
        submit_inspector_token(
            &mut polish,
            "polish-node",
            true,
            &polish_tx,
            std::time::Instant::now(),
        );
        assert!(
            polish_rx.try_recv().is_err(),
            "pending Generate cannot enqueue twice"
        );
        assert!(!polish.api_token_request.accept(
            "unknown",
            scope,
            "viewer",
            "client",
            std::time::Instant::now()
        ));
        synchronize_token_target(&mut polish, Some("another-node"), scope, true);
        assert!(!polish.api_token_request.accept(
            &nonce,
            scope,
            "viewer",
            "client",
            std::time::Instant::now()
        ));
        assert!(polish.generated_api_token.is_none());
        synchronize_token_target(&mut polish, Some("polish-node"), scope, true);
        let now = std::time::Instant::now();
        submit_inspector_token(&mut polish, "polish-node", true, &polish_tx, now);
        let DbCommand::MintApiToken {
            correlation_id: Some(failed_nonce),
            ..
        } = polish_rx.try_recv().unwrap()
        else {
            panic!("retry must carry a fresh nonce")
        };
        assert_ne!(nonce, failed_nonce);
        polish.api_token_request.fail(&failed_nonce, now);
        assert!(
            polish.api_token_request.pending.is_none() && polish.api_token_request.error.is_some()
        );
        submit_inspector_token(&mut polish, "polish-node", true, &polish_tx, now);
        let DbCommand::MintApiToken {
            correlation_id: Some(expired_nonce),
            ..
        } = polish_rx.try_recv().unwrap()
        else {
            panic!("mint must stay correlated")
        };
        let late = now + std::time::Duration::from_secs(31);
        polish.api_token_request.expire(late);
        assert!(polish.api_token_request.error.is_some());
        assert!(!polish
            .api_token_request
            .accept(&expired_nonce, scope, "viewer", "client", late));
        submit_inspector_token(&mut polish, "polish-node", true, &polish_tx, now);
        let DbCommand::MintApiToken {
            correlation_id: Some(scope_nonce),
            ..
        } = polish_rx.try_recv().unwrap()
        else {
            panic!("mint must stay correlated")
        };
        assert!(!polish.api_token_request.accept(
            &scope_nonce,
            "VERSE#wrong",
            "viewer",
            "client",
            now
        ));
        submit_inspector_token(&mut polish, "polish-node", true, &polish_tx, now);
        let DbCommand::MintApiToken {
            correlation_id: Some(valid_nonce),
            ..
        } = polish_rx.try_recv().unwrap()
        else {
            panic!("mint must stay correlated")
        };
        assert!(polish
            .api_token_request
            .accept(&valid_nonce, scope, "viewer", "client", now));
        polish.generated_api_token = Some("private-fixture".into());
        polish.generated_api_token_client = Some("client".into());
        let (_, masked) = surface(
            true,
            vec![],
            &mut panel_time,
            &mut polish,
            &polish_nodes,
            &mut notices,
        );
        assert!(
            !painted(&masked).contains("private-fixture"),
            "unrevealed token and curl must stay masked"
        );
        let curl =
            node_properties_curl(endpoint.0.unwrap(), "polish-node", "private-fixture").unwrap();
        assert!(curl.contains("http://127.0.0.1:8882/api/v1/nodes/polish-node/properties"));
        assert!(
            curl.contains("Authorization: Bearer private-fixture") && !curl.contains("/health")
        );
        let date = chrono::DateTime::parse_from_rfc3339("2026-11-02T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            token_expiry_label("2026-11-02T12:30:00Z", date),
            "Expires in 30 minutes"
        );
        assert_eq!(token_expiry_label("2026-11-02T11:00:00Z", date), "Expired");
        assert_eq!(
            token_expiry_label("unavailable", date),
            "Expiry unavailable"
        );
        let mut report_sidebar = crate::ui_shell::right_sidebar::RightSidebarState::default();
        report_sidebar.reveal(crate::ui_shell::right_sidebar::RightSidebarSection::ProposalReport);
        polish.api_token_request.revealed = true;
        crate::ui_shell::right_sidebar::open_report_node_properties(
            "polish-node".into(),
            &mut polish_nodes,
            &mut polish,
            &mut report_sidebar,
        );
        assert_eq!(polish.active_tab(), InspectorTab::Properties);
        assert!(report_sidebar
            .is_active(crate::ui_shell::right_sidebar::RightSidebarSection::Inspector));
        assert_eq!(
            polish_nodes.pending_sidebar_select.as_deref(),
            Some("polish-node")
        );
        assert_eq!(
            polish_nodes.selected.as_ref().unwrap().node_id,
            "polish-node"
        );
        assert_eq!(polish.api_token_request.scope, scope);
        assert!(
            polish.generated_api_token.is_none()
                && polish.generated_api_token_client.is_none()
                && !polish.api_token_request.revealed
        );
        submit_inspector_token(&mut polish, "polish-node", true, &polish_tx, now);
        assert!(
            polish_rx.try_recv().is_err(),
            "inactive API cannot submit a mint"
        );
        polish.set_active_tab(InspectorTab::ApiAccess);
        submit_inspector_token(&mut polish, "polish-node", true, &polish_tx, now);
        let DbCommand::MintApiToken {
            correlation_id: Some(report_nonce),
            ..
        } = polish_rx.try_recv().unwrap()
        else {
            panic!("mint must stay correlated")
        };
        crate::ui_shell::right_sidebar::open_report_node_properties(
            "polish-node".into(),
            &mut polish_nodes,
            &mut polish,
            &mut report_sidebar,
        );
        synchronize_token_target(&mut polish, Some("polish-node"), scope, true);
        assert!(polish.api_token_request.pending.is_none());
        assert!(!polish
            .api_token_request
            .accept(&report_nonce, scope, "viewer", "client", now));
        polish.set_active_tab(InspectorTab::ApiAccess);
        assert!(!polish
            .api_token_request
            .accept(&report_nonce, scope, "viewer", "client", now));
        let (_, returned) = surface(
            true,
            vec![],
            &mut panel_time,
            &mut polish,
            &polish_nodes,
            &mut notices,
        );
        assert!(
            polish.generated_api_token.is_none()
                && polish.generated_api_token_client.is_none()
                && !painted(&returned).contains("private-fixture")
        );
        synchronize_token_target(&mut polish, Some("polish-node"), "VERSE#other", true);
        assert!(
            polish.generated_api_token.is_none() && polish.generated_api_token_client.is_none()
        );
        synchronize_token_target(&mut polish, None, "", true);
        assert!(polish.api_token_request.pending.is_none());
        let (closed_tx, closed_rx) = crossbeam::channel::unbounded();
        drop(closed_rx);
        synchronize_token_target(&mut polish, Some("polish-node"), scope, true);
        submit_inspector_token(&mut polish, "polish-node", true, &closed_tx, now);
        assert!(
            polish.api_token_request.pending.is_none() && polish.api_token_request.error.is_some()
        );
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
