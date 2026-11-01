//! Primary-window accessibility and QA state projection; see src/AGENTS.md §qa-debug.

use crate::vision_bridge::ViewScope;
use bevy::a11y::AccessibilityRequested;
use bevy::diagnostic::FrameCount;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::time::Real;
use bevy::window::PrimaryWindow;
use bevy_egui::{
    egui::accesskit as ak, EguiContext, EguiOutput, EguiPostUpdateSet, EguiPreUpdateSet,
    PrimaryEguiContext,
};
use fe_runtime::qa_debug::{redact, QaFailure, QaLogRing, QaOp, QaReceiver, TreeFilter};
use fe_ui::actions::UiManager;
use fe_ui::dialogs::ActiveDialog;
use fe_ui::navigation_manager::NavigationManager;
use fe_ui::node_manager::NodeManager;
use fe_ui::plugin::{SpawnedNodeMarker, ToolState, ViewportRect};
use fe_ui::ui_shell::right_sidebar::{active_section, section_label, RightSidebarState};
use fe_ui::verse_manager::VerseManager;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

#[derive(Resource)]
struct QaHost {
    rx: QaReceiver,
    logs: QaLogRing,
}

#[derive(Resource)]
struct QaEnabled(bool);

#[derive(Resource, Default)]
struct FrameNavigation((Option<String>, Option<String>, Option<String>));

fn cursor(nav: &NavigationManager) -> (Option<String>, Option<String>, Option<String>) {
    (
        nav.active_verse_id.clone(),
        nav.active_fractal_id.clone(),
        nav.active_petal_id.clone(),
    )
}

pub fn install(app: &mut App, enabled: bool, rx: QaReceiver, logs: QaLogRing) {
    app.insert_resource(QaEnabled(enabled));
    app.insert_resource(QaHost { rx, logs });
    app.init_resource::<FrameNavigation>();
    app.add_systems(
        PreUpdate,
        prepare_accessibility
            .after(EguiPreUpdateSet::InitContexts)
            .before(EguiPreUpdateSet::BeginPass),
    );
    if enabled {
        app.add_systems(PostUpdate, serve.after(EguiPostUpdateSet::ProcessOutput));
    }
}

fn prepare_accessibility(
    enabled: Res<QaEnabled>,
    requested: Res<AccessibilityRequested>,
    mut contexts: Query<&mut EguiContext, With<PrimaryEguiContext>>,
    nav: Res<NavigationManager>,
    mut frame_nav: ResMut<FrameNavigation>,
    host: Res<QaHost>,
) {
    for mut context in &mut contexts {
        if enabled.0 || requested.get() {
            context.get_mut().enable_accesskit();
        } else {
            context.get_mut().disable_accesskit();
        }
    }
    if enabled.0 {
        frame_nav.0 = cursor(&nav);
        host.logs.set_verse(nav.active_verse_id.as_deref());
    }
}

fn rect(rect: ak::Rect) -> Value {
    json!({ "x":rect.x0, "y":rect.y0, "w":(rect.x1-rect.x0).max(0.0), "h":(rect.y1-rect.y0).max(0.0) })
}

fn node_name(
    node: &ak::Node,
    nodes: &HashMap<ak::NodeId, &ak::Node>,
    protected: &HashSet<ak::NodeId>,
) -> String {
    if let Some(label) = node.label() {
        return redact(label);
    }
    let labels = node
        .labelled_by()
        .iter()
        .filter(|id| !protected.contains(id))
        .filter_map(|id| nodes.get(id))
        .map(|n| n.label().or(n.value()).unwrap_or(""))
        .collect::<Vec<_>>();
    if !labels.is_empty() {
        return redact(&labels.join(" "));
    }
    if matches!(node.role(), ak::Role::Label | ak::Role::TextRun) {
        return redact(node.value().unwrap_or(""));
    }
    String::new()
}

