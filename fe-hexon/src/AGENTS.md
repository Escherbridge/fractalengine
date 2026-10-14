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

## §blob-store (RT-FSBLOB, 2026-10-11; default root corrected OF-1/S1b, 2026-10-14)

`registry::FsBlobStore` shares the runtime store's canonical layout
(`{root}/{hex[0..2]}/{hex}`, fe-runtime/src/AGENTS.md §blob-store) and its
default root (`data/blobs`, CWD-relative, `FE_BLOB_DIR` override — no longer
the per-user `{data_local_dir}/fractalengine/blobs`, kept only as
`legacy_default_path()`, fe-sync/src/AGENTS.md §blob-store), so hexon installs and
the P2P/runtime store are ONE content-addressed store. `store` refuses a
non-64-hex hash or a `blake3(data)` mismatch (`RegistryError::Package`) and
writes atomically; `has` answers `false` for an invalid hash. The old flat
`hexon_blobs/{hash}` directory is not migrated: production never constructs a
`HexonRegistry` (only `TilesetRegistry`), so nothing was ever installed
there. Test fixtures must use `signature::asset_hash(&bytes)`, not fake hashes.

**Install refuses missing or mismatched blobs (PAS-P5 S4, D-76 (3)).**
`HexonRegistry::install` checks every catalogued entry BEFORE storing any
blob: an absent blob is `RegistryError::MissingAssetBlob(hash)` (it used to be
a `warn!` and the install succeeded), a blob that does not hash to its
`asset_hash` is `RegistryError::Package`. Pre-checking keeps a refused install
from leaving half its blobs in the store.

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

**Publish ownership (AP-3.4).** `publish_as` first requires Manager+ via
`RoleLevelPolicy` (`Action::Publish`; the fe-api route additionally demands
Owner), then — when any installed entry already carries the same
`manifest.crate_id` under a *different* `publisher_did` — evaluates
`fe_policy::OwnershipPolicy::new(existing.publisher_did)` against the
package's **signed** `publisher_did`, so only the first publisher may
republish that crate id (any version). The ownership subject is the signed
manifest DID, not the API token: `ApiClaims.sub` is always the node DID, so a
token cannot identify a publisher. Otherwise it calls `install(package,
"global")` and keeps the `AlreadyInstalled` semantics. `uninstall_as` on the
`"global"` (`PUBLISHED_BINDING`) record additionally requires
`OwnershipPolicy` with the manifest's publisher DID; petal unbinding stays
Editor-gated. The API never names `"global"` (its routes require a ULID
`petal_id`), so unpublish is registry-level only today.

## §partial-fetch (PAS-P5 S4 — D-75 C1, D-76 D2)

Fetching a hexon in parts: header first, then only the chunks or members the
caller needs, with every part checked against the one Ed25519-signed
integrity root (fe-format/src/AGENTS.md §integrity, §partial). Plan:
`conductor/tracks/p2p_asset_streaming_20260718/campaign-plan-p5-20261013.md` §3.3.

- **Trait (`partial.rs`).** `PartialHexonFetch` (`async_trait`, dyn-safe, so one
  strategy holds mixed transports as `SharedSource = Arc<dyn PartialHexonFetch>`):
  `source()`, `fetch_header(uri) -> HeaderDoc`, `fetch_chunk(uri, seq)`,
  `fetch_member(uri, path)`, plus `fetch_blob(hash)` with a default `NotFound`
  (the hosted registry has no by-hash route; the P2P transport overrides it).
  Transports return **unverified** bytes. Errors: `NotFound`, `OutOfRange`,
  `Integrity(IntegrityError)`, `Substituted{requested, signed}`, `Transport`,
  `Timeout`; `is_integrity()` is true for the two that prove a source lied.
