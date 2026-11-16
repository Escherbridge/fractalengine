---
type: Implementation Plan
title: "Implementation plan: model library"
tags: [model_library_20261115]
timestamp: 2026-11-15T00:00:00Z
resource: ./spec.md
---

# Implementation Plan: Model library

## Overview

Design: [spec.md](./spec.md). Six phases, ordered by dependency: wire and format contracts, DB arms, the pack
command, the desktop library, integration of the central files, then the user-gated test runs, QA and docs.
Each task is sized for one implementation agent and names the files it owns; `metadata.json` `partitions`
mirrors this. Gate per task: `cargo check -p <crate>` only (root `AGENTS.md` §Test execution policy; no clippy).
Tests are authored inside the tasks below and run only when the user names the batch (Phase 6).

Code rules for every task: terse one-line doc comments, rationale in the directory's `AGENTS.md`; no work on
the Bevy main thread beyond reading resources; `warn!` on every failed channel send; no new test function where
a named journey can be extended.

## Phase 1: Contracts, format and registry

Goal: every type and rule the later phases call exists and checks.

Tasks:
- [ ] ML-01 fe-format GLB check. New `fe-format/src/glb.rs` `check(bytes, max_len) -> Result<GlbFacts, GlbError>`
  (magic, version 2, declared length, JSON chunk first and parses, BIN chunk bounds); `lib.rs` `pub mod glb`.
  One table test `glb::tests::check_table` (valid, bad magic, v1, short, length lie, chunk overrun).
  Owns `fe-format/src/glb.rs`, `fe-format/src/lib.rs`, `fe-format/src/AGENTS.md` (§glb).
  Gate `cargo check -p fe-format` and `cargo check -p fe-format --no-default-features`.
- [ ] ML-02 fe-runtime messages and census. `DbCommand::PlaceStoredGlb { request, petal_id, name, content_hash,
  size_bytes, position }` (reply `GltfImported` / `ImportGltfFailed`, same request owner as `ImportGltf`);
  `DbCommand::ReadPetalAssetHashes { petal_id, correlation_id }` and `DbResult::PetalAssetHashes`; reply-family
  entries; census row `PlaceStoredGlb` = `scene.create` beside `ImportGltf` (`scene_write.rs:59`).
  Owns `fe-runtime/src/messages.rs`, `fe-runtime/src/scene_write.rs`, `fe-runtime/src/AGENTS.md`.
  Gate `cargo check -p fe-runtime`. Serial: central file, nothing else edits it in this track.
- [ ] ML-03 fe-hexon thumbnail decoder and remote client additions. New `fe-hexon/src/thumbnail.rs`
  `decode(bytes) -> Result<Rgba, ThumbnailError>` (PNG signature, 256 KiB, IHDR at most 512 x 512 before decode,
  `image::Limits` 1 MiB); table test `thumbnail::tests::decode_table`; `lib.rs` declares `thumbnail`.
  `remote.rs`: `RemoteHexonSummary` gains `license_type`, `spdx_id`, `attribution`, `preview`, `preview_bytes`
  (`serde(default)`); `fetch_member_capped` on the HTTP transport.
  Owns `fe-hexon/src/thumbnail.rs`, `fe-hexon/src/remote.rs`, `fe-hexon/src/lib.rs` (first writer).
  Gate `cargo check -p fe-hexon --features remote`.
- [ ] ML-04 fe-hexon `ModelLibrary`. New `fe-hexon/src/model_library.rs`: `open(dir, blob_store)`, `install(bytes)`
  (`HexonPackage::open`, type must be `model`, licence present and not paid (D-7), attribution rule,
  `handle_model_install`, atomic `{id}@{version}.hexon` write), `list()` (header scan), `remove(uri)`,
  `entry(uri, entry_id)`, `packages_for_hashes(&[hash])`; appends `pub mod model_library` to `lib.rs` after ML-03.
  Owns `fe-hexon/src/model_library.rs`, `fe-hexon/src/AGENTS.md` (§model-library, §thumbnail).
  Depends ML-03. Gate `cargo check -p fe-hexon`.
