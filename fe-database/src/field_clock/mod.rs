//! Per-key (HLC, author) clock; see field_clock/AGENTS.md.

pub(crate) mod judge;
pub(crate) mod merge;
pub(crate) mod migrate;
pub(crate) mod publish;
pub(crate) mod rejudge;
pub(crate) mod store;
pub(crate) mod wire;

pub(crate) use merge::{upsert_own, winner};
#[allow(unused_imports)] // Only a replicated_row test reaches it here; lib.rs calls migrate:: directly.
pub use migrate::seed_legacy_floor;
pub(crate) use publish::publish_clocked_row;
pub(crate) use store::{author_frontier, clock_write_sql, load_versions, versions_write_sql};
pub(crate) use wire::{apply_keys, encode_own, keys_of, parse_own, FieldVersion, CLOCK_COLUMN};
