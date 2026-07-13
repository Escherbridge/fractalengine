# fe-runtime/src — module rationale

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
