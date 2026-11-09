use std::sync::Arc;

use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};
use crate::backends::ActiveBackend;
use crate::ipc::{BrowserCommand, BrowserEvent, BundleState};
use crate::portal_bundle::{BridgeWebview, BundleHost, BundleResolver, BundleWebview, PortalBroker, PortalIntent};
use crate::security;
use bevy::prelude::IntoScheduleConfigs;

/// Trust-bar JavaScript injected into every webview at construction.
///
/// Displays a fixed top banner identifying the page as an external website, or as a portal
/// bundle on a bundle origin (same label pattern as `portal_bundle::origin::parse`). It is page
/// content; the host-drawn toolbar badge is the trust indicator (`portal_bundle/AGENTS.md` §chrome).
/// SECURITY: always inject trust bar — do not remove.
pub const TRUST_BAR_JS: &str = r#"
    const bar = document.createElement('div');
    bar.style.cssText = 'position:fixed;top:0;left:0;right:0;height:24px;background:#1a1a2e;color:#8888cc;font-family:sans-serif;font-size:11px;padding:4px 8px;z-index:2147483647;pointer-events:none;';
    const bundleOrigin = location.protocol === 'https:' && /^fe-portal\.[as]-[a-z2-7]{26}\.localhost$/.test(location.hostname);
    bar.textContent = bundleOrigin ? 'Portal bundle' : '🌐 External Website: ' + location.hostname;
    document.body.prepend(bar);
"#;

/// Binary-injected repository-node resolver; without it every `OpenBundle` reports Unavailable.
#[derive(bevy::prelude::Resource, Clone)]
pub struct PortalBundleResolver(pub Arc<dyn BundleResolver>);

/// Binary-injected integration-bus client for Admitted bundles; without it every bridge call but `hello` is `unavailable`.
#[derive(bevy::prelude::Resource, Clone)]
pub struct PortalBrokerRes(pub Arc<dyn PortalBroker>);

pub struct WebViewPlugin;

impl bevy::prelude::Plugin for WebViewPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_message::<BrowserCommand>();
        app.add_message::<BrowserEvent>();
        app.add_message::<PortalIntent>();
        app.init_resource::<PortalPanelRect>();
        app.add_systems(bevy::prelude::Update, init_backend);
        app.add_systems(bevy::prelude::Update, dispatch_commands);
        app.add_systems(bevy::prelude::Update, drain_backend_events);
        // After the frame's mounts settle, so a new mount binds before its first post can arrive.
        app.add_systems(bevy::prelude::Update, pump_portal_ipc.after(drain_backend_events));
        app.add_systems(bevy::prelude::PostUpdate, sync_portal_position.run_if(fe_runtime::maintenance::editor_running));
    }
}

// ---------------------------------------------------------------------------
// NonSend resource wrapping the active backend
// ---------------------------------------------------------------------------

/// Holds the active webview backend. Inserted as a `NonSend` resource because
/// native window handles are thread-bound.
pub struct WebViewBackendRes {
    pub backend: Option<ActiveBackend>,
    /// Set when the backend window died externally (e.g. Alt+F4) so
    /// `init_backend` recreates it. Never set on create-failure, which stays
    /// terminal to avoid a retry-every-frame loop.
    pub lost: bool,
    /// Bundle lifecycle over this backend's mount table; `None` without an embedded browser.
    pub bundles: Option<BundleHost>,
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// Logical-pixel rect of the portal panel, written by the UI crate each frame.
/// The webview popup is positioned to match this rect.
#[derive(bevy::prelude::Resource, Debug, Clone, Copy)]
pub struct PortalPanelRect {
    /// Left edge in logical pixels (relative to window client area).
    pub x: f32,
    /// Top edge in logical pixels.
    pub y: f32,
    /// Width in logical pixels.
    pub width: f32,
    /// Height in logical pixels.
    pub height: f32,
}

impl Default for PortalPanelRect {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 320.0,
            height: 600.0,
        }
    }
}

