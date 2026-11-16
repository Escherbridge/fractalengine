---
type: Track Spec
title: Model library (browse, install and place 3D model packages; pack and sign them)
description: In-app library of registry model packages with thumbnail, licence and attribution, install, placement into the active petal and petal credits, plus a pack command that seals a folder of GLB files into a signed model package.
tags: [feature, model_library_20261115, pending]
timestamp: 2026-11-15T00:00:00Z
resource: ./metadata.json
plan: ./plan.md
---

# Model library

Product context: [product.md](../../product.md), [tech-stack.md](../../tech-stack.md). Plan: [plan.md](./plan.md).
Evidence is cited at HEAD `94b5842a`.

## 1. Decisions already made (user, 2026-11-15; not reopened)

- UD-1 Build the in-app library first; the registry is filled with model content afterwards.
- UD-2 Registry content may be public domain, CC0 or attribution-licensed. The app and the registry listing
  show each package's licence and attribution before any attribution-licensed package is published.
- UD-3 Outside publishers apply through a web form. The operator publishes accepted packages on their behalf,
  signed by the publisher's own key. No approved-publisher list is built.
- UD-4 Terrain content is a separate lane (already running) and out of scope.

## 2. Background (what exists, verified)

| Fact | Evidence |
|---|---|
| The only registry browse/install UI is the Hexon Manager, and it lists maps only | `fractalengine/src/terrain_bridge.rs:863` `REGISTRY_MAP_TYPE = "terrain_tileset"`; filter in `registry_rows` `:869-888`; listing `:891-922`; installs `:689` (file) and `:815` (registry) |
| fe-ui has no registry access; it queues ops for the main binary | `fe-ui/src/terrain_map/mod.rs:143-179` (`HexonOp`, `PendingHexonOps`), drained by `terrain_bridge.rs:660` `drain_hexon_ops` |
| Hexon Manager tabs and registry row DTO | `fe-ui/src/terrain_map/dto.rs:8-12` (Installed / Available / Downloads), `:112-128` (`RegistryPackageRow`, `RegistryListState`); rendered as the right-sidebar Maps section (`fe-ui/src/ui_shell/right_sidebar.rs:1492`) |
| The registry install path verifies seal, id, version and `content_hash` and caps the download | `terrain_bridge.rs:981-1080` (`remote_source`, `fetch_hexon_with_progress`, `FetchedHexon::Content`); cap `fe-hexon/src/registry_source.rs:58` `PACKAGE_MAX_BYTES = 64 MiB` |
| Model install stores GLB and texture blobs content-addressed | `fe-hexon/src/handlers/model.rs:22-70`; dispatcher `fe-hexon/src/handlers/mod.rs:48-75`; `FsBlobStore` refuses a hash mismatch (`fe-hexon/src/AGENTS.md` §blob-store) |
| `AssetEntry.preview_image` exists and nothing fills or renders it | `fe-format/src/entries.rs:61-89` (field `:78`); builder sets `None` (`fe-hexon/src/publisher.rs:220`) |
| Preview members already exist as `preview/{name}` and are sealed like any body member | `fe-hexon/src/package.rs:40-45` (`ICON_PATH`, `PREVIEW_PREFIX`, `README_PATH`), build `:109-119`, open `:160-176`; builder `HexonBuilder::preview` `fe-hexon/src/publisher.rs:251` |
| Licence data model | `fe-format/src/license.rs:6-28` (`LicenseType` Free / Attribution / Paid, `spdx_id`, `attribution`); `HexonBuilder::license` `publisher.rs:177` |
| `license.json` is a signed header member | `fe-format/src/AGENTS.md` §integrity (header map) and §partial (`HeaderDoc` carries header members) |
| Registry search exposes no licence and no preview | `fe-hexon-registry/src/index.rs:13-33` (`HexonIndexEntry`) |
| A registry member route exists and is membership-checked, octet-stream, nosniff, attachment | `fe-hexon-registry/AGENTS.md` §routes `/api/v1/hexons/{uri}/member/{*path}`; client verification `fe-hexon/src/AGENTS.md` §partial-fetch (`FetchStrategy::fetch_member`) |
| Publish requires a sealed archive and enforces publisher continuity per `hexon_id`; body cap 64 MiB | `fe-hexon-registry/AGENTS.md` §Publish validation; `fe-hexon-registry/src/config.rs:84` |
| Per-entry 64 MiB, JSON 8 MiB, archive 1 GiB | `fe-format/src/limits.rs:8-14` |
| Placement core used by REST and MCP | `fe-api/src/upload.rs:534` `place_asset_core` sends `DbCommand::CreateNodeWithAsset` (`fe-runtime/src/messages.rs:748-756`); callers `fe-api/src/mcp/tools.rs:450`, `fe-api/src/rest.rs:413` |
| GUI GLB import goes through the scene-write door and the shared creation core | `fe-ui/src/dialogs/gltf_import.rs:26-59` (`try_submit_scene_write_on`); `fe-database/src/handlers/crud.rs:307` `import_gltf_handler`; asset + node + baseline in one transaction (`fe-database/src/handlers/AGENTS.md` "Ordinary creation and imported assets") |
| Scene-write census names every creating command | `fe-runtime/src/scene_write.rs:57-59` |
| Authoring CLI exists for extensions only | `tools/fe-ext/AGENTS.md` Commands table (`pack`, `sign`, `inspect`, `keygen`, `index build`) |
| `image` (png, jpeg) is already a fe-hexon dependency | `fe-hexon/Cargo.toml:37` |