- [ ] ML-05 fe-hexon-registry index and publish rule. `HexonIndexEntry` gains the five fields of spec §4.3, read
  in `index_one`; publish refuses a `model` package without `license.json` or with an empty attribution under
  `attribution` (400). Owns `fe-hexon-registry/src/index.rs`, `fe-hexon-registry/src/routes.rs`,
  `fe-hexon-registry/AGENTS.md`, `fe-hexon-registry/tests/registry_test.rs` (extend the publish leg).
  Gate `cargo check -p fe-hexon-registry`; `cargo tree -p fe-hexon-registry -e normal | grep -c bevy` stays 0.
- [ ] Verification: review checkpoint [checkpoint marker]. `/code-review` over ML-01..05 against spec §4.3, §5
  and §6 limits; one-line verdict recorded in `metadata.json` `reviews`.

## Phase 2: DB arms

Goal: an installed blob becomes a node through the existing creation core; credits can be read.

Tasks:
- [ ] ML-06 fe-database placement and credits read. Split `import_gltf_handler` (`handlers/crud.rs:307`) at the
  point after blob storage into a shared stage; the `PlaceStoredGlb` arm enters that stage with a verified
  present blob (hash and size checked against the store), so KTX2 admission, local Editor check and the
  `object_creation` transaction are the same code. `ReadPetalAssetHashes`: Viewer-fenced, distinct
  `content_hash` of live nodes' assets in the petal, capped at 1 024. Dispatch arms in `lib.rs`.
  Extend the real-store journey that drives the native import through the actual handler with a
  `PlaceStoredGlb` leg (success, Viewer refusal before any write, missing blob refused) and a credits read.
  Owns `fe-database/src/handlers/crud.rs`, `fe-database/src/lib.rs`, `fe-database/src/handlers/AGENTS.md`
  (placement from a stored blob), and the journey file named by the tripwire in `metadata.json`.
  Depends ML-02. Gate `cargo check -p fe-database`.
- [ ] Verification: review checkpoint [checkpoint marker]. `/code-review` high: authority fences identical to
  native import, no second creation path, budget treatment as `ImportGltf`.

## Phase 3: Pack command

Goal: a publisher can produce a sealed, signed, verified model package (F1, U1, U5).

Tasks:
- [ ] ML-07 `fe-ext model pack <folder> --key <seed file> [--out <path>]`. New `tools/fe-ext/src/model.rs`
  (parse `library.toml`, validation list of spec §5, `HexonBuilder` build, re-verify with `verify_archive`
  `RequireSealed { pin }` and `HexonPackage::open`, atomic write); subcommand in `main.rs`; `index build` copies
  licence fields into the package detail document (`index.rs`). Extend J8 (`tests/j8_cli.rs`) with a model leg:
  pack a two-model fixture folder, `inspect` it, flip one byte and see `inspect` refuse, then one table of
  refusals (unlisted GLB, symlink, bad GLB, oversize, missing licence, empty attribution, oversize thumbnail,
  non-PNG thumbnail); nothing written on refusal. Fixture folder `tools/fe-ext/tests/fixtures/model-pack/`.
  Owns `tools/fe-ext/src/model.rs`, `tools/fe-ext/src/main.rs`, `tools/fe-ext/src/index.rs`,
  `tools/fe-ext/tests/j8_cli.rs`, `tools/fe-ext/tests/fixtures/model-pack/**`, `tools/fe-ext/AGENTS.md`.
  Depends ML-01, ML-03, ML-04. Gate `cargo check -p fe-ext`.
- [ ] Verification: review checkpoint [checkpoint marker]. `/code-review` on path handling and the
  pre-write validation list; confirm a refused pack writes nothing.

## Phase 4: Desktop library

Goal: browse, detail, install, place and credits in the app (F3-F7, U1-U4, U6, U7).

