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
| `FE_SYNC_RELAY` | `default` | iroh relay mode: `default` / `disabled` / comma-separated relay URLs (n0's hosted relays EOL 2026-12-31) |
| `FE_SYNC_BOOTSTRAP` | (none) | Semicolon-separated iroh `NodeAddr` JSON entries to dial for every opened replica (invalid entries warn + skip) |
| `FE_SHUTDOWN_AFTER_SECS` | (never) | Request the same graceful shutdown a signal would, after N seconds (smoke tests, one-shot containers) |
| `SURREAL_DATASTORE_SYNC_DATA` | `every` | SurrealKV fsync cadence (`never` / `every` / duration >100ms) |
| `RUST_LOG` | (none) | Log level filter (`info`, `debug`, `fe_api=debug`, etc.) |

### Secret injection

The relay uses `EnvBackend` for secrets. Keys are mapped to env vars as `FE_SECRET_{SERVICE}_{ACCOUNT}` (uppercased, special chars replaced with `_`).

- Node keypair: `FE_SECRET_FRACTALENGINE_NODE_KEYPAIR`. If unset, the relay generates an ephemeral keypair on startup (new iroh node id every run).
- Verse namespace secrets: `FE_SECRET_FRACTALENGINE_VERSE_<VERSE_ULID>_NS_SECRET_FRACTALENGINE` = 64 hex chars. A verse's P2P replica opens only when its secret is available from this mapping — without one, startup logs a loud warning and skips that verse. The namespace id is derived from the secret (blake3 keyed hash), so no row update is needed.

## P2P Replication

The relay is a fully applying replica of every verse it hosts:

- **Startup scan** — after the DB thread is up, the relay requests the verse hierarchy and opens a replica for each verse whose namespace secret resolves (see above).
- **Runtime** — every `VerseCreated` opens its replica the same way.
- **Reconciliation** — each open replays the replica's current doc entries through the same inbound apply path, so rows denied before a role or the verse manifest converged locally get a second chance.
- **Inbound admission** — the fe-policy role gate at the DB layer applies (deny-by-default, Editor+, roles resolved from local tables, never wire-supplied).

To join an existing swarm, copy the dialable `NodeAddr` JSON from the peer's startup log (`SyncStatus updated: started — dialable address above`) into `FE_SYNC_BOOTSTRAP`. Note: two SurrealKV connections cannot coexist in one process, so the relay's direct API reader falls back to the command channel whenever the DB writer holds the store — that is normal, not an error.

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
