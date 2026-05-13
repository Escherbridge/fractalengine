//! P2P announcement and discovery layer for hexon crates.
//!
//! This module defines the data types and local stores used for DHT-based
//! hexon crate announcement and peer discovery. The actual network wiring
//! (libp2p Kademlia publish/subscribe) lives in the network layer.

pub mod announce;
pub mod config;
pub mod discover;
pub mod fetch;

pub use announce::{AnnouncementStore, CrateInventory, HexonAnnouncement};
pub use config::{P2pConfig, PeerCandidate, PeerPriority};
pub use discover::{HexonSearchResult, SearchQuery, search_announcements};
pub use fetch::{FetchError, FetchManifestResult, FetchStrategy, verify_fetched_manifest};
