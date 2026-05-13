#[cfg(feature = "fetch")]
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[cfg(feature = "fetch")]
use std::sync::Arc;

const DEFAULT_USER_AGENT: &str = "FractalEngine/0.1 terrain-tile-fetcher";
const DEFAULT_MAX_CONCURRENT_FETCHES: usize = 8;

/// Slippy map tile coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileCoord {
    pub x: u32,
    pub y: u32,
    pub zoom: u8,
}

impl TileCoord {
    pub fn new(x: u32, y: u32, zoom: u8) -> Self {
        Self { x, y, zoom }
    }

    /// Cache-friendly key string: `{zoom}/{x}/{y}`.
    pub fn cache_key(&self) -> String {
        format!("{}/{}/{}", self.zoom, self.x, self.y)
    }

    /// Number of tiles per axis at this zoom level.
    pub fn tiles_at_zoom(zoom: u8) -> u32 {
        1u32 << zoom
    }

    /// Convert lat/lon (WGS84) to tile coordinates at a given zoom level.
    pub fn from_lat_lon(lat: f64, lon: f64, zoom: u8) -> Self {
        let n = Self::tiles_at_zoom(zoom) as f64;
        let x = ((lon + 180.0) / 360.0 * n).floor() as u32;
        let lat_rad = lat.to_radians();
        let y = ((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor() as u32;
        Self {
            x: x.min(n as u32 - 1),
            y: y.min(n as u32 - 1),
            zoom,
        }
    }

    /// Convert tile coordinate back to the north-west corner lat/lon.
    pub fn to_lat_lon(&self) -> (f64, f64) {
        let n = Self::tiles_at_zoom(self.zoom) as f64;
        let lon = self.x as f64 / n * 360.0 - 180.0;
        let lat = (std::f64::consts::PI * (1.0 - 2.0 * self.y as f64 / n))
            .sinh()
            .atan()
            .to_degrees();
        (lat, lon)
    }
}

/// Trait for fetching tile image data from a tile server.
#[cfg(feature = "fetch")]
#[async_trait::async_trait]
pub trait TileSource: Send + Sync {
    /// Fetch a single tile as raw PNG/image bytes.
    async fn fetch_tile(&self, coord: TileCoord) -> Result<Vec<u8>>;

    /// Source identifier for cache keying.
    fn source_id(&self) -> &str;
}

/// XYZ tile source (standard slippy map: `{z}/{x}/{y}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XyzTileSource {
    pub url_template: String,
    pub source_id: String,
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_fetches: usize,
    #[serde(skip)]
    #[cfg(feature = "fetch")]
    pub(crate) client: Option<Arc<reqwest::Client>>,
    #[serde(skip)]
    #[cfg(feature = "fetch")]
    pub(crate) semaphore: Option<Arc<tokio::sync::Semaphore>>,
}

fn default_user_agent() -> String {
    DEFAULT_USER_AGENT.to_string()
}
fn default_max_concurrent() -> usize {
    DEFAULT_MAX_CONCURRENT_FETCHES
}

#[cfg(feature = "fetch")]
impl XyzTileSource {
    pub fn new(url_template: String, source_id: String) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .build()
            .ok()
            .map(Arc::new);
        Self {
            url_template,
            source_id,
            user_agent: default_user_agent(),
            max_concurrent_fetches: DEFAULT_MAX_CONCURRENT_FETCHES,
            client,
            semaphore: Some(Arc::new(tokio::sync::Semaphore::new(
                DEFAULT_MAX_CONCURRENT_FETCHES,
            ))),
        }
    }

    fn get_client(&self) -> Arc<reqwest::Client> {
        self.client.clone().unwrap_or_else(|| {
            Arc::new(
                reqwest::Client::builder()
                    .user_agent(&self.user_agent)
                    .build()
                    .expect("failed to build reqwest client"),
            )
        })
    }

    fn get_semaphore(&self) -> Arc<tokio::sync::Semaphore> {
        self.semaphore.clone().unwrap_or_else(|| {
            Arc::new(tokio::sync::Semaphore::new(self.max_concurrent_fetches))
        })
    }
}

#[cfg(feature = "fetch")]
#[async_trait::async_trait]
impl TileSource for XyzTileSource {
    async fn fetch_tile(&self, coord: TileCoord) -> Result<Vec<u8>> {
        let url = self
            .url_template
            .replace("{z}", &coord.zoom.to_string())
            .replace("{x}", &coord.x.to_string())
            .replace("{y}", &coord.y.to_string());

        let _permit = self.get_semaphore().acquire_owned().await
            .map_err(|_| anyhow::anyhow!("semaphore closed"))?;
        let resp = self.get_client().get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("tile fetch failed: HTTP {}", resp.status());
        }
        Ok(resp.bytes().await?.to_vec())
    }

    fn source_id(&self) -> &str {
        &self.source_id
    }
}

/// TMS tile source (Y-axis flipped: `y = 2^zoom - 1 - y`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TmsTileSource {
    pub url_template: String,
    pub source_id: String,
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_fetches: usize,
    #[serde(skip)]
    #[cfg(feature = "fetch")]
    pub(crate) client: Option<Arc<reqwest::Client>>,
    #[serde(skip)]
    #[cfg(feature = "fetch")]
    pub(crate) semaphore: Option<Arc<tokio::sync::Semaphore>>,
}

#[cfg(feature = "fetch")]
impl TmsTileSource {
    pub fn new(url_template: String, source_id: String) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .build()
            .ok()
            .map(Arc::new);
        Self {
            url_template,
            source_id,
            user_agent: default_user_agent(),
            max_concurrent_fetches: DEFAULT_MAX_CONCURRENT_FETCHES,
            client,
            semaphore: Some(Arc::new(tokio::sync::Semaphore::new(
                DEFAULT_MAX_CONCURRENT_FETCHES,
            ))),
        }
    }

    fn get_client(&self) -> Arc<reqwest::Client> {
        self.client.clone().unwrap_or_else(|| {
            Arc::new(
                reqwest::Client::builder()
                    .user_agent(&self.user_agent)
                    .build()
                    .expect("failed to build reqwest client"),
            )
        })
    }

    fn get_semaphore(&self) -> Arc<tokio::sync::Semaphore> {
        self.semaphore.clone().unwrap_or_else(|| {
            Arc::new(tokio::sync::Semaphore::new(self.max_concurrent_fetches))
        })
    }
}

#[cfg(feature = "fetch")]
#[async_trait::async_trait]
impl TileSource for TmsTileSource {
    async fn fetch_tile(&self, coord: TileCoord) -> Result<Vec<u8>> {
        let flipped_y = TileCoord::tiles_at_zoom(coord.zoom) - 1 - coord.y;
        let url = self
            .url_template
            .replace("{z}", &coord.zoom.to_string())
            .replace("{x}", &coord.x.to_string())
            .replace("{y}", &flipped_y.to_string());

        let _permit = self.get_semaphore().acquire_owned().await
            .map_err(|_| anyhow::anyhow!("semaphore closed"))?;
        let resp = self.get_client().get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("tile fetch failed: HTTP {}", resp.status());
        }
        Ok(resp.bytes().await?.to_vec())
    }

    fn source_id(&self) -> &str {
        &self.source_id
    }
}
