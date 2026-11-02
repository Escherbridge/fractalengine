//! External-browser fallback; see src/AGENTS.md section fallback-backend.

use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};
use url::Url;

/// External browser launcher with no embedded window or navigation history.
pub struct StubBackend {
    active: bool,
    pending_error: Option<String>,
}

impl StubBackend {
    fn refuse_navigation(&mut self, url: &Url) -> anyhow::Result<()> {
        let message = if !self.active {
            "Browser fallback has been closed".to_string()
        } else if !crate::security::is_url_allowed(url) {
            "URL blocked by browser security policy".to_string()
        } else {
            "Embedded browsing is unavailable; choose Open externally to request your browser"
                .to_string()
        };
        self.pending_error = Some(message.clone());
        anyhow::bail!(message)
    }
}

impl WebViewBackend for StubBackend {
    fn create(
        _parent_handle: &raw_window_handle::RawWindowHandle,
        _geometry: WindowGeometry,
        _trust_bar_js: &str,
    ) -> anyhow::Result<Self> {
        tracing::warn!("No webview backend compiled in; embedded navigation is unavailable; external launch requires an explicit action");
        Ok(Self {
            active: true,
            pending_error: None,
        })
    }

    fn navigate(&mut self, url: &Url) -> anyhow::Result<()> {
        self.refuse_navigation(url)
    }

    fn go_back(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("Back navigation is controlled by the external browser")
    }

    fn show(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("There is no embedded browser window to show")
    }

    fn hide(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("The external browser window cannot be hidden by the portal")
    }

    fn reposition(&mut self, _geometry: WindowGeometry) -> anyhow::Result<()> {
        anyhow::bail!("The external browser window cannot be positioned by the portal")
    }

    fn destroy(&mut self) {
        self.active = false;
        self.pending_error = None;
    }

    fn drain_events(&mut self) -> Vec<BackendEvent> {
        self.pending_error
            .take()
            .map(BackendEvent::Error)
            .into_iter()
            .collect()
    }

    fn is_alive(&self) -> bool {
        false
    }
}

pub(crate) fn open_in_system_browser(url: &str) -> anyhow::Result<()> {
    let parsed = Url::parse(url)?;
    anyhow::ensure!(
        crate::security::is_url_allowed(&parsed),
        "URL blocked by browser security policy"
    );
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer.exe");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "linux"))]
    {
        command
            .arg(url)
            .spawn()
            .map(|_| ())
            .map_err(|error| anyhow::anyhow!("external browser launcher could not start: {error}"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = url;
        anyhow::bail!("External browser launch is unavailable on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_external_handoff_reports_no_embedded_success() {
        let parent =
            raw_window_handle::RawWindowHandle::Xlib(raw_window_handle::XlibWindowHandle::new(0));
        let geometry = WindowGeometry {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        };
        let mut backend = StubBackend::create(&parent, geometry, "").unwrap();
        let url = Url::parse("https://example.com/portal?value=one&other=two").unwrap();
        let refusal = backend.navigate(&url).unwrap_err();
        assert!(refusal.to_string().contains("choose Open externally"));
        assert!(!backend.is_alive());
        assert!(
            matches!(backend.drain_events().as_slice(),[BackendEvent::Error(message)] if message == &refusal.to_string())
        );
        assert!(backend.drain_events().is_empty());
        assert!(backend.go_forward().is_err());
        assert!(backend.reload().is_err());
        assert!(backend.go_back().is_err());
        assert!(backend.show().is_err());
        assert!(backend.hide().is_err());
        assert!(backend.reposition(geometry).is_err());
        assert!(
            backend.drain_events().is_empty(),
            "unsupported positioning does not emit per-frame errors"
        );
        for blocked in ["file:///private", "https://127.0.0.1/private"] {
            assert!(backend.navigate(&Url::parse(blocked).unwrap()).is_err());
        }
        backend.destroy();
        assert!(backend.drain_events().is_empty());
        assert!(backend.navigate(&url).is_err());
    }
}
