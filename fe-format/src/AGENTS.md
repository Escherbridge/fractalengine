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

**PAS-P5 S2: `import`/`import_with_budget` now enforce `VerifyIfSealed`.**
Both run `integrity::verify_archive_with_budget(bytes, &VerifyIfSealed,
total_budget)` FIRST, under the same caller-chosen budget, before any
extraction — a tampered sealed archive is refused before a single section is
unpacked, and the caller's smaller budget (the streamed-bundle path) is
enforced across the integrity pass too, not only extraction. For a legacy
unsealed archive this pre-pass only ever reads `manifest.json` (see
`verify_sealed_header`'s `None` short-circuit), so it stays cheap and every
existing decompression-bomb test is unaffected. `HexonArchiveData` gained
`integrity: IntegrityStatus`. Known inefficiency (open issue, not fixed here):
a SEALED archive's members are decompressed twice — once by the integrity
walk, once by extraction — rather than merged into one pass; `import` is an
install/bundling-time path, not a hot per-tile read, so this was judged an
acceptable simplicity/correctness trade for this slice.

`HexonArchive::export_chunked(manifest, tileset_meta, chunks: &[ChunkInput])`
packs already-built chunk archives into one container (`chunks/index.json` +
`chunks/{seq}.hexon`, Stored) — see §bundles and §partial below. It returns
UNSEALED bytes; the caller seals last, same as every other producer.

**`export_stored_deterministic(manifest, extra_members)`** (SPEC-10 B2,
critic tick 7 AC-36b/C21) is the generic byte-stable packer every other
exporter above is NOT: `manifest.json` (canonical JSON via
`signature::canonical_json_bytes`, the same canonicalizer `sign_manifest`
uses) + the fixed empty `entries.json` + `extra_members` in the exact order
given, every member `Stored` with the MS-DOS epoch timestamp
(`zip::DateTime::default()`), no extra fields/comment, fixed `0o644`
attributes — so the archive bytes are a pure function of the member bytes.
First consumer: `fe-query`'s observation-hexon writer
(`fe-query/src/AGENTS.md` §observation-hexon). `member_order(bytes)` (cheap,
central-directory-only) and `read_member(bytes, name, cap)` (the same
`read_entry_capped` zip-bomb guard `import` uses) are its reader-side
companions for a profile that must check exact member order/storage method
and pull a member `import` does not itself extract (e.g.
`observations/*.parquet`) — NOT a third zip-parsing implementation, the same
`zip`/`limits` machinery `import` already uses.

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
- `HexonArchive::import` is still the eager, whole-archive validation path
  (install, bundling) — PAS-P5 S2 makes it enforce `VerifyIfSealed` (see
  §archive above); its extraction shape and existing tests are unchanged.

**PAS-P5 S2 additions** (critic C2: this is THE lazy reader — no second
`reader.rs`/index type was added):
- **Header-phase verification.** `open(path)` (= `open_with_policy(path,
  VerifyIfSealed)`) now ALSO captures the raw bytes of `manifest.json` and
  every present `HEADER_MEMBER_PATHS` member, and runs
  `integrity::verify_sealed_header` over them once, storing the resulting
  `SealCheck`. `open_with_policy(path, RequireSealed { pin })` fails `open`
  itself for an unsealed or tampered archive — nothing is read lazily before
  that check passes. A legacy unsealed archive behaves exactly as before
  (`VerifyIfSealed` is a no-op when there is nothing signed).
- **Verified `read_member`/`read_member_capped`.** When the archive is
  sealed, a member's bytes are checked with `SealedCoverage::check_member`
  before being returned — the SAME coverage rule `HexonArchive::import` and
  `verify_archive` apply (no second guard set). A header member already
  captured at `open` is returned from the cache, not re-read from disk.
- **`layout()` / `header_doc()`.** `layout()` is `Monolithic` or
  `Container { chunk_count }`: for a sealed archive it comes from the SIGNED
  chunk index; for an unsealed one from whether `chunks/index.json` is
  present (read once at `open`).
- **Uncommitted header members (S8, 2026-10-14).** For a sealed archive,
  `open` runs `SealedCoverage::check_member` over every captured header
  member, so a header member injected without being committed by the root
  (e.g. a forged `chunks/index.json` turning a monolithic archive into a
  "container") is `Uncovered` at `open` — the same refusal `verify_archive`
  gives it. Before this, the lazy reader parsed and served such a member
  from its cache unchecked (`read_member_capped` short-circuits header
  members). Test: `an_uncommitted_header_member_in_a_sealed_archive_is_refused_at_open`. `header_doc()` builds the
  `fe_format::partial::HeaderDoc` both transports carry, from the raw bytes
  captured at `open` — infallible (lossy UTF-8; every writer here emits
  strict UTF-8 JSON, so this is a no-op in practice and a hostile non-UTF-8
  byte only ever makes a downstream hash check fail safe).
- **`chunk(seq)`** reads `chunks/{seq}.hexon` through the same verified
  `read_member` path; `Ok(None)` for a monolithic archive or an out-of-range
  `seq`.
- **`integrity_status()`** reports `Sealed{publisher_did}`/`Unsealed`, the
  same shape `HexonArchiveData.integrity` carries.

## §manifest (`manifest.rs`)

`HexonType` is the content taxonomy (Scene, Model, ..., TerrainTileset,
Bundle, ObservationBatch). `TerrainTileset` hexons carry pre-baked
elevation/satellite tiles for offline terrain rendering:
`terrain/tiles/{z}/{x}/{y}.png` (elevation), optional
`terrain/satellite/{z}/{x}/{y}.jpg` (imagery), plus a `tileset_meta` manifest
block with bounds, zoom range, encoding, and tile count.

`ObservationBatch` (SPEC-10 B2) hexons carry two Parquet members
(`observations/readings.parquet`, `observations/payload_refs.parquet`),
written by `fe-query`'s observation-hexon codec (`fe-query/src/AGENTS.md`
§observation-hexon) via `export_stored_deterministic` below — never
`export`/`export_scene`/`export_tileset` (those use `Deflated` compression
and a non-canonical `manifest.json`, which would break SPEC-10 §3.1's
byte-determinism requirement). The manifest carries no `signature`/`integrity`
— the canonical-log kind-5 operation's own signature binds the sealed hexon.

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
`#[derive(Default)]`) whose `crs` agrees with `default_crs()` — both read
`DEFAULT_TILESET_CRS` (`Some("EPSG:4326")` serde fills in for a crs-less
archive) — so a struct literal and a legacy-archive deserialize produce the
same value; every other field is its type's zero value, matching serde's
plain `#[serde(default)]`.
**Producers must build `TilesetMeta` as `TilesetMeta { <required + chosen
fields>, ..Default::default() }`, and must never override `crs`** — a fresh
tileset always stamps `DEFAULT_TILESET_CRS` via `Default`, never an
exhaustive literal — the next additive field gets a default instead of
breaking every call site (this is why
`native_scale`/`ground_sample_distance_m`/`crs`/`scale_bounds` each broke
every producer when they were added). `fe-terrain/src/tiles/builder.rs`'s
`fresh_tileset_meta` helper is the house example: one helper, no `crs`
override, shared by all three of that file's construction sites.

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

