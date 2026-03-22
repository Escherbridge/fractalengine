#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GossipMessage<T> {
    pub payload: T,
    pub sig: Vec<u8>,
    pub pub_key: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AssetId(pub [u8; 32]);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PetalTopic(pub String);

#[derive(Debug, Clone)]
pub struct IrohConfig {
    pub relay_url: Option<url::Url>,
    pub max_concurrent_transfers: usize,
}

impl Default for IrohConfig {
    fn default() -> Self {
        Self {
            relay_url: None,
            max_concurrent_transfers: 8,
        }
    }
}
