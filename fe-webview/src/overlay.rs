use fe_database::{PetalId, RoleId};

#[derive(Debug, Clone)]
pub struct WebViewConfig {
    pub petal_id: PetalId,
    pub initial_url: Option<url::Url>,
    pub role: RoleId,
}

pub trait BrowserSurface: Send + Sync {
    fn show(&mut self, url: url::Url) -> anyhow::Result<()>;
    fn hide(&mut self) -> anyhow::Result<()>;
    fn navigate(&mut self, url: url::Url) -> anyhow::Result<()>;
    fn position(&mut self, x: f32, y: f32, width: f32, height: f32) -> anyhow::Result<()>;
}
