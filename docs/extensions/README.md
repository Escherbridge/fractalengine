---
type: guide
title: Extensions for FractalEngine
track: extension_platform_20261107
status: draft (P3.4, slice E12D; links to examples and tools are checked in P6.5)
---

# Extensions for FractalEngine

An extension is a sealed `.hexon` package that adds behaviour to one petal: simulated fleets,
device adapters, BI connectors, or a toolbar tool. It runs as a WebAssembly component against the
`fractalengine:plugin@2.0.0` contract. It holds no socket, file, clock or SQL string. Everything
it does is declared in its manifest, consented to by an operator, and checked by the host at call time.

## Reading order

1. [Authoring guide](authoring-guide.md): build your first extension, from manifest to signed package.
2. [Cookbook](cookbook.md): short recipes for the common shapes (ticks, ingress, the bus, queries).
3. [Migration 1.0 to 2.0](migration-1-to-2.md): only if you have a 1.0 component.

Reference contracts (normative; the guides above explain them, they do not replace them):

| Contract | File | Owns |
|---|---|---|
| WIT 2.0 world `extension` | [`fe-plugin/wit/fractal-plugin-2.wit`](../../fe-plugin/wit/fractal-plugin-2.wit) | Every guest import and export |
| Manifest v2 | [manifest-v2.md](manifest-v2.md) | What the extension may do, and the validation table |
| Integration bus | [bus.md](bus.md) | Streams, commands, hooks, delivery and limits |
| Package format | [package-format.md](package-format.md) | Sealed members, identity, signatures |
| Governance | [governance.md](governance.md) | Consent, trust roots, policy, kill switches, audit |
| Scoped bindings | [bindings.md](bindings.md) | Which installed extension serves a hook point at a verse, fractal or petal, resolved per peer |

## Kinds

| `kind` | Use it for | Example |
|---|---|---|
| `simulation` | Fleets of movers and sensors driven by a config | [`fleet-sim.extension.json`](../../fe-sdk/schema/examples/fleet-sim.extension.json) |
| `device-adapter` | HTTP, websocket or MQTT devices turned into readings | [`device-adapter.extension.json`](../../fe-sdk/schema/examples/device-adapter.extension.json) |
| `bi-connector` | Pushing query results to an external analytics target | [`bi-push.extension.json`](../../fe-sdk/schema/examples/bi-push.extension.json) |
| `general` | Anything else that uses the same grants | (no first-party example) |

`web-bundle` is reserved for the portal track and is refused for extensions.

## Status

- The contracts above are drafts. The host binds WIT 2.0 only after the build gate opens (D-X2).
- The `fe-ext` CLI and the guest SDK ([R25](../../tools/fe-ext)) arrive in P3. Commands shown in the
  authoring guide are the planned surface until then.
- The first-party examples land with slice E12X; until then the links above point at the schema
  examples that exist today.
