# Portal bundle contract v1

**Status:** draft, author-only (D-X2), 2026-11-08. Track `portal_surface_20261107`.
**Contract id:** `fe-portal-bundle/1`. It is a public contract that bundle authors implement, so it is never changed in place: a breaking change ships as `fe-portal-bundle/2` beside v1.
**Normative words:** MUST, MUST NOT, SHOULD and MAY are normative. Numbered rules are the contract; prose between them is rationale.

This file is T0.1: the bundle container, its identity, its members, the `portal` manifest section and its validator, and how a bundle maps onto assets and repository nodes. [`manifest-v1.schema.json`](manifest-v1.schema.json) is the machine-readable form of §5.

## Documents

| File | Task | Contract | Built by |
|---|---|---|---|
| `README.md` (this file) | T0.1 | container, identity, member paths, MIME table, `portal` section, refusal classes, capability digest, asset mapping, repository nodes | S0 (fe-format validator), S0b (fe-api routes), S0c (replication gates) |
| [`manifest-v1.schema.json`](manifest-v1.schema.json) | T0.1 | JSON Schema 2020-12 for the `portal` section | S0 table, QA fixtures under `tools/qa/portal/bundles`, the sibling's manifest v2 schema (`$ref`) |
| [`protocol.md`](protocol.md) | T1.1 | `fe-portal` custom protocol, origins, CSP, headers, navigation decision table, open/close, fixed QA operations, spikes | S1, SP, S5 |
| [`ipc-v1.md`](ipc-v1.md) | T2.1 | `fe.portal.v1` messages, page API, authorization order, budgets, delivery sources, audit | S2 |
| [`admission.md`](admission.md) | T4.1 | admission and chrome states, transitions, kill switches and the kill list, update policy, static-sandbox feeds, CORS | S4, S3 |
| [`static-export.md`](static-export.md) | T6.1 | later phase: static export for the web | S8 |
| [`ux.md`](ux.md) | T3.1 | repository inspector and chrome UX (slice SU) | S3 |

## Seams this contract consumes

- **`extension_platform_20261107` (EXT-PLATFORM).** Its normative files (`docs/extensions/package-format.md`, `manifest-v2.md`, `bus.md`, `fe-plugin/wit/fractal-plugin-2.wit`, `fe-sdk/schema/**`) did not exist when this was written (2026-11-08 19:52). This contract follows the sibling spec's requirements R1, R3, R11, R13–R15 and R19–R24 instead, and marks every such dependency with an assumption id from §11. When the sibling's files land, a difference is resolved in favour of the sibling, except the `portal` section, which this track owns.
- **S-PLUGIN** (`conductor/open-core.md:54`): publisher signatures, trust roots and the signed revocation list, used unchanged through the sibling's governance.
- **fe-format sealing** (`fe-format/src/integrity.rs`, `archive_index.rs`, `limits.rs`): unchanged.

## 1. Container

1.1. A web bundle is one `.hexon` ZIP archive whose `manifest.json` has `hexon_type: "plugin"`, the `HexonType::Plugin` family that D-X1 adds (`fe-format/src/manifest.rs`, sibling R1) [A-3].
1.2. Its `plugin/extension.json` member is a manifest v2 document with `kind: "web-bundle"` (the discriminator sibling R3 reserves for this track) and a `portal` section (§5). `plugin/extension.json` is a header member (sibling R1 adds it to `HEADER_MEMBER_PATHS`), so the `portal` section is readable, and consent can render, before any body byte is fetched.
1.3. Every body member MUST live under `web/`. The archive MAY carry the header members fe-format already reads (`entries.json` with an empty catalog, `members.json`, `license.json`); `entries.json` is required by fe-format's reader (`fe-format/src/archive_index.rs:169`).
1.4. A web bundle MUST NOT carry `plugin/component.wasm` (refusal `component_present`) or any body member outside `web/` (refusal `member_reserved`). A web bundle is a page, not an extension instance; it never ticks and never runs in the plugin worker.
1.5. `HexonType::Bundle` is a different, pre-existing archive type. A web bundle is never `HexonType::Bundle`.
1.6. **Sealed bundles.** `seal` is the last transform (sibling R1). A sealed bundle commits every `web/` member in `members.json` under the publisher signature. A member that `members.json` does not commit fails every lazy read with `IntegrityError::Uncovered` (`fe-format/src/integrity.rs:178-181`), and the validator refuses it at ingest (`integrity`).
1.7. **Unsealed bundles** are valid. They render only in the static sandbox and can never be admitted (D-X5).
1.8. **Envelope keys.** For `kind: "web-bundle"` every runtime-capability key of manifest v2 MUST be absent: `contract`, `binding`, `scene`, `readings`, `routes`, `tick`, `ingress`, `egress`, `bus`, `kv` and `ui` (refusal `envelope_key_forbidden`). `kv` is banned because a bundle keeps no host-side key-value state; its storage is its own origin's. A bundle declares everything it may do inside `portal`. Descriptive manifest v2 keys stay allowed.

