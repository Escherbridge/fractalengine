# fe-runtime/src — module rationale

## §scene-change

`messages.rs::SceneChange` is the process-wide scene-delta contract. Every
node-scoped variant carries its owning `petal_id`; a WebSocket receiver must
use `SceneChange::petal_id()` to scope both deltas and transform rollbacks.
Producers must not emit a node-scoped scene change when they cannot determine
that petal. This makes deleted-node events safe to filter without a post-delete
database lookup.

## §property-bridge (`bim_primitives_on_paths_20260712`, C5)

`shared_node.rs::PropertyValue` is the Tauri↔Bevy bridge's authoritative
property shape (`String`/`Number`/`Boolean`/`Array`/`Json`). `fe_sdk::property::PropertyValue`
(`fe-sdk/src/property.rs`) is the extension-facing mirror with the same four
scalar shapes plus its own `Json(serde_json::Value)` catch-all. `From`/`Into`
impls at the bottom of `shared_node.rs` keep the two convertible without a
third enum: `fe_sdk::PropertyValue::Array` doesn't exist on the SDK side, so
the `fe-runtime → fe-sdk` direction folds `PropertyValue::Array` into `Json`
losslessly via `serde_json::to_value`. `fe-runtime` depends on `fe-sdk` (not
the reverse) — `fe-sdk` must stay serde-only/engine-decoupled per its own
`AGENTS.md`.

Primitive descriptors (`fe_sdk::primitive::PrimitiveDescriptor`) ride on this
bridge as `PropertyValue::Json(descriptor.to_json())` — see
`fe-ui/src/verse_manager/AGENTS.md` §primitives for the render-side contract.

**Untagged-deser caveat:** `PropertyValue`'s variant order (`String`, `Number`,
`Boolean`, `Array`, `Json`) means untagged deserialization tries earlier
variants first, so a `Json` payload shaped like a scalar or array (e.g.
`Json("hi")`, `Json([1,2,3])`) deserializes back as `String`/`Array`, not
`Json` — round-trip is lossless only for JSON *objects*. Primitive descriptors
are always objects, so this doesn't affect them. Do not "fix" this by
reordering variants or making the enum tagged without confirming nothing
depends on the current untagged wire shape (Tauri↔Bevy bridge).

**Fast-follow:** the two `FsBlobStore` path conventions in `fe-hexon` need
reconciling before the texture registry is wired at runtime (registry starts
empty today, so it's currently inert) — flagged for whoever wires FR-4
registry population / P2 texture install.

## §blob-store (P2P Mycelium Phase A)

`blob_store.rs` defines the `BlobStore` trait, decoupling asset-byte storage
from the database layer. It lives in `fe-runtime` (no crate dependencies) so
that both `fe-database` and `fe-sync` can depend on it without creating a
cycle:

```text
fe-runtime (trait)  <---  fe-database (uses handle)
       ^
       |
    fe-sync (FsBlobStore impl)  --->  fe-database (existing dep)
```

Hashes are raw BLAKE3 digests (`[u8; 32]`); hex encoding is provided for DB
rows (`content_hash` column) and paths (`blob://{hex}.glb`).

`MockBlobStore` must produce the same digest as `FsBlobStore` so tests can
cross-check hashes — that is why `fe-runtime` takes a direct `blake3`
dependency despite the trait itself needing none. If that dep weight ever
becomes an issue, move the mock into `fe-sync` instead.
