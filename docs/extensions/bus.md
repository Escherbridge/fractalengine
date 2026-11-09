---
type: reference
title: Integration bus contract (D-X6)
track: extension_platform_20261107
status: draft (P0.3; broker built in P1.5 / slice E5)
consumers: [extension_platform_20261107, portal_surface_20261107]
---

# Integration bus contract

The bus is how extensions, the UI, remote API clients and portals exchange live data without
holding anything themselves: **streams** (publish/subscribe), **commands** (request/reply) and
**hooks** (deliveries into a guest). One broker runs on the plugin worker of each host. It serves
host data streams from the same handlers REST uses (spec R13), extension streams and commands
under capability checks (R14), and enforces limits, delivery rules and audit (R15).

Guests reach it through `bus` / `bus-hooks` in [`fractal-plugin-2.wit`](../../fe-plugin/wit/fractal-plugin-2.wit);
everything a guest may touch is declared in its manifest's `bus` section ([manifest-v2.md](manifest-v2.md)).

**What the bus is not.** It is host-local and live: no persistence, no replay, no delivery to
other peers. Data crosses peers only as replicated rows (scene versions, readings), which then
appear on each peer's own `host/*` streams. Completeness comes from `host/query`, not from
having watched every event.

## 1. Addressing

| Address | Grammar | Example |
|---|---|---|
| Host stream | `host/<name>`, closed set | `host/query`, `host/readings`, `host/node-changes`, `host/admission` |
| Extension stream | `<package_id>/<stream_id>` | `org.example.fleet-sim/positions` |
| Command | `<package_id>/<command_id>` | `org.example.fleet-sim/start` |
| Instance | `(petal_id, package_id)` | the one instance of a package bound to a petal on this host |

`package_id` is reverse-DNS with at least one dot, so `host` can never be a package and the
`host/` namespace stays reserved. Ids are `[a-z][a-z0-9-]{0,31}`.

**Scope.** Every subscription and call lives at one petal: a guest's binding petal, or the petal
an external caller names. A guest never sees another petal's data, and an extension stream event
published by the instance bound to petal P reaches only subscribers at P.

**Identity.** An extension is judged as its installing identity, which is the host's local DID
(spec §4.11, R22). An external caller is judged as its own DID's role at the petal, read at call
time.