## 2. Identity

2.1. `package_id` is manifest v2 `id`, which MUST equal the hexon manifest's `hexon_id` (sibling R1). Manifest v2 `version` MUST equal the hexon manifest's `version`. Either mismatch is `id_mismatch`.
2.2. `publisher_did` is the DID in `IntegrityStatus::Sealed { publisher_did }` after verification (`fe-format/src/integrity.rs:81-86`). An unsealed bundle has no publisher.
2.3. `content_hash` identifies one build, as lowercase-hex BLAKE3:
  - **sealed:** the package content hash that `docs/extensions/package-format.md` defines (sibling R1: BLAKE3 over the canonical signed manifest bytes with the signature blanked). Those bytes commit the integrity root, and with it every member, so two archives with one sealed `content_hash` serve identical bytes;
  - **unsealed:** BLAKE3 of the archive bytes. An unsealed manifest commits no member, so only the whole archive identifies the build.
2.4. `blob_hash` is BLAKE3 of the archive bytes: the blob-store key and the `asset` row's hash. For an unsealed bundle `blob_hash == content_hash`. For a sealed bundle they differ, and re-zipping the same sealed build changes `blob_hash` but never `content_hash`.
2.5. Admission records, the kill list, audit records and write provenance all key on `(package_id, content_hash)`, never on `blob_hash` (sibling R23, R24; `conductor/open-core.md:54`) [A-9].

## 3. Member paths

3.1. A body member path is `web/` followed by one or more segments separated by `/`. Each segment MUST match `[A-Za-z0-9_~-][A-Za-z0-9._~-]{0,127}`: RFC 3986 unreserved characters, not starting with `.`. The full path is at most 512 bytes and at most 16 segments deep. Paths are case-sensitive. Violations are `member_path_invalid`.
3.2. The grammar is the traversal defence. A browser sends unreserved characters unescaped, so the protocol handler never percent-decodes (`protocol.md` §3). `..`, `.`, dotfiles, `%2e%2e`, backslashes and spaces simply cannot name a member.
3.3. `web/__fe/` is reserved for host-served paths (`protocol.md` §3.4). Any member under it is `member_reserved`.
3.4. A request path `/<p>` maps to member `web/<p>`. A path ending in `/` maps to `web/<p>index.html`, so `/` maps to `web/index.html`. There is no other rewriting and no fallback document.
3.5. `entry` (§5) MUST name an existing member under `web/` whose extension is `html` or `htm` (`entry_missing`, `entry_not_html`). The host opens a bundle by navigating to the entry's own path, so relative URLs inside the entry resolve against its directory.

## 4. MIME table

4.1. The protocol handler sets `Content-Type` from this closed table, by the final extension, case-insensitively. It never sniffs, and it sends `X-Content-Type-Options: nosniff`.
4.2. A member whose extension is not in the table is refused at validation (`member_type_unknown`). The table is closed so that every served byte has a known, non-executable-by-accident type.

| Extensions | Content-Type |
|---|---|
| `html`, `htm` | `text/html; charset=utf-8` |
| `css` | `text/css; charset=utf-8` |
| `js`, `mjs` | `text/javascript; charset=utf-8` |
| `json`, `map` | `application/json` |
| `geojson` | `application/geo+json` |
| `wasm` | `application/wasm` |
| `txt` | `text/plain; charset=utf-8` |
| `md` | `text/markdown; charset=utf-8` |
| `csv` | `text/csv; charset=utf-8` |
| `svg` | `image/svg+xml` |
| `png` / `jpg`, `jpeg` / `gif` / `webp` / `avif` / `ico` | `image/png` / `image/jpeg` / `image/gif` / `image/webp` / `image/avif` / `image/x-icon` |
| `woff` / `woff2` / `ttf` / `otf` | `font/woff` / `font/woff2` / `font/ttf` / `font/otf` |
| `mp3` / `ogg` / `wav` | `audio/mpeg` / `audio/ogg` / `audio/wav` |
| `mp4` / `webm` | `video/mp4` / `video/webm` |
| `glb` | `model/gltf-binary` |
| `bin` | `application/octet-stream` |

