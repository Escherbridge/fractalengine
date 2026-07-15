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

`registry::FsBlobStore::default_path()`/`open_default()` mirror
`fe_sync::FsBlobStore`'s convention (`{dirs::data_local_dir()}/fractalengine/...`)
but live in a separate directory (`hexon_blobs/`) since hexon-installed
material/texture blobs are a distinct content-addressed store from the P2P
sync blob store.

## §authz (auth_policy_pattern_20260710 — Phase 8.4 gap closure)

`authz.rs` adds the policy gate the 8.4 review flagged as missing: registry
mutations (`install_as`, `uninstall_as`) require Editor+ at the petal scope
via `fe_policy::RoleLevelPolicy::standard()`, while discovery reads
(`list_installed_as`, `search_local_as`) are public — `PublicReadPolicy`
composed into the same engine allows anonymous `Action::Read` only. Denials
surface as `RegistryError::PermissionDenied`. The ungated `install`/
`uninstall`/`list_installed`/`search_local` remain for the local trusted path
(the user's own machine, UI-driven installs); the hosted registry service
(`fe-hexon-registry` crate) must route through the `*_as` variants — that
wiring is a logged follow-up, same as the fe-api adapter.
