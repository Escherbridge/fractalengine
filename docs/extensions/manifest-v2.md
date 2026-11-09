---
type: reference
title: Extension manifest v2
track: extension_platform_20261107
status: draft (P0.2; validated by fe-sdk manifest_v2 in P1.2)
---

# Extension manifest v2 (`plugin/extension.json`)

The manifest is the whole capability surface of an extension. The host grants nothing it does
not declare, the consent dialog renders exactly what it declares, and the verse policy caps it.

- **Where it lives.** `plugin/extension.json`, a header member of the sealed plugin hexon
  ([package-format.md](package-format.md)), so consent can render before the component is fetched.
- **Structure.** [`fe-sdk/schema/extension-manifest-v2.schema.json`](../../fe-sdk/schema/extension-manifest-v2.schema.json)
  (JSON Schema 2020-12). Unknown fields are refused everywhere.
- **Policy.** The [validation table](#validation) below is normative. Schema and table run in
  ONE validator (fe-sdk `manifest_v2`, slice E2) that `fe-ext validate` and REST install share,
  so they cannot disagree (spec R3).
- **Worked examples.** [`fleet-sim`](../../fe-sdk/schema/examples/fleet-sim.extension.json) (simulation),
  [`device-adapter`](../../fe-sdk/schema/examples/device-adapter.extension.json) (ingress, egress,
  commands), [`bi-push`](../../fe-sdk/schema/examples/bi-push.extension.json) (a `host/query` consumer).
- **Contract it binds to.** [`fe-plugin/wit/fractal-plugin-2.wit`](../../fe-plugin/wit/fractal-plugin-2.wit).
- **Where it can serve.** [`provides`](#provides--hook-points-bindingsmd) names the hook points a
  scoped binding may select this extension for ([bindings.md](bindings.md)).

## Grant rules

1. **Absent grants nothing.** A missing section, field or list is no access.
2. **An empty scope list is refused**, not silently empty (`empty-scope`; the lists are V4's). Omit
   the field instead. A scope list is one that narrows a grant, where "empty" could be misread as
   "everything" (the 1.0 trap). Declaration lists (`ingress`, `egress`, `ui`, `bus.consume`,
   `bus.expose.streams`, `bus.expose.commands`) only add entries, so an empty one declares nothing
   and is accepted.
3. **`*` must be written out** wherever a wildcard is allowed (property scope, metrics, ad-hoc query SQL).
4. **Patterns** in `property_scope`: an exact key (`fleet.status`), a prefix ending in `*`
   (`fleet.*`), or `*` alone. Nothing else is a pattern.
5. **Everything is per binding.** An instance is one installation bound to one petal (OD-E3);
   every grant is confined to that petal. Scoped bindings ([bindings.md](bindings.md)) only choose
   which installed extension serves a hook point at a verse, fractal or petal; each covered petal
   still gets its own instance, so no grant ever reaches past one petal.

## Field reference

### Identity

| Field | Required | Meaning |
|---|---|---|
| `manifest_version` | yes | `2`. |
| `id` | yes | The `package_id`: reverse-DNS, lowercase, two or more labels (`org.example.fleet-sim`). Equals the sealed hexon's signed `hexon_id` and the id the install names, else `package-invalid` (`id-mismatch`); revocation matches by hash whatever id an entry names ([package-format.md](package-format.md#identity)). The label `host` is never a package id (it has no dot), which keeps `host/*` bus addresses reserved. |
| `version` | yes | SemVer; equals the hexon manifest `version`. |
| `name`, `description` | `name` yes (except `web-bundle`) | Display text (80 / 1 000 chars). |
| `kind` | yes | `simulation`, `device-adapter`, `bi-connector` or `general`. `web-bundle` is reserved for `portal_surface_20261107`, which owns the `portal` section and forbids the runtime keys for that kind (`docs/spec/portal-bundle/README.md` §1.8); the schema therefore does not demand `name`, `contract` or `binding` for it, and the extension host refuses it (`reserved-kind`). |
| `contract` | yes (except `web-bundle`) | `fractalengine:plugin@2.0.0`. Any other major, or a 2.x minor newer than the one the host binds (its imports may not exist here), is `unsupported-contract`. |
| `binding` | yes (except `web-bundle`) | `petal` (the only value in 2.0). |

### `scene`

| Field | Grants |
|---|---|
| `read: true` | `scene.get-node`, `scene.query-nodes` and the `host/node-changes` bus source, all in the bound petal. Snapshot `properties` carry only keys inside `property_scope`. |
| `write: [...]` | Any of `create_node`, `set_transform`, `set_property`, `delete_node`, staged through `txn`. |
| `property_scope: [...]` | Keys readable in snapshots and writable by `set_property`. Required when `write` contains `set_property` or a UI contribution shows properties. |

### `readings`

| Field | Meaning |
|---|---|
| `source_class` | `simulation`, `device` or `connector`, or a list of them. With a list, the operator picks exactly ONE at bind time (never inferred, never defaulted). Consent covers the whole declared set (it is in the digest); the bind dialog shows the pick and the binding records it. The readings door records it as the reading's `source_class` (canonical-log observation membership; `iot_reading` rows have no class column). |
| `metrics` | Metric names `readings.stage` accepts, or `["*"]`. |

`source_did` is always the host's local DID. `manual` and `decoder` are not extension classes.

### `routes`

`true` grants `routes.info`, `routes.sample` and `routes.snap` over any route node of the bound
petal. Geometry reads do not need the route's properties in `property_scope`; the results are
derived numbers, never the raw property.

### `tick` — the cadence budget (spec R4, OD-E4)

| Field | Default | Host ceiling | Meaning |
|---|---|---|---|
| `cadence_ms` | — (required in the block) | floor 100 | `on-tick` period. No block = no ticks and no flushes. |
| `max_ops` | 64 if `scene.write`, else 0 | 64 | Scene ops per flush. |
| `max_readings` | 1 000 if `readings`, else 0 | 1 000 | Readings per flush. |
| `fuel` | 1 000 000 | 20 000 000 | Fuel for each hook call (every hook gets its own refill). |

A writing extension MUST declare `tick` (`cadence-required`): every write is staged and only a
tick flushes it (R5). Host ceilings are host configuration; a local policy may lower them, never raise them.

### `kv`

`{ "max_bytes": n }` (≤ 1 MiB) grants the `kv` interface: host-local state per instance (one
installation at one petal, so two bindings never share keys), `max_bytes` per instance, never
replicated and never on the DB thread.

### `ingress` — endpoints that receive traffic (spec R8)

Every endpoint has an `id` (`[a-z][a-z0-9-]{0,31}`, unique across `ingress` and `egress`) and a
`transport`. Frames reach `ingress-hooks.on-ingress`.

| `transport` | Required | Optional (default) | Behaviour |
|---|---|---|---|
| `http-server` | `methods`, `max_body_bytes` (≤ 1 MiB), `rate_hz` (per caller DID) | `content_types` (any), `role_floor` (`editor`), `sub_paths` (false), `forward_headers` (none; the host strips credential headers before any request reaches a guest, whatever the manifest says — a host-side denylist is the control, the schema rule is a second layer) | Mounted at `/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{id}` on the existing API server, behind its bearer auth. The caller's role at the bound petal must meet `role_floor`. |
| `websocket-accept` | `max_frame_bytes`, `rate_hz` (frames/s per connection), `max_connections` (≤ 256) | `role_floor` (`editor`) | Upgrades on the same mount. |
| `websocket-dial` | `url` (`wss://`, or `ws://` to loopback), `max_frame_bytes`, `rate_hz` | `auth`, `reconnect_ms` (`{min:1000,max:60000}`) | The host dials and redials with jittered backoff. |
| `mqtt` | `broker` (`mqtts://`, or `mqtt://` to loopback), `topics` (filters), `max_payload_bytes`, `rate_hz` | `qos` (`at-most-once`), `auth` | The host is an MQTT client subscribed to `topics`. |

Over-size → 413 (http) or the frame is dropped and counted (websocket, mqtt). Over-rate → 429 or
dropped and counted. An undeclared or disabled endpoint answers the uniform not-found.

- **Mount path (decision, G0 2026-11-08).** The mount is petal-scoped,
  `/api/v1/petals/{petal_id}/extensions/{package_id}/ingress/{id}`, instead of spec R8's
  `/api/v1/extensions/{installation}/ingress/{endpoint}`: a device on another host can know the
  package id but never a host-local installation id, the petal segment selects the one binding
  (OD-E3) and runs the existing petal-scope guard, and it matches the readings door
  (`/api/v1/petals/{petal_id}/iot/readings`). Bus commands follow the same rule ([bus.md](bus.md#8-external-access)).
- **Delegation.** `http-server` and `websocket-accept` endpoints delegate the host's identity: every
  DID at or above `role_floor` can make the extension write, and the rows are authored by the
  host's local DID, not the caller's (`ingress-frame.caller-did` tells the guest who sent it). The
  consent dialog states this per endpoint, exactly as for `external: true` commands.

### `egress` — declared outbound requests (spec R9)

| `transport` | Required | Optional (default) | Behaviour |
|---|---|---|---|
| `http` | `url` (base; `https://`, or `http://` to loopback), `methods`, `rate_hz` (≤ 10) | `max_request_bytes` (1 MiB), `max_response_bytes` (1 MiB, ≤ 8 MiB), `timeout_ms` (10 000, ≤ 30 000), `auth` | `egress.http-request(id, {path, ...})` goes to `url` + `path`; a path that leaves the base is `invalid-input`. Redirects are followed only within the same origin and base path. |
| `mqtt` | `broker`, `topics` (publish filters), `max_payload_bytes`, `rate_hz` | `qos` (`at-most-once`), `retain` (false) | `egress.mqtt-publish` topics must match a declared filter. |

**Loopback** means exactly `127.0.0.1`, `[::1]` or `localhost`. Hosts are exact names (no
wildcards). The host connects only to the declared host and port.

### `auth` — host-side credentials

`{ "scheme": "bearer" | "basic" | "header" | "mqtt-password", "credential": "<slot>", "header"? }`
names a slot in the host's local credential store. The operator fills the slot locally; the value
never enters the manifest, the package, the replicated store or the guest. An endpoint whose slot
is empty stays down and reports `credential-missing` in status.

### `bus` — the integration bus (D-X6; semantics in [bus.md](bus.md))

| Field | Meaning |
|---|---|
| `expose.streams[]` | `{id, max_rate_hz (≤ 50), max_event_bytes (≤ 64 KiB), role_floor (viewer), external (false), consumers?, description?}`. Published as `<package_id>/<id>`. |
| `expose.commands[]` | `{id, role_floor, rate_hz (≤ 10), external (false), args_schema?, description?}`. Answered through `on-command`. |
| `consume[]` | Sources this extension may subscribe to: `{source: "host/node-changes"}`, `{source: "host/readings", metrics?}`, `{source: "host/query", id, sql, min_interval_ms (1000)}`, `{source: "host/admission"}`, or `{source: "<package_id>/<stream>"}`. |
| `call[]` | Foreign commands this extension may call, as `<package_id>/<command>`. |

`external: false` (the default) keeps a stream or command reachable only from this host: the UI,
the local DID over REST/WS/MCP, other extensions and portals. Only `external: true` lets a remote
DID reach it, still behind `role_floor`. That default stops a remote token holder from driving
the host's identity through a command (the confused-deputy path R12 closes for administration).

`host/query` with `sql: "*"` grants any guarded SELECT inside the binding scope; the consent
dialog says so in plain words. A named query (`sql` = one SELECT) is shown verbatim at consent and
must pass the static query guard at validate time (`invalid-query`).

### `ui` — declarative contributions (spec R10, OD-E8)

| Slot | Shape | Renders |
|---|---|---|
| `ToolbarAction`, `SidebarTool` | `command` | A button that calls the command with `{}`. |
| `ContextMenu` | `command`, `when?` | An entry on nodes matching `when` (`{property}` present, or `{property, equals}`); calls with `{"node_id": "<clicked>"}`. |
| `InspectorSection` | `properties` | Read-only key/value rows of the selected node (keys inside `property_scope`). |
| `DashboardWidget`, `ViewportOverlay` | `stream`, `field?` | The label and the latest value at JSON Pointer `field` of an exposed stream. |

Accessible names are `ext:<package_id>/<id>`. A contribution with neither `command`, `properties`
nor `stream` is listed in the Extensions details pane, never rendered as a dead control. Each slot
admits only the fields in its row (plus `id`, `slot`, `label`, `icon`); any other field is `schema`,
never silently ignored. The command a slot calls must accept the args that slot sends (`{}`, or
`{"node_id": "<ULID>"}` for `ContextMenu`), else `scope-conflict` (V13).

### `provides` — hook points ([bindings.md](bindings.md))

`provides[]` lists the hook points this extension can serve when a scoped binding selects it.

- **Absent:** the extension serves no hook point. Only a local bind runs it (governance §Consent).
- **When a binding takes effect.** A binding naming this package takes effect on a peer only if
  all four hold ([bindings.md](bindings.md) §5, §7.1):
  - the installed build provides the binding's hook, and its key;
  - the hook's needs are declared here;
  - consent covers this digest;
  - the peer has accepted the installation for the verse.

| Field | Required | Meaning |
|---|---|---|
| `hook` | yes | A catalogue id, `<family>.<point>` ([bindings.md](bindings.md) §6): `petal.activate`, `petal.map`, `api.routes`, `scene.create`, `scene.transform`, `scene.property`, `ingest.decode`, `ui.panel`, `egress.connector`, `egress.snippet`, `asset.import`, `blob.mirror`, `blob.source` or `sync.tileset-source` in 2.0. Any other id, or a stretch hook this host does not serve, is `unknown-hook` (V17). |
| `keys` | no | Keyed hooks only. The binding keys served: an exact key (a device format id, a lowercase MIME type, a name), a prefix ending in `*`, or `*`. Absent means any key. It is a scope list (Grant rule 2), so an empty one is `empty-scope` (V4). |
| `uses` | per hook | The ids of this manifest's entries the hook runs through (`api.routes` and the egress-backed hooks need at least one). It is a declaration list. <ul><li>`api.routes`: `http-server` or `websocket-accept` ingress ids.</li><li>`ui.panel`: `ui` ids.</li><li>`egress.connector`, `blob.mirror`, `blob.source`, `sync.tileset-source`: `egress` ids, plus named `host/query` ids for `egress.connector`.</li></ul> |
| `config_schema` | no | JSON Schema for the binding's `config`, in the `args_schema` subset of [bus.md](bus.md#4-commands). It must be ≤ 4 KiB; outside the subset it is `schema` (V1). Absent: the binding's `config` must be `{}`. Each resolving peer validates the binding's config against its own installed build. |

- **Providing a hook grants nothing by itself.** Every hook call runs under this manifest's
  grants, in the instance's own petal. `provides` stays in the capability digest (every member
  that grants does), so adding or changing a hook re-prompts.
- **Web bundles.** A `kind: web-bundle` package is bound to `portal.open` without `provides`; the
  portal contract owns its manifest.
- **The hooks export.** A component whose manifest declares `provides` must export the hook-call
  interface that [bindings.md](bindings.md) §7.3 proposes, once the WIT carries it. Otherwise the
  install is refused `missing-export` ([package-format.md](package-format.md#component-checks-at-install)).

## Validation

Every row is refused with its own reason; the first failing row in table order is reported.
`fe-ext validate` exits non-zero with the same reason REST install answers.

| # | Rule | Reason |
|---|---|---|
| V1 | The JSON Schema fails (unknown field, type, pattern, bound, an endpoint URL scheme that does not match its transport, a UI field its slot does not render), or an `args_schema` leaves the subset in bus.md or exceeds 4 KiB | `schema` |
| V2 | `contract` is not `fractalengine:plugin@2.x.y`, or names a 2.x minor newer than the host binds | `unsupported-contract` |
| V3 | `kind` is `web-bundle`, or a `portal` section appears with any other kind | `reserved-kind` |
| V4 | A scope list is present and empty: `scene.write`, `scene.property_scope`, `readings.metrics`, an endpoint's `methods`, `topics`, `content_types` or `forward_headers`, a stream's `consumers`, a consume entry's `metrics`, `bus.call`, a `provides` entry's `keys` | `empty-scope` |
| V5 | An `http`, `ws` or `mqtt` URL to a non-loopback host, or any endpoint URL with userinfo, a query or a fragment | `insecure-endpoint` |
| V6 | `tick.cadence_ms` below the host floor (100) | `cadence-below-floor` |
| V7 | `scene.write` or `readings` declared without `tick` | `cadence-required` |
| V8 | `source_class` outside the kind's set: `simulation` → exactly `simulation`; `device-adapter` → within {`device`, `connector`, `simulation`}; `bi-connector` → `connector`; `general` → any | `source-class-mismatch` |
| V9 | A UI `command` that `bus.expose.commands` does not declare | `undeclared-command` |
| V10 | A UI `stream`, or a `consume` of this package's own `<id>/<stream>`, that `bus.expose.streams` does not declare | `undeclared-stream` |
| V11 | A `consume` source `host/<x>` outside {`query`, `readings`, `node-changes`, `admission`}, or the same source consumed twice (named queries count by `id`) | `unknown-source` |
| V12 | A named `host/query` SQL that fails the static guard (`fe_query::sql_guard`, query mode) | `invalid-query` |
| V13 | Scope conflicts: `InspectorSection` keys outside `property_scope`; `InspectorSection` or `host/node-changes` without `scene.read`; `set_property` without `property_scope`; `auth.scheme: header` without `header`; a UI `command` whose `args_schema` refuses the args its slot sends (`{}`; `ContextMenu`: `{"node_id": "<ULID>"}`) | `scope-conflict` |
| V14 | Duplicate `id` across `ingress` + `egress`, within streams, commands, `ui`, or named queries; or the same `hook` twice in `provides` | `duplicate-id` |
| V15 | A number above the host ceiling (`fuel`, sizes, rates, connections, timeouts) | `over-host-ceiling` |
| V16 | An `http-server` endpoint's `forward_headers` names a credential header: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, or any host-authenticating header (`x-fe-join-lease`, any `x-fe-*`); the list is the `not` rule on `forward_headers` items in the schema | `credential-header` |
| V17 | A `provides[].hook` outside the hook catalogue of the contract the manifest names, or a stretch hook this host does not serve ([bindings.md](bindings.md) §6) | `unknown-hook` |
| V18 | A provided hook whose needs are not declared ([bindings.md](bindings.md) §6 "Needs"): <ul><li>`keys` on a keyless hook;</li><li>a `uses` id that names no entry of the kind the hook needs;</li><li>`api.routes`, `ui.panel` or an egress-backed hook with no `uses`;</li><li>`ingest.decode` without `readings` and `tick`;</li><li>`scene.*` without `scene.read`;</li><li>`asset.import` without `keys`;</li><li>`egress.connector` without a `host/query` or `host/readings` consume.</li></ul> | `hook-capability-missing` |

### Journey J2 (P1.3 swaps `capability.rs::parse_manifest_defaults` and `::validate_property_access_allowed` into this table)

Start from the fleet-sim example and change one thing per row:

| Row | Change | Expect |
|---|---|---|
| valid | none | accepted; capability digest printed |
| empty scope | `scene.property_scope: []` | `empty-scope` |
| non-https egress | add egress `{"id":"sink","transport":"http","url":"http://bi.example.org/","methods":["POST"],"rate_hz":1}` | `insecure-endpoint` |
| loopback http | same with `http://127.0.0.1:9000/` | accepted |
| sub-floor cadence | `tick.cadence_ms: 50` | `cadence-below-floor` |
| simulation as device | `readings.source_class: "device"` | `source-class-mismatch` |
| undeclared UI command | first `ui` entry `command: "launch"` | `undeclared-command` |
| undeclared consumed stream | `bus.consume` += `{"source":"org.example.fleet-sim/tracks"}` | `undeclared-stream` |
| unknown hook | `provides: [{"hook":"petal.teleport"}]` | `unknown-hook` |
| hook without its capability | `provides: [{"hook":"api.routes","uses":["start"]}]` (`start` names a command and a UI entry, not an ingress endpoint) | `hook-capability-missing` |

## Capability digest

Consent covers one exact digest (spec R20, [governance.md](governance.md#consent)).

```
capability_digest = lowercase_hex(blake3(canonical_json(projection(manifest))))
```

- `canonical_json` is fe-format's canonicalizer (sorted keys, no whitespace), the one manifest
  signatures use.
- `projection` removes presentation-only members and keeps everything that grants: `version`,
  `name`, `description`, the whole `tick` block, every `description` under `bus.expose`, and
  `label` and `icon` in each `ui` entry are removed. Every other member stays, including future
  ones, so a new grant can never slip past consent.
- Arrays keep manifest order: reordering a list re-prompts. Authors keep lists stable.
- `kind: web-bundle` uses the portal contract's digest instead (`docs/spec/portal-bundle/README.md`
  §7.2: JCS over the `portal` capability set); governance records consent against whichever digest
  the kind defines.

An update with the same digest does not prompt; any other digest re-prompts and the old version
keeps running until consent is given.

## From the 1.0 `CapabilityManifest`

| 1.0 (`fe-plugin/src/capability.rs`) | 2.0 |
|---|---|
| `verse_scope`, `petal_scope` patterns | `binding: "petal"`; the operator binds; no pattern lists |
| `property_scope` (empty = wildcard) | `scene.property_scope` (empty refused; `*` explicit) |
| `capabilities: ["storage.read"]` | `scene.read: true` |
| `capabilities: ["storage.write"]` | `scene.write: ["set_property"]` |
| `capabilities: ["query.select"]` (raw SQL) | `bus.consume: [{"source":"host/query", ...}]` through the REST guard (OD-E10) |
| `network_scope`, `external_http` (never enforced) | `ingress[]` and `egress[]` declarations |
