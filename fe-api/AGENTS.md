# fe-api — API gateway (REST + WS + MCP)

Runs on its own multi-thread tokio runtime (`spawn_api_thread`), talking to the
Bevy/DB threads over `crossbeam::channel` (`ApiCommand` -> `DbCommand` ->
`DbResult`, matched via `tokio::sync::oneshot` per request — see
`fe-runtime/src/app.rs` `drain_api_commands` + `PendingApiRequests`, read-only
from here). Where a `db_reader: Option<Arc<Surreal<Db>>>` is configured,
handlers may bypass that round-trip entirely with a direct SurrealDB query
(`direct_*` helpers in `rest.rs`/`assets.rs`) — this is the established escape
hatch for reads that don't have (or don't need) a dedicated `DbCommand`.

## §assets

Three GET endpoints, all under the JWT-authenticated router in `server.rs`:

| Method | Path | Notes |
|--------|------|-------|
| GET | `/api/v1/assets/{content_hash}` | raw blob by BLAKE3 hash, `application/octet-stream`, immutable cache headers. No RBAC scope check (content hash carries no ownership) — pre-existing endpoint, unchanged. |
| GET | `/api/v1/assets/by-id/{asset_id}` | resolves `asset` row -> blob, serves with the **real** `content_type`/`name`/`size` from the DB. RBAC scope resolved via the first `node` referencing that `asset_id`. |
| GET | `/api/v1/nodes/{node_id}/asset` | resolves `node.asset_id` -> `asset` row -> blob. RBAC scope resolved from the node's parent chain (same `resolve_node_scope` helper as `/nodes/:id/transform`). |

Both new endpoints require Viewer+ role and require the node's/asset's scope
be covered by the token, require a **valid ULID** path param (reuses
`types::is_valid_ulid` — length + charset check, no separate validator), and
never build a filesystem path from user input: the blob path always comes
from `BlobStore::get_blob_path(hash)`, keyed by the DB's `content_hash`
column, never from the request. Every error path returns a structured JSON
body (`{"ok": false, "error": "..."}`) with a real HTTP status
(400/403/404/500/502/503/501/413) and a `tracing` log line — this deliberately
differs from the rest of `rest.rs`, where most handlers return HTTP 200 with
`{"ok": false, ...}` for historical reasons; asset delivery is byte-stream
territory, so real status codes matter for HTTP caches/CDNs/downloaders.

**Asset scope resolution caveat**: the `asset` table has no scope/owner column
of its own — only `node.asset_id` links an asset into the hierarchy, and
nothing stops two nodes (even in different petals) from pointing at the same
`asset_id`. `by-id` lookups authorize against whichever node happens to be
returned first by `SELECT petal_id FROM node WHERE asset_id = $aid LIMIT 1`.
This is fine for the common case (one asset, one importing node) but is not a
real multi-owner model; if assets need independent RBAC, that's a schema
change in `fe-database` (out of scope here — see integration requests below).

**Directory-asset extension point**: the asset model is heading toward "any
file, or a directory of files behind a placeholder" (the P2P bucket / "3D
visual IPFS" idea). Today `serve_asset_by_id` special-cases a sentinel
`content_type` of `application/x-fe-directory` and responds `501 Not
Implemented` with a structured JSON body instead of guessing at bytes; when
directory assets are real, that branch is where a manifest/listing response
slots in (e.g. `GET .../asset` returning a file listing + per-entry sub-URLs
when `content_type` is the directory sentinel, falling through to today's
single-blob behavior otherwise) — no route or RBAC changes needed, only that
one branch grows.

**Integration requests** (would require edits outside `fe-api/**`, so left as
requests rather than done here):
- `fractalengine/src/main.rs` currently passes `blob_store: None` into
  `fe_api::ApiConfig` (a separate `FsBlobStore` handle is only wired to Bevy,
  not the API thread) — so on the actual GUI binary today, every asset
  endpoint (old and new) hits the `blob_store` `None` branch and returns 503,
  even though `db_reader` is wired and asset *metadata* queries would
  succeed. Wiring `blob_store: Some(blob_store.clone())` in `main.rs` (the
  handle is already `Clone`) closes this gap.
- A `DbCommand::GetNodeAsset { node_id }` / `DbCommand::GetAssetMeta
  { asset_id }` variant (returning name/content_type/size_bytes/content_hash)
  would let these endpoints work over the crossbeam channel when no
  `db_reader` is configured (e.g. a future relay-only deployment). Today they
  return 503 in that case, same as `blob_store` being absent.
