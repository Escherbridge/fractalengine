---
type: guide
title: Extensions for FractalEngine
track: extension_platform_20261107
status: draft (P3.4, slice E12D, reviewed by G3; links to examples and tools are checked in P6.5)
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
4. [Scoped bindings](bindings.md): only if your extension serves a hook point.

Reference contracts (normative; the guides above explain them, they do not replace them):

| Contract | File | Owns |
|---|---|---|
| WIT 2.0 world `extension` | [`fe-plugin/wit/fractal-plugin-2.wit`](../../fe-plugin/wit/fractal-plugin-2.wit) | Every guest import and export |
| Manifest v2 | [manifest-v2.md](manifest-v2.md) | What the extension may do, and the validation table |
| Integration bus | [bus.md](bus.md) | Streams, commands, hooks, delivery and limits |
| Package format | [package-format.md](package-format.md) | Sealed members, identity, signatures |
| Governance | [governance.md](governance.md) | Consent, trust roots, policy, kill switches, audit |
| Local admin channel | [admin-channel.md](admin-channel.md) | `fe-ext admin`: the local, OS-user-only route to the Extensions panel's operations; consent stays in the GUI |
| Extensions panel | [extensions-ui.md](extensions-ui.md) | Views, states, names, Browse and store flows, registry sources |
| Scoped bindings | [bindings.md](bindings.md) | Which installed extension serves a hook point at a verse, fractal or petal, resolved per peer |

## Kinds

| `kind` | Use it for | Example |
|---|---|---|
| `simulation` | Fleets of movers and sensors driven by a config | [`extensions/fleet-sim`](../../extensions/fleet-sim) |
| `device-adapter` | HTTP, websocket or MQTT devices turned into readings | [`extensions/device-adapter`](../../extensions/device-adapter) |
| `bi-connector` | Pushing query results to an external analytics target | [`extensions/bi-push`](../../extensions/bi-push) |
| `general` | Anything else that uses the same grants | `fe-ext new general <name>` ([`tools/fe-ext/src/new.rs`](../../tools/fe-ext/src/new.rs)) |

The fleet-sim and device-adapter `extension.json` files are byte copies of the schema examples of
the same name in [`fe-sdk/schema/examples/`](../../fe-sdk/schema/examples). `web-bundle` is reserved for the portal
track and is refused for extensions (`reserved-kind`).

## Tools

- [`tools/fe-ext`](../../tools/fe-ext): the authoring CLI (`new`, `build`, `validate`, `pack`, `sign`,
  `countersign`, `inspect`, `run --sandbox`, `keygen`). Design notes: [`tools/fe-ext/AGENTS.md`](../../tools/fe-ext/AGENTS.md).
- [`tools/fe-ext/guest`](../../tools/fe-ext/guest/src/lib.rs): the Rust guest SDK (generated 2.0 bindings, the
  `Extension` trait and `extension!` macro, write-window, readings-batching and snap-to-alert helpers).

## Status

- The contracts above are drafts. The host binds WIT 2.0 only after the build gate opens (D-X2).
- The CLI, the guest SDK and the examples are written but not yet built or run (code-first swarm,
  2026-11-08); J8 (`cargo test -p fe-ext -- j8`) is their journey once the build gate opens.
- `fe-ext run --lab` (multi-peer runs through fe-sim) hosts the first-party simulation package (R30); other kinds run in `--sandbox` only.
- Scoped bindings ([bindings.md](bindings.md)) are implemented in code at turn 2 and untested; the build gate and refine pass verify them (spec R37). Terrain is the first `petal.map` provider.
- Extension administration is GUI-only plus the local admin channel; REST and MCP refuse it (governance.md §Local identity).
