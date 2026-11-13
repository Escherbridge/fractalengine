---
type: reference
title: fe-plugin wasm — component host for fractalengine:plugin@2.0.0
track: extension_platform_20261107
slice: E1
---

# fe-plugin/src/wasm

Two engines live here. `component/` is the host for WIT `fractalengine:plugin@2.0.0`, world
`extension` (`fe-plugin/wit/fractal-plugin-2.wit`), the ONLY third-party contract (D-X1, spec R2).
`mod.rs::WasmEngine` is the old core-module engine, kept import-free so `lifecycle.rs` still
compiles until its owners move activation onto `ComponentHost`.

## §retired-core-abi

The 1.0 `fe` core ABI (`host_imports.rs`, hand-written `func_wrap` imports with `-1` sentinels) is
deleted. `WasmEngine`'s linker is empty, so a core module that imports anything cannot instantiate,
and `ComponentHost::compile` refuses any core module as `unsupported-contract` before it is even
parsed (`check_binary` reads the layer bytes of the preamble). The swap of its old test is the 2.0
journey in `component/instance.rs`.

## §bindgen

`component/bindings.rs` runs `wasmtime::component::bindgen!` on the FILE
`wit/fractal-plugin-2.wit`, never the directory (the directory also holds the 1.0 package; a WIT
directory parses as one package; `fe-plugin/wit/AGENTS.md` §one-file). Because only one version of
`fractalengine:plugin` is in the resolve, generated modules are unversioned:
`bindings::fractalengine::plugin::{types,context,scene,txn,readings,routes,ingress,egress,bus,kv,log}`
for imports and `bindings::exports::fractalengine::plugin::{lifecycle,ingress_hooks,egress_hooks,bus_hooks}`
for exports. Imports and exports are synchronous and non-trappable: a host import never traps the
guest; it returns `host-error` (WIT rule "a refused call returns host-error and changes nothing").

Linking: `ComponentHost::new` adds `wasmtime_wasi::p2::add_to_linker_sync` and
`Extension::add_to_linker::<InstanceState, HasSelf<InstanceState>>`. The engine is separate from
`WasmEngine`: component model on, fuel on, on-demand allocation; memory is bounded per store by
`StoreLimits` (16 MiB per linear memory, 32 core instances, 8 memories, 32 tables) instead of the
pooling allocator, whose per-module counts do not fit a component's several core instances.

## §install-checks

`ComponentHost::compile` implements package-format.md §Component checks, in order:

