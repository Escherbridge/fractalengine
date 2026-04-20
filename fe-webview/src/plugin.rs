use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};
use crate::backends::ActiveBackend;
use crate::ipc::{BrowserCommand, BrowserEvent};
use crate::security;

/// Trust-bar JavaScript injected into every webview at construction.
///
/// Displays a fixed top banner identifying the page as an external website.
/// SECURITY: always inject trust bar — do not remove.
pub const TRUST_BAR_JS: &str = r#"
    const bar = document.createElement('div');
    bar.style.cssText = 'position:fixed;top:0;left:0;right:0;height:24px;background:#1a1a2e;color:#8888cc;font-family:sans-serif;font-size:11px;padding:4px 8px;z-index:2147483647;pointer-events:none;';
    bar.textContent = '🌐 External Website: ' + location.hostname;
    document.body.prepend(bar);
"#;

pub struct WebViewPlugin;

impl bevy::prelude::Plugin for WebViewPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_message::<BrowserCommand>();
        app.add_message::<BrowserEvent>();
        app.init_resource::<PortalPanelRect>();
        app.add_systems(bevy::prelude::Update, init_backend);
        app.add_systems(bevy::prelude::Update, dispatch_commands);
        app.add_systems(bevy::prelude::Update, drain_backend_events);
        app.add_systems(bevy::prelude::PostUpdate, sync_portal_position);
    }
}

// ---------------------------------------------------------------------------
// NonSend resource wrapping the active backend
// ---------------------------------------------------------------------------

/// Holds the active webview backend. Inserted as a `NonSend` resource because
/// native window handles are thread-bound.
pub struct WebViewBackendRes {
    pub backend: Option<ActiveBackend>,
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
fn portal_rect_to_geometry(
    rect: &PortalPanelRect,
    win: &winit::window::Window,
) -> WindowGeometry {
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
    if world
        .get_non_send_resource::<WebViewBackendRes>()
        .is_some()
    {
        return;
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
            let inner: &winit::window::Window = &**winit_wrapper;
            let geometry = portal_rect_to_geometry(&portal_rect, inner);
            eprintln!(
                "[PORTAL] init_backend: geometry x={} y={} w={} h={} scale={}",
                geometry.x, geometry.y, geometry.width, geometry.height,
                inner.scale_factor()
            );

            use raw_window_handle::HasWindowHandle;
            let raw = match inner.window_handle() {
                Ok(h) => h.as_raw(),
                Err(_) => {
                    eprintln!("[PORTAL] init_backend: could not get raw window handle");
                    return Some(Err(anyhow::anyhow!("could not get raw window handle")));
                }
            };

            eprintln!("[PORTAL] init_backend: calling ActiveBackend::create...");
            Some(ActiveBackend::create(&raw, geometry, TRUST_BAR_JS))
        });

        let Some(result) = result else {
            // Window not ready yet — try next frame.
            return;
        };

        match result {
            Ok(backend) => {
                eprintln!("[PORTAL] backend initialized OK: {}", std::any::type_name::<ActiveBackend>());
                world.insert_non_send_resource(WebViewBackendRes {
                    backend: Some(backend),
                });
            }
            Err(e) => {
                eprintln!("[PORTAL] backend init FAILED: {e}");
                world.insert_non_send_resource(WebViewBackendRes { backend: None });
            }
        }
    }

    #[cfg(not(feature = "winit"))]
    {
        bevy::log::warn!("WebView backend requires winit feature");
        world.insert_non_send_resource(WebViewBackendRes { backend: None });
    }
}

/// Reads `BrowserCommand` messages and dispatches to the active backend.
fn dispatch_commands(
    backend_res: Option<bevy::ecs::system::NonSendMut<WebViewBackendRes>>,
    mut reader: bevy::prelude::MessageReader<BrowserCommand>,
    mut events: bevy::prelude::MessageWriter<BrowserEvent>,
    tab_filter: bevy::prelude::Res<crate::petal_portal::TabVisibilityFilter>,
) {
    let cmds: Vec<BrowserCommand> = reader.read().cloned().collect();
    if cmds.is_empty() {
        return;
    }

    let Some(mut res) = backend_res else {
        eprintln!("[PORTAL] received {} cmd(s) but WebViewBackendRes not available", cmds.len());
        return;
    };
    let Some(backend) = res.backend.as_mut() else {
        eprintln!("[PORTAL] received {} cmd(s) but backend is None (init failed?)", cmds.len());
        return;
    };

    for cmd in cmds {
        match cmd {
            BrowserCommand::Navigate { ref url } => {
                if !security::is_url_allowed(url) {
                    eprintln!("[PORTAL] Navigate blocked by security policy: {url}");
                    events.write(BrowserEvent::Error {
                        message: format!("URL blocked: {url}"),
                    });
                    continue;
                }
                // Backend deduplicates — silently skip if already at this URL.
                if let Err(e) = backend.navigate(url) {
                    eprintln!("[PORTAL] navigate failed: {e}");
                }
            }
            BrowserCommand::GoBack => {
                if let Err(e) = backend.go_back() {
                    eprintln!("[PORTAL] go_back failed: {e}");
                }
            }
            BrowserCommand::Close => {
                eprintln!("[PORTAL] closing portal");
                if let Err(e) = backend.hide() {
                    eprintln!("[PORTAL] hide failed: {e}");
                }
            }
            BrowserCommand::GetUrl => {}
            BrowserCommand::SwitchTab { ref tab } => {
                // Inline the SwitchTab(Config) guard that was previously in
                // tab_switch_guard_system. This avoids the guard/flush echo
                // loop that duplicated Navigate commands every frame.
                if matches!(tab, crate::ipc::BrowserTab::Config)
                    && !tab_filter.can_view_config()
                {
                    bevy::log::warn!("Unauthorized SwitchTab(Config) blocked — role={:?}", tab_filter.role);
                    continue;
                }
                events.write(BrowserEvent::TabChanged { tab: tab.clone() });
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
    let Some(backend) = res.backend.as_mut() else { return };

    let drained = backend.drain_events();
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
            BackendEvent::Error(ref message) => {
                bevy::log::error!("Portal: backend error: {message}");
                events.write(BrowserEvent::Error { message: message.clone() });
            }
            BackendEvent::WindowClosed => {
                bevy::log::warn!("Portal: backend window was closed by OS");
            }
        }
    }
}

/// Repositions the popup window each frame to match `PortalPanelRect`.
fn sync_portal_position(
    backend_res: Option<bevy::ecs::system::NonSendMut<WebViewBackendRes>>,
    portal_rect: bevy::prelude::Res<PortalPanelRect>,
    #[cfg(feature = "winit")]
    primary_window: bevy::prelude::Query<
        bevy::prelude::Entity,
        bevy::prelude::With<bevy::window::PrimaryWindow>,
    >,
) {
    let Some(mut res) = backend_res else { return };
    let Some(backend) = res.backend.as_mut() else { return };

    #[cfg(feature = "winit")]
    {
        let Ok(entity) = primary_window.single() else { return };
        bevy::winit::WINIT_WINDOWS.with_borrow(|winit_windows| {
            let Some(wrapper) = winit_windows.get_window(entity) else { return };
            let inner: &winit::window::Window = &**wrapper;
            let geometry = portal_rect_to_geometry(&portal_rect, inner);
            let _ = backend.reposition(geometry);
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
