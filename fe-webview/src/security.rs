//! Shared production URL policy; see fe-runtime/src/AGENTS.md section portal URLs.

pub use fe_runtime::portal_url::{
    embedded_allowed, embedded_preview_allowed, is_url_allowed, BLOCKED_HOSTS, BLOCKED_RANGES,
};