- **Policy (`p2p/fetch.rs::FetchStrategy`).** The ONLY place bytes are
  checked, so no transport can skip a check:
  - `fetch_header(uri, sources, pin)` runs `verify_fetched_header`:
    `fe_format::verify_header` under `RequireSealed { pin }` (network paths
    never accept unsealed hexons), then **binds** the signed manifest to the
    requested uri (`hexon_id`, and `version` when given → `Substituted`), then
    **re-derives the layout** from the signed `chunks/index.json` and refuses
    a doc whose claimed layout differs (a "Monolithic" claim over a signed
    container would otherwise make seq 0 the trust boundary). These two
    checks close the substitution/downgrade gap in `VerifiedHeader` at the
    strategy layer; S8 also fixes it in fe-format itself.
  - Every follow-up fetch requests the signed `hexon_id@version`
    (`FetchedHeader::hexon_uri`), never the caller's unversioned uri, so a
    second source cannot answer for a newer version.
  - `fetch_chunk`: container → blake3 must equal the signed `BundleEntry`
    digest; a seq the signed index does not list is `OutOfRange` before any
    source is asked. Monolithic → only seq 0 (the whole archive), verified
    off the async thread: its `manifest.json` must be byte-identical to the
    header's signed manifest (no same-publisher swap), then full
    `verify_archive(RequireSealed { pin: header publisher })`.
  - `fetch_member`: a path the signed root does not name (`committed_digest`,
    or an `assets/` path) is `NotFound` with no network call;
    `manifest.json` must equal the header text byte for byte (coverage
    itself never digests the manifest); everything else goes through
    `SealedCoverage::check_member` (the one guard set).
  - `fetch_by_hash(hash)`: blake3 of the bytes must equal `hash` — the generic
    asset primitive (`asset.content_hash` == `AssetEntry.asset_hash` == the
    iroh blob hash) that device payloads and bucket pull-through will call.
  - **Source order:** non-demoted before demoted, then peers by
    `PeerPriority` (`Lan < LowLatency < Remote`), then registries; ties keep
    the caller's order. An integrity failure **demotes** the source for the
    life of the strategy (shared across clones) and the next one is tried;
    other failures just move on. `FetchOutcome { value, served_by, attempts }`
    names every source passed over (`demoted()` filters the integrity ones);
    when all fail, `FetchFailure.error` is the most alarming attempt
    (integrity > timeout > transport > out-of-range > not-found).
  - **Semaphore:** one `max_concurrent_downloads` permit is held for the
    whole call — every source attempt, verification and pacing sleep.
  - **Pacing:** `max_download_rate_bytes_per_sec` (dead config until now) is
    enforced by one shared window: after each verified transfer the caller
    sleeps `pacing_delay(window_bytes, rate, window_elapsed)` — the pure
    function the table test covers. A window idle for longer than its owed
    time + 1 s restarts, so unused allowance never becomes a burst.
  - Per-source request timeout (default 120 s, `with_request_timeout`).
- **Legacy check retired.** `verify_fetched_manifest`/`FetchManifestResult`/
  `FetchError` (the fe-hexon `signature.txt`-era scheme D-76 retires; zero
  production consumers) are gone; `acquire_download_slot` now returns
  `PartialFetchError`.
- **HTTP transport (`remote.rs`, feature `remote`).** `impl PartialHexonFetch
  for RemoteRegistryClient` over the registry's `/meta`, `/chunk/{seq}` and
  `/member/{*path}` (fe-hexon-registry/AGENTS.md §routes). URLs are built
  segment by segment (percent-encoded); a member path with an empty, `.` or
  `..` component is refused client-side. 404 → `NotFound`; every body is read
  under a cap (header 64 MiB, member 64 MiB, chunk 1 GiB) so a hostile
  registry cannot exhaust memory.
- **Conformance suite (`partial/conformance.rs`, feature `conformance`).**
  `conformance::run(fetcher, &ConformanceFixtures::build()?) -> ConformanceReport`
  is a public async fn any transport crate's test calls after making the
  fixtures available (a sealed monolithic model hexon with an asset and a GPX
  member, a sealed tileset, a sealed 3-chunk container — all sealed with the
  deterministic `fixture_key()`). It drives `FetchStrategy`, not the raw
  transport, so a transport passes only if production would accept its bytes:
  header verified with publisher and layout, pin accepted and wrong pin
  refused, every chunk byte-equal, out-of-range, probe members byte-equal,
  unnamed member not found, unknown uri not found, plus two raw-transport
  error-mapping cases. HTTP proof: `tests/partial_fetch_http.rs`
  (`http_source_passes_conformance`, `strategy_falls_back_from_a_tampering_source_to_an_honest_one`);
  P2P proof: S5's `partial_fetch_p2p.rs`. Both tests are gated on
  `remote`+`conformance`, so they run under `--all-features` (plan §10 C10).
