//! R5 device format descriptor + device payload write path (CDL-SPEC10 A3) —
//! see `src/AGENTS.md` §device-payload. Mirrors `handlers::iot_reading`'s
//! shape: durable-first, then one `ReplicationEvent` per accepted row.

use fe_query::{Filter, InsertBuilder, QueryBuilder, QueryValue};
use fe_sdk::device_format::DeviceFormatDescriptor;
use fe_sdk::source_class::{is_valid_format_id, SourceClass};

use fe_runtime::messages::{
    DeviceFormatDto, DeviceFormatRejection, DevicePayloadDto, DevicePayloadIngestRejection,
    DevicePayloadInput,
};

use crate::op_log::next_hlc_timestamp;
use crate::query_helpers::exec_query;
use crate::repo::Db;
use crate::{BlobStoreHandle, ReplicationSender};

/// Ingestion-HLC window width that buckets `observation_window_member` rows
/// (SPEC-10 §3.4 rule 2: "one half-open interval of ingestion HLC wall time").
/// CDL-SPEC10 B3 confirmed the one-minute width and moved the bucket from
/// sensor time to ingestion HLC, so a late sensor timestamp can never land in
/// an already-sealed window (`canon_log/AGENTS.md` §observation).
pub const OBSERVATION_WINDOW_BUCKET_MS: i64 = 60_000;

// ---------------------------------------------------------------------------
// Device format registration
// ---------------------------------------------------------------------------

