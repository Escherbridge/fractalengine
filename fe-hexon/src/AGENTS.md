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
`manifest.hexon_id` under a *different* `publisher_did` — evaluates
`fe_policy::OwnershipPolicy::new(existing.publisher_did)` against the
package's **signed** `publisher_did`, so only the first publisher may
republish that hexon id (any version). The ownership subject is the signed
manifest DID, not the API token: `ApiClaims.sub` is always the node DID, so a
token cannot identify a publisher. Otherwise it calls `install(package,
"global")` and keeps the `AlreadyInstalled` semantics. `uninstall_as` on the
`"global"` (`PUBLISHED_BINDING`) record additionally requires
`OwnershipPolicy` with the manifest's publisher DID; petal unbinding stays
Editor-gated. The API never names `"global"` (its routes require a ULID
`petal_id`), so unpublish is registry-level only today.

**Install keys and petal scope.** Installs are keyed by `(petal_id, hexon_uri)`: the registry map is petal
first, then `{hexon_id}@{version}` (`registry.rs:196`). `install_published` (`registry.rs:346`) copies the
`"global"` publish record into the target petal and reuses the stored blobs, refusing with
`MissingAssetBlob` when one has gone missing. It refuses `"global"` as a target (`registry.rs:351`).
The hexon_id collision rule is `published_by_another` (`registry.rs:585`), checked on publish
(`authz.rs:55`). A per-petal uninstall sends `DbCommand::UninstallCrate { petal_id: Some(..) }`
(`fe-runtime/src/messages.rs:1359`), which the DB thread routes to `uninstall_crate_in_petal`
(`fe-database/src/lib.rs:2379`); `None` keeps the legacy every-petal form.

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

## §registry-source-remote (EXT E16, feature `remote`)

- The protocol-v1 registry client sits behind `registry_source::RegistrySource`. `remote::remote_extension_source(label, base_url, trust)` is the Extensions Browse source: anonymous by construction (no token parameter) and listing `type=plugin` only, re-filtered client-side (R-REG-03, R-REG-10). `remote::remote_source(label, base_url, token, trust)` keeps the operator-token form for the tileset path. Both build a `ProtocolV1Source` over `RemoteProtocolV1Transport` (sync facade on a private current-thread runtime; call from a worker thread only, never inside async code, and drop it there too). The async `RemoteRegistryClient` stays for the partial-fetch path and the Hexon Manager; it has 10 s connect and 60 s read-idle timeouts and a capped download (R-REG-15, R-REG-16).
- **Type dispatch.** `verify_fetched_hexon` returns `FetchedHexon::Plugin` for `HexonType::Plugin` (the full plugin verify) and `FetchedHexon::Content` for anything else (tilesets): id check first, publisher trust root (or `allow_unsigned_dev`), version, then `content_hash`, reading `manifest.json` through fe-format's zip 2 reader, the one `verify_archive` checks (R-REG-18). `ProtocolV1Source::fetch_hexon` is the whole-archive entry point; `fetch` refuses a tileset as `not-a-plugin`. The Hexon Manager's chunked install stays on `FetchStrategy` (R-REG-19, a user question on its trust model).
- Protocol 1 check (store-client contract v1): `config.json` carries `registry_protocol: 1` and is the authority; the `X-Hexon-Registry-Protocol` header is optional and refused only when present and different. The in-repo `fe-hexon-registry` now stamps that header on every response (R-REG-22). An unsigned `index_key_id` never widens trust; the index key is pinned per endpoint and is never a package or revocation signer. A registry advertising neither is refused with `ProtocolMismatch` before any list or download (fail closed); `remote_registry_test.rs` pins that refusal with a silent registry.
- Every download is capped at the listed `size` (or `PACKAGE_MAX_BYTES` when the registry lists none) while it streams, then goes through `verify_fetched`: seal and trust roots, expected package id (so a mirror cannot serve a revoked package under a new id), listed version, listed `content_hash` (BLAKE3 of the manifest with the signature blanked). `entry.url` must be the registry uri `id[@version]` of that same entry (`requested_uri_matches`, R-REG-11), in the store-client contract v1 grammar (id `^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$`, not ending `.json`; version without build metadata). Extension sources send no credentials at all; the tileset path sends the operator's token only to URLs built from the configured base.
- Listings carry each build's `content_hash` (the registry hashes the canonical manifest in `index_one`), so `fetch` compares against a real value; an uncanonicalisable manifest lists an empty hash and is refused `ContentHashMismatch` (R-REG-25).
- **Progress.** `ProtocolV1Transport::download_with_progress` (`registry_source.rs:1869`) defaults to reporting once, at the end. `fetch_hexon_with_progress` (`registry_source.rs:1969`) runs `requested_uri_matches`, `check_protocol` and `refuse_killed` first, then downloads and verifies. The remote readers `get_capped_response_with_progress` and `download_capped_with_progress` (`remote.rs:365`, `:403`) refuse a `Content-Length` above the cap before reading the body. Progress is throttled to `max(64 KiB, total/100)`. The total is the `Content-Length` when present, else the listed size capped at the limit, else 0 (unknown). The final event is `(n, n)`. `verify_fetched_hexon` runs on the complete bytes only. The readers are generic over `F: FnMut + ?Sized`, so the download futures stay `Send`.

## §entitlement (`entitlement_client.rs`, `entitlement_gate.rs`; needs fe-format feature `entitlement`)

fe-hexon's Cargo.toml turns the `fe-format/entitlement` feature on; the primitives live in `fe-format/src/AGENTS.md` §entitlement.

