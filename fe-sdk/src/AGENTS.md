# fe-sdk/src — module rationale

`fe-sdk` is the **stable, serde-only** extension API surface. It has exactly two
dependencies (`serde`, `serde_json`) and must never pull in engine internals
(Bevy, SurrealDB, wasmtime, rhai). It is the single source of truth for the data
types the plugin/extension API is expressed in — `fe-plugin` and
`fe-plugin-test` both re-export these rather than defining parallel copies.

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