## §partial (`partial.rs`) — PAS-P5 S2: the header document both transports carry

`HeaderDoc { v, uri, manifest, header_members, archive_size, layout }` is the
ONE shape the HTTP `/meta` route (S3) and the P2P HashSeq child 0 (S5) both
ship: raw `manifest.json` text plus raw text of every `HEADER_MEMBER_PATHS`
member present, never re-serialized, so a client's hash over
`String::as_bytes()` equals the digest `seal` committed to. `layout` is
`Monolithic` or `Container { chunk_count }`, read off `HexonArchiveIndex::open`
(`archive_index.rs`).

- **`verify_header(doc, policy) -> Result<VerifiedHeader, IntegrityError>`**
  is the one client-side check both transports run, with no archive bytes
  needed: it rebuilds the `BTreeMap<String, Vec<u8>>` header-members map from
  the doc's text and calls `integrity::verify_sealed_header` — the same
  function the eager `verify_archive` path uses, so there is one signature/pin/
  digest guard, not two.
- **S8 binding of the unauthenticated doc fields (2026-10-14, mandatory
  finding on S2 `cbbd806`).** `HeaderDoc.uri`, `.layout` and `.archive_size`
  sit OUTSIDE the signature. `verify_header` now (1) requires `doc.uri ==
  "{manifest.hexon_id}@{manifest.version}"` (`IntegrityError::Substituted`
  otherwise — a validly signed header of hexon A cannot be served under uri
  B); (2) derives the layout itself — from the signed `chunks/index.json`
  when sealed, from the doc's own `chunks/index.json` text when unsealed —
  and refuses any doc whose claim differs (`Unsupported`: no Monolithic
  downgrade that would let seq 0 skip per-chunk digests, no over/under-
  claimed count); (3) never copies `archive_size` into `VerifiedHeader`
  (it is advisory display data only; every fetch cap is a constant).
  A sealed doc's `header_members` may hold only committed members (an extra
  one is `Uncovered`), so callers reading the map see only signed text.
  `VerifiedHeader { uri, hexon_id, version, layout, status }` is therefore
  derived entirely from authenticated (or, unsealed, self-consistent)
  bytes. Tests: `a_signed_header_served_under_another_uri_is_refused`,
  `a_layout_claim_that_contradicts_the_signed_chunk_index_is_refused`,
  `archive_size_is_advisory_and_never_reaches_the_verified_header`,
  `an_uncommitted_header_member_in_a_sealed_doc_is_refused`.
