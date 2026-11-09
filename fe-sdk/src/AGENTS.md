# fe-sdk/src — module rationale

`fe-sdk` is the **stable, serde-light** extension API surface. Its dependency rule is
exactly three dependencies (`serde`, `serde_json`, `blake3`) and it must never pull in
engine internals (Bevy, SurrealDB, wasmtime, rhai). `blake3` was admitted 2026-10-14
(CDL-W4 W4-9, campaign-plan critic C7) for §legacy-identifier only: it is a pure hash,
not an engine internal, and SPEC-10 §4.5 rule 2 needs one implementation that every
consumer can reach. It is the single source of truth for the data
types the plugin/extension API is expressed in — `fe-plugin` and
`fe-plugin-test` both re-export these rather than defining parallel copies.

## §modules

| Module | Purpose |
|--------|---------|
| `api` | `ApiExtensionHandle`, `ExtensionRoute`, `HttpMethod` — HTTP route contributions |
| `node` | `NodeSnapshot` — read-only view of a node |
| `path_asset` | `PathAssetDescriptor` — repeated-model path stamping (§path-asset) |
| `primitive` | `PrimitiveDescriptor`, `PrimitiveKind` — BIM primitive shape (§primitive) |
| `property` | `PropertyValue`, `PropertyBag` — typed properties |
| `scene` | `SceneChange`, `SceneChangeBatch` — scene mutations |
| `transaction` | `PluginTransaction` trait — batched writes |
| `context` | `PluginContext` trait — engine services |
| `storage` | `ExtensionStorageApi` — node-property + KV access (§storage) |
| `query` | `ExtensionQueryApi`, `is_select_only` — SELECT-only reads (§query) |
| `texture` | `TextureRegistry`, `TextureEntry` — content-addressed textures (§texture) |
| `ui` | `UiSlot`, `UiContribution`, `UiExtensionRegistry` — UI slot contributions |
| `events` | `PluginEvent`, `EventSubscription` — inter-plugin events |
| `legacy_identifier` | `legacy_identifier`, `LegacyTable` — SPEC-10 §4.5 text id -> canonical 32-byte identifier (§legacy-identifier) |
| `source_class` | `SourceClass`, `is_valid_format_id` — SPEC-10 §7.1 source classes + §2.2 rule 4 `format_id` grammar (§source-class) |
| `manifest_v2` | `ExtensionManifest`, `parse_and_validate`, `capability_digest` - extension manifest v2 serde + the ONE validator (see manifest-v2 below) |
| `device_format` | `DeviceFormatDescriptor` — R5 device extension format descriptor (§device-format) |

## §storage

`storage.rs` defines [`ExtensionStorageApi`] — the host-provided read/write
surface over node properties and a per-extension key/value store. The engine
**binary** implements this trait; the plugin host (`fe-plugin`) holds it as an
`Arc<dyn ExtensionStorageApi>` and routes Rhai/WASM calls through it. Keeping the
trait here (not in `fe-plugin`) is what lets the plugin engine stay free of any
`fe-database` dependency — the binary injects the concrete implementation.

- Property values reuse [`PropertyValue`] so there is one property type end to
  end (no JSON-blob-only path for typed properties).
- KV methods take a `namespace` argument. The host fills this with the calling
  extension's own id so **extensions can never read each other's keys** — the
  namespace is not attacker-controlled from inside a script.
- `validate_node_id` / `validate_key` are the host-boundary input guards
  (non-empty, length-capped). They fail closed with `StorageError::InvalidInput`.
- `StorageError` hand-rolls `Display`/`Error` (no `thiserror`, to keep the dep
  set minimal). Errors are always propagated, never swallowed.

## §query