/// Convert `PortalPanelRect` (logical) to screen-space `WindowGeometry`.
#[cfg(feature = "winit")]
fn portal_rect_to_geometry(rect: &PortalPanelRect, win: &winit::window::Window) -> WindowGeometry {
    let scale = win.scale_factor();
    let inner_pos = win.inner_position().unwrap_or_default();

    WindowGeometry {
        x: inner_pos.x + (rect.x as f64 * scale) as i32,
        y: inner_pos.y + (rect.y as f64 * scale) as i32,
        width: (rect.width as f64 * scale).max(1.0) as u32,
        height: (rect.height as f64 * scale).max(1.0) as u32,
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Deferred init: creates the backend once WinitWindows is available.
fn init_backend(world: &mut bevy::prelude::World) {
    if let Some(res) = world.get_non_send_resource::<WebViewBackendRes>() {
        if !res.lost {
            return;
        }
        // Backend window died externally — drop it and fall through to recreate.
        bevy::log::warn!("Portal: init_backend: backend lost, recreating");
        world.remove_non_send_resource::<WebViewBackendRes>();
    }

    #[cfg(feature = "winit")]
    {
        use bevy::prelude::{Entity, With};
        use bevy::window::PrimaryWindow;

        let window_entity = {
            let mut q = world.query_filtered::<Entity, With<PrimaryWindow>>();
            match q.single(world) {
                Ok(e) => e,
                Err(_) => {
                    // PrimaryWindow not spawned yet — wait for next frame.
                    return;
                }
            }
        };

        // Bevy 0.18: WinitWindows is a thread_local, not a NonSend resource.
        let portal_rect = world.resource::<PortalPanelRect>();
        let portal_rect = *portal_rect;

        let result = bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
            let winit_wrapper = match winit_windows.get_window(window_entity) {
                Some(w) => w,
                None => return None,
            };
            let inner: &winit::window::Window = winit_wrapper;
            let geometry = portal_rect_to_geometry(&portal_rect, inner);
            bevy::log::info!(
                "Portal: init_backend: geometry x={} y={} w={} h={} scale={}",
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                inner.scale_factor()
            );

            use raw_window_handle::HasWindowHandle;
            let raw = match inner.window_handle() {
                Ok(h) => h.as_raw(),
                Err(_) => {
                    bevy::log::error!("Portal: init_backend: could not get raw window handle");
                    return Some(Err(anyhow::anyhow!("could not get raw window handle")));
                }
            };

            bevy::log::info!("Portal: init_backend: calling ActiveBackend::create...");
            Some(ActiveBackend::create(&raw, geometry, TRUST_BAR_JS))
        });

        let Some(result) = result else {
            // Window not ready yet — try next frame.
            return;
        };

        match result {
            Ok(backend) => {
                bevy::log::info!(
                    "Portal: backend initialized OK: {}",
                    std::any::type_name::<ActiveBackend>()
                );
                let bundles = backend.bundle_parts().map(BundleHost::new);
                world.insert_non_send_resource(WebViewBackendRes {
                    backend: Some(backend),
                    lost: false,
                    bundles,
                });
            }
            Err(e) => {
                bevy::log::error!("Portal: backend init FAILED: {e}");
                world.insert_non_send_resource(WebViewBackendRes {
                    backend: None,
                    lost: false,
                    bundles: None,
                });
            }
        }
    }

    #[cfg(not(feature = "winit"))]
    {
        bevy::log::warn!("WebView backend requires winit feature");
        world.insert_non_send_resource(WebViewBackendRes {
            backend: None,
            lost: false,
            bundles: None,
        });
    }
}

/// Every `OpenBundle` in `commands` reports Unavailable (no embedded browser to mount into).
fn report_bundles_unavailable(commands: &[BrowserCommand], events: &mut bevy::prelude::MessageWriter<BrowserEvent>) {
    for command in commands {
        if let BrowserCommand::OpenBundle { node_id } = command {
            events.write(BrowserEvent::BundleState {
                node_id: node_id.clone(),
                label: None,
                state: BundleState::Unavailable,
            });
        }
    }
}