### 2.1 Corrections to the facts this track was briefed with

- **REST/MCP crate install is not live in the shipped binaries.** Both binaries pass `hexon_registry: None`
  (`fractalengine/src/main.rs:832`, `fractalengine-relay/src/main.rs:522`), so `fe-api/src/hexon.rs:363-365`
  answers "hexon registry not configured". Model packages can be installed today only in tests.
- **`place_asset_core` is `pub(crate)` in fe-api** (`upload.rs:534`) and needs `ApiState` plus a `Door`; the GUI
  cannot call it. The reusable core is the DB side (`object_creation`, reached by `CreateNodeWithAsset` and
  `ImportGltf`).
- **Thumbnail members already exist** (`preview/{name}`, `package.rs:43`); only the catalogue link
  (`preview_image`), the index field and the client are missing.

## 3. User flows and observable outcomes

| ID | Flow | Observable outcome |
|---|---|---|
| F1 | Publisher runs the pack command on a folder of GLB files plus `library.toml`, with their own key | A `.hexon` file that `fe-ext inspect` verifies, `hexon_type = model`, one placeable entry per GLB, a `preview/<entry_id>.png` per entry, `license.json` present; the command prints id, version, content hash, publisher DID |
| F2 | Operator publishes the file for the publisher (UD-3) | `POST /api/v1/hexons/publish` with the operator's publish token returns 201; search lists the package with its licence fields; a later publish of the same id by a different key is a 409 (existing continuity rule) |
| F3 | User opens the Models section and browses | Rows from the configured registry, filtered to `type=model`; text search and a tag filter narrow the list; each row shows name, publisher DID (clipped), size, licence chip (`CC0`, `CC-BY-4.0`, `Public domain`) and, for Attribution, the attribution line |
| F4 | User opens a row's detail | The thumbnail loads (or a neutral placeholder), with licence and attribution read from the package's signed header, and the list of models in it |
| F5 | User installs | Progress shows, then the package appears under Installed with its models; blobs are in the shared store; nothing is placed yet |
| F6 | User places a model from an installed package | A node appears at the camera focus in the active petal, selected, exactly as a local GLB import; peers receive it through the ordinary scene publication |
| F7 | User views the petal's credits | A Credits list in the Models section names every installed package whose models are placed in the active petal, with licence and attribution text |

Unhappy paths that are reachable and behave distinctly (others collapse into the generic "install failed: reason"):

| ID | Case | Behaviour |
|---|---|---|
| U1 | Package over 64 MiB | Pack refuses before writing; registry publish answers 413; client refuses on `Content-Length` before the body (`registry_source.rs:58`); the row shows "Too large to install (64 MiB limit)" and no Install button when the listed size is over the cap |
| U2 | Unsigned or tampered package | Client fetch refuses (seal, id, version, `content_hash` checks); nothing is written to the library or the blob store; the row shows "Failed verification" |
| U3 | Registry unreachable | Browse shows the error inline with Retry; Installed, Place and Credits keep working offline |
| U4 | Entry not placeable (`is_placeable = false`) | No Place button; the bridge refuses a forged op with a warning |
| U5 | Attribution licence with empty attribution | Refused at pack ("attribution is required for an Attribution licence") and at publish (400); a model package with no `license.json` is also refused at both |
| U6 | Paid licence on a model package | Listed with "Not supported in this build", no Install (non-goal N1) |
| U7 | User lacks Editor in the active petal | Place is disabled with the role reason (`panels::widgets::role_gated`); the DB remains the authority |

