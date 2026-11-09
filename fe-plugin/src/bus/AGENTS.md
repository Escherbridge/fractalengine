---
type: reference
title: fe-plugin/src/bus — integration bus broker (D-X6)
track: extension_platform_20261107
slice: E5 (P1.5)
---

# fe-plugin/src/bus — integration bus broker

The broker behind `docs/extensions/bus.md` (spec R13-R15, D-X6). One `Bus` per host, cloned into the
plugin worker, fe-api (REST/WS/MCP) and the bridge. Guests reach it through WIT `bus` / `bus-hooks`;
UI widgets, WS connections and portal IPC are *clients* of the same broker.

| File | Owns |
|---|---|
| `mod.rs` | Vocabulary: `HostError` (WIT `host-error`), `BusRefusal` (what external callers see), `InstanceKey`, `Source` addresses, `Caller`/`CallerKind`, `BusEvent`, `EndReason`, `CommandError`/`CommandOutcome`, the bus.md §6 limits, `canonical_json`/`json_digest`. |
| `broker.rs` | `Bus`: registration, guest imports, external calls, clients, host feeds, the worker turn, status. |
| `query.rs` | `QueryState`: the OD-3 pacing state machine for one `host/query` subscription. |
| `sources.rs` | Injected services (`QuerySource`, `SceneSnapshotSource`, `RoleSource`, `BusPolicy`, `AuditSink`) and feed payloads (`ReadingEvent`, `AdmissionEvent`). |
| `rate.rs` | `TokenBucket` on the injected clock. |
| `schema.rs` | `validate_args`: the `args_schema` subset of bus.md §4. |
| `tests.rs` | Journey J4 (bus.md §10). |

## §sources — nothing in here knows SQL, the DB or axum

The broker never runs a query, reads the scene or decides a role itself. The binary (slice E7) injects
`BusSources`, mirroring the `HostEnv` idiom (`../AGENTS.md` §host-env):

- `QuerySource` wraps fe-api's guarded core (`query_guard::guard_and_prepare_query` +
  `run_guarded_query_via_state`). `prepare` returns `fe_query::PreparedSql`'s `tables` and executable SQL;
  `run` answers once on a crossbeam channel, off the worker; `try_acquire_rate` spends the identity's
  `QUERY_RATE_PER_SEC` shared with REST. This is how R13's "the same rows `/api/v1/query` returns" holds:
  there is no second SQL path.
- `SceneSnapshotSource` answers a petal's `NodeDto` + `properties` list off the worker. The broker cuts
  `properties` to the subscriber's property scope again (projection never trusts the source).
- `RoleSource` and `BusPolicy` are called **under the broker lock**: they must be in-memory, non-blocking
  and must never call back into the bus. `AuditSink::record` is called after the lock is released (audits
  are collected in an `Effects` buffer), and must not block either.
- `BusSources::fail_closed()` serves nothing, grants no role, allows no external access and logs audits.

Feeds go the other way: the bridge calls `feed_node_changes` (committed changes only; `transform_failed`
is dropped here as well), `feed_node_changes_lagged` (broadcast lag → resync), `feed_readings`,
`feed_admission`, and `feed_table_write` + `enable_change_feed` for extra tables such as `device_payload`.

## §broker — one mutex, short critical sections, no I/O under the lock

- **State.** `BrokerState` holds instances, clients, subscriptions (`BTreeMap`, so iteration and batches
  are id-ordered), pending calls and rate buckets. Methods destructure it (`let BrokerState { subs,
  instances, .. } = &mut *state`) so disjoint fields can be borrowed together through the guard.
- **Two-phase work.** Anything that calls an injected service is split: plan under the lock
  (`plan_guest`/`plan_client`), call the service with the lock released (`materialize`, `launch`), then
  insert or attach under the lock again, re-checking that the subscriber still exists.
- **The worker turn** is `poll()`: `settle` (collect finished runs and snapshots, coalesce pending scene
  changes into ≤256-change deltas), `launch` (start due runs and requested snapshots), `settle` again (so a
  synchronous source delivers in the same turn), `expire_calls` (5 s → `unknown-outcome`, never retried; idle
  buckets swept every 10 s). The worker also calls `dispatch(key, hooks, max)` per instance; priority is call
  results, then commands, then one event batch (≤64 events / 256 KiB, round-robin across subscriptions, `seq`
  order inside each; a query snapshot over 256 KiB travels alone).