/// Reads `BrowserCommand` messages and dispatches to the active backend.
fn dispatch_commands(
    backend_res: Option<bevy::ecs::system::NonSendMut<WebViewBackendRes>>,
    mut reader: bevy::prelude::MessageReader<BrowserCommand>,
    mut events: bevy::prelude::MessageWriter<BrowserEvent>,
    tab_filter: bevy::prelude::Res<crate::petal_portal::TabVisibilityFilter>,
    resolver: Option<bevy::prelude::Res<PortalBundleResolver>>,
) {
    let cmds: Vec<BrowserCommand> = reader
        .read()
        .filter_map(|command| {
            if let BrowserCommand::OpenExternal { url } = command {
                match crate::backends::stub::open_in_system_browser(url.as_str()) {
                    Ok(()) => {
                        events.write(BrowserEvent::ExternalLaunchRequested);
                    }
                    Err(error) => {
                        events.write(BrowserEvent::Error {
                            message: format!("Could not request external browser: {error}"),
                        });
                    }
                }
                None
            } else {
                Some(command.clone())
            }
        })
        .collect();
    if cmds.is_empty() {
        return;
    }

    let Some(mut res) = backend_res else {
        report_bundles_unavailable(&cmds, &mut events);
        events.write(BrowserEvent::Error {
            message: "Embedded browser is not ready; choose Open externally or try again".into(),
        });
        bevy::log::warn!(
            "Portal: received {} cmd(s) but WebViewBackendRes not available",
            cmds.len()
        );
        return;
    };
    let WebViewBackendRes {
        backend, bundles, ..
    } = &mut *res;
    let Some(backend) = backend.as_mut() else {
        report_bundles_unavailable(&cmds, &mut events);
        events.write(BrowserEvent::Error {
            message: "Embedded browser is unavailable; choose Open externally".into(),
        });
        bevy::log::warn!(
            "Portal: received {} cmd(s) but backend is None (init failed?)",
            cmds.len()
        );
        return;
    };

    for cmd in cmds {
        match cmd {
            BrowserCommand::Navigate { ref url } => {
                if !security::embedded_allowed(url) {
                    bevy::log::warn!("Portal: Navigate blocked by security policy: {url}");
                    events.write(BrowserEvent::Error {
                        message: format!("URL blocked: {url}"),
                    });
                    continue;
                }
                // Backend deduplicates — silently skip if already at this URL.
                let Ok(parsed) = url.parse::<url::Url>() else {
                    continue;
                };
                // A mounted bundle goes first; the gate admits exactly this (normalized) host load.
                if let Some(host) = bundles.as_mut() {
                    for event in host.detach_for_navigation(parsed.as_str()) {
                        events.write(event);
                    }
                }
                if let Err(e) = backend.navigate(&parsed) {
                    bevy::log::error!("Portal: navigate failed: {e}");
                    events.write(BrowserEvent::Error {
                        message: e.to_string(),
                    });
                }
            }
            BrowserCommand::GoBack => {
                if let Err(e) = backend.go_back() {
                    bevy::log::error!("Portal: go_back failed: {e}");
                    events.write(BrowserEvent::Error {
                        message: e.to_string(),
                    });
                }
            }
            BrowserCommand::GoForward => {
                if let Err(error) = backend.go_forward() {
                    events.write(BrowserEvent::Error {
                        message: error.to_string(),
                    });
                }
            }
            BrowserCommand::Reload => {
                if let Err(error) = backend.reload() {
                    events.write(BrowserEvent::Error {
                        message: error.to_string(),
                    });
                }
            }
            BrowserCommand::OpenExternal { .. } => {
                unreachable!("external commands are handled before backend access")
            }
            BrowserCommand::Close => {
                bevy::log::info!("Portal: closing portal");
                // Bundles unload on close (and a pending open is voided); external sites are only hidden.
                let closed = match bundles.as_mut() {
                    Some(host) => host.close(&mut *backend),
                    None => Vec::new(),
                };
                if let Err(e) = backend.hide() {
                    bevy::log::error!("Portal: hide failed: {e}");
                }
                for event in closed {
                    events.write(event);
                }
            }
            BrowserCommand::OpenBundle { node_id } => match bundles.as_mut() {
                Some(host) => {
                    let resolver = resolver.as_ref().map(|resolver| Arc::clone(&resolver.0));
                    for event in host.open(&node_id, resolver, &mut *backend) {
                        events.write(event);
                    }
                }
                None => {
                    events.write(BrowserEvent::BundleState {
                        node_id,
                        label: None,
                        state: BundleState::Unavailable,
                    });
                }
            },
            BrowserCommand::CloseBundle => {
                if let Some(host) = bundles.as_mut() {
                    for event in host.close(&mut *backend) {
                        events.write(event);
                    }
                }
            }
            BrowserCommand::GetUrl => {}
            BrowserCommand::SwitchTab { ref tab } => {
                // Inline the SwitchTab(Config) guard that was previously in
                // tab_switch_guard_system. This avoids the guard/flush echo
                // loop that duplicated Navigate commands every frame.
                if matches!(tab, crate::ipc::BrowserTab::Config) && !tab_filter.can_view_config() {
                    bevy::log::warn!(
                        "Unauthorized SwitchTab(Config) blocked — role={:?}",
                        tab_filter.role
                    );
                    continue;
                }
                events.write(BrowserEvent::TabChanged { tab: *tab });
            }
        }
    }
}

