# extensions/bi-push

First-party push connector (`kind: bi-connector`, track extension_platform P5.5, R33): the named query `latest`
(`host/query`) is delivered as JSON or CSV over HTTPS to the declared `sink` endpoint. The manifest is
`fe-sdk/schema/examples/bi-push.extension.json` plus the `egress.connector` entry in `provides` (bindings.md §8.4).

## Build and run

Standalone crate (own `[workspace]`), built for `wasm32-wasip2` by `fe-ext build`; its own `[workspace]` table keeps it
out of the root workspace without an `exclude` entry. It tracks its own `Cargo.lock`, created by the first
`cargo check --manifest-path` (never hand-written; see `.gitignore`).

    fe-ext build --project extensions/bi-push
    cargo test --manifest-path extensions/bi-push/Cargo.toml     # payload unit test (src/payload.rs)

The sandbox leg belongs in J8 (`tools/fe-ext/tests/j8_cli.rs`), like the other examples, so this crate never pulls the
host stack into its own target directory (G3-C12, G5). The Rust validator parses `provides` (`fe-sdk/src/manifest_v2.rs`, V17/V18), so `fe-ext validate`/`pack` accept this
manifest: `egress.connector` needs the `sink` egress id and the `latest` `host/query` consume it declares.

## §package-and-install

Recipe the GUI QA path uses (key files stay outside the repo; `fe-ext keygen` never overwrites):

    fe-ext keygen publisher.key                      # publisher key; a second `keygen index.key` is the index key
    fe-ext build --project extensions/bi-push
    fe-ext validate --project extensions/bi-push
    fe-ext pack --project extensions/bi-push --key publisher.key --out packages/   # sealed, signed .hexon
    fe-ext inspect packages/<id>-0.1.0.hexon          # ids, content hash, capability digest, signer
    fe-ext index build --packages packages --base-url <public-root> --key index.key --out site
    fe-ext index verify site --index-key <did:key of index.key>
    fe-ext admin ...                                  # install-file verb on the running app's local channel

- `pack` is the only step that needs the publisher key; `sign` re-seals under another key, `countersign` adds a
  signer without moving `content_hash`.
- Install is GUI-only plus the local `fe-ext admin` channel, never REST or MCP; the GUI consent dialog always
  confirms the capability grants (governance.md). A registry install names the source and package instead of a file.
- `fe-ext index` and `fe-ext admin` are wired by I-FEXT/A-CLI; exact flag spelling is in `tools/fe-ext/AGENTS.md`.

## §payload

`src/payload.rs` is pure. JSON re-emits `{data, row_count, truncated}`, the `/api/v1/query` shape, so the sink sees the
rows REST returns. CSV is RFC 4180 (CRLF): header = union of row keys in first-seen order, and within one row the keys
come sorted (the guest's serde_json has no `preserve_order`, so its map is key-ordered). Null is empty, nested values are compact
JSON. A body over the sink's `max_request_bytes` (1 MiB) is dropped with a warning, never cut.

- **Formula injection (OWASP).** Node names, metrics and units are written by any verse editor and replicate, so a
  CSV opened in a spreadsheet must not run them. A text cell (and a header cell) starting with `= + - @ TAB CR` gets
  a leading `'` and is always quoted; numbers, negatives included, are never touched (steer 2026-11-08 23:00).

## §delivery

The host re-runs `latest` only when `iot_reading` changes (bus.md §2.3), so the guest never polls the data. `on_events`
keeps ONLY the newest snapshot (no backlog behind a slow sink) under a growing `generation`. Each `on_tick` (the 5 s
cadence is the push interval, spec R33) sends the newest generation the sink has not acknowledged, one request at a
time; a 2xx records it as delivered, and a refusal, a non-2xx or a transport error leaves it for the next tick. A sink
that was down therefore catches up on the next interval even when the petal goes quiet (before G5 a failed push waited
for the next data change). The shared `rate_hz: 1` bucket belongs to the host.

## §config

The manifest declares `provides: egress.connector`, so the crate enables the guest SDK's `hooks` feature and the
component exports `hooks` (without it `fe-ext build`/`validate`/`pack` refuse `missing-export`; J8 pins both sides).
`on_hook` answers the connector's `configure` call by adopting the binding's `format` (json|csv; the host already
checked it against `config_schema`, an unknown name is still `invalid-input`); any other hook id is `Failed`, so the
built-in serves it. Without a binding (sandbox, an unbound install) `format` falls back to kv key `bi-push/format`
(a JSON string), then json. The bound value wins over the kv seed.

## Parity

The pull API (`/api/v1/query`, export, share) is core and unchanged. Loopback-sink parity with `/api/v1/query` is
asserted in J10 once the sandbox feeds bus events (it does not yet, so the payload unit test covers rendering only).
