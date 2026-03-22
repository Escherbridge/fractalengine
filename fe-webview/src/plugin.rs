use crate::ipc::{BrowserCommand, BrowserEvent, BrowserTab};
use crate::overlay::{BrowserSurface, WebViewConfig};
use crate::security;

#[allow(dead_code)]
const TRUST_BAR_JS: &str = r#"
    const bar = document.createElement('div');
    bar.style.cssText = 'position:fixed;top:0;left:0;right:0;height:24px;background:#1a1a2e;color:#8888cc;font-family:sans-serif;font-size:11px;padding:4px 8px;z-index:2147483647;pointer-events:none;';
    bar.textContent = '🌐 External Website: ' + location.hostname;
    document.body.prepend(bar);
"#;

pub struct WebViewPlugin;

impl bevy::prelude::Plugin for WebViewPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_event::<BrowserCommand>();
        app.add_event::<BrowserEvent>();
        app.add_systems(bevy::prelude::Update, webview_command_system);
        app.add_systems(bevy::prelude::PostUpdate, webview_position_system);
    }
}

#[cfg(feature = "webview")]
pub struct WryBrowserSurface {
    webview: Option<wry::WebView>,
    config: WebViewConfig,
    current_tab: BrowserTab,
}

#[cfg(feature = "webview")]
impl WryBrowserSurface {
    pub fn new(config: WebViewConfig) -> Self {
        Self {
            webview: None,
            config,
            current_tab: BrowserTab::ExternalUrl,
        }
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
    mut commands: bevy::prelude::EventReader<BrowserCommand>,
    mut events: bevy::prelude::EventWriter<BrowserEvent>,
) {
    for cmd in commands.read() {
        match cmd {
            BrowserCommand::Navigate { url } => {
                if !security::is_url_allowed(url) {
                    events.send(BrowserEvent::Error {
                        message: format!("URL blocked: {}", url),
                    });
                } else {
                    events.send(BrowserEvent::UrlChanged { url: url.clone() });
                }
            }
            BrowserCommand::Close => {}
            BrowserCommand::GetUrl => {}
            BrowserCommand::SwitchTab { tab } => {
                events.send(BrowserEvent::TabChanged { tab: *tab });
            }
        }
    }
}

fn webview_position_system() {}