`query.rs` defines [`ExtensionQueryApi`] — a **SELECT-only** query surface — plus
the [`is_select_only`] guard. The guard is a byte-for-byte port of the
keyword-block idiom in `fe-database`'s `DbCommand::RawQuery` handler: reject any
`;`, anything not starting with `SELECT`, and any blocked SurrealQL keyword
appearing as a whole word (identifier boundaries respected so `created_at`
doesn't trip `CREATE`).

The guard lives here — rather than being copy-pasted into `fe-plugin` — because
both `fe-plugin` (real host) and `fe-plugin-test` (mock) already depend on
`fe-sdk` and must enforce the *exact same* policy. Centralizing prevents drift.
`fe-plugin` still owns enforcement (its `HostEnv::query_select` calls the guard
before delegating); this module just provides the one canonical implementation.

`MAX_QUERY_LEN` / `MAX_RESULT_ROWS` cap query size and result cardinality.

## Capability name constants

`CAP_STORAGE_READ` / `CAP_STORAGE_WRITE` / `CAP_QUERY_SELECT` are the canonical
grant strings. They live in `fe-sdk` so the host (`fe-plugin` capability tokens),
the mock (`fe-plugin-test`), and extension authors all reference one spelling.

## §primitive (`bim_primitives_on_paths_20260712`, FR-1/FR-2/C5)

`primitive.rs` defines [`PrimitiveDescriptor`]/[`PrimitiveKind`] — the
`{kind, dims, texture_ref}` shape a node's `primitive` property carries as
JSON. This is the single canonical descriptor shape; `fe-runtime`'s
`SharedNode::PropertyValue::Json` carries the serialized form (C5 — see
`fe-runtime/src/shared_node.rs` §property-bridge), and `fe-ui`'s render
branch (`fe-ui/src/verse_manager/spawn.rs`) parses it via `from_json`. No
second descriptor type exists anywhere in the workspace — extend this one.

## §path-asset (`hexon_path_asset_stamp_20260713`)

`path_asset.rs` defines [`PathAssetDescriptor`]/[`SpacingMode`] — the
`{asset_path, spacing_mode, spacing_value, count, tangent_align}` shape a
**track** node's `path_asset` property carries as JSON. It rides the property
bag as `PropertyValue::Json` exactly like [`PrimitiveDescriptor`] (§primitive),
so it round-trips losslessly through `SetNodeProperty`/`NodePropertiesLoaded`.

The descriptor names a single model asset (`asset_path`, a `blob://{hash}.glb`
URI or asset ref) that `fe-ui` stamps repeatedly along the track's persisted
`gpx_points` — one scene instance per arc-length sample point. `spacing_mode`
selects the sampling policy: `FixedSpacing` places instances every
`spacing_value` world-units of arc length; `FixedCount` distributes `count`
instances evenly across the whole path. `tangent_align` rotates each instance
to face the local path tangent. The consuming reconcile system + the
fe-ui-local arc-length sampler live in
`fe-ui/src/verse_manager/path_asset_reconcile.rs` (see that crate's
`AGENTS.md` §path-asset-stamp) — this crate stays engine-free and owns only
the serde shape.

All fields except `asset_path` are `#[serde(default)]`-tolerant so partial
descriptors written by older/other tools still parse; `asset_path` is required
(a stamp with no model is meaningless) and its absence is a parse error.

## §texture (FR-4, C6)

`texture.rs` defines [`TextureRegistry`]/[`TextureEntry`] — copy-adapted from
[`ui::UiExtensionRegistry`] (`register`/`unregister_all(plugin_id)`/`get`).
Entries reference a content-addressed `blob_hash` already installed via a
hexon package; v1 never accepts raw texture bytes from a plugin (C6). The
engine wraps this in a Bevy `Resource` newtype at the call site (`fe-ui`)
since this crate must stay bevy-free — see
`fe-ui/src/verse_manager/AGENTS.md` §primitives.

## Traits vs. structs (context/transaction)

`context.rs` and `transaction.rs` define **object-safe traits** — the
extension-author-facing contract. `fe-plugin` keeps its own concrete host-side
`PluginContext`/`PluginTransaction` structs (they carry crossbeam channels and
capability tokens, which are engine-internal). These are deliberately *not*
unified onto the SDK traits: they are different layers (stable contract vs.
runtime implementation). Only the serde **data** types are unified.

## §legacy-identifier (CDL-W4 W4-9, SPEC-10 §4.5)

`legacy_identifier.rs` is the **single** implementation of SPEC-10 §4.5's mapping from a
hot-plane text id (verse, petal, node ULIDs) to a canonical-log `Identifier32`:

```text
legacy_identifier(table, id) =
    BLAKE3(ASCII("fe-legacy-identifier-v1") || 00 || canonical_cbor([table, id]))
```

- **Why here.** SPEC-10 §4.5 rule 2 says the observation family's mapping and SPEC-8's
  candidate-scope derivation MUST be the same function, and that no second one may exist.
  fe-sdk is the lightest crate every consumer reaches: fe-canonical-log depends only on
  fe-sdk, fe-database gained the edge in W4-9 for its `node-transform-v1` mapping, and
  CDL-SPEC10 B1 (kind-5 `anchors`) and B2 (hexon `anchor` column) call this function
  rather than hashing locally.
- **Returns `[u8; 32]`, not `Identifier32`.** `Identifier32` lives in fe-canonical-log,
  which depends on this crate; callers wrap the bytes (`Identifier32(bytes)`).
- **Hand-encoded CBOR, no CBOR crate.** The value is always a two-element array of two
  text strings, so the encoder is the array header `0x82` plus two shortest-form text
  headers (RFC 8949 §4.2.1). `fe-canonical-log/tests/legacy_identifier_golden.rs` checks
  it against fe-canonical-log's own canonical encoder at every text-length header boundary
  (0, 1, 23, 24, 255, 256, 65 535, 65 536 bytes), so the two encoders cannot drift apart
  silently.
- **ASCII only, fail closed.** Canonical CBOR text must be Unicode NFC and this crate has
  no normalizer. A non-NFC id would hash to bytes a canonical encoder refuses or
  normalizes, which is a second, divergent mapping. Every non-ASCII id is therefore
  refused (`LegacyIdentifierError::NonAsciiId`); every hot-plane id today is an ASCII ULID.
- **Enumerable, so never in clear (rule 3).** Anyone who knows a node id can compute its
  identifier. Callers keep the output inside encrypted payloads, sealed hexons, blinded
  topics or a header scope, never in clear manifest statistics.
- **Provisional.** The domain `fe-legacy-identifier-v1` is a SPEC-10 provisional number
  (D-CL34), listed in fe-canonical-log's single register (`src/AGENTS.md` §Provisional wire
  numbering) for CDL-PROV batch ratification.