**Local versus external.** An extension stream or command with `external: false` (the default)
is reachable only from this host: the UI, the local DID over REST/WS/MCP, other extensions and
portals. Remote DIDs reach only `external: true` entries, still behind `role_floor`, and only
while the verse policy allows bus external access ([governance.md](governance.md#policy-row)).

## 2. Host streams

Each host stream is consumed only if the manifest lists it under `bus.consume` (or, for a
portal, its bundle manifest declares it) and consent covers it. Subscribing to anything else is
`capability-denied`.

### 2.1 `host/node-changes`

The committed scene change feed of one petal: the feed `/ws` scene subscriptions carry
(`ApiState.entity_change_tx`, local and replicated changes alike). Requires `scene.read`.

- **params:** `{}`.
- **On subscribe:** `snapshot` events, then deltas. Snapshot data:
  `{"part": 0, "last": true, "nodes": [NodeDto + "properties"]}`, at most 500 nodes per part.
  `NodeDto` is fe-runtime's (`node_id, petal_id, name, position, rotation [x,y,z,w], scale,
  has_asset, asset_path`); `properties` holds only keys inside `scene.property_scope`.
- **Delta data:** `{"changes": [<SceneChange>, ...]}` — fe-runtime `SceneChange` JSON exactly as
  `/ws` sends it (`{"op": "node_added" | "node_removed" | "node_renamed" | "node_transform" |
  "property_changed", ...}`), so one parser serves both. Rotation is as on that wire: quaternion
  inside `node_added.node`, Euler XYZ radians in `node_transform`.
- **Filtering:** committed changes only (never optimistic `/ws` transforms, never
  `transform_failed`); `property_changed` only for keys inside `scene.property_scope`.
- **Coalescing:** all changes since the previous delivery go out as one delta (≤ 256 changes per
  event, more events if needed).
- **Loss:** a lagged feed or a full queue sends `resync-required`, then a fresh snapshot.
- An extension sees its own committed writes here too; its tick receipt says what it wrote.

### 2.2 `host/readings`

Rows accepted by the readings door (REST ingest, extension flushes, and replicated rows applied
on this host), for one petal.

- **params:** `{"metrics"?: [names], "anchors"?: [node ids, ≤ 256]}`, within the consumed `metrics`.
- **Deltas only** (history is a `host/query`):
  ```json
  {"readings": [{"reading_id": "01J…", "node_id": "01J…", "metric": "altitude_m",
                 "value": 112.5, "units": "m", "recorded_at": "2026-11-08T19:50:00+00:00",
                 "recorded_at_ms": 1762631400000, "source_did": "did:key:z6Mk…",
                 "origin": "local"}]}
  ```
  `origin` is `local` or `replicated`. At most 1 000 readings per event.
- **Loss:** a full queue drops the oldest events and counts them (`dropped-before`); readings are
  independent facts, so there is no resync. A consumer that needs completeness re-queries.

### 2.3 `host/query` — change-triggered re-runs (OD-3)

One SELECT, registered at subscribe time and run through fe-api's guarded core
(`query_guard::guard_and_prepare_query` + `run_guarded_query_via_state`, the path
`POST /api/v1/query` takes), judged as the subscriber's identity, scoped to the binding petal.
For the same SQL, vars and scope it returns exactly the rows `/api/v1/query` returns (R13 AC).

- **params:** `{"query": "<named id>", "vars"?: {…}}` for a named query, or
  `{"sql": "SELECT …", "vars"?: {…}}` when the manifest consumed `sql: "*"`.
- **Snapshot data:** `{"data": [rows], "row_count": n, "truncated": bool}` (`data` is
  `/api/v1/query`'s field). REST's caps apply (10 000 rows, 8 MiB); a guest event is further capped
  at 1 MiB. Over a cap, rows are cut at the boundary and `truncated` is true.
- **Every delivery is a `snapshot`:** the full current result, never a diff.

**OD-3 answer (normative): a query subscription re-runs only when something in its scope
changed. There is no fixed re-run interval.**

```
on subscribe:
    prepared  = guard(sql, scope = binding petal)          # refused: invalid-input
    tables    = prepared.tables                            # fe_query PreparedSql FROM targets
    every table must have a change feed (table below)      # else: not-available("<table> has no change feed")
    run now -> snapshot                                    # the initial result

on scope write (table, petal) with table in tables and petal == binding petal:
    dirty = true                                           # many writes, one flag
on bus.refresh(subscription), or a change of the identity's role or the binding:
    dirty = true

each broker turn, per subscription:
    if dirty and not running
       and now >= last_started + min_interval_ms           # coalescing floor, >= 1000 ms
       and the identity's query rate budget has room:      # shared with REST, QUERY_RATE_PER_SEC = 10
        dirty = false; running = true; last_started = now
        run (async, off the worker)
    if dirty and the rate budget is spent: stay dirty, count query.deferred

on run complete:
    running = false
    if blake3(canonical rows) != last_hash: deliver snapshot, last_hash = new hash
    else: count query.unchanged (no event)
```

**Change feeds** (scope writes come from the same sources as `host/node-changes` and
`host/readings`, local and replicated):

| FROM table | Change feed | 2.0 status |
|---|---|---|
| `node` | committed scene changes in the petal | available |
| `iot_reading` | readings door accepts in the petal | available |
| `device_payload` | device-payload door accepts in the petal | available when E5/E7 inject that feed; until then `not-available` |
| `petal`, `fractal`, `verse`, `room`, `model`, `field_def`, `crate_registry`, `crate_entry` | none yet | `not-available` at subscribe |

A query over a table with no feed is refused at subscribe rather than left silently stale; adding
a feed later lifts the refusal without a contract change.

**Consequences.**
- An idle petal costs zero reads, however many subscribers it has. A burst of N writes costs at
  most one run per `min_interval_ms` per subscription, plus one trailing run.
- The worst case equals the old "at least 1 s" interval design: one run per second per subscription,
  ten per second per identity. The difference is only in the idle case, which is the common one.
- **Time-relative predicates** (`time::now()` windows) are evaluated at run time. Rows ageing out
  of a window are not a change, so the host does not notice them; a consumer that needs that
  calls `bus.refresh` (for example from `on-tick`) or filters by `recorded_at_ms` itself.
- The run happens off the worker; ticks and the render loop never wait on it.

### 2.4 `host/admission`

Admission changes on this host (spec R23), so dependent surfaces react without polling.

- **params:** `{}`. **Deltas only:**
  `{"package_id": "…", "content_hash": "…", "kind": "extension" | "web-bundle",
    "state": "admitted" | "disabled" | "policy-denied" | "revoked" | "removed",
    "reason"?: "…", "verse_id"?: "…"}`.
- **Visibility:** an extension sees events for its own package and for packages named in its
  `consume` and `call` lists. Host-internal clients (the UI, the portal broker client) see all.
- States and their meaning: [governance.md](governance.md#admission-record).

## 3. Extension streams

- **Publish:** `bus.publish(stream_id, event)`. The stream must be in `bus.expose.streams`; the
  event is one JSON value ≤ `max_event_bytes`.
- **Rate ceiling:** a token bucket at `max_rate_hz` (burst `ceil(max_rate_hz)`). A publish over
  the ceiling returns `rate-limited` and counts `bus.dropped.rate`; the publisher's tick timing is
  unchanged (R15 AC).
- **Fan-out:** to every subscriber at the publisher's petal whose consume list (or bundle
  manifest) names the stream, whose consent covers it, and that the producer's `consumers` list
  (if any) includes. External subscribers also need `external: true` and `role_floor`.
- **Data:** each delivery is `kind: delta` with `data` = the published JSON.
- **Producer stops:** subscribers get `ended` with `{"reason": "producer-inactive"}`; they may
  subscribe again after a `host/admission` `admitted` event.

## 4. Commands

A command is answered by the one instance of `<package_id>` bound to the call's petal.

**Checks, in order:**

| # | Check | Local caller gets | Remote caller gets |
|---|---|---|---|
| 1 | an active instance of the package is bound at the petal | `not-found` | uniform not-found |
| 2 | the command is in `bus.expose.commands` | `capability-denied` | uniform not-found |
| 3 | remote caller: `external: true` and the verse policy allows bus external access | — | uniform not-found |
| 4 | the caller's role at the petal ≥ `role_floor` | `capability-denied` | uniform not-found |
| 5 | an extension caller lists the target in `bus.call` (and is not calling itself) | `capability-denied` | — |
| 6 | per-caller rate (10/s per DID across the host) and the command's `rate_hz` | `rate-limited` | 429 |
| 7 | `args` validates against `args_schema` (absent schema: `{}` only) | `invalid-input` | 400 |
| 8 | the instance's command queue (16) has room | `rate-limited` | 429 |

Then `on-command(command, args, caller)` runs on the worker, with its own fuel. The reply must
arrive within 5 s, or the caller gets `unknown-outcome` (504) and the call is not retried. Results
are ≤ 64 KiB JSON. A guest `command-error` maps to `invalid-input` → 400, `refused` → 409,
`failed` → 502.

**Callers** (`caller.kind`): `ui` (Extensions UI and slot buttons, local DID), `rest`, `mcp`,
`extension` (judged as the local DID; `caller.package-id` set; result through `on-call-result`),
`portal` (a mounted bundle, local DID; `caller.package-id` is the bundle's package).

Writes a command handler stages are authored by the host's local DID and flush with the next
tick. That is why reaching a command from a remote DID is opt-in (`external: true`) and floored:
a command is a deliberate delegation of the host's identity.

**`args_schema` subset.** `type`, `properties`, `required`, `additionalProperties` (boolean only),
`enum`, `const`, `minimum`, `maximum`, `minLength`, `maxLength`, `items` (one schema), `minItems`,
`maxItems`, `description`; nesting depth ≤ 4; ≤ 4 KiB. No `$ref`, combinators, `pattern` or
`format`. Args are ≤ 16 KiB.

## 5. Delivery

**Envelope.** Guests receive WIT `bus-event`; JSON clients receive the same fields:

```json
{"sub": 7, "source": "org.example.fleet-sim/positions", "seq": 42, "kind": "delta",
 "dropped_before": 0, "at_ms": 1762631400000, "data": {"count": 4, "movers": []}}
```

- `seq` increases by one per event on a subscription; a gap equals `dropped_before` on the next delivery.
- `kind`: `snapshot`, `delta`, `resync-required`, `ended` (data `{"reason": …}`).
- Ordering holds within one subscription only.

**Queues and drops.**

| Source | Subscriber queue | When full |
|---|---|---|
| `host/node-changes` | 256 (guest) / 1 024 (external) | `resync-required`, then a fresh snapshot |
| `host/readings` | same | drop oldest, count `bus.dropped.queue` |
| `host/query` | 1 (latest result replaces an undelivered one) | replace, count `bus.dropped.queue` |
| `host/admission` | 64 | drop oldest, count |
| extension streams | 256 / 1 024 | drop oldest, count |
| commands | 16 per instance | refuse `rate-limited` |

Nothing blocks the worker or the render loop. A guest receives at most 64 events (256 KiB) per
`on-events` call, in `seq` order per subscription.

**`ended` reasons:** `unsubscribed`, `producer-inactive`, `consent-revoked`, `role-lost`,
`instance-stopped`, `policy-denied`, `revoked`.

## 6. Limits

| Limit | Value |
|---|---|
| Subscriptions per instance | 16 |
| Subscriptions per external connection | 16 |
| Stream event | declared, ≤ 64 KiB |
| Stream rate | declared, ≤ 50/s |
| Command args / result | 16 KiB / 64 KiB |
| Command rate | declared ≤ 10/s per command; 10/s per caller DID across the host (`limits.rs` ingest precedent) |
| Command reply timeout | 5 s |
| `host/query` floor | `min_interval_ms` ≥ 1 000 (default 1 000) |
| `host/query` rate | the identity's `QUERY_RATE_PER_SEC` (10), shared with REST |
| Guest delivery batch | 64 events, 256 KiB |

## 7. Audit and counters

**Audited** (one op-log entry each, through governance's audit `OpType`, R21): consent grants
covering bus entries, every subscribe and unsubscribe (actor DID, source), every command call
(actor DID, command address, `args_digest` = blake3 of canonical args, outcome) and every denial.
**Not audited:** individual events, deliveries and query re-runs; the data they carry is already in
the log once it becomes rows.

**Counters** (in `GET` extension status, R11): `bus.published`, `bus.delivered`,
`bus.dropped.rate`, `bus.dropped.queue`, `bus.subscriptions`, `bus.commands.ok`,
`bus.commands.refused.<reason>`, `bus.query.runs`, `bus.query.unchanged`, `bus.query.deferred`.

## 8. External access

| Client | Surface |
|---|---|
| REST | `POST /api/v1/petals/{petal_id}/extensions/{package_id}/commands/{command}`, body `{"args": {…}}`, reply `ApiResponse` with the result in `data`. |
| WS (`/ws`) | Extension streams only. Client: `{"type": "bus_subscribe", "petal_id", "source": "<package_id>/<stream>"}` and `{"type": "bus_unsubscribe", "sub"}`. Server: `{"type": "bus_subscribed", "sub", "source"}` and `{"type": "bus_event", …envelope}`. Unknown, foreign and forbidden targets get the identical denial `/ws` already gives (`ws_unknown_and_foreign_ids_get_identical_denials`). Host streams keep their existing WS/REST paths. |
| MCP | `extension_command {petal_id, package_id, command, args}`; `list_extensions` (read-only). |
| Portal IPC (`fe.portal.v1`) | One more broker client; table below. |

**Portal IPC mapping** (portal_surface_20261107 §6.4; caller kind `portal`, identity = local DID,
grants = the bundle manifest's declarations ∩ consent):

| `fe.portal.v1` | Bus |
|---|---|
| `subscribe {stream: "node"}` | `host/node-changes` at the mount's petal |
| `subscribe {stream: "readings", …}` | `host/readings` |
| `subscribe {stream: "query", query: <name>}` | `host/query` with the bundle's declared named query (never ad-hoc SQL) |
| `subscribe {stream: "<package_id>/<stream>"}` | that extension stream |
| `invoke {package, command, args}` | command call |
| `emit {stream, data}` | `publish` on a stream the bundle declares, addressed `<bundle package_id>/<stream>` |
| delivery `{sub, seq, kind, data}` | the envelope above; `kind` maps 1:1 (`resync-required` ↔ `resync_required`), `ended` closes the subscription |

A hidden portal pauses its query subscriptions (the broker skips them; they stay dirty) and a
resumed one gets one run if anything changed meanwhile.

## 9. Worked examples

### 9.1 A fleet publishes positions; the toolbar starts it

`org.example.fleet-sim` ([manifest](../../fe-sdk/schema/examples/fleet-sim.extension.json)) is
bound to petal `P`.

1. An editor clicks **Start fleet** (accessible name `ext:org.example.fleet-sim/start`). The UI calls
   `org.example.fleet-sim/start` at `P` with `{}`, caller `{kind: ui, local: true}`. Checks 1-8
   pass; `on-command` stages `fleet.status = "running"` and returns `{"started": 1}`. One audit
   entry: `command-call … outcome ok`.
2. Next tick (500 ms cadence) the guest samples each mover with `routes.sample`, stages four
   `set-transform` ops and two readings, and publishes
   `{"count": 4, "movers": [{"node_id": "01J…", "progress": 0.12}, …]}` on `positions`.
3. The flush commits as ONE `PluginCommit` (4 ops, 2 readings). `host/node-changes` subscribers
   at `P` get one delta with four `node_transform` changes; `host/readings` subscribers get one delta.
4. The dashboard widget `ext:org.example.fleet-sim/movers` shows `/count` = 4.
5. A script publishing at 10 Hz against `max_rate_hz: 4` gets `rate-limited` for the excess;
   `bus.dropped.rate` grows and tick timing is unchanged.

### 9.2 A remote editor calls a command

Peer B's editor (role Editor at `P`) sends
`POST /api/v1/petals/P/extensions/org.example.fleet-sim/commands/start` to peer A.

- `start` is `external: false` → uniform not-found (404), one `denial` audit entry. The response
  is identical to calling a package that is not installed.
- With `external: true` in a later version (a capability delta, so consent re-prompts) and the
  verse policy allowing bus external access: a Viewer gets the uniform not-found (below
  `role_floor: editor`); the Editor gets 200 with `{"started": 1}`. The fleet writes as peer A's
  identity, and peers judge those rows by peer A's role.

### 9.3 A BI connector on `host/query` (the OD-3 timeline)

`org.example.bi-push` ([manifest](../../fe-sdk/schema/examples/bi-push.extension.json)) consumes
the named query `latest` with `min_interval_ms: 5000` and pushes to its `sink` egress.

| t (s) | Event | Broker | Guest |
|---|---|---|---|
| 0 | `bus.subscribe("host/query", {"query": "latest"})` | guard passes; tables = {`iot_reading`}; run 1 | `snapshot` (500 rows) → stores it; `on-tick` POSTs it |
| 0-60 | nothing written in `P` | no runs | ticks find nothing new to push |
| 61.0 | a REST batch of 20 readings lands in `P` | dirty | — |
| 61.0 | (5 s floor since run 1 has passed) | run 2 | `snapshot` → pushed on the next tick |
| 61.4-63.0 | 6 more batches (local and replicated) | dirty again during and after run 2 | — |
| 66.0 | floor reached | run 3 (one run for six batches) | `snapshot` |
| 70.0 | a node is renamed in `P` | `node` is not in tables: ignored | — |
| 80.0 | a batch lands whose rows fall outside `LIMIT 500` | run 4; same hash | no event (`query.unchanged` +1) |

A fixed 1 s interval would have run this query 80 times; change-triggered re-runs ran it 4 times.

### 9.4 A portal shows the same query

A bundle declaring the named query `latest` sends `subscribe {stream: "query", query: "latest"}`
over IPC. The broker treats it exactly like 9.3, with caller kind `portal`. While the portal is
hidden its subscription skips runs; on show it gets one run if `P` changed meanwhile, and none if not.

## 10. Journey J4 (P1.5, new, `fe-plugin/src/bus/`)

With a fake query source, a fake scene feed and a fake readings feed:

1. `host/query` returns the rows the guarded core returns for the same SQL and scope.
2. With no writes for 10 simulated seconds, the query source sees zero runs after the initial one.
3. Twenty writes inside one floor window cause exactly one re-run; a write to a table the query
   does not read causes none; an identical result delivers no event.
4. A query naming `petal` is refused at subscribe with `not-available`.
5. A stream pushed past its ceiling counts `dropped` and the publisher gets `rate-limited`.
6. A full command queue answers `rate-limited`.
7. An undeclared consume is refused with `capability-denied`.
8. A grant, a call and a denial each produce exactly one audit entry.

## 11. Decision record

- **OD-3 (2026-11-08, EXT-PLATFORM E0):** query subscriptions re-run only when a write touched a
  table the query reads inside its scope, coalesced behind a per-subscription floor (≥ 1 s) and the
  identity's shared query rate. No fixed interval; idle costs zero reads. Tables without a change
  feed are refused at subscribe. Supersedes spec R13's "re-runs at a declared interval of at least
  1 s" and answers portal_surface_20261107 OD-3.
