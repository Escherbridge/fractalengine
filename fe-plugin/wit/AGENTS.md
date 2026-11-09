---
type: reference
title: fe-plugin WIT contracts — design notes
track: extension_platform_20261107
---

# fe-plugin/wit — design notes

Two files, two package versions:

| File | Package | Status |
|---|---|---|
| `fractal-plugin-2.wit` | `fractalengine:plugin@2.0.0`, world `extension` | The only third-party contract (D-X1, spec R2). `UNSTABLE - not yet bound` until P1.1 (E1) binds it with component-model bindgen. |
| `hexon-plugin.wit` | `fractalengine:plugin@1.0.0`, world `fractal-plugin` | Superseded. Kept for the first-party core-ABI fixtures until P1.1 swaps them. A plugin package declaring it is refused with `unsupported-contract`. |

## §one-file

The 2.0 package is one file, not one file per interface (plan P0.1 said "split per interface").
A WIT directory is parsed as ONE package, and this directory already holds the 1.0 package, so a
per-interface split would either collide with `hexon-plugin.wit` or move it and break every
citation of its path. E1 points `wasmtime::component::bindgen!` at the file path
(`path: "wit/fractal-plugin-2.wit"`), never at the directory. When 1.0 is deleted (after P1.1),
the file may be split; the package text does not change.

## §staged-writes

`txn` and `readings` only stage. One window per instance, flushed once per tick as ONE
`DbCommand::PluginCommit` (spec R5; the tripwire "one flush per instance per tick"). The flush is
atomic across scene ops and readings: a single invalid op or unknown anchor refuses the whole
flush and nothing lands (spec R6, journey J3). Writes staged in non-tick hooks (ingress, events,
commands) wait for the next flush, which is why a writing extension must declare a cadence
(manifest rule `cadence-required`).

**A trap discards the window.** A guest trap, fuel exhaustion or `wasi:cli/exit` in ANY hook (not
only `on-tick`) degrades the instance, and the window is discarded unflushed: `on-deactivate`
reason `degraded` gets no final flush. So a command or ingress hook that traps halfway through
staging can never leave a partial batch for the next tick to commit.

`stage-create-node` returns an ordinal, not an id: ids exist only after the DB commits, and they
come back in `commit-receipt.created-node-ids` on the next `tick-info`. Referencing a node created
in the same window (a provisional id usable by later ops) is deferred: it needs a `PendingOp`
change in fe-sdk and the batch handler, and nothing in wave 1 needs it.

`set-transform` takes a patch with a unit quaternion (the fe-sdk `NodeSnapshot` convention).
The DB stores Euler XYZ radians (`DbCommand::TransformPersist`); E4 converts inside the batch.

## §async-completions

Nothing a guest calls may block the plugin worker, which runs every instance on the host:

- `egress.http-request` and `bus.call` return an id immediately; the result arrives through the
  exported `egress-hooks.on-egress-result` and `bus-hooks.on-call-result` on a later worker turn.
  A synchronous `bus.call` would also let A call B call A on one thread.
- `egress.mqtt-publish` and `ingress.send` are fire-and-queue; immediate refusals (capability,
  rate, full queue) come back as `host-error`, delivery failures are counted in status.

The cost is four exported interfaces where the spec sketch had three (`egress-hooks` is new).
Every component must export all four; the guest SDK (P3.2) supplies no-op defaults so an
extension that uses no egress writes nothing for it.

## §handles

Subscriptions, calls, requests and connections are plain `u64` handles in host tables, not WIT
resources. A handle is valid only for the instance that received it; a foreign or stale handle is
`invalid-input`. Resources would add drop semantics that bindgen and the host's single-threaded
worker do not need.

## §websocket-direction

A websocket endpoint is declared ONCE, under `ingress` (mode `websocket-accept` or
`websocket-dial`), and carries both directions. Sending on it is `ingress.send`, not the spec
sketch's `egress.ws-send`, so consent lists each socket once and an "egress-only" socket is just a
dial endpoint whose inbound frames the guest ignores. `egress` keeps the request-shaped transports
(`http-request`, `mqtt-publish`).

## §time

The guest has no clock source. `context.now-ms`, `tick-info.now-ms`, `ingress-frame.received-at-ms`
and `bus-event.at-ms` are values from the host's injected clock: the wall clock in production, the
`SimClock` only inside the fe-sim lab (R4, R30). `context.now-ms` is frozen for one hook call, so
two runs of `fe-ext run --sandbox` over the same fixture are byte-identical (R25).

## §wasi-policy

Guests target `wasm32-wasip2` (plan A-6), so a component may import WASI 0.2 interfaces next to
this world. The host links a closed list, virtualized, and refuses any other import at install
with `package-invalid` (reason `forbidden-import <name>`):

| WASI interface | Host behaviour |
|---|---|
| `wasi:cli/environment` | empty args and env |
| `wasi:cli/exit` | exit traps the call; the instance degrades |
| `wasi:cli/stdin` | empty stream |
| `wasi:cli/stdout`, `wasi:cli/stderr` | routed to `log` at info / warn, same caps |
| `wasi:io/{error,streams,poll}` | only as the cli streams need |
| `wasi:clocks/{wall-clock,monotonic-clock}` | frozen at `context.now-ms` (not a clock source) |
| `wasi:random/{random,insecure,insecure-seed}` | a per-instance stream seeded from `blake3(installation-id ‖ activation count)`; the fixture seed in sandbox and lab. Deterministic, so NOT cryptographically secure: a guest holds no secret worth protecting (credentials stay host-side), and the guest SDK docs say so |
| `wasi:filesystem/*` | linked with zero preopens |

Never linked: `wasi:sockets/*`, `wasi:http/*`, and anything else. The exact WASI patch version is
whatever `wasmtime-wasi` plugin_host_revival S0 pins.

## §limits

Host-side bounds the WIT does not express (constants live in fe-sdk with E2; the manifest may ask
for less, never more):

| Bound | Value |
|---|---|
| ops per flush | 64 (manifest `tick.max_ops` lowers it) |
| readings per flush | 1 000 (manifest `tick.max_readings` lowers it) |
| bytes per flush | 65 536 (`MAX_HOST_BYTES`) |
| query-nodes rows | 1..200 (`MAX_NODE_QUERY_ROWS`) |
| any `json` argument | 65 536 bytes, finite numbers only |
| `bus-hooks.on-events` batch | 64 events, 256 KiB (a larger `host/query` snapshot, ≤ 1 MiB, arrives alone) |
| `on-command` result | 65 536 bytes |
| kv | keys ≤ 256 bytes, values ≤ 65 536 bytes, manifest `kv.max_bytes` (≤ 1 MiB) per instance |
| log | 4 KiB per message, 50 messages/s per instance (excess dropped and counted) |
| instance memory | 16 MiB (today's pooled cap; OD-E4 measures) |
