# Relay operations

Runbook for operating a self-hosted FractalEngine relay (`fractalengine-relay`, the headless `fe-relay`
binary) and the iroh relay it can use for peer-to-peer sync. Written for the operator of one host.
Every variable below was checked against the code that reads it.

## 1. Environment variables

Defaults are what the process uses when the variable is unset.

| Variable | Default | Read at | Effect |
|----------|---------|---------|--------|
| `FE_BIND_ADDR` | `0.0.0.0:8765` | `fractalengine-relay/src/main.rs:387` | REST/API listen address. Bind `127.0.0.1:8765` when a TLS proxy runs on the same host (§4). |
| `FE_DB_PATH` | `data/fractalengine.db` | `fractalengine-relay/src/main.rs:137` | SurrealKV datastore path. The container image sets `/data/fractalengine.db`. |
| `FE_BLOB_DIR` | `<parent of FE_DB_PATH>/blobs` when unset | `fractalengine-relay/src/main.rs:60-76` | Blob store. Left unset, it follows `FE_DB_PATH` so the two stay together. |
| `FE_CORS_ORIGINS` | `*` | `fractalengine-relay/src/main.rs:388` | Comma-separated browser origins allowed by CORS. Set explicit origins in production. |
| `FE_API_METRICS_BIND` | unset (no listener) | `fe-api/src/network_ops.rs:40-66` | Address of the `/metrics` listener. Unset opens no listener. `/metrics` has no authentication, so bind loopback (for example `127.0.0.1:9464`) and scrape locally. A non-loopback value logs a warning. |
| `FE_API_USAGE_LOG` | unset (metering off) | `fe-api/src/network_ops.rs:37-55` | Path of a JSON-lines usage log. Records route templates only, never ids. An unwritable path logs a warning and metering stays off. |
| `FE_LOG_FORMAT` | `text` | `fe-runtime/src/log_format.rs` | `json` writes one JSON object per line. Any other value keeps the text format. |
| `FE_SYNC_RELAY` | `default` (n0 public fleet) | `fe-sync/src/relay_config.rs:183` | Relay for peer sync: `default`, `disabled`, or comma-separated `https://` URLs. Overrides the Settings value. Set it in production, because the n0 fleet is rate-limited. |
| `FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN` | unset | `fe-sync/src/relay_config.rs` (`RelayAccessToken::from_env`) | Bearer token for your own relay. See §3 and §5. |
| `FE_SYNC_RELAY_ADVERTISE_POLICY` | `dialonly` | `fe-sync/src/relay_config.rs:255` | What to do with relays other verse members advertise. See §5. |
| `FE_SYNC_ALLOW_PRIVATE_RELAYS` | off | `fe-runtime/src/relay_advertisement.rs:12` | Set to `1`, `true` or `yes` to accept advertised relays on private addresses and over `http://`. See §5. |
| `FE_SYNC_RELAY_ADVERT_SIGNED` | `compat` | `fe-sync/src/relay_config.rs:88` | `compat` accepts unsigned advertisement rows; `require` refuses them. Any other value fails closed to `require`. |
| `FE_SHUTDOWN_AFTER_SECS` | unset (run until signalled) | `fractalengine-relay/src/main.rs:735-752` | Requests the same graceful shutdown as SIGTERM after N seconds. Used for one-shot runs and smoke tests. |
| `SURREAL_DATASTORE_SYNC_DATA` | `every` (set by the relay when unset) | `fractalengine-relay/src/main.rs:42` | SurrealKV fsync mode: `never`, `every`, or a duration above 100 ms. |
| `RUST_LOG` | unset | tracing filter | Log filter, for example `info` or `fe_sync=debug`. |

The Docker-specific variables are listed in [`docker/README.md`](../docker/README.md).

## 2. Backup and restore (SurrealKV)

The relay keeps its datastore at `FE_DB_PATH` and its blobs in `FE_BLOB_DIR`. Copy both while the
relay is stopped. Copying a live SurrealKV store can capture a torn write.

**Quiesce.** Stop the relay with SIGTERM so it runs its graceful shutdown and flushes the store.
`docker stop` sends SIGTERM, but its default 10-second grace period can be too short, so pass a longer one:

```bash
docker stop -t 30 fe-relay
```

For a one-shot run, let the process stop itself with `FE_SHUTDOWN_AFTER_SECS=300`. Confirm the
process has exited before copying (`docker wait fe-relay` returns, or `docker ps` no longer lists it).

**Backup (stop-the-world copy).** With the relay stopped, copy the datastore and the blob directory
together so they come from the same moment. Find the real volume name with `docker volume ls`
(compose prefixes it with the project name, for example `docker_relay-data`):

```bash
docker run --rm -v docker_relay-data:/data -v "$PWD/backup:/backup" alpine \
  sh -c 'cp -a /data/fractalengine.db /backup/ && if [ -d /data/blobs ]; then cp -a /data/blobs /backup/; fi'
```

`/data/fractalengine.db` is the `FE_DB_PATH` of the image and `/data/blobs` is the derived blob
directory. If you set either variable yourself, copy those paths instead. SurrealKV may store the
datastore as a file or as a directory, and `cp -a` copies either. Start the relay again once the copy
finishes.