1. Size ≤ 16 MiB, wasm magic, layer 1 (component). A core module is `unsupported-contract`.
2. Every import name passes `wasi::check_import` (§wasi-policy). A `fractalengine:plugin` import
   whose version is not `2.0.x` is `unsupported-contract` (a newer minor's imports may not exist).
3. The four exports of world `extension` exist (`missing-export <name>`).
4. `instantiate_pre` + `ExtensionPre::new` type-check the whole world (`unlinkable`,
   `export-mismatch`).

`compile_cached` does the same through `AotCache::{get_or_compile_component,load_compiled_component}`
(the cache key is the blake3 of the component bytes, the same digest package-format.md calls
`component_digest`). Nothing here verifies signatures, seals or revocation: those run before the
body is compiled (lifecycle / E9 / E10), against both `content_hash` and `component_digest`.

## §instance-state

`InstanceState` is the `T` of each `wasmtime::Store`. It holds the host-minted WIT `binding`, the
admitted `PluginContext` (v2 token from `CapabilityToken::mint_v2`, the petal-pinned reader), the
`SharedWindow` the tick scheduler flushes, a frozen clock cell, the kv slot, the broker, the route
cache, the per-instance log sink, the WASI context and the store limits. `InstanceSetup::check`
refuses a binding that names another petal or package than the context/token (a host bug, never a
guest-reachable path).

**Clock.** `now_ms` is an `AtomicU64` frozen once per hook call: `on-tick` uses the scheduler's
`TickInfo::now_ms`, `on-ingress` the frame's `received-at-ms`, every other hook the injected clock.
`context.now-ms`, WASI wall/monotonic clocks and the log rate window all read the same cell, so two
sandbox runs over one fixture are byte-identical (R25).

## §imports

`component/imports.rs` (context, scene, txn, readings, kv, log), `routes.rs`, `broker.rs`.

- **scene** requires `grants_v2.scene_read` explicitly (a write grant also carries `storage.read`,
  which must not open reads) and then goes through `PluginContext::get_node` / `query_nodes`, which
  pin the petal and project properties to `property_scope`. Node properties serialize in key order.
- **txn** builds a `PendingOp`, runs `PluginContext::authorize_op` (write kind + property scope +
  bound petal), then `StagedWindow::stage_op` (SDK validation, op/byte limits). Nothing writes; the
  scheduler flushes the window once per tick (R5). `stage-create-node` returns its ordinal in the
  current window: the counter restarts whenever a stage finds the window empty, which is exactly
  "after a flush took it or a discard emptied it" without the window having to expose its ops.
- **readings** needs a bind-time source class in the token, checks every metric with
  `authorize_reading`, then stages all-or-nothing: the batch goes into a clone of the window that
  replaces it only when every reading fit (`budget-exceeded` past `max_readings`). `recorded-at`
  stays absent when the guest omits it; the flush path fills the flushing tick's time.
- **kv**: keys 1..=256 bytes, values ≤ 64 KiB of valid JSON, total bytes ≤ manifest `kv.max_bytes`
  (≤ 1 MiB) or `budget-exceeded`. `KvBackend` is the persistence seam; `MemoryKv` lasts for the
  process. A file-backed backend (so `on-install` sees state survive restarts) is the binary's job.
- **log** and guest stdout/stderr share `wasi::GuestLog`: 4 KiB per message (cut at a char
  boundary), 50 messages per second of the frozen clock, the excess counted in `log_dropped`.

## §host-errors

Context refusals are plain strings; `state::context_error` maps them by wording (denied →
`capability-denied`, lease/ancestry → `revoked`, missing service → `not-available`, reader/sink →
`backend`, bounds → `invalid-input`). Flush refusals arrive as fe-runtime `PluginRefusal` text
(`"<case>: <detail>"`), and `refusal_error` restores the exact WIT case for `tick-info.last-commit`.
A `StagedWindow` "full" is `budget-exceeded`; an SDK-invalid value is `invalid-input`.

## §routes

`routes` reads the route node's geometry, flattens it once with the SDK
(`decode_anchor_rows` + `flatten_anchor_path`, the renderer's f32 de Casteljau) and answers with the
SDK's f64 math (`snap_to_polyline`, `position_at_progress_f64`), so REST tracking, the renderer and
guests share one implementation (R7). Sources, in order: `gpx_points` (anchor rows), then legacy
`route_points` (`[{lat, ele, lon}]` read as `[lat, ele, lon]`, exactly as `fe-api` tracking does).

Geometry needs no `property_scope` (manifest-v2.md §routes): with a `geometry_reader` the host reads
the node unfiltered, re-checking the same petal and verse pin as `PluginContext` (scope before and
after, node id and petal equality). Without one it falls back to the scope-filtered context read.
The cache keeps up to 32 flattened routes and rebuilds an entry whenever the raw property differs,
so it is correct without the bus; `ExtensionInstance::invalidate_route` is the hook for
`host/node-changes`.

## §broker-seam

`ExtensionBroker` is how E5 (bus, egress, ingress brokers) plugs in without touching this
directory. Every method gets the caller's `Binding` and has a `not-available` default; `NoBroker`
is the default, so a host without E5 still links the full world. The Host impls check the lease and
the JSON bound first. Completions flow back through `ExtensionInstance::{on_events, on_command,
on_call_result, on_egress_result, on_ingress, on_connection}`, which take the generated record
types (re-exported through `component::bindings`). The broker must enforce declared sizes and
rates on what it accepts (egress bodies, mqtt payloads, ws frames): the import side here only checks
the lease and the 64 KiB JSON bound.

`component/bus_bridge.rs` joins E5's integration bus: `BusBroker` answers the `bus` imports through
`crate::bus::Bus` (keyed by `InstanceKey { petal_id, package_id }` from the binding), and
`ExtensionInstance` implements `crate::bus::BusHooks`, so the worker drives the guest with
`bus.dispatch(&key, &mut *handle.lock()?, n)`. Bus mirror types convert 1:1 to the generated WIT
types; a hook fault answers a command `failed` and is otherwise only logged, because the instance
already degraded and the scheduler stops it on its next tick. The matching bridge for E5's
`net::NetHooks` (ingress/egress) is `component/net_bridge.rs` (§net-bridge).

## §instance

`ExtensionInstance` owns the store and the bound exports. Every hook goes through one `call`:

1. refuse if degraded;
2. refuel to the hook budget (except `on-tick`: `TickScheduler` already called `refuel`);
3. freeze the clock;
4. run the guest **outside any async context** (`outside_async`).
5. On any error (fuel, trap, `wasi:cli/exit`) the instance degrades, the window is discarded
   unflushed, and every later hook is refused with `HookFault::Degraded`. This applies to ALL hooks,
   not only `on-tick`, so a command or ingress hook that traps halfway through staging can never
   leave a partial batch for the next flush (`fe-plugin/wit/AGENTS.md` §staged-writes).

`outside_async` exists because wasmtime-wasi's sync shims (`blocking-write-and-flush`, `poll`)
call `in_tokio`, which does `Handle::block_on` and panics inside a runtime; the plugin worker runs
inside `Runtime::block_on`. On a multi-thread runtime the call goes through `block_in_place`
(allowed on a `block_on` thread, and it exits the runtime context); on a current-thread runtime it
runs on a scoped OS thread; with no runtime it runs inline.

`InstanceHandle` (`Arc<Mutex<ExtensionInstance>>`) implements `tick::TickInstance`, so one handle
goes to `TickScheduler` and clones go to the brokers; every caller is on the plugin worker, so the
lock is uncontended. `InstanceStore` keys live handles by installation id (one per installation).
`window_for_token` builds the `StagedWindow` from the token's effective tick budget.

## §wasi-policy

Implements `fe-plugin/wit/AGENTS.md` §wasi-policy: no args, env, preopens or sockets; stdin
closed; stdout → log at info, stderr → log at warn; the wall clock frozen at the hook instant;
randomness is a 128 KiB blake3-XOF stream per purpose, keyed by installation id and activation
count, cycled by `wasmtime_wasi::Deterministic` (NOT cryptographic, by design); `max_random_size`
64 KiB (a larger request traps, which degrades the instance).

Two deviations from that table (recorded as contract changes for its owner):

- **`wasi:clocks/monotonic-clock` is refused at install.** wasmtime-wasi's `subscribe-duration` /
  `subscribe-instant` pollables sleep in REAL time (`Deadline::Never` for durations that overflow),
  whatever the frozen clock returns, and fuel does not run while the host blocks. A guest's
  `std::thread::sleep(Duration::MAX)` would hang the plugin worker and every instance on it. The
  frozen monotonic clock stays configured as defense in depth. Re-admitting the interface needs a
  shadowed, never-blocking implementation of its subscribe functions (`Linker::allow_shadowing`)
  with a test; guests read time through `context.now-ms` meanwhile.
- **The five `wasi:cli/terminal-*` interfaces are admitted**: wasm32-wasip2 std imports them for
  its stdio, and here they only ever report "not a terminal".

Everything else, `wasi:sockets/*` and `wasi:http/*` included, is `forbidden-import`. With
monotonic-clock out, nothing a guest can import blocks the worker: stdin is closed, guest stdio is
always writable, there are no file descriptors, and `wasi:io/poll` has nothing time-based to wait on.

## §test-guests

No guest toolchain runs in tests. `component/test_guest.rs` writes the core module by hand in WAT,
embeds world `extension` from the real WIT file (`wit_component::embed_component_metadata`) and
encodes it with `ComponentEncoder` (dev-dependencies `wit-component` / `wit-parser` 0.244, used
only here). The core signatures are the canonical-ABI flattenings, worth re-deriving if the WIT
changes: a param list over 16 flat values (`binding`, `ingress-frame`) arrives as one pointer;
any result over one flat value (`result<_, string>`, `option<ingress-reply>`) is returned through a
pointer (exports) or a trailing return-pointer param (imports, e.g. `stage-set-transform`'s 15
flat params + 1). `fleet_setup` binds the shipped fleet-sim manifest to petal `p1` of
`MockHostEnv`. Journeys: `component/instance.rs`
(`component_instantiates_ticks_and_stages_through_the_shared_window`) and the component block of
`mod.rs::fuel_exhaustion_marks_degraded`. Both run in batch R11 (`cargo test -p fe-plugin`);
`cargo test -p fe-plugin -- wasm::` selects just this directory.

## §read-budget (G1)

Fuel meters guest instructions, not host time. `scene.get-node`, `scene.query-nodes` and a route-cache
miss are each a synchronous DB round trip on the plugin worker (`DbSceneBackend`, up to 15 s), so a guest
looping on them inside one hook could hold the one worker, and the DB lane, for minutes and starve every
other instance's ticks. `InstanceState::take_read` caps them at `MAX_SCENE_READS_PER_HOOK` (64) per hook
call; `ExtensionInstance::call` resets the count on every guest entry. Over budget is `budget-exceeded`,
which the guest can handle (read less per tick, or subscribe to `host/node-changes` instead).
`query-nodes` is capped in `context.rs::query_nodes`: `NodeQuery::validate` bounds `limit` to
`MAX_NODE_QUERY_ROWS` (200) and a page over `MAX_HOST_BYTES` (64 KiB serialized) is refused, so one read
cannot return an unbounded result.

## §hooks

`component/hooks.rs` binds the optional `hooks` export of world `extension-with-hooks` on the same raw
instance `instantiate` creates; plain 2.0 components stay on world `extension` and keep working. A component
that declares `provides` must export it (`check_provides`). Each call has a catalogue deadline and byte caps
(`hook_limits`, 64 KiB JSON), its own fuel refill and a frozen clock; a trap or timeout degrades the instance.
Host-initiated calls and decode stamping: src/AGENTS.md §hook-calls.

## §net-bridge

`component/net_bridge.rs` is the N3 seam from src/net/AGENTS.md: it implements `NetHooks` over the component
and routes the `ingress`/`egress` imports to `Net`. Credential-bearing headers are stripped in both directions
before a guest sees or sets them. The worker drives it per instance with `Net::dispatch(key, &mut handle, n)`
through the `HostBroker` that pairs the bus and net brokers (§broker-seam).
