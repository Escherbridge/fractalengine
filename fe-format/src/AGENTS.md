# fe-format/src — Hexon v1.0.0 format rationale

Spec: `docs/hexon-format-spec.md`. This crate is the pure format layer
(serde + zip + ed25519 + blake3) with no engine or DB dependencies, so every other
crate that reads or writes `.hexon` archives can depend on it without cycles.

## §features

`runtime` (default) pulls `fe-runtime` — and through it bevy — for exactly two
things: `From<NodeDto> for ExportNode` (consumed by fe-api) and the
`is_safe_path_id` re-export (consumed by fe-terrain). Both are `#[cfg(feature =
"runtime")]`. Bevy-free consumers (the hosted registry, PAS-P5 S3) depend on
`fe-format = { default-features = false }` and still get the whole archive,
signature and integrity surface. Gate: `cargo check -p fe-format
--no-default-features` and `cargo tree -p fe-format --no-default-features -e
normal | grep -c bevy` == 0. fe-api opts into `runtime` explicitly. (HU-P1's
"fe-format gains zero dependencies" check is amended by D-76: `blake3` is added,
`fe-runtime` becomes optional.)

## §archive (`archive.rs`)

`HexonArchive` reads/writes the `.hexon` ZIP. It replaces the old
`FractalArchive` and handles both scene-type hexons (an `entities/` directory
of `ExportNode`s) and generic hexons (models, materials, terrain, ...).
`HexonArchiveData` is the fully-extracted view: manifest, entry catalog,
license, nodes, field defs, and schema.

`import` decodes under the 1 GiB total budget (`limits.rs`);
`import_with_budget(bytes, budget)` takes a smaller one (never larger). S8b:
fe-terrain decodes peer-supplied streamed bundles under 64 MiB, because
several stay resident at once. `is_safe_path_id` is re-exported from
fe-runtime (`fe-runtime/src/AGENTS.md` §safe-id) so fe-terrain can guard
ids without a fe-runtime edge.

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

## §integrity (`integrity.rs`) — D-76 Merkle-extended signature root

One Ed25519 signature (the unchanged §signature scheme) covers everything:

```text
manifest.json  (signed; canonical JSON, signature blanked)
 └─ integrity: { v: 1, algo: "blake3",
                 header: { "entries.json": h, "license.json": h, "schema.json": h,
                           "terrain/tileset_meta.json": h, "terrain/config.json": h,
                           "members.json": h, "chunks/index.json": h } }  (those present)
      ├─ members.json      { "<zip path>": h, ... }  every body member (tiles, satellite,
      │                    entities/*, terrain/tracks/*, chunks/{seq}.hexon, ...)
      ├─ assets/{h}        self-certifying: h == blake3(bytes) AND h is referenced by the
      │                    header-committed entries.json (asset_hash or sub_assets value)
      └─ chunks/index.json a serialized `TilesetBundleIndex`; every bundle's
                           `chunks/{seq}.hexon` must be committed in members.json with
                           the same blake3 (each chunk is itself a sealed hexon)
```

- **Digests are blake3 over the exact stored (decompressed) member bytes.** Nothing
  but the manifest is canonicalised, so JSON formatting can never break a digest.
  `IntegrityRoot` is strings + integers only (no canonical-JSON floats, R4).
- **Coverage invariant.** Every member except `manifest.json` must be in the
  `header` map, in `members.json`, or be a self-certifying `assets/{h}`; anything
  else is `Uncovered`. This is what catches a post-seal rewrite such as the splat
  append (`fe-terrain/src/splat/format.rs`), so **`seal` is always the last
  transform** in every producer.
- **Assets are ALWAYS self-certifying in a sealed archive** (never listed in
  `members.json`, even if a producer's `members.json` names them): this is the
  A6 `asset_hash` check. `seal` refuses an asset whose name is not its blake3
  (`DigestMismatch`) or that the catalog does not reference
  (`AssetNotReferenced`). A catalogued hash with no `assets/` member is NOT an
  error: the asset may travel outside the archive (fetch-by-hash in the one
  `content_hash` identifier space) and is self-certifying wherever it is fetched,
  so removing one can deny but never inject.
- **The header map may only name `HEADER_MEMBER_PATHS`** (small JSON, read in the
  header phase under the JSON cap); anything else is `Unsupported`. This keeps the
  header phase bounded for PAS-P5 S2's `HeaderDoc`.
- **`seal(bytes, key)`** reads every member once under the shared decompression
  budget, raw-copies them (`ZipWriter::raw_copy_file`, no recompression; CRC and
  compression method preserved), drops any previous `members.json` (re-seal is
  idempotent), writes `members.json`, sets `manifest.publisher_did` to the key's
  DID (a signature only verifies against its own DID, so any other value would
  produce an unverifiable archive; this is what lets `seal_hexon` re-seal legacy
  archives carrying placeholder DIDs), sets `integrity`, and signs. It refuses a
  `members.json` over `MAX_JSON_ENTRY_BYTES` (R7: >~80k members must be a chunked
  container) and a chunk table that disagrees with its chunk members.
- **`verify_sealed_header(manifest_json, header_members, policy)`** is the header
  phase over raw bytes (the shape both transports carry): signature → pin →
  header digests → parse `members.json` / `entries.json` / `chunks/index.json`
  (digests are checked BEFORE parsing, so tampering reports `DigestMismatch`, not
  a parse error). It returns a `SealedCoverage` whose `check_member(_digest)` is
  the ONE coverage rule every lazy reader applies per member (S2 extends
  `HexonArchiveIndex` with it; no second guard set).
- **`verify_archive(bytes, policy)`** = header phase + every member through
  `check_member` + every `members.json` path present (`MissingMember`). A member
  whose zip CRC fails reads as `Malformed`, a re-zipped tamper as `DigestMismatch`.
- **Policies.** `VerifyIfSealed` (default) enforces a seal fully when present and
  lets a legacy archive through as `IntegrityStatus::Unsealed` (local installs,
  A2). `RequireSealed { pin }` refuses unsealed archives (`NotSealed`) and, with a
  pin, any other publisher (`PublisherMismatch`); network paths and registry
  publish use it. Stripping `integrity` from a sealed archive downgrades it to
  unsealed, which only `VerifyIfSealed` accepts — by design.
- **Keys never live in files.** `examples/seal_hexon.rs <in> <out>` reads the
  32-byte hex seed from `FE_SECRET_HEXON_SEAL_KEY` and self-verifies the output
  under `RequireSealed { pin: <its DID> }` before writing it.