**Restore.** Stop the relay, replace the data volume contents with the backup, and start it again.
Never restore into a running relay:

```bash
docker stop -t 30 fe-relay
docker run --rm -v docker_relay-data:/data -v "$PWD/backup:/backup" alpine \
  sh -c 'rm -rf /data/fractalengine.db /data/blobs && cp -a /backup/fractalengine.db /data/ && if [ -d /backup/blobs ]; then cp -a /backup/blobs /data/; fi'
docker start fe-relay
```

Then confirm `GET /ready` returns 200 (health endpoints are in [`docker/README.md`](../docker/README.md)).

## 3. Self-hosting iroh-relay 1.x

Run your own iroh relay and point `FE_SYNC_RELAY` at it instead of the n0 public fleet. The image is
pinned by digest. It is the same digest the relay-assisted acceptance test uses
(`fe-sync/tests/relay_assisted_test.rs:30`):

```
n0computer/iroh-relay:v1.3.0@sha256:7a51e3826531dba5eee73d8b48a8f27284684d98c4d0da271d99127cc43b9d57
```

Create `docker/iroh-relay/iroh-relay.toml` on the host. The compose service below mounts it:

```toml
http_bind_addr = "0.0.0.0:3340"
access.shared_token = ["<a long random string>"]
```

`access.shared_token` admits only clients that present one of the listed tokens. Generate the value
with a random source, and do not commit the file.

Start the service from `docker/compose.dev.yml`. It sits under the `relay` profile, so a plain
`docker compose up` does not start it:

```bash
cd docker
docker compose -f compose.dev.yml --profile relay up -d iroh-relay
```

The service publishes only `127.0.0.1:3340`. Put TLS in front of it (§4). Then point each
FractalEngine node at it:

```bash
FE_SYNC_RELAY=https://relay-iroh.example.com
FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN=<the same string as shared_token>
```

Check that `https://relay-iroh.example.com/healthz` answers.

## 4. Reverse-proxy TLS in front of the relays

The REST relay listens on `127.0.0.1:8765` and the iroh relay on `127.0.0.1:3340`. A reverse proxy
terminates TLS for both. Replace `relay.example.com` and `relay-iroh.example.com` with your own names.

**Caddy** (automatic certificates; WebSocket upgrades pass through without extra configuration):

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:8765
}

relay-iroh.example.com {
    reverse_proxy 127.0.0.1:3340
}
```

**nginx** (certificates from your own ACME client or CA; the upgrade headers keep WebSocket
connections open):

```nginx
server {
    listen 443 ssl http2;
    server_name relay.example.com;
    ssl_certificate     /etc/ssl/relay.example.com/fullchain.pem;
    ssl_certificate_key /etc/ssl/relay.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8765;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_read_timeout 3600s;
    }
}
```

Repeat the `server` block for `relay-iroh.example.com` with `proxy_pass http://127.0.0.1:3340`.

Do not publish `8765` or `3340` on a public interface. The `/metrics` listener (`FE_API_METRICS_BIND`)
never goes through the proxy: keep it on loopback and scrape it locally.

## 5. Verse-advertised relays

A verse member can advertise a relay for other members to dial. An advertisement is untrusted input,
and these rules decide what your node does with it.

**Policy (`FE_SYNC_RELAY_ADVERTISE_POLICY`).**

| Value | Behaviour |
|-------|-----------|
| `dialonly` (default) | Admitted advertisements are listed and allowed as dial targets. They are never added to the relay map. |
| `pinned:<url>,<url>` | As `dialonly`, and also adds the named relays to the relay map. Name only relays you run or trust. |
| `deny` | Ignores advertisements entirely. |

An unrecognised value logs a warning and falls back to `dialonly`.

**Address rules.** The validator always refuses loopback, unspecified, link-local (including the cloud
metadata address `169.254.169.254`), multicast and broadcast addresses, and `localhost`. Private
ranges (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7) are refused unless
`FE_SYNC_ALLOW_PRIVATE_RELAYS=1` is set. Advertised relays must use `https://` unless that same
opt-in is set. A private relay is never promoted to the relay map automatically, even with the opt-in.

**Token isolation.** `FE_SECRET_FRACTALENGINE_SYNC_RELAY_TOKEN` is attached as an
`Authorization: Bearer` header only to your own configured relays, over `https://` or loopback
`http://`. It is never sent to the default n0 fleet or to an advertised relay, and it is never written
to `settings.ron` or replicated to peers. A plaintext non-loopback relay URL is refused before the
token would be sent.

**Signed adverts.** `FE_SYNC_RELAY_ADVERT_SIGNED=require` refuses unsigned rows from peers that run
older code. Keep the default `compat` for one release after an upgrade, then set `require`.

## Related

- Docker images, compose profiles and the container variable table: [`docker/README.md`](../docker/README.md).
- Relay design notes, the self-host recipe source and the acceptance test: `fe-sync/src/AGENTS.md` §relay-health.
