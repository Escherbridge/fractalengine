# Docker Deployment

Two images live here, both following the same alpine musl multi-stage pattern:

- `Dockerfile.relay` — headless relay (`fe-relay`, port 8765)
- `Dockerfile.hexon-registry` — hosted hexon package registry (`fe-hexon-registry`, port 8790)

`compose.dev.yml` runs both for local development. All commands below work
with `docker` or `podman` interchangeably.

## Build

From the repository root:

```bash
docker build -f docker/Dockerfile.relay -t fractalengine-relay .
```

## Run

```bash
docker run -d \
  --name fe-relay \
  -p 8765:8765 \
  -v relay-data:/data \
  fractalengine-relay
```

## Docker Compose

```yaml
version: "3.8"

services:
  relay:
    build:
      context: ..
      dockerfile: docker/Dockerfile.relay
    ports:
      - "8765:8765"
    volumes:
      - relay-data:/data
    environment:
      FE_BIND_ADDR: "0.0.0.0:8765"
      FE_DB_PATH: "/data/fractalengine.db"
      FE_CORS_ORIGINS: "https://app.example.com,https://admin.example.com"
      RUST_LOG: "info"
    restart: unless-stopped
    healthcheck:
      test: ["CMD", "wget", "--spider", "-q", "http://localhost:8765/api/v1/health"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 10s

volumes:
  relay-data:
```

Save as `docker/docker-compose.yml` and run:

```bash
cd docker
docker compose up -d
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FE_BIND_ADDR` | `0.0.0.0:8765` | Listen address |
| `FE_DB_PATH` | `/data/fractalengine.db` | SurrealDB storage path (inside container) |
| `FE_CORS_ORIGINS` | `*` | Comma-separated CORS origins |
| `RUST_LOG` | (none) | Log filter (`info`, `debug`, etc.) |

## Data Persistence

The `/data` volume contains the SurrealDB database. Mount a named volume or host path to persist data across container restarts.

## Health Checks

- **Liveness:** `GET /api/v1/health` — always 200
- **Readiness:** `GET /ready` — 200 when DB is initialized, 503 otherwise

---

# Hexon Registry (`Dockerfile.hexon-registry`)

Hosts `.hexon` packages over HTTP per `docs/hexon-format-spec.md` §API
Endpoints — the endpoint set a local FractalEngine instance uses to search,
fetch, and publish packages. Design notes: `fe-hexon-registry/AGENTS.md`.

## Build & run with podman

From the repository root:

```bash
podman build -f docker/Dockerfile.hexon-registry -t fe-hexon-registry .
podman run -p 8790:8790 -v ./sample-hexons/dist:/data/hexons fe-hexon-registry
```

`sample-hexons/dist` is produced by:

```bash
cargo run -p fe-hexon --example build_sample_hexons
```

Then verify:

```bash
curl http://localhost:8790/health
curl "http://localhost:8790/api/v1/hexons/search?q=alpine"
curl http://localhost:8790/api/v1/hexons/alpine-demo-terrain/download -o alpine.hexon
```

## Dev compose (relay + registry)

```bash
podman-compose -f docker/compose.dev.yml up
```

Brings up the relay on `:8765` and the hexon registry on `:8790` with
`sample-hexons/dist` mounted as the package directory.

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `FE_REGISTRY_DIR` | `/data/hexons` | Directory scanned for `*.hexon` (non-recursive) |
| `FE_REGISTRY_BIND` | `0.0.0.0:8790` | Listen address |
| `FE_REGISTRY_TOKEN` | (unset) | Bearer token gating all `/api/v1` routes; unset = open access (warning logged) |
| `FE_REGISTRY_READONLY` | `false` | Reject `POST /api/v1/hexons/publish` when `true` |
| `RUST_LOG` | `info` | Log filter |

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | ok + indexed package count (public) |
| GET | `/api/v1/hexons/search?q=&tags=&type=` | Search index |
| GET | `/api/v1/hexons/{uri}` | Manifest (`uri` = `id` or `id@version`; latest when unversioned) |
| GET | `/api/v1/hexons/{uri}/entries` | entries.json catalog |
| GET | `/api/v1/hexons/{uri}/entries/{entry_id}/asset` | Stream one blob |
| GET | `/api/v1/hexons/{uri}/download` | Stream the whole `.hexon` (`application/x-hexon+zip`) |
| POST | `/api/v1/hexons/publish` | Upload raw `.hexon` bytes (409 on duplicate `id@version`) |