/// Typed registration failures (F24 pattern — mirrors `IotIngestError`).
#[derive(Debug, thiserror::Error)]
pub enum DeviceFormatError {
    #[error("{0}")]
    Rejected(#[from] DeviceFormatRejection),
    #[error("device_format write failed: {0}")]
    Db(String),
}

impl DeviceFormatError {
    /// The typed 422-class detail when this is a validation failure, `None`
    /// for DB failures — see `IotIngestError::validation_rejection`.
    pub fn rejection(&self) -> Option<DeviceFormatRejection> {
        match self {
            Self::Rejected(r) => Some(r.clone()),
            Self::Db(_) => None,
        }
    }
}

/// Declare a device payload format for `verse_id`. Role enforcement
/// (Manager+) is the caller's job (REST/MCP, or the inbound replication
/// gate) — this function only enforces the data-level invariants: a
/// well-formed `format_id`/`mime`, and immutability per `(verse_id,
/// format_id)` (a conflicting `mime` is refused; re-declaring the SAME mime
/// is an idempotent no-op).
pub async fn register_format(
    db: &Db,
    verse_id: &str,
    descriptor: &DeviceFormatDescriptor,
    declared_by: &str,
) -> Result<(), DeviceFormatError> {
    if !is_valid_format_id(&descriptor.format_id) {
        return Err(DeviceFormatRejection::InvalidFormatId {
            format_id: descriptor.format_id.clone(),
        }
        .into());
    }
    if descriptor.mime.trim().is_empty() {
        return Err(DeviceFormatRejection::EmptyMime.into());
    }

    if let Some(existing) = fetch_format(db, verse_id, &descriptor.format_id)
        .await
        .map_err(|e| DeviceFormatError::Db(e.to_string()))?
    {
        if existing.mime != descriptor.mime {
            return Err(DeviceFormatRejection::ConflictingMime {
                format_id: descriptor.format_id.clone(),
                existing_mime: existing.mime,
            }
            .into());
        }
        return Ok(()); // same mime re-declared: idempotent no-op.
    }

    db.query(
        "CREATE device_format CONTENT { \
         format_id: $fid, verse_id: $vid, mime: $mime, schema: $schema, \
         declared_by: $by, created_at: $now }",
    )
    .bind(("fid", descriptor.format_id.clone()))
    .bind(("vid", verse_id.to_string()))
    .bind(("mime", descriptor.mime.clone()))
    .bind(("schema", descriptor.schema.clone()))
    .bind(("by", declared_by.to_string()))
    .bind(("now", chrono::Utc::now().to_rfc3339()))
    .await
    .map_err(|e| DeviceFormatError::Db(e.to_string()))?
    .check()
    .map_err(|e| DeviceFormatError::Db(e.to_string()))?;
    Ok(())
}

/// The declared format for `(verse_id, format_id)`, or `None`.
pub async fn fetch_format(
    db: &Db,
    verse_id: &str,
    format_id: &str,
) -> anyhow::Result<Option<DeviceFormatDto>> {
    let q = QueryBuilder::new()
        .select(&["*"])
        .from("device_format")
        .filter(Filter::eq("verse_id", verse_id))
        .and(Filter::eq("format_id", format_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows.into_iter().next().map(format_dto_from_row))
}

/// Every `device_format` declared for `verse_id`.
pub async fn list_device_formats(db: &Db, verse_id: &str) -> anyhow::Result<Vec<DeviceFormatDto>> {
    let q = QueryBuilder::new()
        .select(&["*"])
        .from("device_format")
        .filter(Filter::eq("verse_id", verse_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows.into_iter().map(format_dto_from_row).collect())
}

fn format_dto_from_row(row: serde_json::Value) -> DeviceFormatDto {
    let text = |key: &str| {
        row.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    DeviceFormatDto {
        format_id: text("format_id"),
        verse_id: text("verse_id"),
        mime: text("mime"),
        schema: row.get("schema").cloned().filter(|v| !v.is_null()),
        declared_by: text("declared_by"),
        created_at: text("created_at"),
    }
}

// ---------------------------------------------------------------------------
// Device payload ingest
// ---------------------------------------------------------------------------

/// Typed ingestion failures (F24 pattern — mirrors `IotIngestError`).
#[derive(Debug, thiserror::Error)]
pub enum DevicePayloadIngestError {
    #[error("{0}")]
    Rejected(#[from] DevicePayloadIngestRejection),
    #[error("device_payload write failed: {0}")]
    Db(String),
}

impl DevicePayloadIngestError {
    pub fn rejection(&self) -> Option<DevicePayloadIngestRejection> {
        match self {
            Self::Rejected(r) => Some(r.clone()),
            Self::Db(_) => None,
        }
    }
}

/// One asset row's fields needed for validation + replication re-emit.
struct AssetRow {
    content_type: String,
    petal_id: Option<String>,
    row_json: serde_json::Value,
}

/// Ingest one device payload, mirroring `insert_readings_with_replication`:
/// durable-first, then one `ReplicationEvent` per accepted row (the
/// referenced `asset` rows first, then the `device_payload` row — SPEC-10
/// §2.3), then a local-only `observation_window_member` row (SPEC-10 §3.4
/// rule 5). Returns `(payload_id, asset_ids)`; a known client `payload_id`
/// is an idempotent no-op that returns the existing row unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn insert_device_payloads(
    db: &Db,
    petal_id: &str,
    verse_id: Option<&str>,
    source_did: &str,
    source_class: SourceClass,
    payload: &DevicePayloadInput,
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
) -> Result<(String, Vec<String>), DevicePayloadIngestError> {
    // Idempotent retry: a known client `payload_id` returns the existing row
    // untouched, before any other validation re-runs.
    if let Some(payload_id) = payload.payload_id.as_deref() {
        if let Some(existing) = fetch_payload(db, payload_id)
            .await
            .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?
        {
            return Ok((existing.payload_id, existing.asset_ids));
        }
    }

    if payload.asset_ids.is_empty() {
        return Err(DevicePayloadIngestRejection::EmptyAssetList.into());
    }

    // The anchor must be an existing node of this petal (scope integrity,
    // same rule as `insert_readings_with_replication`).
    if !node_is_in_petal(db, petal_id, &payload.node_id)
        .await
        .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?
    {
        return Err(DevicePayloadIngestRejection::UnknownAnchor {
            node_id: payload.node_id.clone(),
        }
        .into());
    }

    // The format must be declared for this petal's OWN verse (resolved
    // locally, never trusting the caller's replication-only `verse_id`
    // parameter — that one may legitimately be `None`).
    let owning_verse = owning_verse_id(db, petal_id)
        .await
        .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?;
    let descriptor = match &owning_verse {
        Some(v) => fetch_format(db, v, &payload.format_id)
            .await
            .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?,
        None => None,
    };
    let Some(descriptor) = descriptor else {
        return Err(DevicePayloadIngestRejection::UnknownFormat {
            format_id: payload.format_id.clone(),
        }
        .into());
    };

    // Every asset must exist, be anchored to THIS petal, and carry the
    // descriptor's exact `mime`.
    let assets = fetch_assets(db, &payload.asset_ids)
        .await
        .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?;
    for asset_id in &payload.asset_ids {
        let Some(asset) = assets.get(asset_id) else {
            return Err(DevicePayloadIngestRejection::UnknownAsset {
                asset_id: asset_id.clone(),
            }
            .into());
        };
        if asset.petal_id.as_deref() != Some(petal_id) {
            return Err(DevicePayloadIngestRejection::ForeignAssetAnchor {
                asset_id: asset_id.clone(),
            }
            .into());
        }
        if asset.content_type != descriptor.mime {
            return Err(DevicePayloadIngestRejection::MimeMismatch {
                asset_id: asset_id.clone(),
            }
            .into());
        }
    }

    let recorded = match &payload.recorded_at {
        Some(raw) => chrono::DateTime::parse_from_rfc3339(raw)
            .map_err(|_| DevicePayloadIngestRejection::InvalidTimestamp { raw: raw.clone() })?
            .with_timezone(&chrono::Utc),
        None => chrono::Utc::now(),
    };
    let (hlc_packed, _hlc_str) = next_hlc_timestamp();
    let payload_id = payload
        .payload_id
        .clone()
        .unwrap_or_else(|| ulid::Ulid::new().to_string());

    let row = serde_json::json!({
        "payload_id": payload_id.clone(),
        "node_id": payload.node_id,
        "petal_id": petal_id,
        "verse_id": verse_id,
        "format_id": payload.format_id,
        "recorded_at": recorded.to_rfc3339(),
        "recorded_at_ms": recorded.timestamp_millis(),
        "hlc_timestamp": hlc_packed as i64,
        "asset_ids": payload.asset_ids,
        "source_did": source_did,
    });
    let q = InsertBuilder::insert_into("device_payload")
        .values(row.clone())
        .build();
    exec_query(db, &q)
        .await
        .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?;

    // Durable first, then publish: referenced asset rows first, then the
    // payload row itself (SPEC-10 §2.3 ingestion-story order).
    if let (Some(verse_id), Some(store)) = (verse_id, blob_store) {
        for asset_id in &payload.asset_ids {
            if let Some(asset) = assets.get(asset_id) {
                crate::replicate_row_with_petal(
                    repl_tx,
                    store,
                    verse_id,
                    "asset",
                    asset_id,
                    &serde_json::to_vec(&asset.row_json).unwrap_or_default(),
                    asset.petal_id.clone(),
                );
            }
        }
        match serde_json::to_vec(&row) {
            Ok(bytes) => crate::replicate_row_with_petal(
                repl_tx,
                store,
                verse_id,
                "device_payload",
                &payload_id,
                &bytes,
                Some(petal_id.to_string()),
            ),
            Err(e) => tracing::warn!(
                payload_id,
                petal_id,
                "device payload accepted but not serialised for replication: {e}"
            ),
        }
    } else if repl_tx.is_some() {
        tracing::warn!(
            petal_id,
            verse_id = ?verse_id,
            "device payload ingest: replication sender wired but verse_id or blob store missing — payload will not be published"
        );
    }

    // SPEC-10 §3.4 rule 5 (A1 hand-back item 3): record window membership at
    // the door, after the durable insert, for THIS locally-accepted payload
    // only. Never gated on replication being wired — B3's sealer needs this
    // regardless of whether the payload was published.
    let (hlc_wall_ms, hlc_counter) = unpack_hlc(hlc_packed);
    record_window_membership(
        db,
        "device_payload",
        &payload_id,
        petal_id,
        source_class,
        hlc_wall_ms,
        hlc_counter,
    )
    .await
    .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?;

    Ok((payload_id, payload.asset_ids.clone()))
}

/// The `device_payload` row for `payload_id`, or `None`.
async fn fetch_payload(db: &Db, payload_id: &str) -> anyhow::Result<Option<DevicePayloadDto>> {
    let q = QueryBuilder::new()
        .select(&["*"])
        .from("device_payload")
        .filter(Filter::eq("payload_id", payload_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows.into_iter().next().map(payload_dto_from_row))
}

/// List `device_payload` rows for `petal_id`, optionally filtered by anchor
/// node, format and a `[from_ms, to_ms]` recorded-time window.
pub async fn list_device_payloads(
    db: &Db,
    petal_id: &str,
    node_id: Option<&str>,
    format_id: Option<&str>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
) -> anyhow::Result<Vec<DevicePayloadDto>> {
    let mut q = QueryBuilder::new()
        .select(&["*"])
        .from("device_payload")
        .filter(Filter::eq("petal_id", petal_id));
    if let Some(node_id) = node_id {
        q = q.and(Filter::eq("node_id", node_id));
    }
    if let Some(format_id) = format_id {
        q = q.and(Filter::eq("format_id", format_id));
    }
    if let Some(from_ms) = from_ms {
        q = q.and(Filter::gte("recorded_at_ms", from_ms));
    }
    if let Some(to_ms) = to_ms {
        q = q.and(Filter::lte("recorded_at_ms", to_ms));
    }
    let mut res = exec_query(db, &q.build()).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows.into_iter().map(payload_dto_from_row).collect())
}

fn payload_dto_from_row(row: serde_json::Value) -> DevicePayloadDto {
    let text = |key: &str| {
        row.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    DevicePayloadDto {
        payload_id: text("payload_id"),
        node_id: text("node_id"),
        petal_id: text("petal_id"),
        verse_id: row
            .get("verse_id")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        format_id: text("format_id"),
        recorded_at: text("recorded_at"),
        recorded_at_ms: row
            .get("recorded_at_ms")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        asset_ids: row
            .get("asset_ids")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        source_did: text("source_did"),
    }
}

/// Whether `node_id` is an existing node of `petal_id`. `pub(crate)` so the
/// inbound replicated-row referential-confinement check
/// (`handlers::replicated_row`) shares this exact query rather than a second
/// hand-rolled one ("shared cores + separate guards = drift", RUNBOOK §5).
pub(crate) async fn node_is_in_petal(
    db: &Db,
    petal_id: &str,
    node_id: &str,
) -> anyhow::Result<bool> {
    let q = QueryBuilder::new()
        .select(&["node_id"])
        .from("node")
        .filter(Filter::eq("petal_id", petal_id))
        .and(Filter::eq("node_id", node_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(!rows.is_empty())
}

/// Whether `asset_id` exists and is anchored to `petal_id`. `pub(crate)` for
/// the same reason as [`node_is_in_petal`].
pub(crate) async fn asset_anchored_to_petal(
    db: &Db,
    asset_id: &str,
    petal_id: &str,
) -> anyhow::Result<bool> {
    let q = QueryBuilder::new()
        .select(&["asset_id"])
        .from("asset")
        .filter(Filter::eq("asset_id", asset_id))
        .and(Filter::eq("petal_id", petal_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(!rows.is_empty())
}

/// The verse that owns `petal_id`, walking petal -> fractal -> verse.
pub(crate) async fn owning_verse_id(db: &Db, petal_id: &str) -> anyhow::Result<Option<String>> {
    let scope = crate::handlers::crud::resolve_petal_scope_handler(db, petal_id).await?;
    Ok(scope.and_then(|s| crate::scope::parse_scope(&s).ok().map(|p| p.verse_id)))
}

/// Fetch every row of `ids` from `asset`, keyed by `asset_id`.
async fn fetch_assets(
    db: &Db,
    ids: &[String],
) -> anyhow::Result<std::collections::HashMap<String, AssetRow>> {
    if ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let q = QueryBuilder::new()
        .select(&["*"])
        .from("asset")
        .filter(Filter::is_in(
            "asset_id",
            QueryValue::Array(ids.iter().map(|s| QueryValue::from(s.clone())).collect()),
        ))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let asset_id = row.get("asset_id")?.as_str()?.to_string();
            let content_type = row.get("content_type")?.as_str().unwrap_or("").to_string();
            let petal_id = row
                .get("petal_id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            Some((
                asset_id,
                AssetRow {
                    content_type,
                    petal_id,
                    row_json: row,
                },
            ))
        })
        .collect())
}

/// Unpack a `next_hlc_timestamp` packed value into `(wall_ms, counter)` as
/// signed ints (matching `op_log::init_hlc`'s own unpacking).
fn unpack_hlc(packed: u64) -> (i64, i64) {
    ((packed >> 16) as i64, (packed & 0xFFFF) as i64)
}

/// SPEC-10 §3.4 rule 5: record that `row_id` of `table` was accepted at this
/// node's own ingest door, in `petal_id`'s window for `hlc_wall_ms`'s bucket.
/// `pub(crate)` so `handlers::iot_reading` (the other local door) shares it —
/// ONLY these two doors may ever call it; the inbound replication path
/// (`handlers::replicated_row`) must never reach this function, under any
/// input, including a replicated row whose `source_did` names this node
/// itself (the signing-oracle gap this table exists to close).
///
/// Gated on the SPEC-8 mode (SPEC-10 §8 rule 2): in `legacy_only` and
/// `seam_ready` it records nothing, so the ingest path stays byte-identical.
pub(crate) async fn record_window_membership(
    db: &Db,
    table: &str,
    row_id: &str,
    petal_id: &str,
    source_class: SourceClass,
    hlc_wall_ms: i64,
    hlc_counter: i64,
) -> anyhow::Result<()> {
    if !crate::canon_log::observation::door_records_window_membership() {
        return Ok(());
    }
    let window_bucket = crate::canon_log::observation::window_bucket_of(hlc_wall_ms);
    let id = ulid::Ulid::new().to_string();
    db.query(
        "CREATE observation_window_member CONTENT { \
         id: $id, table: $table, row_id: $row_id, petal_id: $petal_id, \
         source_class: $source_class, window_bucket: $window_bucket, \
         hlc_wall_ms: $hlc_wall_ms, hlc_counter: $hlc_counter }",
    )
    .bind(("id", id))
    .bind(("table", table.to_string()))
    .bind(("row_id", row_id.to_string()))
    .bind(("petal_id", petal_id.to_string()))
    .bind(("source_class", source_class.as_str().to_string()))
    .bind(("window_bucket", window_bucket))
    .bind(("hlc_wall_ms", hlc_wall_ms))
    .bind(("hlc_counter", hlc_counter))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("observation_window_member CREATE failed: {e}"))?;
    Ok(())
}