- **AC-23(d) (critic tick 5, 2026-10-14):** the P2P partial-fetch adapter
  (`P2pPartialFetch`/`HexonLocator`/`publish_to_transport`, `partial/p2p.rs`)
  has **no production consumer yet** — production P2P downloads are served by
  `fe_sync::TilesetTransfer`, not `FetchStrategy` over this adapter (see
  AC-23 in `.omc/accountability/tick-20261014-0722.md` for the staged-state
  disposition and `PAS-P5-P2P-POLICY` ledger row). The two paths still carry
  the SAME HashSeq root for one seeded tileset:
  `fe-hexon/tests/partial_fetch_p2p.rs::a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling`
  asserts `root == direct.root` between `fe_terrain::tiles::bundle_for_seeding`
  → `fe_sync::TilesetTransfer::publish_files` and this adapter's own
  `publish_to_transport`, so production seeders pass this suite too, even
  though nothing in production calls `fetch_header`/`fetch_chunk` through it
  for a tileset download. `verify_fetched_header` itself (the ONE header
  check, `p2p/fetch.rs`) IS a production consumer independent of this
  adapter — `fe-terrain`'s streamed (not-installed) tile path calls it
  directly from `StreamedTileSource::from_header_doc` (W2b F1; this crate
  already dev-depended on `fe-terrain` for its own P2P flow tests, and
  `fe-terrain` now also depends on THIS crate normally — a supported cargo
  dev-dependency cycle, not a build cycle — see `fe-terrain/src/AGENTS.md`
  §streamed-tilesets).

## §partial-fetch-p2p (PAS-P5 S5 — `partial/p2p.rs`)

The P2P transport behind `PartialHexonFetch`, over the PAS-P3
`fe_runtime::blob_transport::BlobTransport` seam (iroh-free, D-77). It lives
here, not in fe-sync: the trait is in fe-runtime, and fe-hexon depends on no
transport crate (plan §3.5, assumption A1).

- **HashSeq layout.** `publish_to_transport(transport, archive_path)` opens
  the archive with `RequireSealed` and refuses unsealed input. It then stores
  `[HeaderDoc::to_json(), chunk_0..n]` for a container (each chunk read via
  `HexonArchiveIndex::chunk`, so it is verified against the signed index on
  the way out) or `[HeaderDoc, whole archive]` for a monolithic archive
  (`verify_archive_against_header` runs first). The root is tagged
  `fe/hexon/<hexon_id@version>`. Archive I/O runs on `spawn_blocking`.
  It takes a PATH, not bytes as the plan said, because the one header reader
  (`HexonArchiveIndex`) is file-backed and a seeder already holds the file.
- **One root per hexon.** The P3P4 seeding path (`fe_terrain::tiles::
  bundle_for_seeding` → `TilesetTransfer::publish_files`) writes the same
  child 0 bytes and the same chunk bytes, so it publishes the same root
  (test `a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling`).
- **`HexonLocator`** maps a uri to `(root, providers)`. It is filled by the
  caller or from a P3 tileset ad (`insert_advertisement(tileset_id, root_hex,
  provider_addr)`). A lookup of `id@version` falls back to the bare `id`
  entry. `sources(transport, uri, priority)` builds one `P2pPartialFetch`
  per provider. Ads come from the network, so it is bounded (final review):
  `MAX_LOCATOR_ENTRIES` uris (a new uri past it is dropped, known ones still
  update) and `MAX_LOCATOR_PROVIDERS` providers per uri.
- **`P2pPartialFetch { transport, locator, peer, priority }`** resolves a root
  once per fetcher (HashSeq root-only fetch plus child 0, cached by root,
  bounded), so a header fetch never touches a chunk. It still returns
  unverified bytes, since `FetchStrategy` owns verification, but it refuses
  three things as `Integrity`/`Substituted` before handing anything back:
  a blob that does not hash to the child it asked for (a poisoned peer
  store; the in-memory transport returns forged bytes rather than erroring);
  a chunk child the peer's own signed chunk index does not name, checked
  before downloading it; and a root whose signed header names a different
  hexon than requested. Members come from the header (manifest and header
  members), from a chunk (a container's only body members), or from the
  whole archive read in memory under the per-entry caps (monolithic).
  `fetch_blob` overrides the default so `fetch_by_hash` works peer-to-peer.
- **Caps.** Header child 16 MiB and chunk 64 MiB, which equal fe-sync's
  `MAX_INDEX_BYTES`/`MAX_CHUNK_BYTES`, so a P3P4-seeded hexon is fetchable
  both ways. A whole monolithic archive is capped at
  `limits::MAX_ARCHIVE_UNCOMPRESSED_BYTES`, and at most 1 + 65 536 HashSeq
  children are accepted.
- **Tests** (`tests/partial_fetch_p2p.rs`, feature `conformance`):
  `in_memory_p2p_source_passes_conformance` (the A7 proof, the same
  `conformance::run` as HTTP), `poisoned_peer_store_is_refused`,
  `iroh_loopback_fetches_one_chunk_without_the_rest` (real endpoints and
  stacks; chunks 0 and 2 never reach the fetcher's store), and the reseed
  test above. fe-sync and iroh are dev-dependencies only, for the loopback
  and seeding tests.
