# fe-plugin/src — module rationale

`fe-plugin` is the plugin **host** (thread 7): Rhai engine, Wasmtime embedding,
lifecycle, capability tokens, and the channel bridge to the Bevy ECS. It now
depends on `fe-sdk` and treats `fe-sdk` as the single source of truth for the
stable data types.

## §architecture

`PluginHostPlugin` (lib.rs) spawns a dedicated plugin runtime thread
(thread 7) with its own Tokio runtime. Plugins communicate with the Bevy ECS
through bounded crossbeam channels using the `PluginCommand`/`PluginResult`
enums:

```text
Bevy ECS  <--crossbeam-->  Plugin Thread (Tokio)
  |                            |
  | drain_plugin_results()     | FractalExtension callbacks
  | apply_tick_events()        | RhaiEngine sandboxed scripts
  |                            | worker turn: command -> bus.poll -> ticks.poll
```

Native plugins implement `FractalExtension` directly. Rhai scripts are wrapped
by `rhai::RhaiEngine`, which bridges host API calls back through the command
channel. `drain_plugin_results` processes at most 64 results per frame to avoid
stalling the main loop, and flips a plugin to `Degraded` on a `PluginResult::Error`.

**One worker turn** (`plugin_thread_loop`): wait on the command channel no longer than the next tick deadline, run
the command, then `Bus::poll` (settle runs and snapshots, expire calls), then `TickHost::poll`, which ticks every
due scheduler and then runs each running seat's bus and network dispatch (§bus-seat). The worker never builds a
broker: the binary pre-inserts the `TickHost` and `BusHandle` and builds the one `Net` (§bus-seat); the lab
pre-inserts a `TickHost` on its `SimClock`. Exit stops every seat `host-stopping` (OD-E1 order).

**Module map.** Third-party guests are 2.0 components only (`wasm/component`, src/wasm/AGENTS.md); the 1.0 core
ABI is retired, so `WasmEngine` links no imports. The integration bus is `bus/` (src/bus/AGENTS.md), the extension
network client is `net/` (src/net/AGENTS.md), and scoped bindings are `bindings/` (src/bindings/AGENTS.md).
Governance and the instance lifecycle are `lifecycle.rs` (§governance, §update), time is `tick.rs`
(§tick-scheduler, §bus-seat) and the write window is `transaction.rs` (§write-window). The local administration
channel for `fe-ext admin` is `admin/` (§admin).

## §admin

`admin/` is the shared half of the local extension-administration channel: wire protocol and OS-user transport,
used by the app's listener and by `fe-ext`. It is not an API surface and never mounts on the network listener
(extension administration is GUI plus this local channel only; consent is always confirmed in the GUI). Platform
deps are target-gated in `Cargo.toml` (`windows-sys` 0.61 for the pipe DACL and SID checks, `libc` for `geteuid`).
Contract `docs/extensions/admin-channel.md`; rationale in `src/admin/AGENTS.md`.

## §lifecycle

This is the 1.0 core-module path, kept for first-party fixtures and the signed-package journey. 2.0 components start,
update and uninstall through §update under the R19 gate (§governance).

`lifecycle.rs::PluginManager` wraps a `WasmEngine`, `AotCache`, and
`PluginRegistry` behind the four lifecycle ops. State flow:

```text
install_plugin      → verify signature → AOT compile → register in registry (Installed)
activate_with_bytes → load from AOT   → instantiate  → call on_activate → Active
deactivate_plugin   → call on_deactivate → registry Deactivated
uninstall_plugin    → call on_uninstall  → remove from registry + delete files
```

Notes:

- Install verification is ed25519 publisher signatures (see §signed-packages);
  the old optional BLAKE3 `expected_hash` check is gone. The capability manifest is parsed from the
  signed `manifest_bytes`, never passed beside them, and install records `{content_hash, manifest}`.
- `activate_with_bytes` runs only that record: bytes whose hash differs answer `PayloadMismatch`,
  a plugin with no record answers `NotVerified`, and the token is minted from the signed manifest.
  Install and activation both need admission (§admission-gate); deactivate and uninstall never do.
- There is no `activate_plugin`: activation is `activate_with_bytes`, because the
  Wasm bytes must come from plugin package storage.
- `deactivate`/`uninstall` call the plugin's `on_deactivate`/`on_uninstall`
  exports best-effort (errors ignored) — teardown must always complete.
- `uninstall_plugin` takes the original `wasm_bytes` because AOT-cache
  eviction is keyed by the hash of the source bytes.

## §type-unification

