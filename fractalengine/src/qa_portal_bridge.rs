//! Scoped owner-thread native fixture coordinator; see src/AGENTS.md section QA Portal evidence.

use crate::vision_bridge::ViewScope;
use bevy::prelude::*;
use fe_runtime::bridge::BridgeRequest;
use crate::portal_host::resolver::PortalResolverRes;
use fe_runtime::qa_portal::{
    self as fixture, FixtureCommand, FixtureObservation, FixtureOp, FixtureOutcome, FixtureReceipt,
    FixtureReceiver, FixtureResult,
};
use fe_webview::portal_bundle::admission::GovernanceView;
use fe_ui::{
    actions::UiManager, navigation_manager::NavigationManager, node_manager::NodeManager, plugin::LocalUserRole,
};
use fe_webview::{backend::WebViewBackend, plugin::WebViewBackendRes};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Context {
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

pub(crate) fn context(
    nav: &NavigationManager,
    nodes: &NodeManager,
    ui: &UiManager,
    role: &LocalUserRole,
) -> Option<Context> {
    if role.role != Some(fe_database::RoleLevel::Owner) || !nav.has_valid_ancestry() {
        return None;
    }
    let selected = nodes.primary()?;
    // A website or a bundle portal; the native surface checks each op's origin (bundle ops never use qa_web).
    if ui.portal.opened_for_entity() != Some(selected.entity) {
        return None;
    }
    Some(Context {
        scope: nav.active_scope()?,
        entity: selected.entity,
        node_id: selected.node_id.clone(),
    })
}

/// Engage sets host-disabled (the Disable all state); disengage restores what it was before the first engage.
fn kill_switch(
    resolver: &crate::portal_host::resolver::PortalResolver,
    engage: bool,
    restore_to: &mut Option<bool>,
    scope: String,
) -> FixtureResult {
    let view = resolver.view();
    if engage {
        restore_to.get_or_insert_with(|| view.host_disabled());
        view.set_host_disabled(true);
    } else {
        view.set_host_disabled(restore_to.take().unwrap_or(false));
    }
    let disabled = view.host_disabled();
    FixtureResult {
        receipt: FixtureReceipt {
            scope,
            operation: FixtureOp::PortalKillSwitch { engage },
            popup_session: 0,
            page_generation: 0,
            url: String::new(),
            // The legacy observation is a required field; the outcome rides in `portal`.
            observation: FixtureObservation {
                page: "killswitch".into(),
                title: String::new(),
                heading: String::new(),
                second_link: String::new(),
                third_link: String::new(),
                ready: true,
                invoked: engage,
            },
            link_invoked: false,
            native_refusal_observed: false,
            capture_source: None,
            width: None,
            height: None,
            sha256: None,
            portal: Some(serde_json::json!({"killswitch": {"engaged": engage, "host_disabled": disabled}})),
        },
        png: None,
    }
}

#[allow(clippy::too_many_arguments)] // Bevy system parameters
fn serve(
    mut host: ResMut<Host>,
    nav: Res<NavigationManager>,
    nodes: Res<NodeManager>,
    ui: Res<UiManager>,
    role: Res<LocalUserRole>,
    view: ViewScope,
    backend: Option<NonSendMut<WebViewBackendRes>>,
    markers: Query<&fe_ui::plugin::SpawnedNodeMarker>,
    resolver: Option<Res<PortalResolverRes>>,
    mut restore_to: Local<Option<bool>>,
) {
    let mut backend = backend;
    if host.active.is_none() && host.rx.is_empty() {
        return;
    }
    let current = context(&nav, &nodes, &ui, &role).filter(|context| {
        markers.get(context.entity).is_ok_and(|marker| {
            marker.node_id == context.node_id
                && Some(marker.petal_id.as_str()) == nav.active_petal_id.as_deref()
        })
    });
    if let Some(job) = host.active.take() {
        let valid = current.as_ref() == Some(&job.context)
            && view.covers(&job.request.caller_scope, Some(&nav))
            && job.request.op.is_current()
            && !job.request.reply_tx.is_closed();
        let native = backend
            .as_mut()
            .and_then(|resource| resource.backend.as_mut());
        let native = match native {
            Some(native) if valid => native,
            other => {
                if let Some(native) = other {
                    native.fixture_cancel();
                }
                job.request.reply(Err(fixture::stale()));
                return;
            }
        };
        if let Some(result) = native.fixture_poll() {
            if !job.request.op.is_current() || !view.covers(&job.request.caller_scope, Some(&nav)) {
                job.request.reply(Err(fixture::stale()));
                return;
            }
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
        // The kill switch is host governance, not a page job: no mounted portal or backend is needed.
        if let FixtureOp::PortalKillSwitch { engage } = request.op.operation {
            let reply = if !view.covers(&request.caller_scope, Some(&nav))
                || role.role != Some(fe_database::RoleLevel::Owner)
            {
                Err(fe_runtime::qa_debug::QaFailure::denied())
            } else if !request.op.is_current() || request.reply_tx.is_closed() {
                Err(fixture::stale())
            } else {
                match (resolver.as_deref(), nav.active_scope()) {
                    (Some(resolver), Some(scope)) => Ok(kill_switch(&resolver.0, engage, &mut restore_to, scope)),
                    _ => Err(fixture::unavailable()),
                }
            };
            request.reply(reply);
            continue;
        }
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
        if !request.op.is_current() || request.reply_tx.is_closed() {
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