fn role_key(role: &str) -> String {
    role.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub(crate) fn flatten(update: &ak::TreeUpdate, filter: &TreeFilter) -> Vec<Value> {
    let nodes = update
        .nodes
        .iter()
        .map(|(id, node)| (*id, node))
        .collect::<HashMap<_, _>>();
    let root = update
        .tree
        .as_ref()
        .map(|tree| tree.root)
        .unwrap_or(update.focus);
    let mut protected = HashSet::new();
    let mut visited = HashSet::new();
    let mut protection = vec![(root, false)];
    while let Some((id, inherited)) = protection.pop() {
        if visited.len() >= 20000 || !visited.insert(id) {
            continue;
        }
        if let Some(node) = nodes.get(&id) {
            let hidden_value = inherited || node.role() == ak::Role::PasswordInput;
            if hidden_value {
                protected.insert(id);
            }
            for child in node.children() {
                protection.push((*child, hidden_value));
            }
        }
    }
    protected.extend(nodes.keys().filter(|id| !visited.contains(id)).copied());
    let mut stack = vec![(root, None, ak::Affine::IDENTITY, false, false, false)];
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    while let Some((id, parent, ancestor_transform, disabled, hidden, password)) = stack.pop() {
        if seen.len() >= 20000 || !seen.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(&id) else {
            continue;
        };
        let transform =
            ancestor_transform * node.transform().copied().unwrap_or(ak::Affine::IDENTITY);
        let disabled = disabled || node.is_disabled();
        let hidden = hidden || node.is_hidden();
        let password = password || node.role() == ak::Role::PasswordInput;
        for child in node.children().iter().rev() {
            stack.push((*child, Some(id), transform, disabled, hidden, password));
        }
        let role = format!("{:?}", node.role()).to_ascii_lowercase();
        let interactive = node.supports_action(ak::Action::Click)
            || node.supports_action(ak::Action::Focus)
            || node.supports_action(ak::Action::SetValue);
        let name = if password || protected.contains(&id) {
            String::new()
        } else {
            node_name(node, &nodes, &protected)
        };
        if filter.interactive && !interactive {
            continue;
        }
        if filter
            .name
            .as_ref()
            .is_some_and(|f| !name.to_lowercase().contains(&f.to_lowercase()))
        {
            continue;
        }
        if filter
            .role
            .as_ref()
            .is_some_and(|f| role_key(f) != role_key(&role))
        {
            continue;
        }
        let bounds = node
            .bounds()
            .map(|r| rect(transform.transform_rect_bbox(r)));
        result.push(json!({"id":id.0.to_string(), "parent":parent.map(|p| p.0.to_string()), "role":role, "name":name,
            "value":if password { None } else { node.value().map(redact) }, "bounds":bounds, "enabled":!disabled, "hidden":hidden,
            "interactive":interactive, "focused":id==update.focus, "selected":node.is_selected(), "toggled":node.toggled().map(|t| format!("{t:?}").to_ascii_lowercase())}));
    }
    result
}

#[derive(SystemParam)]
struct UiState<'w, 's> {
    nav: Res<'w, NavigationManager>,
    verses: Res<'w, VerseManager>,
    nodes: Res<'w, NodeManager>,
    tool: Res<'w, ToolState>,
    ui: Res<'w, UiManager>,
    right: Res<'w, RightSidebarState>,
    viewport: Res<'w, ViewportRect>,
    transforms: Query<'w, 's, (&'static Transform, &'static SpawnedNodeMarker)>,
    asset: Res<'w, fe_ui::asset_ops::PendingAssetOps>,
    gpx: Res<'w, fe_ui::gpx_ops::PendingGpxOps>,
    path: Res<'w, fe_ui::path_ops::PendingPathOps>,
    gis: Res<'w, fe_ui::gis::GisPanelState>,
    clock: Res<'w, Time<Real>>,
    lock: Option<Res<'w, fe_renderer::camera::CameraCaptureLock>>,
}

fn dialog_title(dialog: &ActiveDialog) -> Option<&'static str> {
    match dialog {
        ActiveDialog::None => None,
        ActiveDialog::CreateEntity { .. } => Some("Create entity"),
        ActiveDialog::ContextMenu { .. } => Some("Context menu"),
        ActiveDialog::GltfImport { .. } => Some("Import GLTF"),
        ActiveDialog::NodeOptions { .. } => Some("Node options"),
        ActiveDialog::InviteDialog { .. } => Some("Invite"),
        ActiveDialog::JoinDialog { .. } => Some("Join Verse"),
        ActiveDialog::PeerDebug => Some("Peer debug"),
        ActiveDialog::HexonManager { .. } => Some("Maps"),
        ActiveDialog::PetalManifest { .. } => Some("Petal manifest"),
        ActiveDialog::EntitySettings { .. } => Some("Entity settings"),
    }
}