4.3. **The JSON cap.** fe-format's `read_member` reads any member whose name ends in `.json` under `MAX_JSON_ENTRY_BYTES` (8 MiB), and every other member under 64 MiB (`fe-format/src/archive_index.rs:326-333`, `limits.rs:8,12`). A `web/**/*.json` member larger than 8 MiB would therefore pass ingest and fail to serve, so the validator refuses it (`archive_limit`). Ship large data as `geojson`, `csv` or `bin`.

## 5. The `portal` section v1

The section is the value of `portal` in `plugin/extension.json`. Every object in it is closed: an unknown key is `vocabulary_unknown`, so a key like `csp.script` (remote script) can never be smuggled in. All list keys are required, and an empty list grants nothing; `false` booleans likewise. Requiring every key keeps the consent rendering total: there is no default the user did not see.

| Key | Type | Rule |
|---|---|---|
| `schema` | `"fe-portal-bundle/1"` | anything else is `schema_unsupported` |
| `entry` | member path | §3.5 |
| `title` | string, 1–120 chars | shown in the inspector and chrome |
| `csp.connect`, `csp.img`, `csp.font`, `csp.media`, `csp.frame` | list of origins, ≤ 16 each | each an exact `https://host[:port]` origin with no path, query, fragment, userinfo or wildcard (`origin_invalid`) that also passes `fe_runtime::portal_url::is_url_allowed` (`origin_blocked`) |
| `csp.wasm` | boolean | adds `'wasm-unsafe-eval'` (`protocol.md` §5) |
| `csp.inline_styles` | boolean | adds `'unsafe-inline'` to `style-src` only |
| `navigation` | list of origins, ≤ 16 | top-frame destinations the page may leave to; same origin rule as `csp` |
| `subscriptions.node` | boolean | the page may subscribe to scene changes in its bound petal (`ipc-v1.md` §5.1) |
| `subscriptions.readings` | boolean | the page may subscribe to readings anchored in its bound petal (§5.2) |
| `subscriptions.queries` | list ≤ 16 of `{name, sql, params}` | named guarded queries (§5.3 of ipc-v1); `name` matches `[a-z][a-z0-9_]{0,47}`; `sql` is one `SELECT`, ≤ 8 KiB, no `;`; `params` maps `[a-z][a-z0-9_]{0,31}` (not starting `fe_`) to a type in `string | integer | number | boolean | node_id` (`query_invalid`) |
| `commands.set_property.keys` | list ≤ 32 of key patterns | present only when the bundle writes properties; a pattern is an exact key or a `prefix.*` wildcard matching `[a-z0-9_]+(\.[a-z0-9_]+)*(\.\*)?` (`key_pattern_invalid`) |
| `commands.invoke` | list ≤ 16 of `{package, command}` | extension commands, called through the bus as `<package>/<command>` with caller kind `portal` (sibling R14) |
| `hooks` | list ≤ 16 of `{source}` | foreign extension streams the page consumes, `<package_id>/<stream>`; a `host/…` source is `target_invalid` (host streams are `subscriptions`) |
| `streams` | list ≤ 8 of `{id, max_rate_hz, max_event_bytes, consumers?}` | streams the page produces, published as `<package_id>/<id>`; `max_rate_hz` 1–10; `max_event_bytes` 1–16 384; optional `consumers` names the only packages that may consume it (sibling R14) |
| `ui_intents` | list of `focus_node`, `select_node` | local UI requests; never a write |

