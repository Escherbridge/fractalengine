---
type: reference
title: fe-plugin/src/bus — integration bus broker (D-X6)
track: extension_platform_20261107
slice: E5 (P1.5); F-BUS (turn 2, portal clients)
---

# fe-plugin/src/bus — integration bus broker

The broker behind `docs/extensions/bus.md` (spec R13-R15, D-X6). One `Bus` per host, cloned into the
plugin worker, fe-api (REST/WS/MCP) and the bridge. Guests reach it through WIT `bus` / `bus-hooks`;
UI widgets, WS connections and portal IPC are *clients* of the same broker.

| File | Owns |
|---|---|
| `mod.rs` | Vocabulary: `HostError` (WIT `host-error`), `BusRefusal` (what external callers see), `InstanceKey`, `Source` addresses, `Caller`/`CallerKind`, `BusEvent`, `EndReason`, `CommandError`/`CommandOutcome`, the bus.md §6 limits, REST's row/body caps, the portal budgets, `portal_key_allowed`/`is_node_id`/`is_content_hash`, `canonical_json`/`json_digest`. |
| `broker.rs` | `Bus`: registration, guest imports, external calls, clients (portal clients too: `PortalGrants`, `client_publish`, `client_call_command`, `client_set_property` with `WriteTicket`/`WriteOutcome`), host feeds, the worker turn, status. |
| `query.rs` | `QueryState`: the OD-3 pacing state machine for one `host/query` subscription. |
| `sources.rs` | Injected services (`QuerySource`, `SceneSnapshotSource`, `RoleSource`, `BusPolicy`, `AuditSink`, and the separately installed `PortalWriteSink`) and payloads (`ReadingEvent`, `AdmissionEvent`, `PortalWrite`). |
| `rate.rs` | `TokenBucket` on the injected clock (`with_burst` for the portal write budget). |
| `schema.rs` | `validate_args`: the `args_schema` subset of bus.md §4. |
| `tests.rs` | Journey J4 (bus.md §10), extended with the portal-client leg (§portal-clients, §portal-writes, §row-caps). |

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
  buckets swept every 10 s: a bucket goes once it is full and nothing took from it for 60 s. Only `try_take`
  moves `touched_ms`; before R-LIFE the sweep's own refill reset the clock, so per-DID buckets never left and a
  stream of fresh remote DIDs grew the maps without bound). The worker also calls `dispatch(key, hooks, max)` per instance; priority is call
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
bundle's declared consume entries at its mount petal (host sources and extension streams alike; until F-BUS
the stream branch checked only the consume list, so a portal could subscribe to a declared hook at another
petal), `host/query` by named query only. A producer's `consumers` list restricts *package* consumers
(instances and portals). A paused client's query subscriptions stay dirty and run once on resume.

## Seams for other slices

