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
  |                            | RhaiEngine sandboxed scripts
```

Native plugins implement `FractalExtension` directly. Rhai scripts are wrapped
by `rhai::RhaiEngine`, which bridges host API calls back through the command
channel. `drain_plugin_results` processes at most 64 results per frame to avoid
stalling the main loop, and flips a plugin to `Degraded` on a `PluginResult::Error`.

## §lifecycle

`lifecycle.rs::PluginManager` wraps a `WasmEngine`, `AotCache`, and
`PluginRegistry` behind the four lifecycle ops. State flow:

```text
install_plugin  → verify signature → AOT compile → register in registry (Installed)
activate_plugin → load from AOT   → instantiate  → call on_activate → Active
deactivate_plugin → call on_deactivate → registry Deactivated
uninstall_plugin  → call on_uninstall  → remove from registry + delete files
```

Notes:

- Install verification is ed25519 publisher signatures (see §signed-packages);
  the old optional BLAKE3 `expected_hash` check is gone. The capability manifest is parsed from the
  signed `manifest_bytes`, never passed beside them, and install records `{content_hash, manifest}`.
- `activate_with_bytes` runs only that record: bytes whose hash differs answer `PayloadMismatch`,
  a plugin with no record answers `NotVerified`, and the token is minted from the signed manifest.
  Install and activation both need admission (§admission-gate); deactivate and uninstall never do.
- `activate_plugin` only validates registry state
  (`Installed`/`Deactivated`/`Degraded(_)` → activatable) and then returns
  `NotImplemented`; the real path is `activate_with_bytes`, because the Wasm
  bytes must come from plugin package storage.
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
- **WASM**: the `Linker` is shared across plugins, so gating is *call-time*
  against the store's capability token. A denied/failed call logs via `tracing`
  and returns a sentinel (`-1` / no-op), never a trap.

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

`wit/hexon-plugin.wit` gains a `query-api` interface **alongside** `node-api`
(additive to `@1.0.0`). Values cross the boundary as JSON strings so no new
record types are forced on existing `node-api` consumers. The world imports it
(`import query-api;`); guests that don't use it are unaffected.

## §wit-resolution (OD-2)

Two WIT files exist; parse one at a time (`fe-plugin/wit/AGENTS.md`).
`hexon-plugin.wit` (`fractalengine:plugin@1.0.0`) carries a SUPERSEDED header and
stays only for first-party core-ABI test fixtures. `fractal-plugin-2.wit`
(`fractalengine:plugin@2.0.0`, D-X1) is the third-party contract. Neither is the
running ABI: the WASM host still registers core-ABI `func_wrap` imports with `-1`
denial sentinels (`wasm/host_imports.rs`), and no component-model bindgen exists.
Until extension_platform P1.1 lands bindgen, make no third-party publishing claim
that rests on the WIT. Any WIT edit needs a matching `host_imports.rs` change or an
UNSTABLE marker.

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


## Scene host

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
This does not implement the aspirational component-model WIT ABI.

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
  lowercase. `RevocationSet::extend` unions the lists of several revocation-capable roots.
- **Not built yet: the OD-8 pull.** Nothing fetches `revocation_url` today (`TrustRoots::revocation_sources`
  has no caller). The puller must keep the newest `issued_at` per signer and refuse an older list, or a
  replayed old list silently un-revokes; it lands with the kill switch (extension_platform E10).

## §registry-install

`registry_client.rs` (S4) fetches over registry protocol v1 and hands the bytes to `install_plugin`.

- **Protocol first.** `/health` must answer `X-Hexon-Registry-Protocol: 1` (or `protocol_version: 1`
  in the body, read under a 64 KiB cap), and the download response must carry the header too.
- **Identity triangle.** `fetch_package(uri)` refuses a package whose `signatures.json` `package_id`
  differs from the requested uri's id (`{id}` or `{id}@{version}`, the registry's `split_uri`);
  `install_plugin` then requires the signed manifest's `id` to equal that same id.
- **Bounded.** Connect timeout 10 s, request timeout 120 s, archive ≤ 64 MiB, JSON members ≤ 1 MiB.
- **Licensed packages are refused** (`EntitlementUnavailable`) until hexon_entitlements_20261031 S1
  ships `fe_format::entitlement`. Then enable fe-format's `entitlement` feature and, for
  `LicenseType::Paid`, call `verifier.verify(token, package_id, content_hash)` and
  `open_paid_entry(content_key, payload)` before the hash check: one call site, no new verifier code.
- **Interim v1 layout**: `plugin/manifest.json` (CapabilityManifest 1.0 + `id`), `plugin/payload.wasm`
  (core module), `plugin/signatures.json` (`{package_id, content_hash, signatures}`), read with
  `HexonArchive::read_member` and no sealed-hexon check. docs/extensions/package-format.md supersedes it
  (sealed `HexonType::Plugin`, `plugin/extension.json`, `plugin/component.wasm`, counter-signatures,
  `content_hash` over the sealed manifest) when extension_platform E9/E13 land; this client then moves to
  fe-hexon as the protocol-v1 `RegistrySource`.

## §consent-ux

The consent modal lives in fe-ui (`fe-ui/src/AGENTS.md` §plugin-consent). fe-plugin supplies the data it
shows: `CapabilityManifest` carries the verse, petal and property scope, the named capabilities and the
network posture. `validate_access` enforces an empty scope as a wildcard, and the modal must render that as
"All". Nothing requests an install yet (review.md #17), because neither binary builds a `PluginManager`.

## §tick-scheduler (extension_platform P1.3, R4/R5)

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
- **Fuel.** `TickInstance::refuel(budget.fuel)` runs before every hook (each hook gets its own refill). A fault
  (`FuelExhausted` or a trap) discards the window unflushed, stops ticking, unbinds the context (the lease is gone, so
  no final flush) and reports `Degraded`.
- **`StagedWindow`** (`transaction.rs`) is the per-instance write window the guest imports stage into, shared with
  the scheduler as `SharedWindow`. Limits come from the effective `TickBudget`; the window holds ops and readings and
  flushes as ONE `StagedFlush`. `PluginContext::authorize_op` / `authorize_reading` check the token at stage time and
  `commit_staged` re-checks at flush through `StagedCommitSink` (ops + readings atomic); with no readings and no staged
  sink it falls back to the legacy `TransactionSink`.
- **Deactivation.** `TickScheduler::stop(reason)` runs `on-deactivate`, then one final flush unless the reason is
  `RoleLost`, `PolicyDenied`, `Revoked` or `Degraded` (lease already gone). `TickHost::stop` then unbinds the context.
- **Seams for others.** E1 implements `TickInstance` over the component store and stages through the shared window;
  E4 implements `StagedCommitSink` over `PluginCommit`; E7 builds the `TickScheduler` with a `ContextTickSink` and
  `TickHost::add`. `ContextTickSink` commits on a short-lived thread so the worker never blocks on the DB.

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

`TickHost::add_on_bus(plugin_id, scheduler, bus, spec, hooks)` is the activation seam that joins an
instance to the integration bus: `Bus::register_instance(spec)` runs first (a refusal leaves nothing
scheduled), then the scheduler is added and the instance's `BusHooks` (for a 2.0 component, the same
`InstanceHandle` the scheduler ticks) is kept as its seat. Each worker turn runs `bus.poll()` and then
`ticks.poll()`, so `dispatch` (at most `BUS_CALLS_PER_POLL` units: call results, commands, one event
batch) delivers what the broker settled in the same turn, and only while the scheduler is `Running`.

Stop order is the OD-E1 rule: the seat leaves the bus FIRST (queued calls answer `not-available`,
subscribers see `ended` with `revoked`/`policy-denied`/`role-lost`/`instance-stopped`), then the
scheduler runs `on-deactivate` and its one final flush unless the lease is gone. A degraded instance
leaves the bus in the same `poll` that degrades it. Counters stay readable by installation until
uninstall calls `Bus::forget` (E10's lifecycle). An extension without a `tick` block has no scheduler
and so no seat yet; the activation path that instantiates components (E10) decides how those are hosted.
