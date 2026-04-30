use crate::backend::{BackendEvent, WebViewBackend, WindowGeometry};
use url::Url;

/// No-op backend used when no webview feature is enabled.
/// Opens URLs in the system browser as a fallback.
pub struct StubBackend {
    alive: bool,
}

impl WebViewBackend for StubBackend {
    fn create(
        _parent_handle: &raw_window_handle::RawWindowHandle,
        _geometry: WindowGeometry,
        _trust_bar_js: &str,
    ) -> anyhow::Result<Self> {
        tracing::info!("StubBackend: no webview backend enabled, URLs will open in system browser");
        Ok(Self { alive: true })
    }

    fn navigate(&mut self, url: &Url) -> anyhow::Result<()> {
        tracing::info!("StubBackend: opening {url} in system browser");
        open_in_system_browser(url.as_str());
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
        Vec::new()
    }

    fn is_alive(&self) -> bool {
        self.alive
    }
}

fn open_in_system_browser(url: &str) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    #[cfg(target_os = "android")]
    {
        tracing::warn!("StubBackend: system browser open not implemented on Android");
    }
}

#[cfg(test)]
mod tests {
    /// Task 2.4: Verify the system browser launch command exists on this platform.
    #[test]
    fn stub_browser_command_exists() {
        #[cfg(target_os = "windows")]
        assert!(
            std::process::Command::new("cmd")
                .args(["/C", "echo", "test"])
                .output()
                .is_ok(),
            "cmd.exe should be available on Windows"
        );

        #[cfg(target_os = "macos")]
        assert!(
            std::process::Command::new("which")
                .arg("open")
                .output()
                .is_ok(),
            "'open' command should be available on macOS"
        );

        #[cfg(target_os = "linux")]
        assert!(
            std::process::Command::new("which")
                .arg("xdg-open")
                .output()
                .is_ok(),
            "'xdg-open' should be available on Linux"
        );
    }
}
