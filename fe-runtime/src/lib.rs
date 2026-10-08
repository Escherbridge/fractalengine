pub mod app;
pub mod bevy_blob_reader;
pub mod blob_store;
pub mod channels;
pub mod diag15m; // DIAG-15M: temporary render diagnostics
pub mod distributed_query;
pub mod messages;
pub mod peer_registry;
pub mod shared_node;
pub mod timeseries;
pub mod wiring;
pub use channels::{ApiChannels, CHANNEL_BUFFER, TRANSFORM_BROADCAST_BUFFER};
pub use messages::EntityType;
pub use peer_registry::{PeerEntry, PeerRegistry};
pub use shared_node::{validate_asset_path, PropertyValue, SharedNode, WebViewInteraction};
pub use timeseries::{TimeseriesMode, VerseTimeseriesSettings, DEFAULT_BUCKET_WIDTH_MS};
pub use wiring::EngineConfig;
