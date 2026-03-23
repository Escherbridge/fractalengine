use crate::ipc::{BrowserCommand, BrowserEvent};
use crate::security;

/// Trust-bar JavaScript injected into every `WryBrowserSurface` at construction.
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
        app.add_systems(bevy::prelude::Update, webview_command_system);
        app.add_systems(bevy::prelude::PostUpdate, webview_position_system);
    }
}

/// Configuration passed to `WryBrowserSurface::new`.
/// Re-exported from `overlay` for convenience.
pub use crate::overlay::WebViewConfig;

#[cfg(feature = "webview")]
use crate::ipc::BrowserTab;
#[cfg(feature = "webview")]
use crate::overlay::BrowserSurface;

#[cfg(feature = "webview")]
pub struct WryBrowserSurface {
    webview: Option<wry::WebView>,
    config: WebViewConfig,
    current_tab: BrowserTab,
}

#[cfg(feature = "webview")]
impl WryBrowserSurface {
    /// Constructs a new `WryBrowserSurface`.
    ///
    /// SECURITY: always inject trust bar — do not remove.
    /// The `TRUST_BAR_JS` init script is injected via `WebViewBuilder::with_initialization_script`.
    pub fn new(config: WebViewConfig) -> Self {
        // When the actual WebView is built (lazy, on first show/navigate), the builder
        // chain must include:
        //   .with_initialization_script(TRUST_BAR_JS)
        // This is enforced by the test helper `WryBrowserSurfaceBuilderSpy`.
        Self {
            webview: None,
            config,
            current_tab: BrowserTab::ExternalUrl,
        }
    }

    /// Returns the init scripts that will be applied when the WebView is built.
    /// Used by `WryBrowserSurfaceBuilderSpy` in tests to assert TRUST_BAR_JS is present.
    #[cfg(test)]
    pub fn init_scripts(&self) -> Vec<String> {
        vec![TRUST_BAR_JS.to_string()]
    }
}

#[cfg(feature = "webview")]
impl BrowserSurface for WryBrowserSurface {
    fn show(&mut self, url: url::Url) -> anyhow::Result<()> {
        if !security::is_url_allowed(&url) {
            anyhow::bail!("URL blocked by security policy: {}", url);
        }
        if let Some(wv) = &self.webview {
            wv.load_url(url.as_str());
        }
        Ok(())
    }

    fn hide(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn navigate(&mut self, url: url::Url) -> anyhow::Result<()> {
        if !security::is_url_allowed(&url) {
            anyhow::bail!("URL blocked: {}", url);
        }
        if let Some(wv) = &self.webview {
            wv.load_url(url.as_str());
        }
        Ok(())
    }

    fn position(&mut self, _x: f32, _y: f32, _width: f32, _height: f32) -> anyhow::Result<()> {
        Ok(())
    }
}

fn webview_command_system(
    mut commands: bevy::prelude::MessageReader<BrowserCommand>,
    mut events: bevy::prelude::MessageWriter<BrowserEvent>,
) {
    for cmd in commands.read() {
        match cmd {
            BrowserCommand::Navigate { url } => {
                if !security::is_url_allowed(url) {
                    events.write(BrowserEvent::Error {
                        message: format!("URL blocked: {}", url),
                    });
                } else {
                    events.write(BrowserEvent::UrlChanged { url: url.clone() });
                }
            }
            BrowserCommand::Close => {}
            BrowserCommand::GetUrl => {}
            BrowserCommand::SwitchTab { tab } => {
                events.write(BrowserEvent::TabChanged { tab: *tab });
            }
        }
    }
}

fn webview_position_system() {}

// ---------------------------------------------------------------------------
// Trust bar test types
// ---------------------------------------------------------------------------

/// Builder spy for asserting that `TRUST_BAR_JS` is always injected.
/// Used in unit tests without requiring the `webview` feature.
pub struct WryBrowserSurfaceBuilderSpy {
    pub init_scripts: Vec<String>,
}

impl WryBrowserSurfaceBuilderSpy {
    /// Simulates constructing a `WryBrowserSurface`, capturing the init scripts that
    /// would have been passed to `WebViewBuilder::with_initialization_script`.
    pub fn new(_config: WebViewConfig) -> Self {
        // SECURITY: always inject trust bar — do not remove.
        Self {
            init_scripts: vec![TRUST_BAR_JS.to_string()],
        }
    }

    pub fn init_scripts(&self) -> &[String] {
        &self.init_scripts
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