5.1. **Reserved property keys.** `node_kind` and every key under `portal.` are host-reserved. A `set_property.keys` pattern that matches or covers one of them (`node_kind`, `portal.*`, `portal.bundle_asset`, and any bare `*`) is `key_pattern_invalid`, and the host refuses those keys at write time regardless (`ipc-v1.md` §6.1). A bundle can therefore never rewrite its own repository-node reference and swap in other code.
5.2. **No duplicates.** Duplicate origins, query names, key patterns, invoke targets, hook sources or stream ids are `duplicate`.
5.3. **Static parse only.** The validator checks the SQL shape cheaply. The authoritative check is fe-api's query guard, run at every subscribe and re-run (ipc-v1 §5.3). A query that passes validation can still be refused at subscribe.

## 6. Validation and refusal classes

The validator is a pure function over the header phase plus the member directory (no body bytes, except the digest check for sealed archives that fe-format already performs). The checks run in this order and the first failure wins, so each fixture maps to exactly one class. S0's table test has one row per class.

| # | Class | Condition | Chrome reason (`admission.md` §2) | Ingest status |
|---|---|---|---|---|
| 1 | `too_large` | archive bytes over the cap (§8.4), or declared uncompressed total over 8 × the cap | too large | 413 |
| 2 | `archive_limit` | fe-format limits: entry over 64 MiB, archive over 1 GiB uncompressed, more than 200 000 entries (`fe-format/src/limits.rs:8-14`), or a `.json` member over 8 MiB (§4.3) | too large | 413 |
| 3 | `integrity` | the seal fails to verify (any `IntegrityError`) | integrity failure | 422 |
| 4 | `manifest_missing` | no `plugin/extension.json` | malformed manifest | 422 |
| 5 | `manifest_malformed` | not JSON, or a key has the wrong type | malformed manifest | 422 |
| 6 | `kind_not_web_bundle` | manifest v2 `kind` is not `web-bundle` | malformed manifest | 422 |
| 7 | `envelope_key_forbidden` | §1.8 | malformed manifest | 422 |
| 8 | `id_mismatch` | §2.1 | malformed manifest | 422 |
| 9 | `schema_unsupported` | `portal.schema` is not `fe-portal-bundle/1` | malformed manifest | 422 |
| 10 | `vocabulary_unknown` | an unknown key anywhere inside `portal` (for example `csp.script`) | malformed manifest | 422 |
| 11 | `limit_exceeded` | a list longer than its §5 bound | malformed manifest | 422 |
| 12 | `origin_invalid` | a `csp` or `navigation` entry is not an exact `https` origin | malformed manifest | 422 |
| 13 | `origin_blocked` | an origin fails `is_url_allowed` (localhost, loopback, private, link-local) | malformed manifest | 422 |
| 14 | `query_invalid` | §5 query rules | malformed manifest | 422 |
| 15 | `key_pattern_invalid` | §5 key grammar or §5.1 | malformed manifest | 422 |
| 16 | `target_invalid` | an invoke, hook or stream id breaks its grammar, or a hook names `host/…` | malformed manifest | 422 |
| 17 | `duplicate` | §5.2 | malformed manifest | 422 |
| 18 | `component_present` | §1.4 | malformed manifest | 422 |
| 19 | `member_reserved` | a body member outside `web/`, or under `web/__fe/` | malformed manifest | 422 |
| 20 | `member_path_invalid` | §3.1 | malformed manifest | 422 |
| 21 | `member_type_unknown` | §4.2 | malformed manifest | 422 |
| 22 | `entry_missing` | `entry` names no member | malformed manifest | 422 |
| 23 | `entry_not_html` | `entry` is not `.html`/`.htm` | malformed manifest | 422 |

R1's two named rows land here: a remote script origin is row 10 (`csp.script` is not in the vocabulary), and a `web/__fe/` member is row 19.

## 7. Capability digest and capability increase

7.1. The **capability set** of a bundle is its `portal` section minus `schema`, `entry` and `title`.
7.2. The **capability digest** is BLAKE3 over the RFC 8785 (JCS) canonical JSON of the capability set, as lowercase hex. Consent is recorded against this digest, the way sibling R19/R20 record consent against an exact capability digest [A-10].
7.3. A **capability increase** from build X to build Y of one package is any of:
  - an origin in a `csp` list or in `navigation` that X lacks;
  - `wasm` or `inline_styles` going from `false` to `true`;
  - `subscriptions.node` or `subscriptions.readings` going from `false` to `true`;
  - a query name X lacks, or a changed `sql` or `params` under an existing name (consent showed the SQL);
  - a `set_property` key that Y's patterns match and X's do not (pattern containment, not string equality);
  - an `invoke` target, `hooks` source, `streams` id or `ui_intents` value that X lacks, or a raised `max_rate_hz` / `max_event_bytes`, or a removed `consumers` restriction.