impl UiState<'_, '_> {
    fn snapshot(&self, ppp: f32, frame: u32) -> Value {
        let selection = self.nodes.selected.as_ref().and_then(|selected| {
            let (transform, marker) = self.transforms.get(selected.entity).ok()?;
            let petal = self.nav.active_petal_id.as_deref()?;
            let in_verse = self.verses.verses.iter().filter(|v| Some(v.id.as_str()) == self.nav.active_verse_id.as_deref()).flat_map(|v| &v.fractals).filter(|f| Some(f.id.as_str()) == self.nav.active_fractal_id.as_deref()).flat_map(|f| &f.petals).any(|p| p.id == petal);
            (marker.petal_id == petal && marker.node_id == selected.node_id && in_verse).then(|| json!({"node_id":selected.node_id,"transform":{"position":transform.translation.to_array(),"rotation":transform.rotation.to_array(),"scale":transform.scale.to_array()}}))
        });
        let section = active_section(
            &self.right,
            selection.is_some(),
            matches!(self.ui.portal, fe_ui::portal::PortalState::Open { .. }),
        );
        let mut windows = section.map(|s| vec![section_label(s)]).unwrap_or_default();
        if let Some(title) = dialog_title(&self.ui.active_dialog) {
            windows.push(title);
        }
        if self.gis.open {
            windows.push("Data");
        }
        let viewport = self.viewport.0;
        let pending = self.asset.0.len()
            + self.gpx.0.len()
            + self.path.0.len()
            + usize::from(self.ui.join_request.pending)
            + usize::from(self.ui.gltf_import_request.pending);
        let notice = self.ui.active_notice(self.clock.elapsed_secs_f64()).map(|(n, _)| json!({"id":n.id,"severity":format!("{:?}",n.severity).to_ascii_lowercase(),"text":redact(&n.text)}));
        json!({"frame":frame,"coordinate_space":"physical_pixels","pixels_per_point":ppp,
            "navigation":{"verse_id":self.nav.active_verse_id,"verse_name":redact(&self.nav.active_verse_name),"fractal_id":self.nav.active_fractal_id,"fractal_name":redact(&self.nav.active_fractal_name),"petal_id":self.nav.active_petal_id,"petal_name":self.nav.active_petal_id.as_ref().and_then(|id| self.verses.find_petal(id)).map(|p| redact(&p.name))},
            "selection":selection,"active_tool":format!("{:?}",self.tool.active_tool).to_ascii_lowercase(),"open_windows":windows,"active_section":section.map(|s| s.slug()),
            "viewport":{"x":viewport.min.x*ppp,"y":viewport.min.y*ppp,"w":viewport.width()*ppp,"h":viewport.height()*ppp},
            "notice":notice,"status":self.ui.conditions().map(|(key,text)| json!({"key":key,"text":redact(text)})).collect::<Vec<_>>(),
            "pending":{"ui_known":pending,"db":null,"sync":null},"settled":!self.nodes.gesture_active && self.lock.is_none() && pending==0,
            "settled_definition":"no active UI gesture, capture lock or exposed pending UI operations; DB/sync and terrain completion unknown"})
    }
}

fn serve(
    host: Res<QaHost>,
    frame_nav: Res<FrameNavigation>,
    frame: Res<FrameCount>,
    view: ViewScope,
    state: UiState,
    mut contexts: Query<(&mut EguiContext, &EguiOutput), With<PrimaryEguiContext>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    host.logs.set_verse(state.nav.active_verse_id.as_deref());
    for request in host.rx.try_iter().take(32) {
        if !view.covers(&request.caller_scope, Some(&state.nav)) {
            request.reply(Err(QaFailure::denied()));
            continue;
        }
        if cursor(&state.nav) != frame_nav.0 {
            request.reply(Err(QaFailure {
                code: "ui_frame_changing",
                message: "navigation changed during this UI frame; retry",
            }));
            continue;
        }
        let Ok((mut context, output)) = contexts.single_mut() else {
            request.reply(Err(QaFailure {
                code: "ui_unavailable",
                message: "primary UI context unavailable",
            }));
            continue;
        };
        let ppp = context.get_mut().pixels_per_point();
        let result = match &request.op {
            QaOp::UiTree(filter) => match output.platform_output.accesskit_update.as_ref() {
                Some(update) => Ok(
                    json!({"frame":frame.0,"active_verse_id":state.nav.active_verse_id,"coordinate_space":"physical_pixels","pixels_per_point":ppp,
                    "window":windows.single().ok().map(|w| json!({"width":w.physical_width(),"height":w.physical_height()})),"nodes":flatten(update,filter)}),
                ),
                None => Err(QaFailure {
                    code: "ui_tree_not_ready",
                    message: "primary accessibility tree not ready; retry",
                }),
            },
            QaOp::UiState => Ok(state.snapshot(ppp, frame.0)),
            QaOp::Logs(filter) => Ok(host.logs.read(filter)),
        };
        request.reply(result);
    }
}
