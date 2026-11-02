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
