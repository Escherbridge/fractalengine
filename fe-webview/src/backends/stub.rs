//! External-browser fallback; see src/AGENTS.md section fallback-backend.

use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};
use url::Url;

/// External browser launcher with no embedded window or navigation history.
pub struct StubBackend {
    active: bool,
    pending_error: Option<String>,
}

impl StubBackend {
    fn navigate_with(
        &mut self,
        url: &Url,
        open: impl FnOnce(&str) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let message = if !self.active {
            "Browser fallback has been closed".to_string()
        } else if !crate::security::is_url_allowed(url) {
            "URL blocked by browser security policy".to_string()
        } else {
            match open(url.as_str()) {
                Ok(()) => "Embedded browsing is unavailable; external browser launch requested"
                    .to_string(),
                Err(error) => format!("Could not launch the external browser: {error}"),
            }
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
        tracing::warn!("No webview backend compiled in; navigation uses the external browser, with no embedded portal");
        Ok(Self {
            active: true,
            pending_error: None,
        })
    }

    fn navigate(&mut self, url: &Url) -> anyhow::Result<()> {
        self.navigate_with(url, open_in_system_browser)
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

fn open_in_system_browser(url: &str) -> anyhow::Result<()> {
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
        let mut launched = Vec::new();
        let handoff = backend
            .navigate_with(&url, |url| {
                launched.push(url.to_owned());
                Ok(())
            })
            .unwrap_err();
        assert_eq!(launched, vec![url.as_str()]);
        assert!(handoff
            .to_string()
            .contains("external browser launch requested"));
        assert!(
            !backend.is_alive(),
            "an external launch never creates an embedded window"
        );
        assert!(
            matches!(backend.drain_events().as_slice(),[BackendEvent::Error(message)] if message==&handoff.to_string())
        );
        assert!(backend.drain_events().is_empty());
        assert!(backend
            .navigate_with(&url, |_| anyhow::bail!("launcher missing"))
            .unwrap_err()
            .to_string()
            .contains("launcher missing"));
        assert!(
            matches!(backend.drain_events().as_slice(),[BackendEvent::Error(message)] if message.contains("Could not launch"))
        );
        assert!(backend.go_back().is_err());
        assert!(backend.show().is_err());
        assert!(backend.hide().is_err());
        assert!(backend.reposition(geometry).is_err());
        assert!(
            backend.drain_events().is_empty(),
            "unsupported positioning does not emit per-frame errors"
        );
        for blocked in ["file:///private", "https://127.0.0.1/private"] {
            assert!(backend
                .navigate_with(&Url::parse(blocked).unwrap(), |_| panic!(
                    "blocked URL reached launcher"
                ))
                .is_err());
        }
        backend.destroy();
        assert!(backend.drain_events().is_empty());
        assert!(backend
            .navigate_with(&url, |_| panic!("closed fallback reached launcher"))
            .is_err());
    }
}