- **Gate** (`EntitlementGate::open`): an unlicensed package never consults the verifier. A licensed one is `Entitled` only when
  the token verifies for this package, the locally recomputed `content_hash` (`archive_content_hash`), the licensee DID and the
  device key; otherwise it is `Preview` and only `free_entries` load. `load_at` opens a paid entry in memory per load and the
  caller must never persist the plaintext. A sealed entry that fails to open is `OpenFailed`, never a plaintext fallback.
  Status labels for the Hexon Manager: `installed`, `installed-preview`, `entitlement-required`, `entitled`.
- **Client** (`EntitlementClient::token_for`): returns the held token while it is valid, renews it before `exp` (margin bounded
  by `RENEW_MARGIN_BOUNDS_SECS`), and asks the endpoint only when its `config.json` advertises `entitlements` (never probes).
  The `TokenTransport` seam keeps HTTP out of the logic; refusals map to `TokenRequestError` by envelope code, then status.
  Every issued token is verified before it is stored, so a forged answer is never kept. Refusals such as `not_entitled` are
  definitive (`is_definitive`); `429` honours `Retry-After` clamped to `MAX_RATE_LIMIT_BACKOFF_SECS`.
- **Store**: `FileTokenStore` keeps one record per (licensee, package); `MemoryTokenStore` is for tests.
  Tokens are bound to one `content_hash`, so an update asks again.
- **Wiring**: the registry `InstallRecord` holds the licence terms and the archive-recomputed `content_hash`
  (`registry.rs:79`, set in `install` at `:231`, bound for publish by `bind_published_archive` at `:323`).
  `gate_for` (`registry.rs:440`) with no recorded hash tries no token and returns preview only. `load_entry`
  (`registry.rs:469`) is the in-process load path; no other crate calls either. Plaintext of a sealed entry
  stays in memory. `bind_published_archive` refuses an archive whose signed manifest is not the recorded
  build before trusting its hash.
- **Credentials**: the token request is the only call that signs anything; catalogue GETs and downloads send no credentials
  (docs/spec/store-client/contract-v1.md rule 1.3).

## §store-client (`registry_source.rs`, `remote.rs`)

Contract v1 text: `docs/spec/store-client/contract-v1.md`. `registry_source` owns config/index/detail/attestation verification,
the pinned index key and the `RegistrySource` seam; `remote` (feature `remote`) is the HTTP transport. The rules are summarised in
§registry-source-remote above; this section records only the crate wiring: `entitlement_client` and `entitlement_gate` are declared
in `lib.rs` and reuse `registry_source::{resolve_location, CatalogConfig, CATALOG_PREFIX, FEATURE_ENTITLEMENTS}`.

- **Publish.** `RemoteRegistryClient::publish` (`remote.rs`, `publish` at about `:128`) POSTs sealed `.hexon` bytes as a raw `application/octet-stream` body to `/api/v1/hexons/publish`, through the same `post` helper that applies the bearer, and the same-origin redirect policy as search and download (`same_origin_redirects`, `remote.rs:32`). The registry re-validates the seal and publisher continuity.

## §auth

Extension sources send no credentials; the tileset path sends the operator's bearer token only to URLs built from the
configured base (`FE_HEXON_REGISTRY_TOKEN`, env-only, never persisted). The token endpoint is the sole signed request.

## §unified-format

fe-hexon is the runtime layer. Every on-disk type is fe-format's: `manifest.rs:5` and `signature.rs:3` re-export it, and there is one canonical-JSON signing scheme (fe-format/src/AGENTS.md §signature).

- **Build.** `HexonPackage::build` (`package.rs:88`) runs `HexonArchive::export`, adds the presentation members, and calls `fe_format::seal` as the LAST transform (`package.rs:132`).
- **Open.** `HexonPackage::open` (`package.rs:138`) runs `import_with_budget` (`:145`) and the seal check. `signature_valid()` (`package.rs:70`) is true only when the seal names the manifest's own publisher and `verify_manifest` passes over the stored manifest bytes (`sealed_and_signed`, `package.rs:197-204`).
- **Install gate.** `check_content_installable` (`handlers/mod.rs:38`) refuses Script, PetalSnapshot and Plugin before any install write (`registry.rs:260`).
- **Memory (NFR-5).** A snapshot or package archive is read fully into memory. This is documented, not engineered. The snapshot byte cap is `MAX_SNAPSHOT_ARCHIVE_BYTES` in `fe-api/src/petal_snapshot.rs:33`.
- **Decision.** fe-format owns the shape and fe-hexon re-exports it (merge-gate default, user question 4).

PetalSnapshot layout: fe-format/src/AGENTS.md §petal-snapshot; DB seam: fe-database/src/handlers/AGENTS.md ## petal-snapshot.

## §petal-snapshot

- `build_snapshot_archive` (`snapshot.rs:131`) hash-verifies every referenced blob before attaching it (`snapshot.rs:154`, `BlobHashMismatch`).
- `restore_snapshot_archive` (`snapshot.rs:259`) re-hashes every blob on import and cross-checks asset rows against the shipped blobs (`snapshot.rs:270-279`). It writes blobs only after both pass. Blobs this restore newly wrote are listed in `newly_stored` and removed again by `discard_new_blobs` (`snapshot.rs:250`) when the import is refused; pre-existing blobs are never listed.

## §lint-scope

`lib.rs` no longer has a crate-wide `#![allow(dead_code)]`. Each of the 14 modules that hold private functions carries its own `#[allow(dead_code)]` (`lib.rs:2-29`). New code must not widen these allows. Remove an allow when its module has no dead code left.