Previously `fe-plugin` carried type definitions parallel to `fe-sdk`. It no
longer does. `lib.rs` re-exports the canonical `fe-sdk` data types
(`NodeSnapshot`, `PropertyValue`, `PropertyBag`, `SceneChange`,
`SceneChangeBatch`) so downstream code has one place to import them.

Key change: `FractalExtension::on_scene_change` now takes/returns
`fe_sdk::SceneChange` (the stable SDK mirror) instead of the engine-internal
`fe_runtime::messages::SceneChange`. The extension-author-facing trait should
reference the stable type; no external crate implements `FractalExtension`
(verified: only `fe-plugin` + `fe-plugin-test` touch these types, and
`fe-terrain` uses only `fe_sdk::ui` / `fe_sdk::api`), so this is a
non-breaking, behavior-preserving swap.

What is **not** unified: the concrete host structs `context::PluginContext` and
`transaction::PluginTransaction`. These are the runtime layer (crossbeam
senders, capability tokens, pending-op batching) and intentionally differ from
`fe-sdk`'s object-safe `PluginContext`/`PluginTransaction` *traits*, which are
the extension-author contract. Different layers, kept separate on purpose.

## §host-env

`host_env.rs` defines `HostEnv` — the bundle of binary-injected service trait
objects (`Arc<dyn ExtensionStorageApi>`, `Arc<dyn ExtensionQueryApi>`). It is the
one place capability gating + host-boundary validation happen, so the Rhai and
WASM paths share exactly one fail-closed policy:

1. capability check (`token.has_capability`) — fail closed with `CapabilityDenied`,
2. input validation (`fe_sdk::storage::validate_*`, `fe_sdk::query::validate_query_len`),
3. SELECT-only guard for queries (`fe_sdk::query::is_select_only`),
4. delegate to the injected trait object (or `NotAvailable` if none injected),
5. cap the query result at `MAX_RESULT_ROWS`.

The engine never depends on `fe-database`; the binary constructs a `HostEnv`
(mirroring the existing `ApiExtensionHandle` injection pattern) and attaches it
to `PluginContext` via `with_host_env`. `PluginContext::new` keeps its original
4-arg signature (empty `HostEnv`) so existing call sites and tests are unchanged.

## §storage-query

Two capability-gated host surfaces, registered in `rhai/storage_api.rs` and
`wasm/host_imports.rs`:

- `node_get_properties(node_id) -> Map`
- `node_set_property(node_id, key, value)`
- `query_select(sql, params) -> Array` (SELECT-only)
- `ext_storage_get(key) -> value` / `ext_storage_set(key, value)` (per-extension KV,
  namespaced by `plugin_id`)

Gating differs by runtime by necessity:

- **Rhai**: the engine is built per-plugin, so gating is *registration-time* —
  a missing grant means the function is **never registered**. Calling it then
  raises a clean Rhai "function not found" error, never a panic. (Defense in
  depth: `HostEnv` re-checks the grant anyway.)
- **WASM**: the 1.0 core imports this section first described are retired with
  the core ABI. A 2.0 component reaches the scene and `kv` through the component
  host's imports and guarded SQL through the bus `host/query` call, each gated at
  call time against the instance token (src/wasm/AGENTS.md, src/bus/AGENTS.md); a
  denial is a typed `host-error`, never a trap.

All host-fn failures surface to the script as typed errors *and* to `tracing`
(`rhai/storage_api.rs::to_script_error`, and the `tracing::warn!` calls in the
WASM imports). Fuel/op limits are unchanged: the new Rhai fns run under the same
`on_progress` op cap; the new WASM imports run under the same per-tick fuel.

## Capability manifest

`capability.rs` gains a `capabilities: Vec<String>` grant list on both
`CapabilityManifest` and the minted `CapabilityToken`, plus
`has_capability(&str)`. Absent = denied (fail closed). Canonical grant strings
live in `fe-sdk` (`CAP_STORAGE_READ`, `CAP_STORAGE_WRITE`, `CAP_QUERY_SELECT`).

## WIT

The 1.0 `query-api` (`wit/hexon-plugin.wit`) is retired with the 1.0 core ABI;
see §wit-resolution for the running contract.

## §wit-resolution (OD-2)

Two WIT files exist; parse one at a time (`fe-plugin/wit/AGENTS.md`).
`hexon-plugin.wit` (`fractalengine:plugin@1.0.0`) carries a SUPERSEDED header and
stays only for first-party test fixtures. `fractal-plugin-2.wit`
(`fractalengine:plugin@2.0.0`, D-X1) is the running third-party ABI: the component
host binds it with `wasmtime::component::bindgen!` (world `extension`, plus the
optional `hooks` export of world `extension-with-hooks`; src/wasm/AGENTS.md). A core
module is refused as `unsupported-contract`. Any WIT edit needs the matching host
import or export change in `wasm/component/` in the same commit.

