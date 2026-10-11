# fe-hexon/src — module rationale

## §material-loader (`bim_primitives_on_paths_20260712`, FR-3)

`handlers/material.rs` owns two things: `MaterialHandle` (install-time
verification, pre-existing) and the FR-3 loader (`resolve_material_textures`,
`DecodedTexture`, `ResolvedMaterial`) — resolving a handle's role→blob-hash
map through `registry::FsBlobStore` and decoding each blob to raw RGBA8 +
dimensions via the `image` crate. This crate stays Bevy-agnostic by design
(it's a headless registry/package crate); the caller (`fe-ui`, which owns
Bevy) wraps the decoded bytes into `bevy::image::Image` + `StandardMaterial`
— see `fe-ui/src/verse_manager/AGENTS.md` §primitives. Missing/undecodable
blobs resolve to `None` per role rather than failing the whole material.

## §blob-store (RT-FSBLOB, 2026-10-11)

`registry::FsBlobStore` shares the runtime store's canonical layout
(`{root}/{hex[0..2]}/{hex}`, fe-runtime/src/AGENTS.md §blob-store) and its
default root (`{data_local_dir}/fractalengine/blobs`), so hexon installs and
the P2P/runtime store are ONE content-addressed store. `store` refuses a
non-64-hex hash or a `blake3(data)` mismatch (`RegistryError::Package`) and
writes atomically; `has` answers `false` for an invalid hash. The old flat
`hexon_blobs/{hash}` directory is not migrated: production never constructs a
`HexonRegistry` (only `TilesetRegistry`), so nothing was ever installed
there. Test fixtures must use `signature::asset_hash(&bytes)`, not fake hashes.

## §package-open limits (REG-DECOMP, 2026-10-11)

`HexonPackage::open` reads every entry through
`fe_format::read_entry_capped` with one shared budget per package (JSON /
text entries 8 MiB, binary 64 MiB, 1 GiB total, ≤ 200 000 entries — fe-format
AGENTS.md §decompression-limits), so a crate install (`fe-api/src/hexon.rs`)
refuses a zip bomb instead of inflating it. This is why fe-format is a
regular dependency here. Behaviour change: a read error on the optional
`icon.png` / `README.md` / `schema.json` now fails `open` instead of being
treated as absent (a non-UTF-8 README/schema is still treated as absent).

## §authz (auth_policy_pattern_20260710 — Phase 8.4 gap closure)

`authz.rs` gates registry mutations (`install_as`, `uninstall_as`) at a petal
scope and exposes petal-constrained discovery helpers
(`list_installed_as`, `search_local_in_petal_as`, `get_installed_as`). The API adapter
resolves the caller's petal scope and checks token containment before invoking
these methods; an installed crate bound to one petal cannot be read or removed
through another petal's API scope. The ungated `install`/`uninstall`/
`list_installed`/`search_local` methods remain only for the trusted local
desktop path. Network-facing callers must use the `*_as` methods.
