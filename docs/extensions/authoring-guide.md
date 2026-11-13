---
type: guide
title: Extension authoring guide
track: extension_platform_20261107
status: draft (P3.4, slice E12D, reviewed by G3; the CLI is written, not yet built)
---

# Authoring guide

This walks one extension from an empty folder to a signed package that installs on a dev host.
The steps are the same for every kind (see [README](README.md#kinds)). Every command below is the
real `fe-ext` surface ([`tools/fe-ext/src/main.rs`](../../tools/fe-ext/src/main.rs)).

## 1. Scaffold, then start from the manifest

```bash
fe-ext new general my-ext        # or simulation | device-adapter | bi-connector; writes ./my-ext/
```

`new` writes a guest crate, `extension.json`, `fixtures/basic.json` and a journey test, and refuses
to write anything if its own manifest fails validation. The simulation, device-adapter and
bi-connector scaffolds copy the shipped schema examples with your id; the general one is the
template in [`tools/fe-ext/src/new.rs`](../../tools/fe-ext/src/new.rs) (`manifest_for`):

```json
{ "manifest_version": 2, "id": "local.my-ext", "version": "0.1.0", "name": "my-ext",
  "kind": "general", "contract": "fractalengine:plugin@2.0.0", "binding": "petal",
  "scene": { "read": true, "write": ["set_property"], "property_scope": ["my_ext.*"] },
  "tick": { "cadence_ms": 1000, "max_ops": 16, "max_readings": 16, "fuel": 200000 } }
```

The manifest is the whole capability surface: the host grants nothing it does not declare. Four
rules decide most validation errors (full table in [manifest-v2.md](manifest-v2.md#validation)):

- **Absent grants nothing.** A missing section means no access. A *scope* list that is present and
  empty (`scene.write`, `property_scope`, `metrics`, an endpoint's `methods` or `topics`, ...) is
  refused as `empty-scope`, so omit the field instead. Declaration lists such as `ingress: []` may be empty.
- **A writing extension must declare `tick`.** Writes are staged and only a tick flushes them.
  `scene.write` or `readings` without `tick` fails as `cadence-required`.
- **`*` must be written out.** A wildcard in `property_scope` or `metrics` is explicit, never implied.
- **The id is the signed identity.** `id` must equal the `hexon_id` the package is signed under, or
  install refuses it as `id-mismatch`. `fe-ext pack` takes the id from the manifest, so they match.

Then open a worked example, [`extensions/fleet-sim/extension.json`](../../extensions/fleet-sim/extension.json),
and change one thing at a time. Each grant you add is a line on the operator's consent prompt.

## 2. Implement the world

The component implements the `extension` world in [`fractal-plugin-2.wit`](../../fe-plugin/wit/fractal-plugin-2.wit).
It imports ten interfaces (`context` through `log`) and exports four (`lifecycle`, `ingress-hooks`,
`egress-hooks`, `bus-hooks`). The guest may call only what the manifest grants.

In Rust, the guest SDK ([`tools/fe-ext/guest/src/lib.rs`](../../tools/fe-ext/guest/src/lib.rs)) does the
wiring: implement its `Extension` trait (only `on_tick` is required; every other hook has a no-op
default) and call `extension!(YourType)` once. [`extensions/fleet-sim/src/lib.rs`](../../extensions/fleet-sim/src/lib.rs)
is a complete guest in about 230 lines.

### Serving a hook point (`provides`)

A scope can bind your extension to a hook point such as `petal.map` ([bindings.md](bindings.md)).
Declare it in the manifest, then answer the host's calls:

```json
"provides": [{ "hook": "petal.map" }]
```

`provides` is checked at validation: a hook outside the catalogue is `unknown-hook` (V17), and a hook
whose needs the manifest does not declare (bindings.md section 6) is `hook-capability-missing` (V18). Keyed hooks
may list `keys` (an exact key, a `prefix*` or `*`). A provider builds against the `extension-with-hooks`
world: the guest SDK's `hooks` module parses the generated `hook-call` with `HookCall::parse`, matches
the key with `key_matches` and routes it with `route`, answering through `HookOutput` (`json`, `empty`,
`allow`, `refuse`). A service hook also receives a `configure` call at start and on every config
change (`HookCall::is_configure`). A binding grants nothing: the hook runs only on peers that installed,
consented to and accepted your package.

The rule that shapes every guest is stated at the top of the WIT file:

```wit
// - Writes are STAGED into one per-instance window and flushed once per tick as ONE
//   DbCommand::PluginCommit (spec R5). Nothing in this file writes to the store directly.
// - A refused call returns host-error and changes nothing; it never traps.
```

So a guest never "saves" a transform. It stages one, and the host commits it with everything else from that tick:

```wit
stage-set-transform: func(node: node-id, patch: transform-patch) -> result<_, host-error>;
```

Two things to notice:

1. **The `on-tick` hook is the only place a flush is scheduled.** Anything staged in `on-activate`,
   `on-ingress` or `on-command` is flushed after the next tick, not immediately.
2. **Created node ids are not known when you stage.** `stage-create-node` returns an ordinal. The real
   id arrives in the next tick's `tick-info.last-commit` as a `commit-receipt`. Keep the ordinal in `kv` until then.

The install, activate and uninstall hooks receive a `binding` (installation id, petal id, identity DID,
source class). Read the petal from the binding; do not hard-code one. `on-deactivate` receives only a reason.

## 3. Use the host for every effect

- **Readings** go through `readings.stage`. Each reading is validated the same way the REST readings
  door validates it. `source_did` and `source_class` are set by the host, never by you. The SDK's
  `readings_batch::ReadingBatcher` stages what fits the window and keeps the rest for the next tick.
- **Outbound HTTP or MQTT** is queued through `egress`. Results come back later through
  `egress-hooks.on-egress-result`. Calls never block the worker.
- **Inbound traffic** arrives on `ingress-hooks.on-ingress` for each endpoint you declared. Only the
  headers you list in `forward_headers` arrive, and never a credential header.
- **Other extensions and the UI** talk over `bus`. See the [cookbook](cookbook.md#subscribe-to-a-host-stream).
- **Per-instance memory** is `kv`, host-local and never replicated. Declare its size in the manifest first.

## 4. Validate and build

```bash
fe-ext validate --project my-ext   # the manifest table plus host ceilings; the component too once built
fe-ext build --project my-ext      # needs `rustup target add wasm32-wasip2`; writes my-ext/dist/component.wasm
```

`validate` prints the failing row's reason and exits non-zero. Installing on a host gives the same answer,
because both use one validator (`fe_sdk::manifest_v2::parse_and_validate`). A reason such as
`source-class-mismatch` means the manifest and the kind disagree; fix the manifest rather than the
code. `build` then runs the component through the host's install admission, so a missing export or
a forbidden import fails here rather than at install.

## 5. Run in the sandbox

```bash
fe-ext run --sandbox --project my-ext --fixture my-ext/fixtures/basic.json --out trace.json
```

The sandbox runs your component in the real host against an in-memory scene, with a deterministic
tick clock, and prints a `fingerprint` line. Two runs of the same fixture must print the same
fingerprint. If they do not, your guest depends on wall-clock time or on iteration order it does not
control; read `now-ms` from `context` instead. The fixture format is `Fixture` in
[`tools/fe-ext/src/sandbox.rs`](../../tools/fe-ext/src/sandbox.rs); recorded ingress frames pass the
host's endpoint, method, size, media-type and header checks before your guest sees them, exactly as
in production. Worked fixtures: [`extensions/fleet-sim/fixtures/basic.json`](../../extensions/fleet-sim/fixtures/basic.json)
and [`extensions/device-adapter/fixtures/basic.json`](../../extensions/device-adapter/fixtures/basic.json).

### Fixtures

A fixture is a JSON script of ticks, recorded ingress frames and the starting scene. `fe-ext new`
writes `fixtures/basic.json` for your kind; extend that file rather than adding new ones, and keep
one fixture per behaviour you want to pin by fingerprint.

### Parity with the REST door

A device adapter must produce the same reading rows as `POST /api/v1/.../iot/readings` for the same
frames, apart from provenance and `source_class`. The sandbox does not call REST; the check is the
`ext-device` scenario (step 2, R31), which sends identical frames through both doors. Keep the
adapter's decode in one function so the fixture and the scenario exercise the same code.

### Multi-peer runs (`--lab`)

```bash
fe-ext run --lab --scenario my-scenario.json --runs 2 --project my-ext
```

`--lab` hosts the fleet-sim package as the fe-sim reading schedule under the simulator's `SimClock`,
so lab and live run the same component (R30). It needs `--scenario` (the fe-sim script), is mutually
exclusive with `--sandbox`, and with `--runs N` every fingerprint must equal the first. Design notes:
[`tools/fe-ext/AGENTS.md`](../../tools/fe-ext/AGENTS.md) (section lab).

## 6. Pack, sign, install

```bash
fe-ext keygen publisher.key                             # prints the publisher did:key; never overwrites
fe-ext pack --project my-ext --key publisher.key        # sealed and signed: my-ext/dist/local.my-ext-0.1.0.hexon
fe-ext countersign my-ext/dist/local.my-ext-0.1.0.hexon --key reviewer.key
fe-ext inspect my-ext/dist/local.my-ext-0.1.0.hexon     # identity, signers, content_hash, capability digest
```

`pack` seals and signs in one step. `sign` re-seals an existing package under another publisher key,
which gives it a new `content_hash`; `countersign` adds a second signature without touching any sealed
member, so the `content_hash` stays the same.

Install only succeeds if the publisher key (or a counter-signing key) is a trust root on the host.
The operator installs from the Extensions panel (Install from file, or Browse), or from a terminal with
`fe-ext admin install-file <path>`, which talks to the running app over a local channel and still
opens the consent dialog in the app ([admin-channel.md](admin-channel.md)). REST and MCP never install.
The consent dialog then shows the **capability digest**. Consent covers exactly that digest; if you
change any grant, the operator is asked again. Reordering a list also changes the digest, so keep
lists stable across versions.

## Sharpen

- **Pattern:** *declare-then-grant* (a capability manifest). The host grants the intersection of declared
  scope and operator consent, checked again at every call.
- **Architecture:** the guest is a pure function of `(binding, tick-info, staged inputs)`. That is what makes
  the sandbox fingerprint deterministic, and why `now-ms` is injected rather than read.
- **Deferred:** `--lab` covers the fleet-sim package only (R30), and feeding bus commands and egress results into the sandbox
  (today it answers them `not-available`).
- **Efficiency:** batch readings in one `stage` call per tick. The per-flush cap is 1000 readings and 64 ops,
  so one large batch costs one commit instead of many.

*Question to check your grasp:* if an extension stages a transform in `on-activate` and the petal is
deactivated before the next tick, does the transform ever commit? (Answer is in `fractal-plugin-2.wit`,
`lifecycle.on-deactivate`.)