## §capability-policy (auth_policy_pattern_20260710)

`host_env.rs::require()` now delegates to the shared policy engine:
`CapabilityToken::to_auth_context()` (capability.rs) bridges the token into
`fe_policy::AuthContext::Capability`, and a static `PolicyEngine` holding
`fe_policy::CapabilityPolicy` makes the decision (capability name travels as
`Action::Custom`). Behavior is identical to the old inline `has_capability`
check — fail closed, same `HostApiError::CapabilityDenied` — but the decision
and its log now come from the one engine every entry point shares. The
`CapabilityManifest`/`CapabilityToken` types themselves are unchanged by
design (spec: they were already the right shape).


## §scene-host

**One admitted context per `(petal, package)`.** `PluginContexts::stage_instance(key, ..)` refuses while another
instance id holds that `InstanceKey`, and `admit` re-checks, so a local bind and a binding-started instance of one
package never both run at a petal (bindings.md §5.1); an update re-stages under the same id. Staging builds a context
with a live lease but leaves it unadmitted: `TickHost::launch`/`update` `admit` it once the guest activated, or
`discard` it on any refusal (§update). `instance_at(key)` names the admitted holder.

PluginContext holds injected NodeReader and TransactionSink services from the
shared SDK contracts. Empty contexts fail closed. Reads enforce the bound petal,
stored full ancestry, token grants and property-key projection. Queries accept
only the bounded NodeQuery vocabulary. Native and Rhai transaction calls return
a sink receipt after persistence; a channel enqueue is not a committed write.
The runtime channel adapter and production database wiring are separate consumers
of this same boundary; do not restore placeholder reads or success acknowledgements.


### Production and command origins

The GUI binary installs PluginHostPlugin with a DB-backed SceneBackendFactory.
PluginContexts::bind is the trusted admission entry point for a reviewed manifest;
PluginManager::with_contexts uses the same resource for activation. No filesystem
plugin auto-loader or package-management UI is added by this library slice.
The unused activate_plugin placeholder was removed; the real bytes activation
path validates lifecycle state and injects services before running the callback.

Every queued command is wrapped by its PluginContext with an opaque host-minted
PluginSession. The runtime resolves that origin and rejects mismatched plugin
labels, nested envelopes and unbound raw commands. Supplied plugin_id strings
never select another instance's capabilities. Stop/uninstall revokes queued
origins. Native/Rhai calls wait for the actual injected sink; the WASM read and
write imports use that same context rather than fabricated data or receipts.
WASM's existing core ABI has a fixed response buffer at offset zero, now NUL
terminated; reads return -1 on failure and existing void writes warn on failure.
The 2.0 component host is bound to `fe-plugin/wit/fractal-plugin-2.wit` (header `BOUND`; see `fe-plugin/src/wasm/AGENTS.md`).

A DB command that was not enqueued is a definite refusal. A timed-out/disconnected
commit reply is explicitly an unknown outcome and must be reconciled before any
retry; no count or successful receipt is invented. A local atomic commit does
not guarantee peer replication delivery. The old arbitrary SELECT/KV HostEnv
services remain independently injected; this scene bridge supplies no raw SQL
or persistent KV bypass.

Opaque sessions share a revocation flag across direct context clones; unbind
refuses both queued and later direct calls. Duplicate instance IDs cannot replace
an admitted context. Scene host functions enforce grants at call time (the older
storage/query registration gates remain unchanged). Rhai scene property conversion
rejects non-finite/custom values and depth over 16 instead of silently storing
null; explicit unit/null remains JSON null.

AppExit shuts the instance registry, revokes all direct clones, and prevents new
bindings. The command worker polls shutdown at a bounded interval rather than
relying on channel disconnection: admitted contexts themselves hold senders.
This breaks the active-context sender/worker lifetime cycle on application exit.


The session's shared PluginLease also travels through production DB admissions,
so stopping an instance refuses queued work not yet admitted to commit. A commit
already handed to the atomic transaction helper may finish and returns its true
receipt. Reads check revocation before returning payloads. Full result queues use
100 ms send retries with shutdown checks, so backpressure cannot pin worker exit.

Every WASM string argument decodes through a fallible, bounded UTF-8 span read.
Negative, oversized, absent-memory, out-of-bounds and invalid UTF-8 inputs refuse
the entire import before any backend call. JSON-labelled arguments must parse;
invalid JSON never becomes a string, empty filter or empty parameter map. Valid
empty spans remain empty strings for the existing legitimate ABI semantics.