- **E1** implements `BusHooks` over the component and routes the WIT `bus` imports to `subscribe`,
  `unsubscribe`, `refresh`, `publish` and `call`; each hook gets its own fuel refill (the hook caller's job).
- **E3/E7** (worker loop, `../lib.rs` is E3's): call `Bus::poll()` every iteration and `dispatch` for each
  instance; `register_instance` on activation, `unregister_instance(reason)` on stop.
- **E6** (fe-api): `admit_remote` first, then `call_command` (await `CommandTicket::reply` with a timeout a
  little above 5 s) and the client API for the `/ws` `bus_subscribe` target; `BusRefusal::http_status` and
  `CommandError::http_status` give the codes.
- **E10** injects the op-log `AuditSink`, calls `feed_admission` and `revalidate` on policy or role change.
- **P-BROKER** (portal tree, `fractalengine/src/portal_host/broker.rs`): `open_portal_client(installing_did,
  role_ceiling, PortalGrants)` at mount; then `client_subscribe`, `take_client_events`, `set_client_paused`,
  `client_call_command` (invoke), `client_publish` (emit) and `client_set_property` (act; poll
  `WriteTicket::reply` once per frame); `close_client` at unmount. Never build a `Caller` literal for a
  portal: `call_command` refuses kind `portal`. `WriteOutcome` maps to ipc-v1.md §7 codes as: `Committed` →
  `{node_id, key}`; `RoleDenied` and `Refused(revoked)` → `role_denied`; `Refused(invalid-input)` →
  `not_found` (a node outside the petal); `Refused(unknown-outcome)` → `unknown_outcome`; the rest → `backend`.
- **I-BIN / I-POR-BIN** (binary): implement `PortalWriteSink` over `DbCommand::PluginCommit` and call
  `install_portal_writes` once (§portal-writes); feed `AdmissionEvent { kind: WebBundle, .. }` for bundles as
  well, so a revoked bundle stops on the bus in the same call (§portal-clients). `AuditAction::PropertyWrite`
  needs a mapping in the op-log audit sink.

## §delegated-callers (G1 security fix)

fe-identity mints every API token with the NODE's DID as `sub` (`mint_api_token`), so "the caller's DID
is the local DID" is true for every viewer or editor token the operator hands to an agent or client.
`Caller.role_ceiling` carries the token's `max_role`: `local` is recomputed as "local DID AND no ceiling
below Owner", and every role check on a caller (`role_of`: commands, client streams) caps the DID's role
by the ceiling. A delegated token is therefore remote: the uniform not-found, `external: true` plus the
verse policy, and its own rate key (fe-api keys its gate by `sub#jti`). Guests and the UI pass `None`.
`Bus::open_client_capped` is the client form. Subscriptions keep only the identity DID, so revalidation
re-reads the DID's role; a token's ceiling cannot change during its life, so nothing escalates.

## §portal-clients (F-BUS, P-S2; steers 2026-11-08 20:45 and 23:40)

A mounted bundle is a client, not an instance (A-14): no tick, no fuel, no `register_instance`.
`Bus::open_portal_client(installing_did, role_ceiling, PortalGrants)` takes everything from the host-minted
mount binding (ipc-v1.md §6.5): package id, `content_hash`, the web-bundle installation, verse, mount petal,
declared consume entries, streams, invoke targets and `set_property` key patterns. Nothing comes from page
data, and every portal operation derives its identity from the client, never from a `Caller` literal
(`call_command` refuses kind `portal`).

- **Identity and ceiling.** A portal caller is local when its DID is the local DID, whatever its ceiling.
  The binding is minted in-process, so §delegated-callers' "a ceiling below Owner makes it remote" (which
  exists because every API token carries the node DID) does not apply. The ceiling still caps every role
  check (`role_of`): stream floors, command floors (check 4) and the write floor. A portal client opened
  without a ceiling is capped at Viewer (fail closed). The binary computes the ceiling: the installing
  identity's role at the bundle's scope, never above the viewer's token.
- **Host sources** (`host/node-changes`, `host/readings`, `host/query`, `host/admission`) need a local binding
  and a capped role of Viewer or above at the mount petal (R-LIFE). A foreign DID's binding gets the uniform
  not-found, as for emit, invoke and writes. Before this, only the stream branch judged the role.
- **emit** (`client_publish`): a declared stream and a local caller; size within its `max_event_bytes`
  (≤ 64 KiB); rate within `max_rate_hz` capped at 10 Hz, keyed by `(petal, package, stream)` so a reload cannot
  reset it. Fan-out shares `fan_out_stream` with guest publishes, with `from_portal = true`. Each subscription
  records whether its producer is an instance or a portal, and each producer kind feeds only its own
  subscriptions, so a bundle that declares an extension's package id never injects events into that
  extension's subscribers; `lookup_producer` prefers an active instance at the same address. Portal streams
  are local only (no `external`) and readable from Viewer. When no live portal client declares a stream any
  more (close or stop), its subscribers get `ended {producer-inactive}`.
- **invoke** (`client_call_command`): the target must be a declared `commands.invoke` entry (the portal's
  check 5), then checks 1-8 run with the client's caller; the rate key is `Portal(package)`.
- **Stop on admission.** `feed_admission` with `kind: web-bundle` and a non-`admitted` state stops every
  portal client whose package id OR content hash matches (hash under any id, steer 20:45), within the event's
  verse when it names one. Its subscriptions end with the mapped reason (`revoked`, `consent-revoked`,
  `role-lost`, `policy-denied`, `instance-stopped`), its streams' subscribers end `producer-inactive`, and new
  subscribe, emit, invoke and write calls answer `not-available`. Writes already sent still settle and their
  rows stay (OD-E1, no withdrawal). The portal host still closes its client on revoke (R-POR-IPC); this is the
  bus-side backstop.

## §portal-writes (A-13)

`client_set_property(client, node_id, key, value)` is the one-op portal write: ONE `PendingOp::SetProperty`
committed as ONE `DbCommand::PluginCommit` with no readings, outside any tick, with no tick budget.
`admit_write` runs under the lock, in ipc-v1.md §6.1 order:

1. a local binding, else the uniform not-found;
2. a declared, non-reserved key (`node_kind`, `portal` and `portal.*` are refused whatever the patterns say;
   patterns are exact or `prefix.*`, keys `[a-z0-9_]+(.[a-z0-9_]+)*`), else `capability-denied`, audited;
3. a valid provenance pair on the binding and an installed write path, else `not-available`;
4. the node-id shape and at most 16 KiB of value JSON, else `invalid-input`;
5. the shared budgets: the caller key's command bucket (10/s, shared with invoke), the package's write bucket
   (2/s, burst 5: twenty calls in one second land at most seven), and the installing identity's write bucket
   across every bundle (4/s, burst 8: half the ~8 writes/s DB-thread ceiling, so several bundles together
   still cannot starve the editor), else `rate-limited`;
6. the role at write time, capped by the ceiling, at least Editor. Below it the ticket resolves `RoleDenied`
   at once and nothing reaches the DB.

The DB is never called under the broker lock. An admitted write becomes a `PendingWrite` holding the reply
channel, and the sink is called once the lock is released. `settle_writes` runs on every `poll`, and right
after the sink returns for a synchronous sink. It answers the ticket and audits it once as `property-write`,
with the outcome (`ok`, `role-denied` or the `PluginRefusal` kind), the key in the target
(`<package>/set_property/<node>/<key>`), the value's digest and the installation. No reply within
`PORTAL_WRITE_TIMEOUT_MS` (5 s) is `unknown-outcome`, never retried.

**The sink contract** (`PortalWriteSink::commit`, implemented by the binary): mint a `PluginAdmission` for
`write.petal_id` with `capabilities = [CAP_STORAGE_WRITE]`, `property_scope = [key]` (exactly the key the bus
matched), `with_package(PackageIdentity::new(package_id, content_hash))` (the host-stamped provenance, R24) and
`with_scene_writes([SceneWrite::SetProperty])`; bind its scope; send
`DbCommand::PluginCommit { admission, ops: vec![write.op], readings: vec![], reply }` without blocking. The DB
re-checks the local DID's role (Editor floor), the capability and the stored ancestry at use
(`fe-database/src/handlers/plugin_host.rs` `commit`). That is where "judged by the installing identity's role
at write time" is final; the bus check adds only the ceiling the DB does not know.

## §row-caps (G1-C6)

`scene.query-nodes` is row-capped host-side already. `NodeQuery::validate` bounds `limit` to 1..=200
(`fe_sdk::plugin_host::MAX_NODE_QUERY_ROWS`, below REST's 10 000), and `PluginContext::query_nodes` refuses a
reader that returns more than `limit` rows or a page over 64 KiB (`../context.rs`). The G1-C6 premise ("not
row-capped") is stale for that path. Two bus paths trusted their source to cap, and the bus now caps both
itself: a `host/node-changes` snapshot carries at most `REST_ROW_CAP` (10 000) nodes, and the last part then
says `"truncated": true`; a `host/query` snapshot is cut at 10 000 rows, and at REST's 8 MiB body for non-guests
(guests keep the 1 MiB event cap), with `truncated: true`.

## §bindings (bus.md §12)

The bus has no binding concept and needs none. A binding-started instance registers through the same
`register_instance` as a locally bound one: one instance per `(petal, package)`, and a second registration at
the same key is refused, so the activation layer reuses the running instance. Hook-point calls never touch
the broker (no subscription, publish or command slot; status counts them under `hooks.*`). Bound API routes
reach commands through fe-api's `admit_remote` and `call_command`, so their auth, floors, rates and the
host-side credential-header strip are those of the extension routes.
