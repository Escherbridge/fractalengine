# FractalEngine Relay

Headless relay server for FractalEngine. Runs without GPU, windowing, WebView, or OS keychain — designed for Linux/Docker/ARM server deployment.

Thin clients (web browsers, mobile apps) connect via the HTTP/WebSocket API to interact with the scene graph, manage entities, and stream transforms in real time.

## Quick Start

```bash
cargo build --release -p fractalengine-relay
./target/release/fe-relay
```

The relay listens on `0.0.0.0:8765` by default.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FE_BIND_ADDR` | `0.0.0.0:8765` | Listen address (host:port) |
| `FE_DB_PATH` | `data/fractalengine.db` | SurrealDB storage path |
| `FE_P2P_DIR` | `data/p2p` | iroh-docs/blobs data dir (persistent replica store) |
| `FE_CORS_ORIGINS` | `*` | Comma-separated allowed CORS origins |
| `FE_SYNC_RELAY` | `default` | iroh relay mode: `default` / `disabled` / comma-separated relay URLs. `default` = n0's public iroh v1 relays (shared, rate-limited — configure your own for production; see [Relays](#relays-iroh-1x)) |
| `FE_SYNC_BOOTSTRAP` | (none) | Semicolon-separated iroh 1.x `EndpointAddr` JSON entries to dial for every opened replica; the legacy iroh 0.35 `NodeAddr` JSON shape is still accepted (invalid entries and bare ids warn + skip) |
| `FE_SHUTDOWN_AFTER_SECS` | (never) | Request the same graceful shutdown a signal would, after N seconds (smoke tests, one-shot containers) |
| `SURREAL_DATASTORE_SYNC_DATA` | `every` | SurrealKV fsync cadence (`never` / `every` / duration >100ms) |
| `RUST_LOG` | (none) | Log level filter (`info`, `debug`, `fe_api=debug`, etc.) |
| `FE_SIM_ALLOW` | (unset) | `sim-control` builds only: `1` spawns the sim lab bridge (`/api/v1/sim/*`). A sim session overrides the process-wide HLC clock — set only on a dedicated lab relay serving no production verses. Unset → the bridge is not spawned (503) |
| `FE_API_ELEVATED_ALLOW` | (unset) | `1` enables `POST /api/v1/query/elevated` (raw SurrealQL reads/writes, Owner tokens only). It is NOT scope-confined — an Owner of any verse reaches every verse on the node. Scripts run on the DB thread: a write is atomic with one op-log entry, then broadcast and replicated; every DML statement carries a DB-side `TIMEOUT 5s`. Enable only on a single-tenant node; startup logs a WARN when on. Unset → every call is refused (fe-api/AGENTS.md §elevated-query) |
| `FE_API_UPLOAD_QUOTA_BYTES` | `2147483648` (2 GiB) | Cap on registered API-uploaded asset bytes per `asset.owner_did` (today every token's `sub` is the node DID, so effectively node-wide); over quota → 413. Unparseable → default with a WARN (fe-api/AGENTS.md §asset-ingest) |
| `FE_ASSET_GC_INTERVAL_SECS` | `3600` | Seconds between unreferenced-asset GC passes; `0` disables (fe-runtime/src/AGENTS.md §asset-gc) |
| `FE_ASSET_GC_GRACE_SECS` | `86400` | Minimum age of an asset row no node references before GC deletes it and its unshared blob |
| `FE_SIM_WORK_DIR` | `<tmp>/fe-relay-sim` | `sim-control` builds only: scratch root for sim session peers |

### Secret injection

The relay uses `EnvBackend` for secrets. Keys are mapped to env vars as `FE_SECRET_{SERVICE}_{ACCOUNT}` (uppercased, special chars replaced with `_`).

- Node keypair: `FE_SECRET_FRACTALENGINE_NODE_KEYPAIR`. If unset, the relay generates an ephemeral keypair on startup (new iroh node id every run).
- Share-URL signing key (A24): `FE_SECRET_FRACTALENGINE_SHARE_SIGNER`. If unset, a new key generates each launch and every previously issued shareable query URL stops verifying — a dedicated slot, independent of the node keypair (full ops guidance is a later pass).
- Verse namespace secrets: `FE_SECRET_FRACTALENGINE_VERSE_<VERSE_ULID>_NS_SECRET_FRACTALENGINE` = 64 hex chars. A verse's P2P replica opens only when its secret is available from this mapping — without one, startup logs a loud warning and skips that verse. The namespace id is derived from the secret (its Ed25519 verifying key — the id iroh-docs registers the doc under), so no row update is needed.

  **Process-lifetime secrets for REST-created verses.** `POST /api/v1/verses` generates the verse's namespace secret into the relay's `EnvBackend` **in-memory override only** — it is never written to a real environment variable or persisted anywhere. After a relay restart, that override is gone: the verse's replica can never re-open unless you env-inject the slot above (`FE_SECRET_FRACTALENGINE_VERSE_<ULID>_NS_SECRET_FRACTALENGINE`, 64 hex) yourself before the next launch. The GUI binary is unaffected (it persists secrets to the OS keystore). A proper invite-generation-over-REST/MCP flow with real secret persistence is deferred — tracked on the conductor `p2p_mycelium_completion_20260701` track (phase 2 notes) — not scheduled in this mission.

## P2P Replication

The relay is a fully applying replica of every verse it hosts:

- **Startup scan** — after the DB thread is up, the relay requests the verse hierarchy and opens a replica for each verse whose namespace secret resolves (see above).
- **Runtime** — every `VerseCreated` opens its replica the same way.
- **Reconciliation** — each open replays the replica's current doc entries through the same inbound apply path, so rows denied before a role or the verse manifest converged locally get a second chance.
- **Inbound admission** — the fe-policy role gate at the DB layer applies (deny-by-default, Editor+, roles resolved from local tables, never wire-supplied).

To join an existing swarm, copy the dialable `EndpointAddr` JSON from the peer's startup log (`SyncStatus updated: started — dialable address above`) into `FE_SYNC_BOOTSTRAP`. Note: two SurrealKV connections cannot coexist in one process, so the relay's direct API reader falls back to the command channel whenever the DB writer holds the store — that is normal, not an error.

```bash
# iroh 1.x EndpointAddr JSON (what the start banner prints):
FE_SYNC_BOOTSTRAP='{"id":"<64-hex endpoint id>","addrs":[{"Relay":"https://relay.example.com/"},{"Ip":"203.0.113.7:41234"}]}'
# Legacy iroh 0.35 NodeAddr JSON is still parsed (converted to the above):
FE_SYNC_BOOTSTRAP='{"node_id":"<64-hex>","relay_url":"https://relay.example.com/","direct_addresses":["203.0.113.7:41234"]}'
```

With a relay configured, startup waits up to iroh's `NET_REPORT_TIMEOUT` for a home relay so the printed address carries its relay URL; under `FE_SYNC_RELAY=disabled` it prints the local IP addresses immediately.

### Relays (iroh 1.x)

- **Flag day.** The P2P stack runs iroh 1.3 (iroh-docs/iroh-gossip 0.101, iroh-blobs 0.103). iroh 0.35 and 1.x peers **cannot talk to each other** — upgrade every GUI peer and relay node of a swarm together. Within iroh v1, every minor version interoperates.
- **Default relays.** `FE_SYNC_RELAY=default` uses n0's public v1 relays (`{use1,usw1,euc1,aps1}-1.relay.n0.iroh.link`), which n0 runs for the life of iroh v1 but which are shared and rate-limited for relayed traffic. The relay logs one `WARN` at startup when it binds against them.
- **Production.** Run your own iroh relay server (the `iroh-relay` 1.x binary from the `iroh-relay` crate, feature `server`) and set `FE_SYNC_RELAY=https://<your-relay-host>`. Note that `fe-relay` (this binary) is an application peer, **not** an iroh relay server.
- **Health.** `SyncStatus.health` starts `Unknown` when a relay is configured and moves to `Healthy` once a home relay connects; a home-relay watcher degrades it (`Degraded` → `Unreachable`) on repeated connection errors or an auth denial, and recovers it on reconnect (fe-sync/src/AGENTS.md §relay-health).
- **Upgrading an existing `FE_P2P_DIR`.** The first 1.x start copies the dir to `<FE_P2P_DIR>.v035.bak-<unix_ts>`, upgrades its two redb files in place, and writes a `.fe-store-format` marker. Budget twice the dir's disk space for that start. If the migration fails, the node runs offline and the backup is your rollback (fe-sync/src/AGENTS.md §store-migration).

### Windows / per-handle-lock platforms

SurrealKV takes a per-handle file lock, so only one in-process connection to
the store is possible. On Windows the DB thread's writer connection holds
that lock, which leaves the API's direct read connection (`db_reader`) `None`:

- **REST IoT ingest is unaffected.** `POST /api/v1/petals/{p}/iot/readings`
  transparently falls back to the DB-thread command seam when `db_reader` is
  absent and is fully functional on this platform (writes land through the
  same `insert_readings_with_replication` path, so replication fires
  identically either way).
- **The merged analytics table is unavailable while the DB thread's writer
  lives.** `POST /api/v1/analytics/query` resolves its authorization through
  the direct reader by design (no channel fallback — see
  `fe-api/AGENTS.md` §analytics-query), so on this platform it honestly
  answers "analytics authorization unavailable (no direct DB reader)" rather
  than serving a result. This is deliberate fail-closed behavior, not a bug
  to route around.

## Health Endpoints

| Endpoint | Auth | Description |
|----------|------|-------------|
| `GET /api/v1/health` | None | Liveness probe — always returns `200 {"status":"ok"}` |
| `GET /ready` | None | Readiness probe — DB ping round-trip; `200` when responsive, `503` otherwise |

## API Surface

The relay exposes the same API as the GUI binary:

- **REST** — CRUD for verses, fractals, petals, nodes, transforms
- **WebSocket** (`/ws`) — real-time transform streaming, scene subscriptions
- **MCP** (`POST /mcp`) — machine-callable protocol for AI agents
- **Assets** (`GET /api/v1/assets/:hash`) — content-addressed blob delivery with immutable caching (shared blob store, same as the GUI)

All authenticated endpoints require a Bearer JWT. Mint tokens via `MintApiToken` DB command or MCP tool.

## Simulation Lab (sim-control builds)

For scripted, deterministic multi-peer P2P scenarios (synthetic IoT fleets,
scripted faults, time acceleration), rebuild with the `sim-control` cargo
feature and set `FE_SIM_ALLOW=1` — **both** are required (compile feature +
runtime opt-in); default builds and the GUI binary never expose this surface:

```bash
cargo build --release -p fractalengine-relay --features sim-control
FE_SIM_ALLOW=1 ./target/release/fe-relay
```

A live sim session overrides the **process-wide** HLC clock source, so a
sim-control relay must serve no production verses — point it at a scratch
`FE_DB_PATH`/`FE_P2P_DIR`. Full usage, scenario JSON shape, and the REST/MCP
control surface: [docs/simulation-lab.md](../docs/simulation-lab.md).

## Graceful Shutdown

The relay handles `SIGINT` / `SIGTERM` (ctrl+c), or `FE_SHUTDOWN_AFTER_SECS` for windowed/container runs:

1. Sends `DbCommand::Shutdown` (DB thread drains and flushes SurrealDB to disk)
2. Sends `SyncCommand::Shutdown` (sync thread closes replicas and the iroh endpoint)
3. Waits briefly (~750ms) for both to settle, then exits with code `0`

## Docker

See [docker/README.md](../docker/README.md) for container deployment.

```bash
docker build -f docker/Dockerfile.relay -t fractalengine-relay .
docker run -p 8765:8765 -v relay-data:/data fractalengine-relay
```

## Architecture

The relay shares all core crates with the GUI binary but excludes GPU/display dependencies:

```
Shared: fe-runtime, fe-database, fe-policy, fe-network, fe-sync, fe-identity, fe-api
GUI only: fe-renderer, fe-ui, fe-webview, bevy_egui, keyring
```

No feature flags on shared crates — the Cargo dependency graph alone controls the split. The relay uses `MinimalPlugins` + `ScheduleRunnerPlugin` instead of `DefaultPlugins`.
