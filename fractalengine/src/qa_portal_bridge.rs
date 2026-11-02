//! Scoped owner-thread native fixture coordinator; see src/AGENTS.md section QA Portal evidence.

use crate::vision_bridge::ViewScope;
use bevy::prelude::*;
use fe_runtime::bridge::BridgeRequest;
use fe_runtime::qa_portal::{self as fixture, FixtureCommand, FixtureOutcome, FixtureReceiver};
use fe_ui::{
    actions::UiManager, navigation_manager::NavigationManager, node_manager::NodeManager,
    plugin::LocalUserRole, portal::PortalState,
};
use fe_webview::{backend::WebViewBackend, plugin::WebViewBackendRes};
use std::time::Instant;

#[derive(PartialEq, Eq)]
struct Context {
    scope: String,
    entity: Entity,
    node_id: String,
}
struct Job {
    request: BridgeRequest<FixtureCommand, FixtureOutcome>,
    context: Context,
}
#[derive(Resource)]
struct Host {
    rx: FixtureReceiver,
    active: Option<Job>,
}

pub fn install(app: &mut App, enabled: bool, rx: FixtureReceiver) {
    app.insert_resource(Host { rx, active: None });
    if enabled {
        app.add_systems(
            PostUpdate,
            serve.after(bevy_egui::EguiPostUpdateSet::ProcessOutput),
        );
    }
}

fn context(
    nav: &NavigationManager,
    nodes: &NodeManager,
    ui: &UiManager,
    role: &LocalUserRole,
) -> Option<Context> {
    if role.role != Some(fe_database::RoleLevel::Owner) || !nav.has_valid_ancestry() {
        return None;
    }
    let selected = nodes.selected.as_ref()?;
    let PortalState::Open {
        opened_for_entity, ..
    } = &ui.portal
    else {
        return None;
    };
    if *opened_for_entity != selected.entity {
        return None;
    }
    Some(Context {
        scope: nav.active_scope()?,
        entity: selected.entity,
        node_id: selected.node_id.clone(),
    })
}

fn serve(
    mut host: ResMut<Host>,
    nav: Res<NavigationManager>,
    nodes: Res<NodeManager>,
    ui: Res<UiManager>,
    role: Res<LocalUserRole>,
    view: ViewScope,
    backend: Option<NonSendMut<WebViewBackendRes>>,
) {
    let mut backend = backend;
    if host.active.is_none() && host.rx.is_empty() {
        return;
    }
    let current = context(&nav, &nodes, &ui, &role);
    if let Some(job) = host.active.take() {
        let valid = current.as_ref() == Some(&job.context)
            && view.covers(&job.request.caller_scope, Some(&nav))
            && Instant::now() < job.request.op.deadline
            && !job.request.reply_tx.is_closed();
        let native = backend
            .as_mut()
            .and_then(|resource| resource.backend.as_mut());
        if !valid || native.is_none() {
            if let Some(native) = native {
                native.fixture_cancel();
            }
            job.request.reply(Err(fixture::stale()));
        } else if let Some(result) = native.unwrap().fixture_poll() {
            let result = result.map(|mut result| {
                result.receipt.scope = job.context.scope;
                result
            });
            job.request.reply(result);
        } else {
            host.active = Some(job);
        }
    }
    for request in host.rx.clone().try_iter().take(4) {
        if host.active.is_some() {
            request.reply(Err(fixture::failure(
                "portal_fixture_busy",
                "Another native fixture operation is pending",
            )));
            continue;
        }
        if !view.covers(&request.caller_scope, Some(&nav)) || current.is_none() {
            request.reply(Err(fe_runtime::qa_debug::QaFailure::denied()));
            continue;
        }
        if Instant::now() >= request.op.deadline || request.reply_tx.is_closed() {
            request.reply(Err(fixture::stale()));
            continue;
        }
        let Some(native) = backend
            .as_mut()
            .and_then(|resource| resource.backend.as_mut())
        else {
            request.reply(Err(fixture::unavailable()));
            continue;
        };
        match native.fixture_start(&request.op) {
            Ok(()) => {
                host.active = Some(Job {
                    request,
                    context: context(&nav, &nodes, &ui, &role)
                        .expect("current context was checked"),
                })
            }
            Err(error) => request.reply(Err(error)),
        }
    }
}
