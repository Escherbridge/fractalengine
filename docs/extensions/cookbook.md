---
type: guide
title: Extension cookbook
track: extension_platform_20261107
status: draft (P3.4, slice E12D, reviewed by G3; each recipe names a real example file or contract section)
---

# Cookbook

Each recipe is a short shape you can copy into a manifest, then point at a working example.
The examples are linked, not copied, so they stay correct as the contracts move. Code snippets
are the contract surface (WIT, manifest JSON); the Rust guest SDK that wraps them is
[`tools/fe-ext/guest`](../../tools/fe-ext/guest/src/lib.rs), and each recipe names the example
code that uses it.

Before you start, read the [authoring guide](authoring-guide.md#1-start-from-a-manifest-not-from-code)
for the grant rules.

## Move a node every tick

*Needs:* `scene.read`, `scene.write: ["set_transform"]`, `routes: true`, `tick`.
*Example:* [`extensions/fleet-sim/extension.json`](../../extensions/fleet-sim/extension.json); the mover loop is
`drive` in [`extensions/fleet-sim/src/lib.rs`](../../extensions/fleet-sim/src/lib.rs) (`routes::sample`, then
`window::set_pose`).

Sample the route instead of doing geometry yourself. `routes.sample` is the same numerical
path the REST tracking uses, and it is cached until the route node changes.

```wit
// routes.sample(route, progress) returns position, tangent, distance-m, segment-index.
// progress is clamped to 0.0..=1.0 by the host.
sample: func(route: node-id, progress: f64) -> result<route-sample, host-error>;
```

Then stage the transform. Nothing moves until the next tick flush:

```wit
stage-set-transform: func(node: node-id, patch: transform-patch) -> result<_, host-error>;
```

*Smart part:* `progress` is clamped by the host, so a bad clock cannot send a mover past the end of its route.
*Watch for:* the route is cached by node change, not by your call. A route edit is seen on the next
`sample`, not on the next tick.

## Stage readings in one batch

*Needs:* `readings` with `source_class` and a `metrics` list; `tick`.
*Example:* [`extensions/device-adapter/extension.json`](../../extensions/device-adapter/extension.json) (`readings.metrics: ["*"]`);
the batching is [`ReadingBatcher`](../../tools/fe-ext/guest/src/readings_batch.rs), queued from `on_ingress` and
flushed from `on_tick` in [`extensions/device-adapter/src/lib.rs`](../../extensions/device-adapter/src/lib.rs).

```wit
stage: func(batch: list<reading>) -> result<_, host-error>;
```

Stage up to the window's `max-readings` (1000 by default) in one call. Anchors are checked at
flush, and **one unknown anchor refuses the whole flush**. Filter your batch against the petal's
node list before staging if you can.

*Watch for:* `source_class` is picked by the operator at bind time when the manifest lists more than one.
Do not assume which one you have; read `binding.source-class`.

## Subscribe to a host stream

*Needs:* `bus.consume` with the stream name.
*Example:* [`bi-push.extension.json`](../../fe-sdk/schema/examples/bi-push.extension.json) (a `host/query` consumer).

Each host stream needs its own `bus.consume` entry, and consent must cover it. `host/node-changes`
also needs `scene.read`. `host/readings` takes an optional `metrics` filter, and `host/admission` takes none.

```wit
subscribe: func(source: string, params: json) -> result<subscription-id, host-error>;
```

Deliveries arrive in batches through `bus-hooks.on-events`, at most 64 events per call, in
`seq` order per subscription. A `seq` gap means drops; a `resync-required` means the stream
lost events and a snapshot follows. Handle `resync-required` by replacing your state, not by patching it.

## Run a named query and push its result

*Needs:* `bus.consume` with `{"source":"host/query","id":...,"sql":...,"min_interval_ms":...}`.
*Example:* [`bi-push.extension.json`](../../fe-sdk/schema/examples/bi-push.extension.json).

The query runs through the same guard REST uses (`POST /api/v1/query`), judged as the
subscriber, scoped to the binding petal. The rule that matters is in [bus.md §2.3](bus.md#23-hostquery--change-triggered-re-runs-od-3):

```text
a query subscription re-runs only when something in its scope changed.
There is no fixed re-run interval.
```

So a BI push costs nothing while the petal is idle. Set `min_interval_ms` as a floor, never below 1000.
Each delivery is a full `snapshot`, not a diff. Replace your target table with it.

*Watch for:* the named SQL is shown to the operator verbatim at consent and must pass the static guard at
validate time (`invalid-query`). Raw SQL strings from the guest do not exist in 2.0 (OD-E10).

## Publish a stream and let other parts consume it

*Needs:* `bus.expose.streams` on the producer; `bus.consume` on the consumer.
*Example:* the `positions` stream in [`extensions/fleet-sim/extension.json`](../../extensions/fleet-sim/extension.json),
published once per mover step by `drive` in [`extensions/fleet-sim/src/lib.rs`](../../extensions/fleet-sim/src/lib.rs).

Declare the stream on the producer. Its address is `<package_id>/<stream_id>`:

```json
"bus": { "expose": { "streams": [
  { "id": "positions", "max_rate_hz": 10, "max_event_bytes": 16384, "role_floor": "viewer" } ] } }
```

Publish with `bus.publish(name, event)`. Pass the stream id, not the full address.

Remote DIDs cannot read it unless `external: true` is set. Leave it false unless a
remote tool really needs it. The default keeps the stream inside this host.

## Expose a command the UI can call

*Needs:* `bus.expose.commands`, a `ui` entry with `slot` and `command`.
*Example:* [`extensions/device-adapter/extension.json`](../../extensions/device-adapter/extension.json) (command `actuate`);
the handler is `on_command` in [`extensions/device-adapter/src/lib.rs`](../../extensions/device-adapter/src/lib.rs).

```json
"bus": { "expose": { "commands": [ { "id": "actuate", "role_floor": "editor", "rate_hz": 2,
  "args_schema": { "type": "object", "required": ["node_id", "action"],
    "additionalProperties": false,
    "properties": { "node_id": {"type":"string","maxLength":128},
                    "action": {"enum":["on","off","reset"]} } } } ] } }
```

The host validates `args` against `args_schema` **before** your `on-command` runs. Your handler
can trust the shape. It must still reply within 5 seconds, or the caller gets `unknown-outcome` and
the call is not retried.

*Smart part:* `additionalProperties: false` is the only thing standing between a UI button and an
unexpected argument. Keep it.

## Receive device telemetry over HTTP

*Needs:* an `ingress` endpoint with `transport: "http-server"`, `methods`, `max_body_bytes`, `rate_hz`.
*Example:* the `telemetry` endpoint in [`extensions/device-adapter/extension.json`](../../extensions/device-adapter/extension.json);
the frame mapping is [`extensions/device-adapter/src/mapping.rs`](../../extensions/device-adapter/src/mapping.rs).

The endpoint mounts at `/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{id}` behind the
API's bearer auth. Over-size gets 413; over-rate gets 429. Your `on-ingress` receives the caller's DID.

Forward only the headers you need, and never a credential:

```json
"forward_headers": ["x-device-id"]
```

Do not declare `authorization`, `cookie`, `x-api-key` or any header the host uses to authenticate
(any `x-fe-*`). The host strips those whatever the manifest says, and the schema refuses a manifest
that declares one ([manifest-v2.md](manifest-v2.md#validation) V16, `credential-header`). The sandbox
applies the same stripping, so a fixture header that production would drop never reaches your guest there either.

## Connect an MQTT device

*Needs:* an `ingress` endpoint with `transport: "mqtt"`, `topics`, `broker`, `auth`.
*Example:* `site-broker` in [`extensions/device-adapter/extension.json`](../../extensions/device-adapter/extension.json).

The host is the MQTT client. Your guest receives one `ingress-frame` per message, with `topic` set,
and the broker password stays in the host's credential store (`auth.credential` names the slot; the value never
enters the manifest or your component). An endpoint whose slot is empty stays down and reports
`credential-missing`.

*Watch for:* a frame over `max_payload_bytes` is dropped and counted, not delivered truncated.

## Keep your own state

*Needs:* `kv: { "max_bytes": n }`, up to 1 MiB.

`kv` is per instance (one installation at one petal), host-local and never replicated. Use it for cursors,
counters and the ordinal-to-node-id map from `stage-create-node`. Do not use it as a database; the store is
the petal. *Example:* the per-fleet clock (`load_clock` / `save_clock`) in
[`extensions/fleet-sim/src/lib.rs`](../../extensions/fleet-sim/src/lib.rs).

## Sharpen

- **Pattern:** *pull a query, push a snapshot* (change-triggered re-run with a coalescing floor). It keeps
  BI cheap on idle petals and makes every delivery self-contained.
- **Architecture:** streams are live and host-local (no replay, no peer delivery). Peers see data only
  as replicated rows, so a BI connector that must be complete should use `host/query`, not
  `host/node-changes`. The bus doc explains this in its opening section.
- **Deferred:** a bus-command and egress-result feed in the sandbox fixture; until then `on_command` and
  `on_egress_result` are exercised only on a dev host. Snap-to-alert (REST `tracking_alert` semantics) is
  [`tracking::snap_and_stage`](../../tools/fe-ext/guest/src/tracking.rs), not yet used by an example.
- **Efficiency:** prefer one `host/query` subscription over polling `host/node-changes` and recounting in the guest.

*Question to check your grasp:* a BI push runs `host/query` every 1 second, but the petal has had no writes for an hour. How many query runs happen, and which rule in bus.md §2.3 decides it?
