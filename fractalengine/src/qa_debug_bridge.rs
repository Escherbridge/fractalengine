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
use fe_runtime::qa_debug::{
    redact, QaFailure, QaLogRing, QaOp, QaReceiver, TreeFilter, WindowAction,
};
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

#[derive(Resource)]
struct QaControls {
    dialogs: Option<std::sync::Arc<fe_ui::file_dialogs::QaFileDialogs>>,
    listener: crossbeam::channel::Receiver<std::net::SocketAddr>,
}

#[derive(Resource, Default)]
struct FrameNavigation((Option<String>, Option<String>, Option<String>));

fn cursor(nav: &NavigationManager) -> (Option<String>, Option<String>, Option<String>) {
    (
        nav.active_verse_id.clone(),
        nav.active_fractal_id.clone(),
        nav.active_petal_id.clone(),
    )
}

pub fn install(
    app: &mut App,
    enabled: bool,
    rx: QaReceiver,
    logs: QaLogRing,
    dialogs: Option<std::sync::Arc<fe_ui::file_dialogs::QaFileDialogs>>,
    listener: crossbeam::channel::Receiver<std::net::SocketAddr>,
) {
    app.insert_resource(QaEnabled(enabled));
    app.insert_resource(QaHost { rx, logs });
    app.insert_resource(QaControls { dialogs, listener });
    app.init_resource::<FrameNavigation>();
    app.add_systems(
        PreUpdate,
        prepare_accessibility
            .after(EguiPreUpdateSet::InitContexts)
            .before(EguiPreUpdateSet::BeginPass),
    );
    if enabled {
        app.add_systems(
            PostUpdate,
            serve
                .after(EguiPostUpdateSet::ProcessOutput)
                .before(fe_ui::ui_shell::topbar::WindowChromeApply),
        );
    }
}