7.4. Admission's update policy (`admission.md` §6, OD-7) consumes 7.3. A pure `capability_increase(x, y) -> bool` beside the validator is the single implementation; S0 owns it and its table.

## 8. Asset mapping and ingest

8.1. **Store once.** A bundle is stored as one blob (BLAKE3 `blob_hash`) with one `asset` row whose `content_type` is `application/vnd.fractalengine.portal-bundle+hexon` (OD-5). The row has the anchor-petal ownership and reach of any upload (`fe-api/AGENTS.md` §assets, DEC-C21). Bytes never cross the DB channel: the route uses the `store_*` / `register_asset` shape.
8.2. **Upload.** `POST /api/v1/petals/{petal_id}/portal-bundles`, Editor or above at the petal, multipart with one `file` part. The route runs §6 in full before it stores anything.
  - `200` with `data: {asset_id, blob_hash, content_hash, package_id, version, sealed_by, title, entry}`, where `sealed_by` is the publisher DID or `null`;
  - `413` for classes 1–2, with nothing stored;
  - `422` with `error: "<class>"` for classes 3–23, with nothing stored;
  - the existing `403` / uniform `404` for role and scope (§existence-oracle).
8.3. **File index.** `GET /api/v1/assets/by-id/{asset_id}` on a bundle answers the file index instead of `501`. This is the "manifest/listing response" the directory-asset branch reserved (`fe-api/src/assets.rs:374`, `fe-api/AGENTS.md` §assets); the generic `application/x-fe-directory` sentinel keeps answering `501`.
  - Body: the standard `ApiResponse` envelope with `data: {asset_id, content_type, blob_hash, content_hash, package_id, version, sealed_by, title, entry, capability_digest, members: [{path, size, blake3, mime}]}`; `blake3` is the committed digest, or `null` when unsealed.
  - Members are listed, never served. No route on the fe-api origin serves a bundle member inline, and every REST blob response keeps `Content-Disposition: attachment` and `X-Content-Type-Options: nosniff` (`fe-api/src/assets.rs:437-451`). A bundle runs only on its own `fe-portal` origin.
8.4. **Size cap.** The default cap is 32 MiB of archive bytes (OD-4), configurable per node with `FE_PORTAL_BUNDLE_MAX_BYTES`. The ladder's top tier (25 MB) fits. A bundle replicated from a peer with a higher cap shows "Blocked — too large" and is never mounted.
8.5. **Summary read.** `GET /api/v1/portal-bundles/{asset_id}` (S0b) returns the §8.3 summary without `members`, plus `admission: {state, reason}` (`admission.md` §2) when, and only when, the caller is the host's local identity (sibling R12): admission is a local fact and a remote token holder learns nothing about it. `state` is `admitted`, `static_sandbox`, `revoked` or `blocked`; `reason` is the snake_case §2 reason code, and on `static_sandbox` it is `cannot_admit` when the local identity may not admit this build (unsealed, or governance's may-admit verdict is no), otherwise absent. The Repository section uses it to offer or omit Admit... (`ux.md` §2).

## 9. Repository nodes

9.1. A repository node is an ordinary `node` row with `properties.node_kind = "repository"` and `properties.portal.bundle_asset = "<asset_id>"` (OD-12). Property keys are flat dotted strings, the convention `stamp.override.*` already uses (`fe-ui/src/actions/asset.rs:34-36`), and values are JSON (`SetPropertyRequest.value`, `fe-api/src/types.rs:48-51`). There is no schema change.
9.2. `fe_query::spatial_nodes::NodeKind` gains `Repository` with tag `repository`, so `GET /api/v1/petals/{p}/nodes?kind=repository` lists repository nodes; the vocabulary is a closed enum (`fe-query/src/spatial_nodes.rs:28-42`).
9.3. `properties.portal.feeds` is optional: a JSON object from feed name (`[a-z][a-z0-9_]{0,47}`) to a signed share URL. It is the static sandbox's only data path (`admission.md` §7). Share URLs are bearer credentials, so attaching one publishes that feed to everyone who can read the node; the inspector says so in the feed row's accessible description.
9.4. Every `portal.*` key is written through the existing property routes, by Editor or above, and replicates through the existing property path, judged by the writer's role at write time (QA-0131).
9.5. **Replicated rows never execute.** A repository node or bundle row arriving from a peer changes nothing locally: no prefetch-triggered mount, no admission, no open. Only an explicit local **Open** mounts a bundle (D-X5; `admission.md` §3).

