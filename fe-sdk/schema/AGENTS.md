---
type: reference
title: fe-sdk/schema — manifest v2 schema notes
track: extension_platform_20261107
---

# fe-sdk/schema

| File | Role |
|---|---|
| `extension-manifest-v2.schema.json` | JSON Schema 2020-12 for `plugin/extension.json`. Structure only. |
| `examples/fleet-sim.extension.json` | Simulation kind; the base manifest of journey J2 and the J5 tamper table. |
| `examples/device-adapter.extension.json` | Device-adapter kind: all four ingress transports' shapes, MQTT egress, a command. |
| `examples/bi-push.extension.json` | BI connector: a named `host/query` consume and an HTTPS egress (bus.md §9.3). |

## §structure-vs-policy

The schema checks shape (types, patterns, closed enums, `additionalProperties: false`, absolute
maxima). Every rule that needs a distinct refusal reason or a host ceiling — `empty-scope`,
`insecure-endpoint`, `cadence-below-floor`, `source-class-mismatch`, `undeclared-command`, … — is
semantic and normative in `docs/extensions/manifest-v2.md` §Validation. Keeping policy out of the
schema keeps reasons distinct (a schema failure is always `schema`) and lets a host lower its
ceilings without a schema change.

## §parity

Slice E2 (`fe-sdk/src/manifest_v2.rs`) owns the serde types and the one validator `fe-ext validate`
and REST install share. Its parity journey parses every file in `examples/` with serde AND with this
schema and expects both to accept; J2's rows are built by editing `fleet-sim.extension.json`. A
serde field without a schema property (or the reverse) is a defect. Unknown fields are refused on
both sides: a field this host cannot enforce must never be silently ignored.

## §examples

The examples are contract illustrations, not the shipped packages: slice E12X seeds
`extensions/fleet-sim` and `extensions/device-adapter` from them, and slice E17 seeds `extensions/bi-push`.