## Terminal maintenance ownership

`PluginHostService` retains the plugin runtime join. Host maintenance first revokes `PluginContexts` leases, then requests stop and joins the runtime while DB/results continue draining. Revocation is not rollback of an already-admitted commit; the final serial DB barrier remains necessary.

## §admission-gate

Package admission is opt-in (plugin_host_revival OD-4, retargeted by docs/extensions/governance.md
§Host switch): the host thread always runs, and the binary's `plugin-host` Cargo feature plus
`FE_PLUGIN_HOST_ALLOW=1` decide `PackageAdmission` once at startup (`fractalengine/src/AGENTS.md`
§plugin-host). `PluginManager` defaults to admission off; `with_admission(true)` is the only way on.
Off, `install_plugin`, `install_from_registry` and `activate_with_bytes` answer `LifecycleError::HostOff`
before touching any bytes. This is the security boundary while the sandbox is un-audited (SECURITY.md).

## §network-deny

OD-1 closes deny-by-default with no grant path. `CapabilityManifest::from_json` refuses any
`network_scope` other than `"none"` and any `external_http: true`, and `validate_access` denies
`HttpRequest` whatever the token says, so a manifest smuggled past the parser is still denied at use.
Manifest v2 replaces both fields with declared `ingress[]`/`egress[]` (docs/extensions/manifest-v2.md).

## §signed-packages

`package.rs` + `trust_roots.rs` implement seam S-PLUGIN; `lifecycle.rs::install_plugin` is the call site.

- **Signed message** = `manifest_bytes || blake3(payload)` (32 raw digest bytes). A package carries a
  list of `{key_id, signature(hex)}` (at most 16). It is accepted when at least one entry verifies
  against a trust root AND no trust-root entry fails: a failing trusted entry is tampering, never
  outvoted (docs/extensions/package-format.md §Signatures). Entries from unknown keys are ignored, so
  a second party can counter-sign without a format change.
- **Identity is signed.** The signed manifest must carry `"id"` equal to the installing package id
  (`IdMismatch` otherwise). The signature does not cover the id separately; the manifest's `id` is the
  binding, the same rule manifest v2 states (`id` = `package_id`). Without it a mirror could serve a
  revoked, validly signed build under a new id (P2P-QA steer 2026-11-08 20:10).
- **Verification is strict**: `verify_strict` everywhere, and a weak (small-order) public key never
  loads as a trust root, since such a key accepts forged signatures.
- **Errors are typed** (`PackageError`): `Unsigned`, `UntrustedKey` (no entry names a trust root),
  `InvalidSignature` (a trusted key's signature fails or is malformed: never dev-admitted), `IdMismatch`,
  `TooManySignatures`, `Revoked`.
- **Dev flag**: `UnsignedPolicy::AllowDev` exists only under `cfg(debug_assertions)`, so a release
  build cannot construct it; every use logs `tracing::warn!`. It admits unsigned and untrusted-key
  packages only.
- **Trust roots (OD-3)**: `trusted_publishers.toml`, loaded once at startup, no dynamic registration:
  `[[publisher]] key_id, public_key (hex ed25519), revocation (default false), revocation_url (optional)`.
- **Revocation list (public wire contract, schema_version 1, never changed in place)**:
  `{schema_version, issued_at, entries:[{package_id, content_hash, reason, revoked_at}], signer_key_id, signature}`.
  `signature` is ed25519 (hex) over the canonical JSON (keys sorted, compact) of
  `{"entries":...,"issued_at":...}` (`RevocationList::signed_bytes`). Only a trust root with
  `revocation = true` may sign; the host is a consumer, never a publisher. `content_hash` is the hex blake3
  of the payload (`package::payload_hash`).
- **Matching is by hash under ANY package id** (governance.md §Kill switch layers): `RevocationSet::is_revoked(hash)`
  ignores the entry's `package_id`, so a renamed copy of a revoked build stays revoked. Hashes compare
  lowercase. `is_revoked_any` over `PackageFacts::hashes()` (`content_hash`, `component_digest`,
  `plaintext_digest`) is the check every sealed 2.0 build goes through (R19 row 5 at every activation, kill-list
  arrival, `ComponentHost::compile_unrevoked`), so a kill list naming any one of them refuses it.
  `RevocationSet::extend` unions the lists of several revocation-capable roots.
- **Trust-root roles.** `[[publisher]] role = "release" | "publisher" | "dev"` (absent = `publisher`; an unknown
  value is a parse error); `TrustedKey::tier()` is the `TrustTier` R19 and the consent dialog show. `dev` roots load
  only through `from_toml_str_with(text, true)` in a debug build, so a release binary never trusts one.
- **Freshness (OD-8).** The intake is `PluginManager::accept_revocation_list` (§governance): it keeps the newest
  `issued_at` per signer (compared as parsed RFC 3339 UTC values) and refuses an older list, since a replayed old
  list would silently un-revoke. `TrustRoots::verify_revocation_list_fresh` is a stateless helper for callers
  without a manager and has no production caller.

## §registry-install

`registry_client.rs` (S4) fetches over registry protocol v1 and hands the bytes to `install_plugin`.

- **Protocol first.** `/health` must answer `X-Hexon-Registry-Protocol: 1` (or `protocol_version: 1`
  in the body, read under a 64 KiB cap), or config.json (`check_protocol`). The download response's header is checked only
  when present.
- **Identity triangle.** `fetch_package(uri)` refuses a package whose `signatures.json` `package_id`
  differs from the requested uri's id (`{id}` or `{id}@{version}`, the registry's `split_uri`);
  `install_plugin` then requires the signed manifest's `id` to equal that same id.
