pub mod app;
pub mod bevy_blob_reader;
pub mod blob_store;
pub mod channels;
pub mod diag15m; // DIAG-15M: temporary render diagnostics
pub mod messages;
pub mod peer_registry;
pub mod shared_node;
pub mod wiring;
pub use channels::{ApiChannels, CHANNEL_BUFFER, TRANSFORM_BROADCAST_BUFFER};
pub use messages::EntityType;
pub use peer_registry::{PeerEntry, PeerRegistry};
pub use shared_node::{validate_asset_path, PropertyValue, SharedNode, WebViewInteraction};
pub use wiring::EngineConfig;