- **`VerifiedHeader`** carries `status` (`Sealed{publisher_did}`/`Unsealed`)
  and, when sealed, the `SealedCoverage` every subsequent `fetch_chunk`/
  `fetch_member` call (S4/S5) checks bytes against via `chunk_digest(seq)` or
  `coverage().check_member(...)`.
- **Format version.** `HEADER_DOC_VERSION = 1`; an unrecognized `doc.v` is
  `IntegrityError::Unsupported` before anything else is parsed.
- **`HeaderDoc::to_json`** (PAS-P5 S5) is the ONE encoder of P2P HashSeq
  child 0. fe-hexon's `publish_to_transport` and fe-terrain's
  `bundle_for_seeding` both call it, so one sealed hexon has one root
  whichever host seeds it, and a `P2pPartialFetch` works against any seeder.
- **`verify_archive_against_header(archive, signed_manifest, publisher, budget)`**
  (S5) is the one whole-archive check for a monolithic partial fetch. The
  archive's raw `manifest.json` must equal the header's signed text byte for
  byte, which rules out a same-publisher swap. The archive must then pass
  `verify_archive_with_budget(RequireSealed { pin: publisher })`. Three
  callers: fe-hexon's `FetchStrategy::fetch_chunk(seq 0)`, fe-terrain's sealed
  P2P install, and fe-terrain's streamed decode, which passes the smaller
  streamed budget. One guard set, no copies.

## §plugin-package (`manifest.rs`, `integrity.rs`)

`HexonType::Plugin` (`"plugin"`) is the sealed third-party extension package (D-X1; contract:
`docs/extensions/package-format.md`, track `extension_platform_20261107` R1).

- **Members.** `plugin/extension.json` (manifest v2) joins `HEADER_MEMBER_PATHS`, so consent can render
  from the header phase before the body is fetched. `plugin/component.wasm` is an ordinary body member
  committed by `members.json`. `entries.json` is required and exactly `{"entries":[]}`: `HexonArchiveIndex::open`
  (the registry's header and download routes) refuses an archive without it (G0-C1; fe-hexon
  `PLUGIN_ENTRIES_JSON`). `seal` stays the last transform; nothing else about sealing changed.
- **One reader.** fe-hexon reads members with zip 0.6 while `verify_archive` reads with zip 2, so the plugin
  verifier re-runs `verify_sealed_header` + `SealedCoverage::check_member` over the exact bytes it returns
  (G2): a ZIP-parser differential can never hand the host a component the root did not commit.