- **Bounded.** Connect timeout 10 s, request timeout 120 s, archive ≤ 64 MiB, JSON members ≤ 1 MiB.
- **Listed hash (store-client v1).** `FetchedPackage::manifest_hash` is `fe_format::integrity::content_hash` of the
  archive's manifest member (`MANIFEST_PATH`), the value the registry lists as `content_hash`.
  `fetch_package_listed(uri, listed_content_hash)` refuses a package whose manifest hash differs from the listed
  one, or that has none (fail closed). Any caller holding a listed hash must use it; the interim 1.0
  `install_from_registry` still calls bare `fetch_package` and relies on the payload-hash check alone.
- **Licensed packages are refused** (`EntitlementUnavailable`, `registry_client.rs:295`) by this v1 client, which fails
  closed on every paid `license.json`. `fe_format::entitlement` has shipped; the paid path is the v2 call site, governance
  row 6 (`fractalengine/src/extension_host/governance.rs`, feature `entitlement`).
- **Retired v1 layout** (still read by this client, `registry_client.rs` `MANIFEST_MEMBER`/`PAYLOAD_MEMBER`/
  `SIGNATURES_MEMBER`): `plugin/manifest.json` (CapabilityManifest 1.0 + `id`), `plugin/payload.wasm`
  (core module), `plugin/signatures.json` (`{package_id, content_hash, signatures}`), read with
  `HexonArchive::read_member` and no sealed-hexon check. The current layout is `docs/extensions/package-format.md`
  (sealed `HexonType::Plugin`, `plugin/extension.json`, `plugin/component.wasm`, counter-signatures,
  `content_hash` over the sealed manifest). The live registry path is the protocol-v1 `RegistrySource` and
  `RemoteRegistryClient` in `fe-hexon/src/remote.rs` (trait in `fe-hexon/src/registry_source.rs`), not this module.

## §consent-ux

The consent modal lives in fe-ui (`fe-ui/src/AGENTS.md` §plugin-consent). fe-plugin supplies the data it
shows: `CapabilityManifest` carries the verse, petal and property scope, the named capabilities and the
network posture. `validate_access` enforces an empty scope as a wildcard, and the modal must render that as
"All". The 2.0 consent dialog (capability digest, publisher and trust tier) is driven by the binary's ops drain,
which builds the `PluginManager` (`fractalengine/src/plugin_bridge.rs`, `extension_host/ops_drain.rs`).

## §tick-scheduler (also §tick; extension_platform P1.3, R4/R5)

`tick.rs` owns time. The plugin worker (`plugin_thread_loop`) waits on its command channel no longer than
`TickHost::next_wait` (earliest deadline, capped at 100 ms), then calls `TickHost::poll`; ticking is never a Bevy
system, so the reactive render loop wakes only for the DB results writes produce. `apply_tick_events` (Update) is the
only Bevy side: it maps `Degraded`/`Stopped` events onto `PluginRegistry`.

- **One scheduler per instance** (`TickScheduler`): the first tick is due one cadence after start. When behind, it
  skips to the latest due instant and counts the rest (`ticks_skipped`); ticks never queue. Invariant:
  `ticks_run + ticks_skipped` = due instants elapsed (the 500 ms x 60 s acceptance reads 120 +/- 1).
- **One flush in flight.** A due instant while the previous flush is unanswered is skipped (backpressure, not a
  queue). A flush silent for `FLUSH_TIMEOUT_MS` is reported `unknown` in the next `tick-info` and never retried.
