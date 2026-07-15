//! Query/egress cost limits (FR-4 delta, plan D4) — see `fe-api/AGENTS.md` §limits.

/// Max rows a `/api/v1/query` JSON response may return.
pub const QUERY_ROW_CAP: usize = 10_000;
/// Max serialized size of a `/api/v1/query` JSON `data` payload (8 MiB).
pub const QUERY_MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Human label for the `/query` byte ceiling in error messages.
pub const QUERY_MAX_RESPONSE_LABEL: &str = "8 MiB";

/// Max rows an export (`export.parquet` / `export.csv` / shared redemption) may return.
pub const EXPORT_ROW_CAP: usize = 500_000;
/// Max byte size of a produced export body (128 MiB).
pub const EXPORT_MAX_BYTES: usize = 128 * 1024 * 1024;
/// Human label for the export byte ceiling in error messages.
pub const EXPORT_MAX_BYTES_LABEL: &str = "128 MiB";

/// Per-DID `/query` rate limit (requests per second) — pre-existing value, now named.
pub const QUERY_RATE_PER_SEC: u32 = 10;
/// Per-DID export-endpoint rate limit (requests per second).
pub const EXPORT_RATE_PER_SEC: u32 = 10;
/// Per-token shared-URL redemption rate limit (requests per second).
pub const SHARE_REDEEM_RATE_PER_SEC: u32 = 10;

/// Max readings per IoT ingestion batch (iot_spatial_reporting plan D4).
pub const IOT_INGEST_MAX_READINGS: usize = 1_000;
/// Per-DID IoT ingestion rate limit (requests per second).
pub const IOT_INGEST_RATE_PER_SEC: u32 = 10;

/// Default shareable-URL TTL (1 hour) — plan D2.
pub const SHARE_DEFAULT_TTL_SECS: u64 = 3600;
/// Maximum shareable-URL TTL (24 hours) — plan D2.
pub const SHARE_MAX_TTL_SECS: u64 = 24 * 3600;
