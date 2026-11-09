# extensions/bi-push

First-party push connector (`kind: bi-connector`, track extension_platform P5.5, R33): the named query `latest`
(`host/query`) is delivered as JSON or CSV over HTTPS to the declared `sink` endpoint. The manifest is
`fe-sdk/schema/examples/bi-push.extension.json` plus the `egress.connector` entry in `provides` (bindings.md §8.4).

## Build and run

Standalone crate (own `[workspace]`), built for `wasm32-wasip2` by `fe-ext build`. Needs `extensions/bi-push` in the
root `Cargo.toml` `exclude` list (see the track findings).

    fe-ext build --project extensions/bi-push
    cargo test --manifest-path extensions/bi-push/Cargo.toml     # payload unit + tests/journey.rs

## §payload

`src/payload.rs` is pure. JSON re-emits `{data, row_count, truncated}`, the `/api/v1/query` shape, so the sink sees the
rows REST returns. CSV is RFC 4180 (CRLF): header = union of row keys in first-seen order, null is empty, nested values
are compact JSON. A body over the sink's `max_request_bytes` (1 MiB) is dropped with a warning, never cut.

## §delivery

The host re-runs `latest` only when `iot_reading` changes (bus.md §2.3), so the guest does no polling. `on_events`
keeps ONLY the newest snapshot (no backlog behind a slow sink); `on_tick` sends it when no request is in flight. A
refused or failed push is retried by the next snapshot, not by a timer. The shared `rate_hz: 1` bucket belongs to the host.

## §config

The guest SDK has no binding-config read yet, so `format` (json|csv) is read from kv key `bi-push/format` (a JSON
string), default json. When the BND turn lands a config read, replace `configured_format`.

## Parity

The pull API (`/api/v1/query`, export, share) is core and unchanged. Loopback-sink parity with `/api/v1/query` is
asserted in J10 once the sandbox feeds bus events (it does not yet, so this crate's journey covers rendering only).