- **Queues and drops** (bus.md §5). Each subscription has a bounded queue and an overflow rule:
  `DropOldest` (readings, admission, extension streams), `ReplaceLatest` (query, capacity 1) and `Resync`
  (node-changes: clear, queue `resync-required`, request a fresh snapshot). `seq` is assigned at enqueue, so
  `dropped_before` on the next delivered event equals the seq gap exactly; the event that triggered a resync
  never got a seq, so it is counted in `bus.dropped.queue` but not in `dropped_before`. `ended` and snapshot
  parts bypass the capacity so they are never the thing dropped.
- **Counters land where status reads them**: a guest subscription's deliveries and drops count on the
  subscribing instance; an external client's drops on a stream count on the producer (the client has no
  status route); `published`/`dropped_rate` on the producer.

## §query — OD-3, change-triggered re-runs

`QueryState` is bus.md §2.3's pseudocode: `dirty` is one flag (many writes, one run), `due()` needs dirty,
idle, unpaused and `now ≥ last_started + min_interval_ms` (floor 1 000 ms). `begin_launch` claims the run
*before* the rate budget is asked outside the lock; a spent budget restores the previous start
(`launch_deferred`) so a deferral never pushes the floor back. A result is delivered only when the blake3 of
its canonical rows (plus the truncated flag) changed; otherwise `query.unchanged` counts. Guests get at most
1 MiB per snapshot (`row_count` reports the rows actually delivered and `truncated` turns true). Tables
compare lowercase because `PreparedSql::tables` is uppercase; a table without a feed is refused at
subscribe (`not-available`), never left silently stale.

## §calls — bus.md §4 checks, in order

`admit_call` runs checks 1-8 exactly in table order (check 0, the remote DID's route rate, is
`Bus::admit_remote`, which fe-api calls first on every extension route and which is never audited).

- `Caller::local` is **recomputed** from the DID (`normalized`); fe-api never asserts it. Remote callers get
  `BusRefusal::NotFound` for checks 1-5, so an `external: false` command and an absent package answer
  identically; each such denial is one audit entry.
- Rate keys (check 6): a remote DID; or, for the local DID, `ui`, local REST/MCP, each extension package,
  each portal package. Both the caller bucket (10/s) and the per-(command, caller) bucket (`rate_hz`) are
  keyed this way, which is what lets a `ui` call succeed in the same second an extension spent its budget.
- An admitted call is one `PendingCall`; it is audited once, when its outcome is known (`resolve_call`):
  `ok`, the guest's `command-error` code, `unknown-outcome` (5 s) or `not-available` (instance stopped).
- **Contract gap.** WIT `on-call-result` carries `result<json, host-error>`, so a guest `command-error` seen
  by an *extension* caller is mapped: `invalid-input` → `invalid-input`, `refused` → `policy-denied`,
  `failed` → `backend` (`CommandError::into_host_error`). REST keeps 400/409/502.

## §clients — UI, WS and portal IPC

`open_client(did, kind, grants)` returns a `ClientHandle` whose `notify` fires when events queue; the
owner then calls `take_client_events`. `ClientGrants::Ui` may take extension streams and `host/admission`
(all packages); `Api` (a REST/WS connection) extension streams only, as bus.md §8 says; `Portal` exactly its
bundle's declared consume entries at its mount petal, `host/query` by named query only. A producer's
`consumers` list restricts *package* consumers (instances and portals). A paused client's query
subscriptions stay dirty and run once on resume.

## Seams for other slices

- **E1** implements `BusHooks` over the component and routes the WIT `bus` imports to `subscribe`,
  `unsubscribe`, `refresh`, `publish` and `call`; each hook gets its own fuel refill (the hook caller's job).
- **E3/E7** (worker loop, `../lib.rs` is E3's): call `Bus::poll()` every iteration and `dispatch` for each
  instance; `register_instance` on activation, `unregister_instance(reason)` on stop.
- **E6** (fe-api): `admit_remote` first, then `call_command` (await `CommandTicket::reply` with a timeout a
  little above 5 s) and the client API for the `/ws` `bus_subscribe` target; `BusRefusal::http_status` and
  `CommandError::http_status` give the codes.
- **E10** injects the op-log `AuditSink`, calls `feed_admission` and `revalidate` on policy or role change.