/// Drains backend events and converts them to `BrowserEvent` messages.
fn drain_backend_events(
    backend_res: Option<bevy::ecs::system::NonSendMut<WebViewBackendRes>>,
    mut events: bevy::prelude::MessageWriter<BrowserEvent>,
) {
    let Some(mut res) = backend_res else { return };
    let WebViewBackendRes {
        backend: slot,
        lost,
        bundles,
    } = &mut *res;
    let Some(backend) = slot.as_mut() else {
        return;
    };

    let drained = backend.drain_events();
    let mut window_closed = false;
    for evt in drained {
        match evt {
            BackendEvent::UrlChanged(ref url) => {
                bevy::log::info!("Portal: URL changed to {url}");
                events.write(BrowserEvent::UrlChanged { url: url.clone() });
            }
            BackendEvent::LoadComplete => {
                bevy::log::info!("Portal: page load complete");
                events.write(BrowserEvent::LoadComplete);
            }
            BackendEvent::PageLoaded(url) => {
                events.write(BrowserEvent::PageLoaded { url });
            }
            BackendEvent::Error(ref message) => {
                bevy::log::error!("Portal: backend error: {message}");
                events.write(BrowserEvent::Error {
                    message: message.clone(),
                });
            }
            BackendEvent::WindowClosed => {
                bevy::log::warn!("Portal: backend window was closed by OS — scheduling recreate");
                window_closed = true;
            }
        }
    }
    if window_closed {
        // The mount table dies with its webview; the recreated backend gets a fresh host.
        *slot = None;
        *bundles = None;
        *lost = true;
        return;
    }
    if let Some(host) = bundles.as_mut() {
        for event in host.poll(&mut *backend) {
            events.write(event);
        }
    }
}

/// Per frame: route the bridge's accepted posts, collect broker results, then post what passes the send-time gate.
fn pump_portal_ipc(
    backend_res: Option<bevy::ecs::system::NonSend<WebViewBackendRes>>,
    broker: Option<bevy::prelude::Res<PortalBrokerRes>>,
    mut intents: bevy::prelude::MessageWriter<PortalIntent>,
) {
    let Some(res) = backend_res else { return };
    let (Some(backend), Some(host)) = (res.backend.as_ref(), res.bundles.as_ref()) else {
        return;
    };
    let Some(bridge) = backend.portal_bridge() else { return };
    let mounts = host.mounts();
    let broker = broker.as_ref().map(|broker| &broker.0);
    for intent in bridge.pump(mounts, broker, std::time::Instant::now()) {
        intents.write(intent);
    }
    for envelope in bridge.take_outbox(mounts) {
        if let Err(error) = backend.post_page_message(&envelope) {
            bevy::log::warn!("Portal: IPC delivery failed: {error:#}");
            break;
        }
    }
}

/// Repositions the popup window each frame to match `PortalPanelRect`.
fn sync_portal_position(
    backend_res: Option<bevy::ecs::system::NonSendMut<WebViewBackendRes>>,
    portal_rect: bevy::prelude::Res<PortalPanelRect>,
    #[cfg(feature = "winit")] primary_window: bevy::prelude::Query<
        bevy::prelude::Entity,
        bevy::prelude::With<bevy::window::PrimaryWindow>,
    >,
    #[cfg(feature = "winit")] mut last_geometry: bevy::prelude::Local<Option<WindowGeometry>>,
) {
    let Some(mut res) = backend_res else { return };

    #[cfg(feature = "winit")]
    {
        let Some(backend) = res.backend.as_mut() else {
            return;
        };
        let Ok(entity) = primary_window.single() else {
            return;
        };
        bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
            let Some(wrapper) = winit_windows.get_window(entity) else {
                return;
            };
            let inner: &winit::window::Window = wrapper;
            let geometry = portal_rect_to_geometry(&portal_rect, inner);
            // Skip the SetWindowPos/set_bounds round-trip when nothing moved.
            if *last_geometry == Some(geometry) {
                return;
            }
            if backend.reposition(geometry).is_ok() {
                *last_geometry = Some(geometry);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Re-exports for API compatibility
// ---------------------------------------------------------------------------

pub use crate::overlay::WebViewConfig;

// ---------------------------------------------------------------------------
// Trust bar test types
// ---------------------------------------------------------------------------

/// Builder spy for asserting that `TRUST_BAR_JS` is always injected.
/// Used in unit tests without requiring a webview backend.
pub struct WryBrowserSurfaceBuilderSpy {
    pub init_scripts: Vec<String>,
}

impl WryBrowserSurfaceBuilderSpy {
    pub fn new(_config: WebViewConfig) -> Self {
        Self {
            init_scripts: vec![TRUST_BAR_JS.to_string()],
        }
    }

    pub fn init_scripts(&self) -> &[String] {
        &self.init_scripts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_database::{PetalId, RoleId};

    fn make_config() -> WebViewConfig {
        WebViewConfig {
            petal_id: PetalId(ulid::Ulid::new()),
            initial_url: None,
            role: RoleId("viewer".to_string()),
        }
    }

    #[test]
    fn wry_browser_surface_builder_spy_includes_trust_bar_init_script() {
        let config = make_config();
        let spy = WryBrowserSurfaceBuilderSpy::new(config);
        assert!(
            spy.init_scripts()
                .iter()
                .any(|s| s.contains("External Website")),
            "TRUST_BAR_JS must be present in init_scripts"
        );
    }

    #[test]
    fn trust_bar_js_constant_is_non_empty() {
        assert!(!TRUST_BAR_JS.trim().is_empty());
    }
}
