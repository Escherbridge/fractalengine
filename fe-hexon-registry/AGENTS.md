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
- **(PAS-P5 S3.)** `HexonIndexEntry` also carries `sealed: bool` and
  `layout: fe_format::Layout`, both read the same cheap, manifest/central-
  directory-only way as everything else in `index_one` — `sealed` is a
  presence check on `manifest.integrity`, `layout` reads `chunks/index.json`'s
  bundle count when present (`Monolithic` otherwise). Neither is a
  cryptographic verification; that happens at publish (§Publish validation)
  and again, per read, in the `/meta`, `/chunk`, `/member` handlers via
  `fe_format::HexonArchiveIndex` — the same **lazy** header-phase reader S2
  built, never `HexonArchive::import` (still eager, still never used here).

### Decompression limits (REG-DECOMP, 2026-10-11; PAS-P5 S3)

Every zip read is bounded (`src/limits.rs`; security audit 2026-07-31
decompression-bomb finding). Per entry: `.json` names ≤ 8 MiB, everything
else ≤ 64 MiB; ≤ 1 GiB total per archive; ≤ 200 000 entries. An entry is
refused when its **declared** size exceeds the limit (before any
allocation) and when its **actual** output does (`take(limit + 1)`), so a
lying header cannot inflate past the cap either.