- **Injected `Clock`.** `WallClock` in production; `ManualClock` in the journey; fe-sim hosts its `SimClock` behind the
  same trait by pre-inserting a `TickHost` before `PluginHostPlugin` (build reuses an existing one).
- **Fuel.** `TickInstance::refuel(budget.fuel)` runs before every `on-tick`; every other hook refills its own
  `hook_fuel` on entry, so no hook inherits another's leftover. The J1 row in `wasm/mod.rs::fuel_budget_refill`
  drains a component's store to zero before each due instant: a tick runs only on the scheduler's refill. A fault
  (`FuelExhausted` or a trap) discards the window unflushed, stops ticking, unbinds the context (the lease is gone, so
  no final flush) and reports `Degraded`.
- **Write window.** Each instance stages into one `SharedWindow` the scheduler flushes (§write-window).
- **Deactivation (OD-E1 order).** `TickScheduler::stop(reason)` first waits (bounded, `FINAL_FLUSH_WAIT`) for the
  commit already in flight, so an admitted commit lands whole with its true receipt; then `on-deactivate`; then one
  final flush unless the reason is `RoleLost`, `PolicyDenied`, `Revoked` or `Degraded` (`lease_gone`: staged writes
  are discarded). `TickHost::stop` leaves the seat before this and unbinds the context after it, so the lease is
  revoked only once the drain is done. Rows already written stay (no withdrawal).
- **No-tick instances.** A manifest without a `tick` block gets `TickScheduler::for_budget(None)` (or
  `without_ticks`): it never ticks and has no deadline, but seats, stops and drains like any other (§bus-seat).
- **Seams for others.** The component host implements `TickInstance` on `InstanceHandle`; fe-database implements
  `StagedCommitSink` over `PluginCommit`; the binary builds each scheduler with a `ContextTickSink` and starts it
  through `TickHost::launch`/`update` (§update). `ContextTickSink` commits on a short-lived thread so the worker
  never blocks on the DB. Status reads go through per-instance counter handles kept outside the host lock, so a
  status read never waits on a running hook.

## §write-window

`StagedWindow` (`transaction.rs`) is the per-instance write window, shared by the guest's `txn`/`readings` imports
and the scheduler as `SharedWindow` (`window_for_token` sizes it from the token's effective `TickBudget`; no `tick`
block means zero limits).

- **Stage time.** `stage_op`/`stage_reading` run the SDK validators and the byte cap (`MAX_WINDOW_BYTES`) and change
  nothing on refusal; the host import authorizes the op or reading against the token first
  (`PluginContext::authorize_op`/`authorize_reading`: scene write kind, property scope, declared metric).
- **Flush.** A due tick takes the whole window as ONE `StagedFlush` (ops + readings). `commit_staged` re-checks the
  token and commits through `StagedCommitSink` atomically (fe-database `PluginCommit`); with no readings and no staged
  sink it falls back to the legacy `TransactionSink`. A flush whose readings anchor on a node the same batch deletes
  is refused whole, never half-applied (R6, G1-C8).
