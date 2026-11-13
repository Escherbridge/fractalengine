# extensions/device-adapter

Example device-adapter extension (`kind: device-adapter`, track extension_platform P3.3): HTTP, websocket and MQTT
frames in, readings and transforms out, MQTT actuation back. The manifest is a byte copy of
`fe-sdk/schema/examples/device-adapter.extension.json`; keep the two in step.

## Build and run

Standalone crate (own `[workspace]`), built for `wasm32-wasip2` by `fe-ext build`. Root `Cargo.toml`
lists it under `exclude` for clarity (see the track findings). It tracks its own `Cargo.lock`, created by the
first `cargo check --manifest-path` (never hand-written; see `.gitignore`).

    fe-ext build --project extensions/device-adapter
    fe-ext run --sandbox --project extensions/device-adapter --fixture extensions/device-adapter/fixtures/basic.json
    cargo test --manifest-path extensions/device-adapter/Cargo.toml     # unit tests only (mapping.rs)

The sandbox journey is the device-adapter leg of J8 (`tools/fe-ext/tests/j8_cli.rs`, `cargo test -p fe-ext -- j8`),
for the same reasons as fleet-sim's (extensions/fleet-sim/AGENTS.md §build-and-run). The sandbox applies the host's
ingress checks before the guest sees a frame: an undeclared endpoint or sub-path is 404, then method 405, size 413
(websocket and mqtt: dropped), media type 415, and only the declared `forward_headers` minus credential headers
arrive, as in fe-plugin `net/ingress.rs`.

## §package-and-install

Recipe the GUI QA path uses (key files stay outside the repo; `fe-ext keygen` never overwrites):

    fe-ext keygen publisher.key                      # publisher key; a second `keygen index.key` is the index key
    fe-ext build --project extensions/device-adapter
    fe-ext validate --project extensions/device-adapter
    fe-ext pack --project extensions/device-adapter --key publisher.key --out packages/   # sealed, signed .hexon
    fe-ext inspect packages/<id>-0.1.0.hexon          # ids, content hash, capability digest, signer
    fe-ext index build --packages packages --base-url <public-root> --key index.key --out site
    fe-ext index verify site --index-key <did:key of index.key>
    fe-ext admin ...                                  # install-file verb on the running app's local channel

- `pack` is the only step that needs the publisher key; `sign` re-seals under another key, `countersign` adds a
  signer without moving `content_hash`.
- Install is GUI-only plus the local `fe-ext admin` channel, never REST or MCP; the GUI consent dialog always
  confirms the capability grants (governance.md). A registry install names the source and package instead of a file.
- `fe-ext index` and `fe-ext admin` are wired by I-FEXT/A-CLI; exact flag spelling is in `tools/fe-ext/AGENTS.md`.

## §ingress-plan

What each declared endpoint needs from the host and from the operator (manifest-v2.md §ingress):

| Endpoint | Transport | Host behaviour | Operator input |
|---|---|---|---|
| `telemetry` | `http-server` | Mounted at `/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/telemetry`; Editor floor, 64 KiB body, JSON only, 10 req/s per caller; only `x-device-id` is forwarded (credential headers are stripped by the host both ways). | none |
| `live` | `websocket-accept` | Upgrades on the same mount; 16 KiB frames, 20 frames/s per connection, at most 16 connections. | none |
| `feed` | `websocket-dial` | The host dials `wss://` (loopback may use `ws://`) and redials with jittered backoff 1 s to 60 s; frames dropped and counted over 16 KiB or 50/s. | credential `device-feed` (bearer), entered at bind and held by the host |
| `site-broker` | `mqtt` | The host subscribes to `site-a/+/telemetry` at-least-once over `mqtts://`; payloads over 16 KiB are dropped and counted. | credential `site-a-broker` (mqtt-password) |

- The `egress` endpoint `actuate` publishes to `site-a/+/command` through the same broker credential.
- The guest never sees a credential or a socket: it receives frames in `on_ingress` and queues publishes.
- Hosts refuse LAN egress targets, so the example hostnames are placeholders to replace at bind; the sandbox
  fixture feeds frames directly and needs none of this.

## §frame

One JSON shape for every transport (`src/mapping.rs`, pure, host-free):

    { "device_id"?: "dev-1",
      "readings"?: [ { "metric": "...", "value": 1.0, "units": "C", "at_ms"?: 0 } ],
      "position"?: [x, y, z], "heading_deg"?: 90 }

- The transport names the device when it can: http header `x-device-id` (the only `forward_headers` entry), mqtt
  topic `site-a/<id>/telemetry`. The body `device_id` is only needed on websocket; if both exist they must agree.
- The node is the one whose `device.id` property equals the id (`scene.query-nodes` property predicate). Unknown id:
  http/websocket answer 404, mqtt drops the frame with a warning.
- Replies: http 202 `{"accepted": n}`; 400 malformed, 404 unknown device, 429 queue full, 503 scene refused.
  Websocket and mqtt stay quiet on success; websocket gets the error body on failure; mqtt never replies.
- `source_class` is not in the frame: the host stamps it from the binding (`device` or `simulation`, picked at bind).

## §window

`on_ingress` only queues (readings in a `ReadingBatcher`, the newest transform and frame time per node). `on_tick`
stages transforms and `device.last_seen` while `window.room().ops >= 2`, then flushes readings by remaining room.
A node that does not fit waits for the next tick. More than 5000 queued readings answers 429, so a flood costs
the sender, not the host. A reading for a deleted node refuses the whole window at flush; the tick logs it from
`last-commit` and the already-staged batch is not retried (it left the queue when it was staged).

## §actuation

`actuate {node_id, action: on|off|reset}` and `reset {node_id}` read the node's `device.id` and queue an
`egress.mqtt-publish` on endpoint `actuate`, topic `site-a/<id>/command`, payload `{"action": ...}`, QoS at-least-once.
The `site-a` prefix is the `SITE` constant and must match the topic filters in `extension.json`.

## Fixtures

`fixtures/basic.json`: node `sensor-1` carries `device.id = dev-1`; five recorded frames cover each transport, a
malformed value and an unknown device. Bus commands and egress completions are not fed by the sandbox yet
(`NoBroker`), so actuation is covered by the manifest and by code review until the broker lands.
J8 asserts: replies `[(1, 202), (2, -), (2, -), (3, 400), (3, 404)]`, four readings on `sensor-1`, the websocket
pose `[12.5, 0, -4]` and `device.last_seen = 2001000`.
