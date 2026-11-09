---
type: guide
title: Extension authoring guide
track: extension_platform_20261107
status: draft (P3.4, slice E12D; CLI commands are planned until P3.1 lands)
---

# Authoring guide

This walks one extension from an empty folder to a signed package that installs on a dev host.
The running example is a `simulation` kind (see [README](README.md#kinds)). The steps are the same for every kind.

## 1. Start from a manifest, not from code

The manifest is the whole capability surface. Write it first, because the host grants nothing it does not declare.

```json
{ "manifest_version": 2, "id": "org.example.fleet-sim", "version": "0.1.0",
  "kind": "simulation", "contract": "fractalengine:plugin@2.0.0", "binding": "petal",
  "scene": { "read": true, "write": ["set_transform"] },
  "readings": { "source_class": "simulation", "metrics": ["speed_mps"] },
  "tick": { "cadence_ms": 1000 } }
```

Four rules decide most validation errors (full table in [manifest-v2.md](manifest-v2.md#validation)):

- **Absent grants nothing.** A missing section means no access. Do not write empty lists: an empty list is refused as `empty-scope`, so omit the field instead.
- **A writing extension must declare `tick`.** Writes are staged and only a tick flushes them. `scene.write` without `tick` fails as `cadence-required`.
- **`*` must be written out.** A wildcard in `property_scope` or `metrics` is explicit, never implied.
- **The id is the signed identity.** `id` must equal the `hexon_id` the package is signed under, or install refuses it as `id-mismatch`.

Open the full worked example, [`fleet-sim.extension.json`](../../fe-sdk/schema/examples/fleet-sim.extension.json), and change one thing at a time. Each change you make is a consent prompt the operator will see.

## 2. Implement the world

The component implements the `extension` world in [`fractal-plugin-2.wit`](../../fe-plugin/wit/fractal-plugin-2.wit).
It imports ten interfaces (`context` through `log`) and exports four (`lifecycle`, `ingress-hooks`, `egress-hooks`, `bus-hooks`). The guest may call only what the manifest grants.

The rule that shapes every guest is stated at the top of that file:

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

1. **The `on-tick` hook is the only place a flush is scheduled.** Anything staged in `on-activate` is flushed at the next tick, not immediately.
2. **Created node ids are not known when you stage.** `stage-create-node` returns an ordinal. The real id arrives in the next tick's `tick-info.last-commit` as a `commit-receipt`. Keep the ordinal in `kv` until then.

The install, activate and uninstall hooks receive a `binding` (installation id, petal id, identity DID, source class). Read the petal from the binding; do not hard-code one. `on-deactivate` receives only a reason.

## 3. Use the host for every effect

- **Readings** go through `readings.stage`. Each reading is validated the same way the REST readings door validates it. `source_did` and `source_class` are set by the host, never by you.
- **Outbound HTTP or MQTT** is queued through `egress`. Results come back later through `egress-hooks.on-egress-result`. Calls never block the worker.
- **Inbound traffic** arrives on `ingress-hooks.on-ingress` for each endpoint you declared.
- **Other extensions and the UI** talk over `bus`. See the [cookbook](cookbook.md#subscribe-to-a-host-stream).
- **Per-instance memory** is `kv`, host-local and never replicated. Declare its size in the manifest first.

## 4. Validate

```bash
fe-ext validate extension/        # planned: P3.1 (E12)
```

`validate` runs the manifest table and prints the first failing row's reason. REST install gives the same answer, because both use one validator. A reason such as `source-class-mismatch` means the manifest and the kind disagree; fix the manifest rather than the code.

## 5. Run in the sandbox

```bash
fe-ext run --sandbox --fixture fixtures/fleet.json   # planned: P3.1 (E12)
```

The sandbox runs your component in the real host against an in-memory scene, with a deterministic tick clock.
Two runs of the same fixture must print the same fingerprint. If they do not, your guest depends on wall-clock time or on iteration order it does not control; read `now-ms` from `context` instead.

## 6. Pack, sign, install

```bash
fe-ext pack          # builds the plugin hexon (planned: P3.1)
fe-ext sign          # publisher signature over signing_bytes
fe-ext inspect <pkg> # prints identity, signer, content_hash and capability digest
```

Install only succeeds if the publisher key (or a counter-signing key) is a trust root on the host. The consent dialog then shows the **capability digest**. Consent covers exactly that digest; if you change any grant, the operator is asked again. Reordering a list also changes the digest, so keep lists stable across versions.

## Sharpen

- **Pattern:** *declare-then-grant* (a capability manifest). The host grants the intersection of declared scope and operator consent, checked again at every call.
- **Architecture:** the guest is a pure function of `(binding, tick-info, staged inputs)`. That is what makes the sandbox fingerprint deterministic, and why `now-ms` is injected rather than read.
- **Deferred:** `--lab` multi-peer runs (R30) and the `general` kind's UI contributions. Revisit when the simulation track's P5 lands.
- **Efficiency:** batch readings in one `stage` call per tick. The per-flush cap is 1000 readings and 64 ops, so one large batch costs one commit instead of many.

*Question to check your grasp:* if an extension stages a transform in `on-activate` and the petal is deactivated before the next tick, does the transform ever commit? (Answer is in `fractal-plugin-2.wit`, `lifecycle.on-deactivate`.)
