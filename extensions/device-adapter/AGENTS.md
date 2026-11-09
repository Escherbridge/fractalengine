# extensions/device-adapter

Example device-adapter extension (`kind: device-adapter`, track extension_platform P3.3): HTTP, websocket and MQTT
frames in, readings and transforms out, MQTT actuation back. The manifest is a byte copy of
`fe-sdk/schema/examples/device-adapter.extension.json`; keep the two in step.

## Build and run

Standalone crate (own `[workspace]`), built for `wasm32-wasip2` by `fe-ext build`. Needs
`extensions/device-adapter` in the root `Cargo.toml` `exclude` list (see the track findings).

    fe-ext build --project extensions/device-adapter
    fe-ext run --sandbox --project extensions/device-adapter --fixture extensions/device-adapter/fixtures/basic.json
    cargo test --manifest-path extensions/device-adapter/Cargo.toml     # unit (mapping.rs) + tests/journey.rs

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
