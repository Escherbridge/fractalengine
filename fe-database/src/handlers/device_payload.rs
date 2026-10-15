//! R5 device format descriptor + device payload write path (CDL-SPEC10 A3) —
//! see `src/AGENTS.md` §device-payload. Mirrors `handlers::iot_reading`'s
//! shape: durable-first, then one `ReplicationEvent` per accepted row.

use fe_query::{Filter, InsertBuilder, QueryBuilder, QueryValue};
use fe_sdk::device_format::{is_valid_mime, DeviceFormatDescriptor};
use fe_sdk::source_class::{is_valid_format_id, SourceClass};

use fe_runtime::messages::{
    DeviceFormatDto, DeviceFormatRejection, DevicePayloadDto, DevicePayloadIngestRejection,
    DevicePayloadInput, DEVICE_PAYLOAD_MAX_ASSETS,
};

use crate::canon_log::observation::{payload_door_digest, HotPayload};
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
    register_format_with_replication(db, verse_id, descriptor, declared_by, None, None).await
}

/// [`register_format`], then — durable first — publish the NEW row to
/// `verse_id`'s replica keyed by `format_id` (the inbound
/// `apply_device_format_row` key). An idempotent re-declare emits nothing.
pub async fn register_format_with_replication(
    db: &Db,
    verse_id: &str,
    descriptor: &DeviceFormatDescriptor,
    declared_by: &str,
    blob_store: Option<&BlobStoreHandle>,
    repl_tx: Option<&ReplicationSender>,
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
    if !is_valid_mime(&descriptor.mime) {
        return Err(DeviceFormatRejection::InvalidMime {
            mime: descriptor.mime.clone(),
        }
        .into());
    }
    // `schema` doc comment (fe-sdk): "informative JSON Schema" — always an
    // object when present; never enforced further in v1 (critic tick 8, AC-49:
    // a non-object schema fell through to an untyped DB-layer rejection).
    if descriptor
        .schema
        .as_ref()
        .is_some_and(|schema| !schema.is_object())
    {
        return Err(DeviceFormatRejection::InvalidSchema.into());
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

    // `schema` is `option<object>`: an absent schema must be omitted, not null.
    let row = crate::repo::strip_json_nulls(serde_json::json!({
        "format_id": descriptor.format_id,
        "verse_id": verse_id,
        "mime": descriptor.mime,
        "schema": descriptor.schema,
        "declared_by": declared_by,
        "created_at": chrono::Utc::now().to_rfc3339(),
    }));
    db.query("CREATE device_format CONTENT $row")
        .bind(("row", row.clone()))
        .await
        .map_err(|e| DeviceFormatError::Db(e.to_string()))?
        .check()
        .map_err(|e| DeviceFormatError::Db(e.to_string()))?;

    // Durable first, then publish (§device-payload: `device_format` replicates
    // insert-if-absent; without this emit a peer never learns a declared format).
    if let Some(store) = blob_store {
        match serde_json::to_vec(&row) {
            Ok(bytes) => crate::replicate_row(
                repl_tx,
                store,
                verse_id,
                "device_format",
                &descriptor.format_id,
                &bytes,
            ),
            Err(e) => tracing::warn!(
                format_id = %descriptor.format_id,
                verse_id,
                "device format declared but not serialised for replication: {e}"
            ),
        }
    } else if repl_tx.is_some() {
        tracing::warn!(
            verse_id,
            "device format declared: replication sender wired but blob store missing — not published"
        );
    }
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
    // Idempotent retry (§device-payload "retry is same-petal, same-principal"):
    // a client `payload_id` must be a ULID, and a known one is a retry ONLY
    // when the existing row is in this petal AND was ingested by this same
    // principal. Any other holder of the id gets a conflict that names
    // nothing about the existing row — returning its `asset_ids` would let a
    // caller in petal P read petal Q's asset ids by replaying an id (IDOR).
    if let Some(payload_id) = payload.payload_id.as_deref() {
        if !is_valid_payload_id(payload_id) {
            return Err(DevicePayloadIngestRejection::InvalidPayloadId {
                raw: payload_id.to_string(),
            }
            .into());
        }
        match fetch_payload(db, petal_id, payload_id)
            .await
            .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?
        {
            Some(existing) if existing.source_did == source_did => {
                return Ok((existing.payload_id, existing.asset_ids));
            }
            Some(_) => return Err(DevicePayloadIngestRejection::PayloadIdConflict.into()),
            None => {
                if payload_id_taken(db, payload_id)
                    .await
                    .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?
                {
                    return Err(DevicePayloadIngestRejection::PayloadIdConflict.into());
                }
            }
        }
    }

    if payload.asset_ids.is_empty() {
        return Err(DevicePayloadIngestRejection::EmptyAssetList.into());
    }
    if payload.asset_ids.len() > DEVICE_PAYLOAD_MAX_ASSETS {
        return Err(DevicePayloadIngestRejection::TooManyAssets {
            max: DEVICE_PAYLOAD_MAX_ASSETS,
        }
        .into());
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
    // The caller's replication `verse_id` names the replica the row is
    // published into; it must be the petal's OWN verse, or a petal's payload
    // (and its asset rows) would be published into a foreign verse's replica.
    if let Some(claimed) = verse_id {
        if owning_verse.as_deref() != Some(claimed) {
            return Err(DevicePayloadIngestError::Db(format!(
                "verse_id '{claimed}' does not own petal '{petal_id}'"
            )));
        }
    }
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

    // `verse_id` is `option<string>`: a `None` must be absent, never JSON null.
    let row = crate::repo::strip_json_nulls(serde_json::json!({
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
    }));
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
    let door_digest = asset_content_hashes_of(&payload.asset_ids, &assets).map_or_else(
        // An asset with no content hash can never be sealed (§8 rule 3); a digest that is not
        // 64 hex digits never matches, so the member stays unbound.
        || "unresolved-asset-content-hash".to_string(),
        |hashes| {
            payload_door_digest(
                &HotPayload {
                    payload_id: payload_id.clone(),
                    node_id: payload.node_id.clone(),
                    petal_id: petal_id.to_string(),
                    format_id: payload.format_id.clone(),
                    recorded_at: recorded.to_rfc3339(),
                    recorded_at_ms: recorded.timestamp_millis(),
                    hlc_timestamp: hlc_packed as i64,
                    asset_ids: payload.asset_ids.clone(),
                    source_did: source_did.to_string(),
                },
                &hashes,
            )
        },
    );
    record_window_membership(
        db,
        "device_payload",
        &payload_id,
        petal_id,
        source_class,
        hlc_wall_ms,
        hlc_counter,
        &door_digest,
    )
    .await
    .map_err(|e| DevicePayloadIngestError::Db(e.to_string()))?;

    Ok((payload_id, payload.asset_ids.clone()))
}

/// Whether a client-supplied `payload_id` is a canonical ULID (26 Crockford
/// characters that decode — the same id shape the server mints).
pub fn is_valid_payload_id(payload_id: &str) -> bool {
    payload_id.len() == 26 && ulid::Ulid::from_string(payload_id).is_ok()
}

/// The `device_payload` row for `payload_id` IN `petal_id`, or `None` —
/// petal-scoped so no caller can read another petal's row through it.
async fn fetch_payload(
    db: &Db,
    petal_id: &str,
    payload_id: &str,
) -> anyhow::Result<Option<DevicePayloadDto>> {
    let q = QueryBuilder::new()
        .select(&["*"])
        .from("device_payload")
        .filter(Filter::eq("payload_id", payload_id))
        .and(Filter::eq("petal_id", petal_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(rows.into_iter().next().map(payload_dto_from_row))
}

/// Whether ANY `device_payload` row holds `payload_id` — a bare yes/no for
/// the conflict decision, never a row.
async fn payload_id_taken(db: &Db, payload_id: &str) -> anyhow::Result<bool> {
    let q = QueryBuilder::new()
        .select(&["payload_id"])
        .from("device_payload")
        .filter(Filter::eq("payload_id", payload_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(!rows.is_empty())
}

/// Whether `payload_id` names a `device_payload` row anchored to `petal_id`.
/// `pub(crate)` so the reading door and the inbound `iot_reading` gate share
/// this one petal-scoped link check.
pub(crate) async fn payload_in_petal(
    db: &Db,
    petal_id: &str,
    payload_id: &str,
) -> anyhow::Result<bool> {
    let q = QueryBuilder::new()
        .select(&["payload_id"])
        .from("device_payload")
        .filter(Filter::eq("payload_id", payload_id))
        .and(Filter::eq("petal_id", petal_id))
        .build();
    let mut res = exec_query(db, &q).await?;
    let rows: Vec<serde_json::Value> = res.take(0)?;
    Ok(!rows.is_empty())
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

/// Whether `asset_id` exists with exactly `content_type` (SPEC-10 §2.2 rule 5's
/// MIME match) — `pub(crate)` for the inbound gate, like [`asset_anchored_to_petal`].
pub(crate) async fn asset_has_content_type(
    db: &Db,
    asset_id: &str,
    content_type: &str,
) -> anyhow::Result<bool> {
    let q = QueryBuilder::new()
        .select(&["asset_id"])
        .from("asset")
        .filter(Filter::eq("asset_id", asset_id))
        .and(Filter::eq("content_type", content_type))
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

/// Each listed asset's BLAKE3 content hash, in list order, as the door validated it; `None`
/// when any asset row carries no 32-byte hex `content_hash`.
fn asset_content_hashes_of(
    asset_ids: &[String],
    assets: &std::collections::HashMap<String, AssetRow>,
) -> Option<Vec<[u8; 32]>> {
    asset_ids
        .iter()
        .map(|asset_id| {
            let text = assets
                .get(asset_id)?
                .row_json
                .get("content_hash")?
                .as_str()?;
            hex::decode(text).ok()?.try_into().ok()
        })
        .collect()
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
/// `door_digest` binds the member to the exact accepted row (`canon_log/AGENTS.md`
/// §observation, door digest).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_window_membership(
    db: &Db,
    table: &str,
    row_id: &str,
    petal_id: &str,
    source_class: SourceClass,
    hlc_wall_ms: i64,
    hlc_counter: i64,
    door_digest: &str,
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
         hlc_wall_ms: $hlc_wall_ms, hlc_counter: $hlc_counter, \
         door_digest: $door_digest }",
    )
    .bind(("id", id))
    .bind(("table", table.to_string()))
    .bind(("row_id", row_id.to_string()))
    .bind(("petal_id", petal_id.to_string()))
    .bind(("source_class", source_class.as_str().to_string()))
    .bind(("window_bucket", window_bucket))
    .bind(("hlc_wall_ms", hlc_wall_ms))
    .bind(("hlc_counter", hlc_counter))
    .bind(("door_digest", door_digest.to_string()))
    .await?
    .check()
    .map_err(|e| anyhow::anyhow!("observation_window_member CREATE failed: {e}"))?;
    Ok(())
}