- **Discard.** `txn.discard`, any hook fault and a `lease_gone` stop drop the window unflushed.
- **Provenance.** The flush's DB admission carries the verified build's `{package_id, content_hash}`
  (`PluginAdmission::with_package`, stamped by the binary's scene factory) and the bind-time source class; the guest
  sets neither. Readings staged during an `ingest.decode` call must also carry that call's `DecodeStamp`
  (`payload_id`, source class `decoder`); the per-call stamp lives on the instance's hook path (§hook-calls).

### Manifest v2 to token

`CapabilityToken::mint_v2(plugin_id, &ExtensionManifest, &V2Binding)` is the only v2 constructor. Scopes come from
the binding (verse and petal), never from the manifest; `strict_scopes = true` makes an EMPTY scope list grant nothing
(v1 keeps empty = wildcard). `scene.read` grants `storage.read` + `query.select`; any `scene.write` kind grants
`storage.write` (and `storage.read`, since writes authorise the petal through the reader) and the exact kinds live in
`V2Grants::scene_write`. Readings are allowed only for declared metrics, and the source class is fixed at bind time
(a class set without the operator's pick refuses to mint). `V2Grants::tick` carries the effective budget
(defaults and host ceilings from `TickBudget::from_manifest`).

### Registry states

`PluginState` gains `Disabled(reason)` (operator, policy, role loss; only `enable` leaves it, landing in
`Deactivated`) and `Revoked(reason)` (terminal: `activate`, `enable` and `disable` refuse; only `unregister`).
`degrade` moves Active to `Degraded` and leaves other states alone.

## §bus-seat (G1, P1 gate)

An `InstanceSeat { bus, spec, bus_hooks, net: Option<NetSeat> }` joins an instance to the integration bus and, when
given, the network broker. For a 2.0 component both hook objects are clones of the same `InstanceHandle` the
scheduler ticks (`BusHooks` in `wasm/component/bus_bridge.rs`, `NetHooks` in `net_bridge.rs`), and its broker is a
`HostBroker` (bus + net imports). `TickHost::launch`/`update`/`add_seated` join the seat before anything runs:
`Bus::register_instance` first, then `Net::register_instance` (mount endpoints, start dial-outs); a refused mount
leaves the bus again and schedules nothing. `add_on_bus` is the bus-only shorthand.

**Dispatch.** Each worker turn runs `bus.poll()`, then `ticks.poll()`: after the schedulers tick, every running seat
gets `Bus::dispatch` (at most `BUS_CALLS_PER_POLL` units: call results, commands, one event batch) and then
`Net::dispatch` (at most `NET_CALLS_PER_POLL`: ingress frames, connection events, egress results). So what the
broker settled in a turn is delivered in that turn, and only while the scheduler is `Running`. A guest that faults
in a bus or network hook is degraded in the next `poll` even if it never ticks.

**One `Net` per host, owned by the binary.** The binary builds it (`extension_host/net_sources.rs`) on its own
small runtime, with the operator's credential store, and puts it in every seat; fe-api's ingress mounts and the
dial-out tasks reach the same broker. The worker never builds a second `Net`: two brokers would split mounts from
dispatch. A host without one (the lab) seats with `net: None`, and egress or ingress imports answer
`not-available`.

**Stop order (OD-E1).** The seat leaves FIRST: the bus answers queued calls `not-available` and ends subscriptions
with `revoked`/`policy-denied`/`role-lost`/`instance-stopped`; the network broker unmounts, stops dial-outs and
closes accepted sockets (1001). Then the scheduler drains the in-flight commit, runs `on-deactivate` and its final
flush unless the lease is gone (§tick-scheduler). A degraded instance leaves both brokers in the `poll` that degrades
it.

**No-tick seats.** A manifest without a `tick` block seats through `TickScheduler::for_budget(None)`, so pure
bus or ingress extensions are hosted like any other (G1-C10). **Uninstall:** `TickHost::uninstall` stops `unbound`,
runs `on-uninstall` with one final flush on the live lease, then forgets; `forget` does the same without
`on-uninstall`. Both call `Bus::forget` and `Net::forget`, so no counters outlive the installation.

## §update (P4.3, start and uninstall)

The 2.0 instance path in `lifecycle.rs`. A `StagedInstance` is built by the binary (component instantiated, context
staged with `stage_instance`, scheduler and seat assembled) and nothing has run yet.

- **Start.** `start_instance` (the operator's local bind or Enable) and `start_binding_instance` (the binding
  activator, id `binding_instance_id(installation, petal)`) run R19 (§governance), then `bind_staged`: the staged
  component digest, context and seat must be exactly the build and scope R19 judged, or `launch-mismatch`. Then
  `TickHost::launch`: join the seat, `on-install` on a first activation, `on-activate`, admit the context, schedule.
  Any refusal leaves the seat and discards the context, so a refused build holds no lease. A binding instance also
  needs local acceptance in its verse and no local Disable (rows 12-13, never satisfied by a replicated row).
- **Update.** `update_instance` runs R19 for the NEW build (consent must cover its capability digest), then
  `TickHost::update`: (1) the running build leaves the shared key, drains its in-flight commit, runs
  `on-deactivate(updating)` and its final flush on its still-live lease; (2) the new build starts on its staged
  context under the same key; (3) only then is the old lease revoked and the new context admitted. If the new build
  refuses, it is discarded and the old one rejoins its seat, runs `on-activate` again and resumes ticking
  (`RolledBack`); if even that fails nothing runs (`PreviousStopped`, the registry hears `stopped`). Two builds are
  never admitted at once.
- **Publisher change.** A build whose publisher or signing root differs from the installed one is a NEW install with
  fresh consent, never an update (steer 2026-11-09 01:50). `ConsentRecord` holds no publisher, so the caller (the ops
  drain) must route that case to install before reaching `update_instance`.
- **Same installation, same package.** `update_instance` refuses `launch-mismatch` (and discards the staged context)
  when the running build under that id belongs to another installation or package (R-LIFE). Without it, a caller
  that picked the wrong id could swap a different package into a running seat under one consent check.
- **Binding-instance cap.** The activator counts eager binding instances against the per-installation cap too
  (bindings.md §5.6 "at most 64 binding-started instances per installation per host"). Eager ones are never
  idle-stopped or evicted, but a start past the cap is held at `capacity` until a binding instance stops in any
  verse (R-LIFE). Without it, a Manager binding `petal.activate` at a verse with N petals started N instances on
  every accepting peer.
- **Local Disable holds.** `disable_instance(installation_id)` stops every instance of the installation and keeps it
  disabled (bindings cannot restart it) until `start_instance` (the operator's Enable) or `enable_installation`.
- **Uninstall.** `uninstall_instance(id)` covers every instance ever started for the installation: `TickHost::uninstall`
  (§bus-seat), then the registry entry goes. Consents, acceptances and the package store are the caller's.

## §governance (R19, R23, OD-E1)

Contract: docs/extensions/governance.md. `lifecycle.rs::Governance` holds this host's local records (consents,
kill-list overrides, acceptances, local Disable, the revocation intake); the binary hydrates them from fe-database at
startup and writes every change through the audit trail. None of it is replicated.

- **R19 activation preconditions, in order** (`Precondition::ORDER`, first failure wins, stable codes): host-off,
  not-local-identity, package-invalid, untrusted-signer, revoked, entitlement-required, policy-denied, role-denied,
  consent-missing. `check_activation` judges a non-local actor on not-local-identity before host-off, so a remote
  caller learns nothing about this host's switches. Row 5 is `is_revoked_any` over the build's digests, unless the
  verse Owner recorded a kill-list override for that verse and build on this host.
- **Gates.** Rows 6-8 go through `GovernanceGates` (entitlement, verse + local policy, role at the petal: Editor+ for
  a writing build, Viewer+ otherwise). `FailClosedGates` is the default and refuses rows 7-8 and licensed builds, so a
  host that never wired real gates runs nothing third-party; the binary installs its own (`with_gates`).
- **Kill-list override.** Refuse by default; an explicit, audited verse-Owner override is scoped to that verse and
  that build, on the Owner's own host only. Every other peer keeps it revoked and its bindings unresolved. There is
  no withdraw action (user decision 2026-11-08 22:00).
- **R23 kill switches.** (a) local Disable per installation or per instance; (b) Disable all (`FE_EXT_DISABLE_ALL=1`
  or `disable_all`), after which activation answers host-off until `enable_all`; (c) a verse policy change re-judges
  every instance in that verse (`reevaluate_verse_policy`, stop `policy-denied`); (d) a kill list arriving through
  `accept_revocation_list`: verified against revocation-capable roots only, the newest `issued_at` kept per signer, an
  older list refused as `Stale`, the same instant unioned (fail closed), then every running build it names stops
  `revoked` unless overridden in its verse. Nothing restarts by itself after any switch.
- **Stop semantics (OD-E1, no withdrawal).** Every stop goes through `stop_instance` -> `TickHost::stop`: the seat
  leaves, the in-flight commit settles, then the lease is revoked. Rows the instance wrote stay, with their
  `{package_id, content_hash}` provenance kept for audit and filtering; nothing is withdrawn or re-judged.

## §hook-calls (bindings.md §6-7)

A component that declares `provides` must export `fractalengine:plugin/hooks@2.0.0`: `ComponentHost` records the
export at admission (`ExtensionComponent::exports_hooks`) and `check_provides(manifest)` answers `missing-export`
otherwise; callers holding the manifest run it right after `compile`. `instantiate` creates the raw instance once,
binds the world and, when exported, the `HooksGuest` on that same instance (`wasm/component/hooks.rs`; per-hook
caps and deadlines are `hook_limits`, from bindings.md §6). `ExtensionInstance` keeps it (`with_hooks`) and serves
`call_hook(request, stamp)`, also on `InstanceHandle`: refuse while degraded, run `HooksGuest::call` with its own
fuel refill and frozen clock, degrade on `HookCallError::Fault` exactly like a tick fault, and apply the
`DecodeStamp` (if any) to every reading staged until the call returns. A guest refusal leaves the built-in serving.
Host-initiated calls (`configure`, `ingest.decode`, `blob.*`, `egress.*`, `petal.map`) come from the binary's
binding worker, never the DB thread or the render loop.

**Decode bytes (steer 2026-11-09 02:25).** `bindings/decode.rs::read_within_cap` owns the order, not the host:
`DecodeHost::declared_files` reports each asset row's size, type and BLAKE3 without reading a blob; the sum is
checked against the 1 MiB hook cap before any read (`TooLarge`); then each file is read through
`read_bounded(file, size + 1)` and kept only when its length and BLAKE3 equal the row (`Mismatch`). A declared
size that lies is cut off, not trusted, and no extension sees a byte the store cannot vouch for.