## 4. Design

### 4.1 Where state lives

- **fe-ui resource `ModelLibraryState`** (new, `fe-ui/src/model_library/`): browse rows, query and tag filter,
  loading and error, the selected row's detail (verified licence, model list), installed packages, thumbnail
  textures (bounded cache), downloads progress, credits for the active petal. It is UI state, like
  `PetalMapState`; it is not a fourth manager. Selection and navigation stay in `NodeManager` and
  `NavigationManager`; placement never writes either directly.
- **Queue `PendingModelOps(Vec<ModelOp>)`** in fe-ui, drained by a new main-binary module
  `fractalengine/src/model_library_bridge.rs`, the same split `HexonOp` / `drain_hexon_ops` uses (root
  `AGENTS.md` §petal-map). A separate queue, not new `HexonOp` arms, because `terrain_bridge.rs` and `HexonOp`
  belong to the running terrain lane (UD-4) and a shared enum would make every slice of both tracks collide.
- **Installed packages: `fe_hexon::model_library::ModelLibrary`** (new), owned by the main binary as a resource.
  It keeps installed archives as files under `data/hexons/models/{id}@{version}.hexon` (atomic write) and
  lists them by scanning headers with `HexonArchiveIndex::open` (lazy, `fe-format/src/AGENTS.md`
  §archive-index), the way `TilesetRegistry` lists maps. Install runs `HexonPackage::open` and
  `handlers::model::handle_model_install`, so blobs land in the shared content-addressed store.

Ops (all executed on worker threads, never the Bevy main thread, results back over one channel):
`List { query, tags }`, `Detail { uri }`, `Thumbnail { uri, member }`, `Install { uri }`, `Remove { uri }`,
`Place { uri, entry_id, petal_id, position }`, `LoadCredits { petal_id }`.

### 4.2 What generalises from the tileset path (no duplication)

| Reused as is | Evidence |
|---|---|
| Registry URL and token resolution | `fe_ui::settings::resolve_hexon_registry_url`, `resolve_hexon_registry_token` (`terrain_bridge.rs:826-848`) |
| Install fetch and verification | `remote_source` + `fetch_hexon_with_progress` with the same `SourceTrust` shape (`terrain_bridge.rs:999-1053`) |
| Search client | `fe_hexon::remote::search_blocking(url, token, query, Some("model"))` (`remote.rs:548`, called at `terrain_bridge.rs:914`) |
| Row clipping and caps | `MAX_REGISTRY_ROWS`, `MAX_REGISTRY_TEXT_CHARS`, `MAX_REGISTRY_ERROR_CHARS` (`dto.rs:104-108`) |
| Row DTO | `RegistryPackageRow` (`dto.rs:112`), wrapped by a model-side `ModelRow` that adds licence and preview, so `dto.rs` is unchanged |
| Model blob install | `handlers::model::handle_model_install` (`model.rs:22`) |
| Card widgets, role gating, confirm, outward link rule | `panels::widgets::{clipped_value, role_gated, confirm_button}`; `hexon_manager.rs:305` `web_url` |
| Placement core | `import_gltf_handler`'s post-storage stage and `object_creation` (§4.5) |

Generalised in place: `registry_rows` (`terrain_bridge.rs:869`) becomes `registry_rows(hits, hexon_type)` in a
new main-binary module `fractalengine/src/registry_listing.rs` that both bridges call (fe-ui cannot see
`RemoteHexonSummary`). The `terrain_bridge.rs` edit is two lines (import and call) and is made by the
integration slice, coordinated with the terrain lane.

### 4.3 Registry index and search additions

`HexonIndexEntry` (`fe-hexon-registry/src/index.rs:13`) gains, read in `index_one` the same cheap way as
`sealed` and `layout`:

- `license_type: Option<String>` (`free` / `attribution` / `paid`), `spdx_id: Option<String>`,
  `attribution: Option<String>` (from `license.json`, clipped to 512 chars at index time);
- `preview: Option<String>`: the package card image member path, the first placeable entry's `preview_image`
  (falls back to `icon.png` when present);
- `preview_bytes: Option<u64>`: that member's uncompressed size from the central directory.

`RemoteHexonSummary` (`fe-hexon/src/remote.rs:69`) gains the same fields with `#[serde(default)]`, so older
registries still parse. The static catalogue written by `fe-ext index build` carries the same licence fields in
each package detail document (additive; `docs/spec/store-client/contract-v1.md` updated).