fn prepare_accessibility(
    enabled: Res<QaEnabled>,
    requested: Res<AccessibilityRequested>,
    mut contexts: Query<&mut EguiContext, With<PrimaryEguiContext>>,
    nav: Res<NavigationManager>,
    mut frame_nav: ResMut<FrameNavigation>,
    host: Res<QaHost>,
    controls: Res<QaControls>,
) {
    for bound in controls.listener.try_iter() {
        if let Some(dialogs) = &controls.dialogs {
            dialogs.set_listener(bound);
        }
    }
    for mut context in &mut contexts {
        fe_ui::ui_shell::layout_policy::begin_frame(context.get_mut(), enabled.0);
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

fn layout_snapshot(ctx: &bevy_egui::egui::Context, ppp: f32) -> Value {
    let o = fe_ui::ui_shell::layout_policy::observations(ctx);
    let physical = |r: bevy_egui::egui::Rect| json!({"x":r.min.x*ppp,"y":r.min.y*ppp,"w":r.width()*ppp,"h":r.height()*ppp});
    json!({"pass":o.pass,"scrolls":o.scrolls.iter().map(|s| json!({
        "content_id":s.content_id.to_string(), "label":s.label,"axes":s.axes,
        "clip":physical(s.clip),"offset":[s.offset.x*ppp,s.offset.y*ppp],
        "content_size":[s.content_size.x*ppp,s.content_size.y*ppp],"layer":s.layer
    })).collect::<Vec<_>>(),"controls":o.controls.iter().map(|c| json!({
        "id":c.id.to_string(),"rect":physical(c.rect),"hit":physical(c.hit),"clip":physical(c.clip),
        "layer":format!("{:?}",c.layer),"allowed":c.allowed,"clickable":c.clickable,"draggable":c.draggable
    })).collect::<Vec<_>>(),"texts":o.texts.iter().map(|t|json!({"id":t.id.to_string(),"truncated":t.truncated,"hover":t.hover})).collect::<Vec<_>>(),
    "coverage":"same-pass shared helper observations; unvisited widget hits, text and floating-area occlusion remain unknown"})
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
    topbar: ResMut<'w, fe_ui::ui_shell::topbar::TopbarState>,
    nav: Res<'w, NavigationManager>,
    verses: Res<'w, VerseManager>,
    nodes: Res<'w, NodeManager>,
    tool: Res<'w, ToolState>,
    ui: Res<'w, UiManager>,
    right: Res<'w, RightSidebarState>,
    inspector: Res<'w, fe_ui::plugin::InspectorFormState>,
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
        if self.topbar.close_pending {
            windows.push("Close FractalEngine?");
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
    mut state: UiState,
    mut contexts: Query<
        (
            &mut EguiContext,
            &EguiOutput,
            &mut bevy_egui::EguiContextSettings,
        ),
        With<PrimaryEguiContext>,
    >,
    windows: Query<&Window, With<PrimaryWindow>>,
    controls: Res<QaControls>,
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
        if let QaOp::DialogAnswers(batch) = &request.op {
            let result = controls
                .dialogs
                .as_ref()
                .ok_or(QaFailure {
                    code: "qa_controls_disabled",
                    message: "QA controls require input opt-in, loopback and an isolated QA root",
                })
                .and_then(|dialogs| {
                    dialogs
                        .enqueue(
                            state.nav.active_verse_id.as_deref().unwrap_or_default(),
                            batch,
                        )
                        .map(|queued| json!({"queued":queued}))
                        .map_err(|error| QaFailure {
                            code: error.code,
                            message: error.message,
                        })
                });
            request.reply(result);
            continue;
        }
        if matches!(request.op, QaOp::Window(_))
            && !controls.dialogs.as_ref().is_some_and(|d| d.enabled())
        {
            request.reply(Err(QaFailure {
                code: "qa_controls_disabled",
                message: "QA controls require input opt-in, loopback and an isolated QA root",
            }));
            continue;
        }
        let Ok((mut context, output, mut context_settings)) = contexts.single_mut() else {
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
                    "window":windows.single().ok().map(|w| json!({"width":w.physical_width(),"height":w.physical_height()})),"layout":layout_snapshot(context.get_mut(), ppp),"nodes":flatten(update,filter)}),
                ),
                None => Err(QaFailure {
                    code: "ui_tree_not_ready",
                    message: "primary accessibility tree not ready; retry",
                }),
            },
            QaOp::UiState => {
                let mut snapshot = state.snapshot(ppp, frame.0);
                snapshot["query_results"] = state.inspector.query_results.summary();
                snapshot["window"] = windows.single().ok().map(|window| json!({"width":window.physical_width(),"height":window.physical_height(),"maximized":state.topbar.maximized,"minimized":state.topbar.minimized,"ui_scale":context_settings.scale_factor})).unwrap_or(Value::Null);
                if let Some(update) = output.platform_output.accesskit_update.as_ref() {
                    let nodes = flatten(update, &TreeFilter::default());
                    if let Some(titles) = snapshot["open_windows"].as_array_mut() {
                        for node in nodes.iter().filter(|node| {
                            matches!(
                                node["role"].as_str(),
                                Some("window" | "dialog" | "alertdialog")
                            )
                        }) {
                            if let Some(title) =
                                node["name"].as_str().filter(|title| !title.is_empty())
                            {
                                let value = json!(title);
                                if !titles.contains(&value) {
                                    titles.push(value);
                                }
                            }
                        }
                    }
                }
                Ok(snapshot)
            }
            QaOp::Logs(filter) => Ok(host.logs.read(filter)),
            QaOp::Window(action) => {
                use fe_ui::ui_shell::topbar::WindowChromeRequest as Request;
                let chrome = match action {
                    WindowAction::Size { width, height } => Some(Request::SetSize {
                        width: *width,
                        height: *height,
                    }),
                    WindowAction::Maximize => Some(Request::Maximize),
                    WindowAction::Restore => Some(Request::Restore),
                    WindowAction::Minimize => Some(Request::Minimize),
                    WindowAction::Close => Some(Request::Close),
                    WindowAction::UiScale { factor } => {
                        context_settings.scale_factor = *factor;
                        None
                    }
                };
                if let Some(chrome) = chrome {
                    state.topbar.try_request_chrome(chrome).map_err(|_| QaFailure {code:"window_request_busy",message:"another window request is pending; retry"})
                        .map(|_| json!({"accepted":true,"action":action,"close_requires_confirmation":true}))
                } else {
                    Ok(json!({"accepted":true,"action":action}))
                }
            }
            QaOp::DialogAnswers(_) => Err(QaFailure {
                code: "qa_operation_unavailable",
                message: "dialog operation unavailable in this frame",
            }),
        };
        request.reply(result);
    }
}
