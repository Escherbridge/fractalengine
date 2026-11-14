//! Per-key (HLC, author) clock; see field_clock/AGENTS.md.
// Seams are consumed by CLOCK-05/06/07/08/10; drop this allow once every item is wired.
#![allow(dead_code, unused_imports)]

pub(crate) mod judge;
pub(crate) mod merge;
pub(crate) mod migrate;
pub(crate) mod publish;
pub(crate) mod rejudge;
pub(crate) mod store;
pub(crate) mod wire;

pub(crate) use judge::{admissible, VerseView};
pub(crate) use merge::{upsert_own, winner};
pub use migrate::seed_legacy_floor;
pub(crate) use publish::publish_clocked_row;
pub(crate) use rejudge::rejudge_authors;
pub(crate) use store::{
    author_frontier, clock_write_sql, load_rows_by_authors, load_versions, versions_write_sql,
};
pub(crate) use wire::{apply_keys, encode_own, keys_of, parse_own, FieldVersion, CLOCK_COLUMN};