Publish validation gains one rule for `hexon_type = model`: `license.json` present; `license_type = attribution`
requires a non-empty `attribution` (400 otherwise). Other types are unchanged (terrain is UD-4's lane).

### 4.4 Trust of listing data versus signed data

Search rows are the registry's claim. The detail view and every install use signed data: `Detail` fetches the
header document (`FetchStrategy::fetch_header`, `RequireSealed`) and reads `license.json` and `entries.json`
from it. If the signed licence differs from the row's, the detail shows the signed one and the row is marked
"Listing out of date". Install re-reads `license.json` from the verified archive and stores what it says.

### 4.5 Placement: one path into the existing creation core

`ModelOp::Place` resolves the entry from the installed package (refuses non-placeable, missing blob) and sends
**one new DB command, `PlaceStoredGlb { request, petal_id, name, content_hash, size_bytes, position }`**, through
the GUI scene-write door (`try_submit_scene_write_on`). The DB arm runs `import_gltf_handler`'s stage after blob
storage: KTX2 admission, local Editor check, then `object_creation` writes asset metadata (local uploader as
`owner_did`, active petal as anchor), node, intention and baseline in one transaction, replying `GltfImported`.

Why this and not `CreateAsset` then `CreateNodeWithAsset`: one atomic write instead of two correlated round trips;
the asset carries local upload proof, so it publishes to peers as the node's dependency
(`scene_publication/AGENTS.md` §Dependencies) exactly like a native import; the native-import request owner gives
the panel the same success/failure correlation the import dialog has (`dialogs/AGENTS.md` "Native glTF import
ownership"). It is not charged to the node write budget, same as `ImportGltf` (`handlers/AGENTS.md`
§write-budget). The census row is added at `fe-runtime/src/scene_write.rs:59`.

### 4.6 Credits (attributions a petal carries)

v1 is a local join with no wire change: `LoadCredits { petal_id }` asks the DB for the distinct `content_hash`
values of live nodes' assets in the petal (new read-only `DbCommand::ReadPetalAssetHashes`, Viewer-fenced), and
the bridge maps each hash to installed packages through `ModelLibrary::packages_for_hashes`. Placed models whose
package is not installed on this device show as "Source not installed here".

### 4.7 Recommendations needing a decision

| ID | Decision | Recommendation | Reversal cost |
|---|---|---|---|
| D-1 | Where the pack command lives | Subcommand `fe-ext model pack` in `tools/fe-ext` (one authoring CLI: reuses `keygen`, key loading, `inspect`, `index build`) | Low: a module move to a new binary |
| D-2 | Thumbnails generated or supplied | Supplied, required: one PNG per model, named in `library.toml`. Generating needs a GPU renderer in the CLI; the screenshot pipeline lives in the GUI binary. In-app capture is a follow-up | Low: generation can be added later as an optional fill |
| D-3 | Install scope | Library-wide on this device (no petal binding, no role needed, local files only); placement is the role-gated step | Medium: per-petal install would need `HexonRegistry` + crate rows, which the shipped binary does not construct (§2.1) |
| D-4 | Placement path | New `PlaceStoredGlb` command into the existing creation core (§4.5) | Medium: one enum variant, one census row, one DB arm |
| D-5 | Credits | Local join (§4.6); replicated provenance (an asset column) deferred | Low now; a replicated column later is a wire change all peers take together |
| D-6 | Licence required for model publish | Yes, at pack and at publish (§4.3) | Low: one validation rule |
| D-7 | Install of a model package with no licence from another registry | Refused ("licence missing") | Low |

## 5. Pack command (`fe-ext model pack`)

Input layout:

```
my-pack/
  library.toml
  chair.glb        chair.png
  table.glb        table.png
```

`library.toml`: `id`, `version`, `name`, optional `description`, `tags`; `[license]` `type` (free / attribution),
`spdx` (optional, for example `CC0-1.0`, `CC-BY-4.0`), `attribution` (required for attribution); `[[model]]`
`file`, `thumbnail`, `label`, optional `tags`, `placeable` (default true), `auto_scale` (default false).

Validation (all before anything is written; one error list, not first-failure):

- Only top-level regular files; symlinks, subdirectories, `..`, absolute paths and non-UTF-8 names are refused.
  Every `.glb` in the folder must be listed and every listed file must exist.
- `entry_id` derives from the file stem and must pass `is_safe_path_id`; duplicates are refused.
- GLB structure via a new pure `fe_format::glb::check` (magic `glTF`, version 2, declared length equals file
  length, JSON chunk first and parses, BIN chunk within bounds). Same rule fe-api's header check applies
  (`fe-api/src/upload.rs:202`), extended to the chunk table.
- Size: each GLB at most 64 MiB (`MAX_ENTRY_UNCOMPRESSED_BYTES`), the packed archive at most 64 MiB
  (`PACKAGE_MAX_BYTES`), checked on the built bytes.
- Licence: present; `paid` refused; `attribution` requires non-empty attribution text (U5).
- Thumbnails: PNG only (signature check), at most 256 KiB, at most 512 x 512 (read from IHDR before decode),
  decodes cleanly through the same `fe_hexon::thumbnail::decode` the app uses.

Output: `HexonBuilder` with `hexon_type = model`, one `AssetEntry` per model (`is_placeable`, `label`, `tags`,
`preview_image = "preview/<entry_id>.png"`), previews via `HexonBuilder::preview`, licence via
`HexonBuilder::license`; `build(&signing_key)` seals last. The command then re-verifies the bytes with
`verify_archive(RequireSealed { pin: <publisher DID> })` and `HexonPackage::open`, and only then writes
`<id>-<version>.hexon` atomically. Key: the 64-hex seed file `fe-ext keygen` writes (`--key`).

## 6. Security and abuse review points

- **Publisher strings** (name, description, tags, attribution, label, publisher DID): length-clipped at index
  and again at the client (`MAX_REGISTRY_TEXT_CHARS`, attribution 512); control and bidirectional-override
  characters stripped; rendered as plain egui labels, never markdown and never a link. No URL in attribution
  text is made clickable.
- **Thumbnail decoding** (`fe_hexon::thumbnail::decode`, the single decoder): PNG allow-list by signature;
  compressed cap 256 KiB enforced by `preview_bytes` before fetch and by the read cap during fetch; IHDR
  dimensions at most 512 x 512 before decode; `image::Limits` with a max allocation of 1 MiB; decode on a worker
  thread; failure yields the placeholder, never a panic (the binary is `panic=abort`). Bytes come only from
  `FetchStrategy::fetch_member`, so every thumbnail is checked against the signed root.
- **Fetch caps**: the member read needs a caller cap (256 KiB) instead of the transport's 64 MiB member cap
  (`fe-hexon/src/remote.rs:18`); a small `fetch_member_capped` is added.
- **Paths**: pack refuses everything in §5; install writes only `{id}@{version}.hexon` with id and version
  already validated by `valid_package_id` / `valid_version`; blobs are content-addressed. No package-supplied
  name ever becomes a filesystem path.
- **Limits**: 64 MiB package, 64 MiB entry, 1 GiB archive budget, 256 KiB thumbnail, 200 rows per listing,
  thumbnail texture cache bounded (64 entries, least recently used evicted), at most 4 thumbnail fetches in flight.
- **Registry**: member and download routes already answer octet-stream, nosniff, attachment
  (`fe-hexon-registry/AGENTS.md` §Security headers); no change.

## 7. Non-goals

- N1 Paid content and entitlements for models (the entitlement gate stays on tilesets).
- N2 Publisher accounts, the web application form (outside this repository), an approved-publisher list (UD-3).
- N3 Materials, skyboxes and sounds in the library (follow-ups; their install handlers exist,
  `handlers/mod.rs:56-62`).
- N4 Packages over 64 MiB.
- N5 Terrain content (UD-4); the Maps section and its registry group are unchanged except the shared row filter.
- N6 Replicated provenance on assets (D-5), a context-menu "Add from library" verb, and in-app thumbnail capture.

## 8. Non-functional requirements

- NFR-1 Performance: no network, disk or decode work on the Bevy main thread; per-frame work in the panel is
  reading resource state only (root `AGENTS.md` anti-pattern 7).
- NFR-2 Security: §6; the phase that decodes thumbnails and renders publisher strings gets a security review
  (plan Phase 4).
- NFR-3 UI: one purpose per section (browse, installed, credits), reuse existing card widgets, no decoration
  (`~/.gemini/UI-UX-guidance.md`); stable accessible names on every control.

## 9. Open questions

- OQ-1 Is "Source not installed here" acceptable for peers in v1, or must attribution travel with the scene now
  (D-5)? Default: acceptable in v1.
- OQ-2 The static catalogue tree has no `/search`; is browse against the registry service enough for v1?
  Default: yes.