- **Counter-signatures.** `plugin/countersignatures.json` is the one member the root never commits, so a
  second party can counter-sign without resealing. `seal` copies it without a digest;
  `SealedCoverage::check_member_digest` exempts it from the uncommitted-member rule. It is read under the
  JSON cap. Its content is checked by the plugin verifier (`fe-hexon/src/package.rs` §plugin), not here,
  with `fe_format::verify_manifest` over the canonical signing bytes (never fe-hexon's raw-bytes
  `signature::verify_manifest`, which no conforming counter-signature passes: G3-C4).
- **Identity.** `integrity::content_hash(manifest_json)` = blake3 of the canonical manifest with
  `signature` blanked, i.e. exactly the bytes `sign_manifest` signs. It commits the integrity root, so it
  changes with any committed member and never with a counter-signature.
- **Old parsers** treat `"plugin"` as an unknown type (see the forward-compat rule in
  `docs/hexon-format-spec.md`); the plugin verifier is the only consumer.
## §portal-bundle (`portal_bundle.rs`, portal_surface_20261107)

Pure validator for signed web bundles: `validate_portal_bundle` decides refusal classes from archive facts and the manifest
`PortalCsp` shape; it does no I/O. Hosts (fe-api upload/export, fe-webview protocol) map its refusals to their own
surfaces and never re-implement the checks. Bundles are `.hexon` archives read through `HexonArchiveIndex`, so the seal
and integrity rules in §archive-index and §integrity apply unchanged. See `fe-api/src/AGENTS.md` §portal-bundles and
`fe-webview/src/portal_bundle/AGENTS.md`.

## §entitlement (`entitlement.rs`, feature `entitlement`)

Off by default; `fe-hexon` enables it (`entitlement = ["dep:jsonwebtoken", "dep:x25519-dalek", "dep:chacha20poly1305"]`),
so the bevy-free consumers (hosted registry) never compile the crypto. Pure primitives, no I/O, no network.

- **Token**: EdDSA JWT carrying `EntitlementClaims` (`sub` licensee identity DID, `package_id`, `content_hash`, `iat`, `exp`,
  `jti`, `kid`, `wrapped_content_key`). `KeyringVerifier` verifies offline against an `IssuerKeyring` of pinned `(kid, key)`
  pairs (never a key taken from the token), checks package, build hash and licensee, applies `TOKEN_CLOCK_LEEWAY_SECS`, and
  caps lifetime at `MAX_ENTITLEMENT_TTL_SECS`. The expected `content_hash` is recomputed locally by the caller.
- **Wrap**: the issuer wraps a 32-byte `ContentKey` to the licensee's X25519 device key (ephemeral-static ECDH, BLAKE3
  `derive_key` over shared + both public keys, XChaCha20-Poly1305 with an AAD naming both keys). Wire form is base64 of
  `version(1) | ephemeral public(32) | nonce(24) | sealed key(48)`. `ContentKey` redacts `Debug`, compares in constant time and
  zeroizes on drop.
- **Paid entries**: `seal_paid_entry` / `open_paid_entry` seal one entry under the content key with the entry id in the AAD
  (a sealed entry cannot be swapped to another entry id). Free entries stay plaintext.
- **Token request**: `TokenRequest` + `token_request_proof` / `verify_token_request` implement contract v1 section 4.9; the proof signs
  `fractalengine/entitlement-token-request/v1 \n canonical JSON minus proof` (domain separation from gossip and API tokens),
  `iat` within `TOKEN_REQUEST_IAT_WINDOW_SECS`, body at most `MAX_TOKEN_REQUEST_BYTES`.
- **Errors**: `EntitlementError::wire_code` maps request failures to the contract codes `proof_invalid` / `invalid_request`.
- **`LocalIssuer`** is the reference issuer used by tests and the private service; it mints, it does not decide who is entitled.
- **Open finding (R-STORE)**: the wrap AAD binds the two public keys only, not the package. Bind `package_id` and `content_hash`
  into the AAD / KDF input and `paid_entry_aad`, and have issuers take `encrypted_key` only from their own catalogue record for
  the requested package. Tracked in the extension_platform checkpoint steer; not changed here (file owned by S-ENT).
