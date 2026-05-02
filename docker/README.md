# Docker Deployment

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