Tasks:
- [ ] ML-08 fe-ui Models section. New `fe-ui/src/model_library/`: `mod.rs` (`ModelLibraryState`, `ModelOp`,
  `PendingModelOps`, `ModelRow` wrapping `RegistryPackageRow` plus licence and preview, result folding),
  `text.rs` (pure `display_text` clip plus control and bidi stripping; table test `text::tests::display_text_table`),
  `panel.rs` (three views in one section: Browse with search and tag filter, Installed with Place per model,
  Credits for the active petal; cards from `panels::widgets`; Place `role_gated`; Remove `confirm_button`;
  thumbnail texture cache 64 entries, 4 fetches in flight, requested only for visible rows), `AGENTS.md`.
  One render journey `panel::tests::model_library_journey` (loading, empty, error with Retry, listing with a
  licence chip, too-large row has no Install, paid row "Not supported", Viewer sees Place disabled with reason).
  Owns `fe-ui/src/model_library/**`. Depends ML-02. Gate `cargo check -p fe-ui`.
  UI rules: `~/.gemini/UI-UX-guidance.md` (pass this path to the agent).
- [ ] ML-09 Main-binary bridge. New `fractalengine/src/model_library_bridge.rs` (`drain_model_ops` on worker
  threads: list via `registry_listing`, detail via `FetchStrategy::fetch_header` `RequireSealed`, thumbnail via
  `fetch_member_capped` then `thumbnail::decode`, install via `fetch_hexon_with_progress` then
  `ModelLibrary::install`, remove, place sends `PlaceStoredGlb` through the scene-write door at the camera focus,
  credits via `ReadPetalAssetHashes` then `packages_for_hashes`), and `fractalengine/src/registry_listing.rs`
  (`registry_rows(hits, hexon_type)` moved from `terrain_bridge.rs:869` with its tests). Table test
  `model_library_bridge::tests::op_outcomes` over the pure outcome mapping only.
  Owns `fractalengine/src/model_library_bridge.rs`, `fractalengine/src/registry_listing.rs`.
  Depends ML-03, ML-04, ML-06, ML-08. Gate `cargo check -p fractalengine`.
- [ ] Verification: security review [checkpoint marker]. `/security-review` (non-negotiable: untrusted image
  decode and publisher strings) over ML-03, ML-08, ML-09: decode caps before allocation, no panic path, verified
  member source only, string sanitising applied at every render site, no clickable publisher text.

## Phase 5: Integration (central wiring, serial)

Goal: the feature is reachable in the running app.

Tasks:
- [ ] ML-10 Central wiring, one agent. `fractalengine/src/main.rs`: construct `ModelLibrary` over the shared blob
  store root and `data/hexons/models`, register the result channel and `drain_model_ops`, declare the two new
  modules. `fe-ui/src/plugin.rs`: `init_resource` for `ModelLibraryState` and `PendingModelOps`; `fe-ui/src/lib.rs`
  `pub mod model_library`. `fe-ui/src/ui_shell/right_sidebar.rs`: a Models section beside Maps (`:1492`).
  `fe-ui/src/verse_manager/db_results/` arm: route `GltfImported` / `ImportGltfFailed` carrying a library request
  to `ModelLibraryState` and select the new node through `NodeManager::select`. `terrain_bridge.rs`: replace the
  local `registry_rows` with `registry_listing::registry_rows(hits, REGISTRY_MAP_TYPE)` (two lines; tell the
  terrain lane first). Owns those files. Depends ML-08, ML-09. Gate `cargo check -p fe-ui -p fractalengine`.
- [ ] ML-11 Test batch registration. In `conductor/test-batches.md`: R11 fe-ui line adds `model_library::`;
  R11 binary line adds `model_library_bridge:: registry_listing::`; the R4 `terrain_bridge::` note says the
  row-filter tests moved to `registry_listing::` (R11); the "Files touched → batches" table lists every new file.
  Owns `conductor/test-batches.md`. Depends ML-10.
- [ ] Verification: review checkpoint [checkpoint marker]. `oh-my-claudecode:critic` on the whole track against
  `conductor/product.md` and the UI guidance; verdict recorded.

## Phase 6: Tests, QA and docs

Goal: evidence and documentation. Nothing here runs without the user naming the batch.

