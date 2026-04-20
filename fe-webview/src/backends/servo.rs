use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};
use url::Url;

/// Servo-based webview backend.
///
/// Servo renders into its own window via Surfman/GL. The window is created
/// by this backend and positioned relative to the main Bevy window.
/// Servo handles all input events within its window.
///
/// ## Platform support
/// - Windows: Surfman with ANGLE (EGL/D3D11)
/// - macOS: Surfman with native GL
/// - Linux: Surfman with EGL or GLX
/// - Android: Surfman with EGL
pub struct ServoBackend {
    // TODO: servo instance, window, rendering context
    pending_events: Vec<BackendEvent>,
    alive: bool,
}

impl WebViewBackend for ServoBackend {
    fn create(
        _parent_handle: &raw_window_handle::RawWindowHandle,
        geometry: WindowGeometry,
        _trust_bar_js: &str,
    ) -> anyhow::Result<Self> {
        tracing::warn!(
            "ServoBackend::create called (stub) — geometry: x={} y={} w={} h={}. \
             Servo embedding not yet implemented; URL will open in system browser.",
            geometry.x, geometry.y, geometry.width, geometry.height
        );
        anyhow::bail!("ServoBackend not yet implemented — URL opened in system browser as fallback")
    }

    fn navigate(&mut self, url: &Url) -> anyhow::Result<()> {
        tracing::warn!("ServoBackend::navigate not yet implemented: {url}");
        Ok(())
    }

    fn go_back(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn show(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn hide(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn reposition(&mut self, _geometry: WindowGeometry) -> anyhow::Result<()> {
        Ok(())
    }

    fn destroy(&mut self) {
        self.alive = false;
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        std::mem::take(&mut self.pending_events)
    }

    fn is_alive(&self) -> bool {
        self.alive
    }
}