## §source-class (CDL-SPEC10 A2, SPEC-10 §7.1, §2.2 rule 4)

`source_class.rs` holds the ONE `SourceClass` enum (device, connector, decoder, manual,
simulation; provisional wire values 1-5, zero never a class) and the ONE `format_id` grammar
check, `is_valid_format_id`.

- **Why here.** `fe-runtime` depends only on `fe-identity` and `fe-sdk`, so it cannot see a
  `SourceClass` defined in `fe-canonical-log`; a second enum in `fe-runtime/src/messages.rs`
  would drift from the canonical one. fe-canonical-log already depends on this crate, so
  `fe_canonical_log::materialize::identity` re-exports this type rather than defining its own.
  CDL-SPEC10 A3's `source_class` field on `InsertIotReadings`/`InsertDevicePayloads` uses this
  same type.
- **No `Default`, on purpose.** SPEC-10 §7.1 rule 6: the class travels explicitly from the
  caller to the ingest door and is never inferred (the simulation lab sets `source_did` to its
  host peer's DID). A `Default` would let a caller omit it and get a class silently.
- **serde form** is snake_case (`"device"`, `"simulation"`, ...); the wire value is the
  separate `wire()`/`from_wire()` pair, used only by the canonical CBOR codec.
- **`is_valid_format_id`** lives beside the class because both enter `ObservationSource`
  signed bytes. A `format_id` is also the key of the R5 device-format descriptor, so A3's
  `device_format` module and its ingest doors MUST call this function rather than re-implement
  the grammar ("shared cores + separate guards = drift", RUNBOOK §5). ASCII-only, so NFC holds
  trivially.

## §device-format (CDL-SPEC10 A3, SPEC-10 §2.2, R5)

`device_format.rs` holds [`DeviceFormatDescriptor`] — `{format_id, mime, schema, description}` —
the serde-only shape a device extension declares before its payloads (R5: "a declared format
descriptor"). `is_well_formed` validates `format_id` via [`source_class::is_valid_format_id`] and
`mime` via `is_valid_mime` (a `type/subtype` RFC 6838 token pair with optional `; params`, at most
255 bytes, no control characters; the fe-database door and its inbound replication gate share it,
and a blob response's `Content-Type` is built from it); it does NOT check uniqueness against an existing declaration in the
same verse — that conflict check belongs to `fe-database::handlers::device_payload`, which is the
only place that can see other rows. `schema` is informative JSON Schema only: R5 says payload bytes
are stored opaque, so nothing in this crate or fe-database validates bytes against it in v1.

## �property-schema

`property_schema` implements the named **bounded property-schema** vocabulary, not
JSON Schema. It uses existing serde dependencies; no resolver, `$ref`, network,
coercion, implicit default or backfill. Each rule requires `type`:
object/array/string/number/integer/boolean/null. Supported constraints:
`nullable`, `enum`, `properties`, nested `required`, `additionalProperties`
(boolean, default true), `items` (one rule), `minItems`/`maxItems`,
`minLength`/`maxLength`, `minimum`/`maximum`. Unknown keywords and constraints on
incompatible kinds fail. Required members must be declared. Outer properties
remain optional; null needs permission. Limits: depth16, rule nodes256,
encoded schema/property65536bytes. String length counts Unicode scalar values.
Validation preserves JSON types. Device-format schema remains independent.


## Plugin host

`plugin_host` is the shared engine-free read/atomic-write contract. NodeQuery
is an exact name/property predicate with a bounded limit, never SQL or code.
PendingOp keeps the existing three operations. The sink must persist the whole
batch before returning its receipt, or refuse before effects. The host validates
64 operations/64 KiB, and readers return at most 200 nodes/64 KiB. Plain property
JSON conversions preserve nested values and integer precision across script hosts.


PluginLease is a nonserialized shared revocation flag minted by the trusted host.
Contexts and queued admissions clone the same lease; cloning never renews it.

## path geometry

path_geometry owns stamp consumers' pure f32 de Casteljau anchor flattening, fixed subdivision
and cumulative-distance order. The editor curve facade re-exports it; renderer caches, stamp
defaults and database metric bounds share these operations without adding engine dependencies.
Raw anchors retain relative handles. Database preparation fences the original raw rows and map
after deriving flattened geometry, so a handle edit cannot reinterpret a previously captured arc.


## Shared stamp identity sampling

path_asset::sample_progresses supplies identical bounded instance identities to the UI renderer
and checked GUI promotion validator, including one-point paths, endpoint fallback, metric spacing
and the 4096 cap. Path geometry flattening and f32 length accumulation remain shared as well.

## Shared path position interpolation

`path_geometry::position_at_progress` is the existing pure f32 arc-length sampler
extracted from native path-asset reconciliation. Both rendered stamps and checked
selection captures now call the same implementation; empty/single/degenerate paths,
clamping and segment-boundary behavior are unchanged. Its cumulative distances come
from the existing shared accumulator. The native reconciliation journeys remain the
behavior checks for that sampler. This adds no engine dependency to the SDK.

## Lossless renderer route source

path_geometry::RouteSourceRow is the shared typed decode contract for host route
materialization and closed-path source witnesses. Positions, timestamps and handles
retain f64 precision; smoothness retains the renderer's existing f32 precision.
The editor's PathPointRow remains a separate f32 draft projection and cannot prove
render-source identity. The host feeds witnesses directly from frozen typed rows,
without passing through the compact persistence writer.

Normalization preserves the existing renderer decoder: missing/non-numeric time
becomes zero; absent, incomplete or all-below-f64-EPSILON handles become absent;
unknown corner codes and smooth/symmetric corners without handles become Corner;
missing smoothness becomes zero. Numeric corner codes use the existing i64 code
conversion. Compact and equivalent zero-handle rows compare equally, while inactive
handles and sub-f32 position/handle changes remain distinct. Non-finite typed source
rows cannot mint a witness. This is decoding, not zone sanitation or triangulation.

## manifest-v2 (`extension_platform_20261107`, slice E2)

`manifest_v2.rs` is the single validator behind `fe-ext validate` and REST install
(`docs/extensions/manifest-v2.md` Validation is normative; `schema/extension-manifest-v2.schema.json`
is structure only). `parse_and_validate(bytes)` runs serde (unknown fields refused), then
`ExtensionManifest::validate`, which walks V1..V15 in table order and reports the first failing row
as a `Refusal` (kebab-case `as_str()` is the reason both doors print).

- **Schema parity without a regex dep.** The JSON-Schema patterns and bounds are re-implemented
  as hand checks in `check_structure` (`is_package_id`, `is_semver`, `split_url`, ...). Edit the
  schema file and these together; the J2 test walks the three shipped examples to catch drift.
- **Contract check is split.** V1 only requires the `fractalengine:plugin@x.y.z` shape, so an old
  major reaches V2 and reports `unsupported-contract` rather than `schema`.
- **`capability_digest`** blake3-hashes the canonical grant projection of the RAW JSON (not the typed
  struct), so a member this build does not model still re-prompts consent. The canonicalizer mirrors
  fe-format's (sorted keys, no whitespace); keep them identical.
- **Named `host/query` SQL** is guarded with `query::is_select_only` here; the REST door may layer
  `fe_query::sql_guard` on top, it can only be stricter.
- **`args_schema`** is checked against the bus.md subset (keywords, depth 4, 4 KiB) at V1.
- **Host ceilings** are constants (`HOST_*`); a local policy lowers them in fe-plugin, never here.

## plugin-host additions (E2)

`PendingOp::SetTransform` patches position/rotation/scale (absent fields keep stored values; scale
must be positive, rotation a nonzero finite quaternion). `StagedReading` is the tick-window reading;
source class and DID are host-stamped, never fields. `validate_staged_readings` bounds count (1000),
metric grammar (1..128 of `[A-Za-z0-9_.:-]`), finiteness and units; anchors are a flush-time check in
the DB handler. `validate_commit(ops, readings)` is the one call for a whole `PluginCommit`.
`SetTransform` is a `set_transform` write in the manifest (`scene.write`) and is applied inside
the same atomic batch as the tick's other ops.

## provides (bindings turn, `manifest_v2.rs`)

`ExtensionManifest.provides: Vec<ProvidedHook>` lists the hook points the package serves
(`hook`, optional `keys`, `uses`, `config_schema`; docs/extensions/bindings.md section 6).
`KNOWN_HOOKS` is the catalogue this host serves; an id outside it is V17 `unknown-hook`. Only the
`KEYED_HOOKS` may carry `keys`. V18 `hook-capability-missing` checks that the hook's needs
(an ingress/egress/ui entry named in `uses`, `readings` + `tick` for `ingest.decode`, `scene.read`
for `scene.*`, ...) are declared. Bounds: 32 entries, 32 keys, 16 `uses`. `provides` is inside the
capability digest, so adding a hook re-prompts consent. A binding grants nothing: this module only
validates what a package claims; resolution and acceptance live in fe-plugin and the binary.

## path-geometry f64 route math (E2)

`snap_to_polyline`, `cumulative_distances_f64`, `position_at_progress_f64` and `RouteSnap` hold the
f64 snap/interpolation formerly private to fe-terrain `iot/path_tracker.rs`; that file should delegate
so the sim, the IoT tracker and the WIT `routes` interface share one implementation. fe-terrain's
`iot/path_tracker.rs` now delegates to it. The f32
renderer helpers stay separate on purpose (their accumulation order is a stamp-placement contract).

## §live-consumers (plugin_host_revival S7)

`ApiExtensionHandle` and `UiExtensionRegistry` have live consumers. `fe-api/src/server.rs` mounts
`fe_terrain::api_ext::terrain_api_routes()` in `build_router`. `fe_ui::plugin::UiExtensions` wraps the
`UiExtensionRegistry`, and `register_first_party_extension_ui` (fractalengine `main.rs`) fills it at Startup.
No third-party package contributes to either registry yet (fractalengine/src/AGENTS.md §plugin-host), so
a change to either shape breaks first-party consumers at once.
