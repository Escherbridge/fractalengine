# fe-hexon-registry — hosted hexon registry service

HTTP service that hosts `.hexon` packages for remote search/fetch/publish, per
`docs/hexon-format-spec.md` §API Endpoints. Library-first: `build_router(state)`
lets tests (and embedders) drive the router in-process; `main.rs` is a thin
env-config + serve wrapper. Containerized via `docker/Dockerfile.hexon-registry`
(mirrors `Dockerfile.relay`).

## §design

### Lazy zip indexing

`HexonArchive::import` eagerly loads **all** blobs into memory, which is
unacceptable for multi-hundred-MB tilesets. The registry never uses it.
Instead (`src/index.rs`):

- Startup / reindex reads only `manifest.json` per package via the zip
  central directory (`zip::ZipArchive::by_name` decompresses just that entry).
- The in-memory index holds metadata only (`HexonIndexEntry`); file bytes stay
  on disk.
- `entries.json` and `assets/{hash}` blobs are read lazily per-request on a
  `spawn_blocking` thread; `download` streams the file via `ReaderStream`
  without loading it.
- Manifest/entries endpoints return the **raw** JSON from the zip (parsed as
  `serde_json::Value`), so unknown/future fields pass through untouched —
  the index extraction also ignores unknown fields by construction.
- Scan is non-recursive over `*.hexon` in `FE_REGISTRY_DIR`; unreadable
  packages are skipped with a warning, never fatal. Publish triggers a full
  rescan (simple + always consistent with the directory).

### URI resolution

`{uri}` = `hexon_id` or `hexon_id@version` (axum percent-decodes path
segments, so `%40` works). Unversioned URIs resolve to the highest version:
SemVer ordering when both parse, valid SemVer beats invalid, lexicographic
fallback otherwise.

### Env config

All config is env-driven for container parity with the relay:
`FE_REGISTRY_DIR` (default `/data/hexons`), `FE_REGISTRY_BIND`
(default `0.0.0.0:8790`), `FE_REGISTRY_TOKEN` (optional),
`FE_REGISTRY_READONLY` (`1`/`true`/`yes`). No config files.

### Auth model

Single optional shared bearer token. When `FE_REGISTRY_TOKEN` is set, **all**
`/api/v1` routes require `Authorization: Bearer <token>` (401 otherwise);
`/health` stays public for container health checks. When unset the registry is
fully open and logs a startup warning — intended for local dev/test only.
Publish is additionally gated by `FE_REGISTRY_READONLY` (403) and rejects
duplicate `id@version` (409). This is deliberately not the spec's RBAC table
(Viewer+/Owner+) — that belongs to the full engine API; this service is a dev
fixture host.

### Publish validation

Body is raw `.hexon` bytes. Validation = open as zip, parse `manifest.json`,
require `hexon_id`/`version`/`hexon_type`/`publisher_did`, and restrict
`hexon_id`/`version` to the spec charset (`[A-Za-z0-9_.-]`, plus `+` for
semver build metadata) so the derived filename `{id}@{version}.hexon` can
never traverse paths.

## §routes

| Method | Path | Notes |
|--------|------|-------|
| GET | `/health` | public; ok + indexed count |
| GET | `/api/v1/hexons/search?q=&tags=&type=` | q substring (id/name/description, case-insensitive); tags comma-list AND; type exact |
| GET | `/api/v1/hexons/{uri}` | raw manifest.json |
| GET | `/api/v1/hexons/{uri}/entries` | raw entries.json |
| GET | `/api/v1/hexons/{uri}/entries/{entry_id}/asset` | blob via entry `asset_hash` → `assets/{hash}` |
| GET | `/api/v1/hexons/{uri}/download` | whole file, `application/x-hexon+zip` |
| POST | `/api/v1/hexons/publish` | raw bytes body (≤1 GiB), writes + reindexes |

JSON endpoints use the fe-api `ApiResponse` envelope (`{ok, data, error}`);
binary endpoints return raw bytes.

## §client

`fe-hexon/src/remote.rs` (cargo feature `remote`) is the consuming client:
`RemoteRegistryClient::new(base_url, token)` with `search`/`manifest`/
`download`. Integration test `fe-hexon/tests/remote_registry_test.rs` spins
this router on an ephemeral port — no container needed.