Tasks:
- [ ] ML-12 Flow journey across real collaborators. Extend `fe-hexon/tests/remote_registry_test.rs` (R11-STORE)
  with one model leg against the in-process registry: publish a sealed model fixture with the publish token,
  search `type=model` shows licence and preview fields, fetch the header and the preview member and decode it,
  install through `ModelLibrary`, `list` shows it, `packages_for_hashes` names it; an unsigned and a tampered
  package are refused with nothing stored; a 413 on an over-cap publish. Owns that file.
- [ ] ML-13 Docs. `docs/model-packages.md` (new publisher guide: folder layout, `library.toml`, licences, keys,
  `fe-ext model pack`, how to submit through the operator); `docs/editor-guide.md` (Models section: browse,
  install, place, credits); `docs/hexon-format-spec.md` (model package conventions: `preview_image`, licence
  required); `docs/spec/store-client/contract-v1.md` (additive licence fields); root `AGENTS.md` one-line
  pointer to `fractalengine/src/AGENTS.md` §model-library (written by ML-10).
- [ ] Verification: user-gated runs [checkpoint marker]. On the user's go only:
  `tools\qa\test_batches.ps1 -Batches R11` (covers R11, R11-STORE lines) and the batch that hosts ML-06's journey
  (see `metadata.json` tripwire), then the QA checklist below.

## Test map

| Flow / case | Where | Batch |
|---|---|---|
| F1, U1, U5 (pack) | J8 model leg, `tools/fe-ext/tests/j8_cli.rs` | R11-STORE `cargo test -p fe-ext -- j8` |
| F2, U5 (publish), index fields | `fe-hexon-registry/tests/registry_test.rs` publish leg | R11-STORE `cargo test -p fe-hexon-registry` |
| F3-F5, U1, U2 (client) | `fe-hexon/tests/remote_registry_test.rs` model leg | R11-STORE `cargo test -p fe-hexon --features remote --test remote_registry_test` |
| F6, U7 (DB authority) | ML-06 journey extension | per tripwire |
| Thumbnail decode limits | `thumbnail::tests::decode_table` | R11 `cargo test -p fe-hexon -- --skip entitlement_client::tests:: --skip entitlement_gate::tests::` |
| GLB structure | `glb::tests::check_table` | R11 `cargo test -p fe-format -- --skip portal_bundle::` |
| Panel states, U3, U4, U6, U7 (UI) | `model_library::panel::tests::model_library_journey`, `text::tests::display_text_table` | R11 `cargo test -p fe-ui --lib -- model_library::` |
| Op outcome mapping, row filter | `model_library_bridge::tests::op_outcomes`, moved `registry_listing::` tests | R11 `cargo test -p fractalengine --bin fractalengine -- model_library_bridge:: registry_listing::` |

Declined: tests of `ModelLibraryState` getters, serde round trips of the new summary fields (covered by the
client journey), and a separate unit test for `ModelLibrary` (the client journey drives it through real files).

## QA checklist (in app, after the user-gated runs)

- [ ] Set the registry URL; open Models; rows load, search and a tag narrow them; licence chip and attribution
  line visible on an attribution package.
- [ ] Open a detail: thumbnail appears; a package with a corrupt preview shows the placeholder, no crash.
- [ ] Install: progress, then Installed lists the models; restart the app and they are still listed.
- [ ] Place: node appears at the camera focus in the active petal and is selected; a second peer sees it.
- [ ] Credits lists the package with its attribution; removing the last node removes it from Credits.
- [ ] Viewer role: Place disabled with the reason. Registry stopped: Browse shows the error and Retry;
  Installed, Place and Credits still work.
- [ ] Oversize row shows "Too large to install", no Install button.
- [ ] Screenshots of Browse, detail and Credits added under `docs/images/` with `MANIFEST.md` entries.

## Deferred (triggers)

- Replicated provenance (asset column): when OQ-1 is answered "must travel now".
- Materials, skyboxes, sounds: after the first model content is published (UD-1).
- In-app thumbnail capture and context-menu "Add from library": after the QA pass.
