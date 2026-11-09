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
| `examples/device-adapter.extension.json` | Device-adapter kind: three of the four ingress transports (`http-server`, `websocket-accept`, `mqtt`; not `websocket-dial`), MQTT egress, two commands (one per UI shape). |
| `examples/bi-push.extension.json` | BI connector: a named `host/query` consume and an HTTPS egress (bus.md §9.3). |

## §structure-vs-policy

The schema checks shape (types, patterns, closed enums, `additionalProperties: false`, absolute
maxima). Every rule that needs a distinct refusal reason or a host ceiling — `empty-scope`,
`insecure-endpoint`, `cadence-below-floor`, `source-class-mismatch`, `undeclared-command`, … — is
semantic and normative in `docs/extensions/manifest-v2.md` §Validation. Keeping policy out of the
schema keeps reasons distinct (a schema failure is always `schema`) and lets a host lower its
ceilings without a schema change.

Three structural rules are conditional (G0 review, 2026-11-08), so a serde-only validator must
mirror them explicitly:

- **URL scheme per transport.** `httpUrl` (`https`/`http`) for egress `http`, `wsUrl` (`wss`/`ws`)
  for `websocket-dial`, `mqttUrl` (`mqtts`/`mqtt`) for both MQTT endpoints. A mismatch is `schema`;
  plaintext to a non-loopback host stays the semantic `insecure-endpoint`.
- **UI slot shapes.** Each slot admits only the fields it renders (manifest-v2.md §ui); any other is
  `schema`, because a field the host would silently ignore breaks the "never silently ignored" rule.
- **`kind: web-bundle`** does not require `name`, `contract` or `binding`; the extension host still
  refuses the kind (`reserved-kind`, V3), and the portal contract validates the `portal` section and
  forbids the runtime envelope keys (`docs/spec/portal-bundle/README.md` §1.8). No cross-tree `$ref`.

## §parity

Slice E2 (`fe-sdk/src/manifest_v2.rs`) owns the serde types and the one validator `fe-ext validate`
and REST install share. Its parity journey parses every file in `examples/` with serde AND with this
schema and expects both to accept; J2's rows are built by editing `fleet-sim.extension.json`. A
serde field without a schema property (or the reverse) is a defect. Unknown fields are refused on
both sides: a field this host cannot enforce must never be silently ignored.

## §examples

The examples are contract illustrations, not the shipped packages: slice E12X seeds
`extensions/fleet-sim` and `extensions/device-adapter` from them, and slice E17 seeds `extensions/bi-push`.