## 10. Examples

A documentation site, static and bridge-free:

```json
{
  "manifest_version": 2,
  "id": "org.example.pump-docs",
  "version": "1.0.0",
  "kind": "web-bundle",
  "portal": {
    "schema": "fe-portal-bundle/1",
    "entry": "web/index.html",
    "title": "Pump station operating procedures",
    "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": [], "wasm": false, "inline_styles": false },
    "navigation": ["https://docs.example.org"],
    "subscriptions": { "node": false, "readings": false, "queries": [] },
    "commands": {},
    "hooks": [],
    "streams": [],
    "ui_intents": []
  }
}
```

A read-only dashboard fed by node changes, readings and one named query:

```json
{
  "manifest_version": 2,
  "id": "org.example.flow-dashboard",
  "version": "2.3.0",
  "kind": "web-bundle",
  "portal": {
    "schema": "fe-portal-bundle/1",
    "entry": "web/index.html",
    "title": "Flow dashboard",
    "csp": { "connect": [], "img": [], "font": ["https://fonts.example.net"], "media": [], "frame": [], "wasm": true, "inline_styles": false },
    "navigation": [],
    "subscriptions": {
      "node": true,
      "readings": true,
      "queries": [
        { "name": "hourly_flow", "sql": "SELECT metric, math::mean(value) AS mean FROM iot_reading WHERE metric = $metric GROUP BY metric", "params": { "metric": "string" } }
      ]
    },
    "commands": {},
    "hooks": [],
    "streams": [],
    "ui_intents": ["focus_node", "select_node"]
  }
}
```

An interactive bundle that writes, calls an extension and produces a stream:

```json
{
  "manifest_version": 2,
  "id": "org.example.fleet-console",
  "version": "0.4.0",
  "kind": "web-bundle",
  "portal": {
    "schema": "fe-portal-bundle/1",
    "entry": "web/console.html",
    "title": "Fleet console",
    "csp": { "connect": [], "img": [], "font": [], "media": [], "frame": [], "wasm": false, "inline_styles": true },
    "navigation": [],
    "subscriptions": { "node": true, "readings": false, "queries": [] },
    "commands": {
      "set_property": { "keys": ["fleet.*", "console.note"] },
      "invoke": [{ "package": "org.example.fleet-sim", "command": "start" }]
    },
    "hooks": [{ "source": "org.example.fleet-sim/positions" }],
    "streams": [{ "id": "selection", "max_rate_hz": 4, "max_event_bytes": 2048 }],
    "ui_intents": ["focus_node"]
  }
}
```

## 11. Assumptions

Each assumption names its reversal cost. A-1 to A-8 are the plan's (`conductor/tracks/portal_surface_20261107/plan.md` §0); these are added by this contract.

- **A-9. One content hash.** The sealed `content_hash` of §2.3 equals the value S-PLUGIN signatures cover, the revocation list names, and the store-client contract v0 calls the "payload digest". If EXT-PLATFORM's `package-format.md` defines it differently, §2.3 follows that file and nothing else here changes. *Low.*
- **A-10. Consent renders the `portal` section.** The sibling's consent dialog (R20) renders §5 for `kind: "web-bundle"` and records consent against §7.2's digest. This track never builds a second dialog. *Medium.*
- **A-11. The sibling's manifest v2 schema references §5.** `fe-sdk/schema/**` `$ref`s `manifest-v1.schema.json` for `portal` when `kind` is `web-bundle`, and forbids §1.8's keys for that kind. *Low.*
- **A-12. Unsealed static rendering stays allowed by default.** No per-verse policy ceiling forbids rendering unsealed bundles in the static sandbox. A policy ceiling for that, if the sibling adds one, is honoured as `Blocked — verse policy`. *Low.*