- **Publish** (`index::validate_package`): reads `manifest.json` capped, then
  streams EVERY entry into a sink under the same per-entry caps and the
  shared 1 GiB budget. A bomb is a 400 and is never written to disk, so it
  cannot be stored and blow up a later GET. `verify_archive(RequireSealed)`
  runs next (see §Publish validation) and independently re-walks every
  member under the same caps for a sealed archive, so a bomb smuggled past
  `validate_package` (there isn't one today) would still be caught there.
- **GET** (`read_zip_bytes`, also used by startup/reindex manifest reads):
  capped per entry. A package placed on disk by other means that trips a cap
  answers 500 with the limit message (the asset route no longer reports it
  as a 404 "blob missing"); `scan_dir` still indexes it when its manifest is
  small.
- `/meta`, `/chunk/{seq}` and `/member/{*path}` are built from
  `fe_format::HexonArchiveIndex` (the lazy header-phase reader), which
  applies `fe_format::limits` directly — no second guard set.
- The **constants** (`MAX_*_BYTES`, `read_entry_capped`, `check_entry_count`)
  are re-exported straight from `fe_format::limits` as of S3 — the reason
  they were duplicated (fe-format pulling in bevy via fe-runtime) is gone
  since fe-format's `runtime` feature became opt-in (PAS-P5 S1). Only
  `entry_cap` (extension → cap) and `drain_entry_capped` (decompress into a
  sink, used by `validate_package`'s whole-archive sweep) stay local —
  registry-specific helpers fe-format has no reason to carry. `cargo tree -p
  fe-hexon-registry -e normal | grep -c bevy` must stay `0`.

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

Body is raw `.hexon` bytes. Validation is two gates, in order:

1. `index::validate_package`: open as zip, bounded-decompress every entry (see
   Decompression limits), parse `manifest.json`, require
   `hexon_id`/`version`/`hexon_type`/`publisher_did`, and restrict
   `hexon_id`/`version` to the spec charset (`[A-Za-z0-9_.-]`, plus `+` for
   semver build metadata) so the derived filename `{id}@{version}.hexon` can
   never traverse paths.
2. **(PAS-P5 S3, D-75/D-76.)** `fe_format::verify_archive(bytes,
   IntegrityPolicy::RequireSealed { pin: None })`: the archive must be sealed
   (signed, every header/body member's blake3 digest matching, every
   `assets/{hash}` member self-certifying against its filename) or the
   publish is a 400. This is what lets the registry trust `publisher_did`
   for the next check — it is self-certifying (names a key, not a
   trustworthy one), so only a passing signature makes it safe to use.
   **Publisher continuity (AP-3.4):** if `hexon_id` is already indexed under
   a *different* (verified) `publisher_did`, the publish is a 409 — one key
   cannot take over another's id, even under a brand-new version. Combined
   with the existing same-`id@version` 409, both duplicate-publish and
   id-takeover land on the same status code with different messages.
3. **(S8, 2026-10-14.)** The duplicate/continuity check, the write and the
   reindex run under `RegistryState::publish_lock`, and the file is created
   with `create_new` (an `AlreadyExists` is a 409). Before S8 the check read
   the index, released it, and wrote later, so N publishers racing on one
   new `hexon_id` could all pass and the last `fs::write` silently replaced
   the winner's file. Test: `concurrent_publishes_of_one_id_admit_exactly_one_publisher`.

### Security headers on publisher-controlled bytes (S3 critic amendment)

`/chunk/{seq}` and `/member/{*path}` always answer with
`Content-Type: application/octet-stream`, `X-Content-Type-Options: nosniff`
and `Content-Disposition: attachment` (`routes::octet_response`), regardless
of what the member's own name or content looks like — the same stored-XSS
rule CDL-SPEC10 A4 applies to fe-api blobs. The `filename` inside the
disposition is the member's last path segment reduced to `[A-Za-z0-9._-]`
(anything else becomes `_`; S8), so a publisher-chosen name with quotes or
`;` cannot reshape the header (test:
`a_hostile_member_name_cannot_shape_the_content_disposition_header`). A member whose zip path happens
to end in `.html` is still served as a forced download, never rendered.
`/member` resolves `path` only by **membership lookup** against the zip's own
central directory (`HexonArchiveIndex::contains`/`read_member`) — it is never
joined onto a filesystem path, so a request such as `../../etc/passwd`
simply fails to match any member and 404s; traversal is impossible by
construction, not by charset-filtering the input.

## §routes

| Method | Path | Notes |
|--------|------|-------|
| GET | `/health` | public; ok + indexed count |
| GET | `/api/v1/hexons/search?q=&tags=&type=` | q substring (id/name/description, case-insensitive); tags comma-list AND; type exact |
| GET | `/api/v1/hexons/{uri}` | raw manifest.json |
| GET | `/api/v1/hexons/{uri}/meta` | `HeaderDoc` (header phase only — no tile/body bytes; `fe_format::HexonArchiveIndex`) |
| GET | `/api/v1/hexons/{uri}/entries` | raw entries.json |
| GET | `/api/v1/hexons/{uri}/entries/{entry_id}/asset` | blob via entry `asset_hash` → `assets/{hash}` |
| GET | `/api/v1/hexons/{uri}/chunk/{seq}` | one container chunk's raw bytes; monolithic archives serve the whole file at `seq=0`, 404 otherwise; octet-stream + nosniff + attachment |
| GET | `/api/v1/hexons/{uri}/member/{*path}` | one raw member, membership-checked against the central directory; octet-stream + nosniff + attachment |
| GET | `/api/v1/hexons/{uri}/download` | whole file, `application/x-hexon+zip` |
| POST | `/api/v1/hexons/publish` | raw bytes body (≤1 GiB), decompression-capped + sealed + publisher-continuity checked (§Publish validation), writes + reindexes |

JSON endpoints use the fe-api `ApiResponse` envelope (`{ok, data, error}`);
binary endpoints return raw bytes.

## §client

`fe-hexon/src/remote.rs` (cargo feature `remote`) is the consuming client:
`RemoteRegistryClient::new(base_url, token)` with `search`/`manifest`/
`download`. Integration test `fe-hexon/tests/remote_registry_test.rs` spins
this router on an ephemeral port — no container needed.
It also implements `fe_hexon::partial::PartialHexonFetch` over `/meta`,
`/chunk/{seq}` and `/member/{*path}` (fe-hexon/src/AGENTS.md §partial-fetch);
`fe-hexon/tests/partial_fetch_http.rs` runs the conformance suite against it.
