//! Scoped owner-thread native fixture coordinator; see src/AGENTS.md section QA Portal evidence.

use crate::vision_bridge::ViewScope;
use bevy::prelude::*;
use fe_runtime::bridge::BridgeRequest;
use crate::portal_host::resolver::PortalResolverRes;
use fe_runtime::qa_portal::{
    self as fixture, FixtureCommand, FixtureObservation, FixtureOp, FixtureOutcome, FixtureReceipt,
    FixtureReceiver, FixtureResult, IpcPostRowName, MountExpectation,
};
use fe_webview::portal_bundle::admission::GovernanceView;
use fe_webview::portal_bundle::qa_rows::{self, IpcPostRow};
use fe_webview::portal_bundle::BridgeWebview;
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

/// Engage makes the view host-disabled (the Disable all check); disengage clears only the kill switch, never an
/// operator's Disable all.
fn kill_switch(resolver: &crate::portal_host::resolver::PortalResolver, engage: bool, scope: String) -> FixtureResult {
    let view = resolver.view();
    view.set_kill_switch(engage);
    let disabled = view.host_disabled();
    host_result(
        scope,
        FixtureOp::PortalKillSwitch { engage },
        "killswitch",
        engage,
        serde_json::json!({"killswitch": {"engaged": engage, "host_disabled": disabled}}),
    )
}

/// One fixed PJ2 row through the open bundle's bridge (`qa_rows::post_row`); a pinned mount that differs is refused.
fn ipc_post(
    resource: &WebViewBackendRes,
    row: IpcPostRowName,
    expect: MountExpectation,
    scope: String,
) -> FixtureOutcome {
    let (Some(native), Some(host)) = (resource.backend.as_ref(), resource.bundles.as_ref()) else {
        return Err(fixture::unavailable());
    };
    let Some(bridge) = native.portal_bridge() else {
        return Err(fixture::unavailable());
    };
    let mounts = host.mounts();
    let pinned = expect != MountExpectation::default();
    if pinned
        && !mounts
            .current()
            .is_some_and(|mount| expect.matches(&mount.package_id, &mount.content_hash))
    {
        return Err(fixture::failure(
            "portal_fixture_mount_mismatch",
            "The open bundle is not the package or build the request names",
        ));
    }
    let Some(outcome) = qa_rows::post_row(bridge, mounts, web_row(row), std::time::Instant::now()) else {
        return Err(fixture::failure(
            "portal_fixture_row_unavailable",
            "This row needs an open bundle (admitted for hidden, stale and control)",
        ));
    };
    Ok(host_result(
        scope,
        FixtureOp::PortalIpcPost { row, expect },
        "ipc_post",
        outcome.delivered,
        serde_json::json!({"ipc_post": {
            "row": row,
            "dropped_origin_delta": outcome.dropped_origin_delta,
            "delivered": outcome.delivered,
            "replied": outcome.replied,
        }}),
    ))
}

/// The fe-runtime mirror of a row to fe-webview's.
fn web_row(row: IpcPostRowName) -> IpcPostRow {
    match row {
        IpcPostRowName::Unsigned => IpcPostRow::Unsigned,
        IpcPostRowName::Unadmitted => IpcPostRow::Unadmitted,
        IpcPostRowName::External => IpcPostRow::External,
        IpcPostRowName::ForeignFrame => IpcPostRow::ForeignFrame,
        IpcPostRowName::Hidden => IpcPostRow::Hidden,
        IpcPostRowName::Stale => IpcPostRow::Stale,
        IpcPostRowName::Control => IpcPostRow::Control,
    }
}

/// Receipt for a host-side op: no page was read, so no URL; the outcome rides in `portal`.
fn host_result(scope: String, operation: FixtureOp, page: &str, invoked: bool, portal: serde_json::Value) -> FixtureResult {
    FixtureResult {
        receipt: FixtureReceipt {
            scope,
            operation,
            popup_session: 0,
            page_generation: 0,
            url: String::new(),
            // The legacy observation is a required field.
            observation: FixtureObservation {
                page: page.into(),
                title: String::new(),
                heading: String::new(),
                second_link: String::new(),
                third_link: String::new(),
                ready: true,
                invoked,
            },
            link_invoked: false,
            native_refusal_observed: false,
            capture_source: None,
            width: None,
            height: None,
            sha256: None,
            portal: Some(portal),
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
                    (Some(resolver), Some(scope)) => Ok(kill_switch(&resolver.0, engage, scope)),
                    _ => Err(fixture::unavailable()),
                }
            };
            request.reply(reply);
            continue;
        }
        // A fixed IPC row is the bridge's own ingress, not a page job: it runs now on the open bundle.
        if let FixtureOp::PortalIpcPost { row, expect } = request.op.operation {
            let reply = if !view.covers(&request.caller_scope, Some(&nav)) || current.is_none() {
                Err(fe_runtime::qa_debug::QaFailure::denied())
            } else if !request.op.is_current() || request.reply_tx.is_closed() {
                Err(fixture::stale())
            } else {
                match (backend.as_deref(), nav.active_scope()) {
                    (Some(resource), Some(scope)) => ipc_post(resource, row, expect, scope),
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
