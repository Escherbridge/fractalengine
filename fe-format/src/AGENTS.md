# fe-format/src — Hexon v1.0.0 format rationale

Spec: `docs/hexon-format-spec.md`. This crate is the pure format layer
(serde + zip + ed25519) with no engine or DB dependencies, so every other
crate that reads or writes `.hexon` archives can depend on it without cycles.

## §archive (`archive.rs`)

`HexonArchive` reads/writes the `.hexon` ZIP. It replaces the old
`FractalArchive` and handles both scene-type hexons (an `entities/` directory
of `ExportNode`s) and generic hexons (models, materials, terrain, ...).
`HexonArchiveData` is the fully-extracted view: manifest, entry catalog,
license, nodes, field defs, and schema.

## §manifest (`manifest.rs`)

`HexonType` is the content taxonomy (Scene, Model, ..., TerrainTileset,
Bundle). `TerrainTileset` hexons carry pre-baked elevation/satellite tiles for
offline terrain rendering: `terrain/tiles/{z}/{x}/{y}.png` (elevation),
optional `terrain/satellite/{z}/{x}/{y}.jpg` (imagery), plus a `tileset_meta`
manifest block with bounds, zoom range, encoding, and tile count.

### §scale — `TilesetMeta` additive fields and `Default`

`native_scale`, `ground_sample_distance_m`, `crs`, and `scale_bounds` were all
added after v1.0.0 shipped, each `#[serde(default, ...)]` so legacy archives
still parse. `TilesetMeta` has a hand-written `impl Default` (not
`#[derive(Default)]`) whose `crs` agrees with `default_crs()` — the same
`Some("EPSG:4326")` serde fills in for a crs-less archive — so a struct
literal and a legacy-archive deserialize produce the same value; every other
field is its type's zero value, matching serde's plain `#[serde(default)]`.
**Producers must build `TilesetMeta` as `TilesetMeta { <required + chosen
fields>, ..Default::default() }`**, never an exhaustive literal — the next
additive field gets a default instead of breaking every call site (this is
why `native_scale`/`ground_sample_distance_m`/`crs`/`scale_bounds` each broke
every producer when they were added).

## §signature (`signature.rs`)

Ed25519 manifest signing/verification as defined in the Hexon v1.0.0 spec.

- **Canonical JSON.** Signatures cover a canonical serialisation — keys sorted
  recursively, no whitespace, arrays order-preserving — so byte-level JSON
  formatting differences never invalidate a signature. Both sign and verify
  first blank the manifest's `signature` field to `""` so the signature never
  covers itself.
- **Sign:** parse JSON → blank `signature` → canonicalise → Ed25519-sign →
  base64 (standard encoding) of the 64-byte signature.
- **Verify:** the mirror image; the public key is extracted from the
  manifest's `publisher_did` rather than passed separately, binding the
  signature to the claimed publisher.
- **did:key encoding.** `did:key:z6Mk...` is multibase base58btc (`z` prefix)
  over a 2-byte multicodec prefix `[0xed, 0x01]` (Ed25519 public key)
  followed by the 32 raw key bytes. `verifying_key_to_did` is the exact
  inverse of `did_key_to_verifying_key`.
