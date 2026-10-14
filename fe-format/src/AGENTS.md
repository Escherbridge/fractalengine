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

## §archive-index (`archive_index.rs`)

`HexonArchiveIndex` is THE lazy `.hexon` reader (critic C5: PAS-P5 extends it
with header docs / verified reads / `chunk(seq)` instead of adding a second
reader). `open(path)` is the header phase: one pass over the zip central
directory (entry-count cap) plus `manifest.json`, `entries.json` and the
optional `terrain/tileset_meta.json` under `MAX_JSON_ENTRY_BYTES` and a shared
archive budget. `read_member(name)` then reads ONE member under
`MAX_ENTRY_UNCOMPRESSED_BYTES` (JSON members under the JSON cap); stored
members are a positioned read, deflated ones are inflated by the zip crate,
and the zip crate checks each member's CRC-32 at EOF.

- **Member table = the zip crate's parsed central directory** (an
  `Arc<Shared>` index map inside `ZipArchive`), not a hand-rolled offset table:
  `ZipArchive<R>` is `Clone` and shares the parsed directory, and each member's
  data offset is cached in a `OnceLock` after its first read, so a read is
  "clone the directory handle + seek + read", never a re-parse. Hand-rolling
  inflate/offsets would duplicate the zip crate for no gain.
- **No long-lived file handle.** The reader is a `LazyFile` whose clones start
  closed: each `read_member` opens the file, reads, and drops the handle. On
  Windows a held handle would block `remove_tileset` / reinstall.
- **Replacement detection.** Size + mtime are captured at `open`; a mismatch
  at read time fails with the typed `ArchiveChanged` (downcastable from
  `anyhow`) instead of reading the new file's bytes at the old offsets. The
  CRC check is the backstop for a same-size, same-mtime rewrite.
- `HexonArchive::import` is unchanged and still the eager, whole-archive
  validation path (install, bundling).

## §manifest (`manifest.rs`)

`HexonType` is the content taxonomy (Scene, Model, ..., TerrainTileset,
Bundle). `TerrainTileset` hexons carry pre-baked elevation/satellite tiles for
offline terrain rendering: `terrain/tiles/{z}/{x}/{y}.png` (elevation),
optional `terrain/satellite/{z}/{x}/{y}.jpg` (imagery), plus a `tileset_meta`
manifest block with bounds, zoom range, encoding, and tile count.

### §bundles — `TilesetBundleIndex` / `ChunkIndex.blake3`

`TilesetBundleIndex { schema, tileset_id, tileset_meta (chunk_index = None),
bundles: Vec<BundleEntry> }` is the deterministic spatial-bundle index (D-73
A4): a tileset split into Hilbert-ordered, size-capped bundles instead of flat
chunks. Each `BundleEntry` carries its `blake3_hex` (verified before
install), `byte_len`, `tile_count`, and the `(zoom, x_range, y_range)` tile
range it covers — `TilesetBundleIndex::bundle_for_tile(z, x, y)` is the *one*
pure tile→bundle lookup (critic C4); nothing else re-implements it.
`ChunkIndex.blake3: Option<String>` is the parallel per-chunk hash slot
(`#[serde(default, skip_serializing_if)]`, so legacy chunks without it stay
byte-identical on re-export) — PAS-P5 signs it. The bundler itself
(`bundle_tileset`, Hilbert ordering, greedy size-fill, determinism) lives in
`fe-terrain/src/tiles/bundle.rs` — see `fe-terrain/src/AGENTS.md` §bundles.

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
