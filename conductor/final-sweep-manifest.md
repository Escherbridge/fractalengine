---
type: manifest
title: Campaign final-sweep manifest
created: 2026-10-13
---

# Campaign final-sweep manifest

Durable list of verification steps that the campaign's **final sweep** (RUNBOOK §7 step 6) must execute beyond `cargo test --workspace`.

This manifest captures everything that cannot be deferred, is not automated in CI, or requires live environment verification. It survives between sessions and the W1 closure rolls up into it.

## Core suite (mandatory, ~2 hours)

All of these must pass green before pushing the campaign to origin.

| Step | Command / Procedure | Evidence |
|---|---|---|
| **Unit + integration tests** | `cargo test --workspace` (3452 baseline) | Test summary with count + zero failures |
| **Harness scenarios** | `cargo run -p fractalengine-test-harness` (real loopback iroh, all 17 scenarios ×2 — W1 added scenario 16 `two_peer_tileset_stream`, `fe-test-harness/src/main.rs:109`; CDL-SPEC10 A6 added scenario 17 `two_peer_device_payload_sync`, `fe-test-harness/src/main.rs:113`) | Terminal output: `17/17 ×2 scenarios pass` |
| **Feature-matrix gate (AC-19)** | `cargo test -p fe-terrain --features fetch,render`; `cargo test -p fe-hexon --all-features`; `cargo test -p fe-format --no-default-features` | Each command's own test summary pasted in; the A7 conformance tests `http_source_passes_conformance` and `in_memory_p2p_source_passes_conformance` must appear **by name** in the `--all-features` output (a vacuous "0 tests matched" is a FAIL, not a skip — ties to W0 review's "zero-match test filter passed vacuously" precedent) |
| **fe-sync store migration** | `FE_REQUIRE_STORE_FIXTURE=1 cargo test -p fe-sync --lib p2p_store_migration` | Test pass; `0.35 → 1.x store upgrade` confirmed |
| **fe-database ignored tests** | `cargo test -p fe-database -- --ignored` (15 deferred tests; judge each) | Judgment table: PASS / SKIP reason / FIX needed for each test |
| **BI egress e2e** | `scripts/bi-egress-verify.ps1` | Output pasted into `conductor/tracks/analytics_egress_20260714/evidence.md` §X with DuckDB rowcount + schema |
| **Hexon install e2e** | `scripts/hexon-install-verify.ps1` | Output pasted into `conductor/tracks/oss_release_20260717/evidence.md` §X |
| **Hexon serve e2e** | `scripts/hexon-serve-verify.ps1` | Output pasted into `conductor/tracks/oss_release_20260717/evidence.md` §X |
| **Relay health** | `scripts/relay-health-verify.ps1` (headless fe-relay 1.x) | Output pasted into `conductor/tracks/p2p_asset_streaming_20260718/evidence.md` §6 |
| **Packaged binary smoke** | Build → extract → launch from different CWD with `FE_SYNC_RELAY=disabled` (AC-9 procedure) | Window opens, data dir beside exe, no panic.log, exact PID tree killed |
| **Relay binary smoke** | Build fe-relay → temp CWD → launch with temp FE_P2P_DIR → `FE_SYNC_RELAY=disabled` (AC-15 procedure) | API server listening, zero ERROR lines, exact PID tree killed |

## W0 / W0b / W0c / W0d must-cover (verify from journals)

Extracted 2026-10-13 from the reviewers' `test_sweep_must_cover` / `test_gate_commands` in the
session journals (`~/.claude/projects/…/a871e6ff-…/subagents/workflows/<wf>/journal.jsonl`). All of
these already PASSED once in the pre-push scoped gates (logs `.omc/research/push-gate-20261013/`,
`.omc/research/w0d-*.log`); the final sweep re-runs them on the campaign tip.

**W0 (`119dbd0`, `bf28971`) — `wf_aa8dded9-e92`**
- `FE_REQUIRE_STORE_FIXTURE=1 cargo test -p fe-sync --lib p2p_store_migration` — output must show `legacy_v035_fixture_migrates_and_reads_back ... ok`, not a skip
- `cargo test -p fe-database asset_backfill` (both backfill tests) and `legacy_imported_dir_is_where_old_imports_were_written`
- the full workspace sweep (crud.rs `load_hierarchy_handler` resolver changed)

**W0b (`6a53a2b`) — `wf_e727510a-f59`** (reviewer listed none): `cargo test -p fe-runtime -p fractalengine`
(packaged_root / file_asset_root); packaged GUI smoke (row above).

**W0c (`da3e6a1`) — `wf_34174034-e00`**
- `cargo test -p fe-runtime --lib ktx2_guard` and `--lib bevy_blob_reader`
- `cargo test -p fe-api --lib upload`, `--test asset_ownership_test`, `--test mcp_integration`
- `cargo test -p fe-database --lib handlers::crud`

**W0d (`558197c`) — `wf_e724d2d0-2de`**
- `cargo test -p fe-database --test shared_handle_test` (2 tests; mutation-checked against a current-thread runtime)
- `cargo test -p fe-database -p fe-api -p fractalengine -p fractalengine-relay`
- relay binary smoke (row above, AC-15) — relay `main()` has never been launched

## W1 PAS-P3P4 (append at W1 close)

*Appended by the orchestrator at W1 close: must-cover lists from slices S1..S8 (sequential), the S8b
security review-and-fix and the final review-and-fix; S9 is docs only.*

Generated 2026-10-14 from the W1 journal (`wf_d8699636-cf0`). Base `53a4c4c` → tip `069f8ea`.

**S1 (`f25f906`) — tests written (9):**
- fe-terrain/tests/tileset_bundles_test.rs::a_bundled_tileset_reassembles_to_identical_tiles
- fe-terrain/tests/tileset_bundles_test.rs::bundling_twice_yields_identical_hashes
- fe-terrain/tests/tileset_bundles_test.rs::elevation_and_satellite_share_a_bundle_and_bundles_are_hilbert_runs
- fe-terrain/tests/tileset_bundles_test.rs::assembly_rejects_a_bundle_whose_hash_disagrees_with_the_index
- fe-terrain/tests/tileset_bundles_test.rs::a_tiny_tileset_ships_as_one_bundle
- fe-terrain/tests/tileset_bundles_test.rs::install_refuses_path_escaping_hexon_ids
- fe-terrain/tests/tileset_bundles_test.rs::bundle_for_tile_table
- fe-format/src/manifest.rs::bundle_index_tests::bundle_for_tile_table
- fe-format/src/manifest.rs::bundle_index_tests::chunk_index_blake3_defaults_absent_for_legacy_json

**S2 (`84cd342`) — tests written (9):**
- fe-sync/tests/tileset_transfer_test.rs::a_published_tileset_downloads_verified_and_complete (:78): two services over one InMemoryNetwork. Every file's blake3 equals its HashSeq child and its bytes equal the source. The index passes through opaque. The report's stored list is [0..4], and the Complete event equals the report.
- fe-sync/tests/tileset_transfer_test.rs::a_tampered_bundle_is_rejected_and_never_persisted (:127): tamper(child2) gives failed == [(2, HashMismatch{expected, actual})]. No 2.bundle and no temp file is left behind, and the download is not complete. After repair, the retry stores exactly [2] and completes.
- fe-sync/tests/tileset_transfer_test.rs::an_interrupted_download_resumes_after_restart_without_refetching (:181): seqs 0-1 are fetched, then the service is dropped. Alice withholds the root, the index and bundles 0-1, and the restarted bob has an empty blob store. pending_downloads reports (2, 4, index_present), resume_all stores only [2, 3] and completes, and a second resume_all is a no-op. This is outcome-based with no call counts.
- fe-sync/tests/tileset_transfer_test.rs::cancel_discards_partial_state (:238): after cancel the dir and the download tag are gone, nothing is pending, fetch_chunk returns UnknownDownload, and a fresh start_download stores [0, 1, 2].
- fe-sync/tests/tileset_transfer_test.rs::a_tileset_id_that_escapes_the_download_dir_is_refused (:283): the ids ../x, a/b, '', .., a\b and C:x each get UnsafeTilesetId from request_meta, fetch_chunk, download, cancel and publish. The sandbox stays completely empty and no tag is written.
- fe-sync/tests/tileset_transfer_test.rs::iroh_transport_round_trips_a_hash_seq_on_loopback (:340): two real DocsStacks with RelayConfig::Disabled. fetch_hash_seq over loopback returns the exact children, one child fetch matches the source bytes, and a URL provider is refused. A full TilesetTransfer download over iroh then completes byte-equal. The test skips if UDP bind fails, as other tests here already do.
- fe-sync/tests/tileset_transfer_test.rs::catalog_survives_a_stack_restart (:405): publish on a DocsStack, shut it down and respawn it on the same dir. A new service starts empty, and catalog_from_tags returns the identical CatalogEntry (same root, total and bytes).
- fe-sync/src/tileset_transfer.rs::tests::tileset_id_guard_table: a pure table of accepted and refused ids, including the Windows device names and the length cap.
- fe-runtime/src/blob_transport.rs::tests: hash_seq_codec_round_trips_and_rejects_ragged_blobs; a_peer_fetches_what_another_published_and_hooks_change_what_is_served (checks that unverified bytes never enter the store); tags_list_by_prefix_and_delete.

**S3 (`e5d8928`) — tests written (6):**
- fe-sync/src/sync_thread.rs::tests::tileset_ad_codec_and_authentication_table — table-driven: a ComputeEnvelope-shaped frame and garbage both fail decode_tileset_ad; a well-formed ad round-trips; tileset_ad_is_authenticated is true only for (direct=true, claimed provider == authenticated sender), false for a forged sender, a relayed delivery, and a malformed provider address.
- fe-sync/src/sync_thread.rs::tests::offline_thread_answers_tileset_commands_honestly — a virtual-transport sync thread (no TilesetTransfer) answers DownloadTileset with an honest offline TilesetMetaReceived (empty root_hash/chunk_hashes) and RequestChunk with an honest offline ChunkFailed, never hanging or silently dropping.
- fe-sync/src/sync_thread.rs::tests::provider_map_sorts_authenticated_first_and_upgrades_in_place — TilesetProviderMap::record/candidates/root_for/bytes_for: authenticated providers sort first, an authenticated entry never gets demoted by a later unauthenticated re-ad, unknown ids return honest empties.
- fe-sync/src/sync_thread.rs::tests::resolve_providers_prefers_explicit_override_and_dedupes — explicit provider override is tried first and de-duplicated against known candidates; resolve_providers_and_root returns a named Err when no root is known.
- fe-sync/src/sync_thread.rs::tests::handle_advertise_tilesets_no_host_is_noop — updated for the new signature (no gossip host -> no-op, never panics).
- fe-sync/src/messages.rs — updated/added Debug+Clone/field-shape tests for RequestChunk, ChunkReceived, TilesetMetaReceived, plus new PublishTileset/DownloadTileset/TilesetDownloadComplete/TilesetPublished/TilesetDownloadResumed.

**S4 (`3d89b60`) — tests written (4):**
- fe-ui/src/terrain_map/events.rs :: tests::closing_the_hexon_manager_does_not_stall_a_download — regression test: with ActiveDialog::None the whole session, TilesetMetaReceived/ChunkReceived/TilesetDownloadComplete still drive TilesetDownloads to completion and queue exactly one HexonOp::AssembleDownload
- fractalengine/src/terrain_bridge.rs :: tests::a_completed_download_is_assembled_and_installed — builds a real S1 bundle set (fe_terrain::tiles::bundle_tileset) on disk, runs the private assemble_and_install against a fresh HexonStore/TilesetRegistry, asserts the tileset is listed and a tile reads back byte-identical
- fractalengine-relay/src/main.rs :: tests::a_headless_relay_publishes_its_seeding_tilesets_at_startup — one seeding + one non-seeding installed tileset; publish_seeding_tilesets drains to exactly one PublishTileset (the seeding id) with its bundle files present on disk
- fe-test-harness/src/scenarios/two_peer_tileset_stream.rs :: run (scenario 16) — real two-peer loopback transport: alice bundles+publishes, bob joins the gossip topic, receives the real advertisement, downloads, assembles, verifies every tile byte-equal; both peers then shut down and bob respawns on the same p2p dir with alice offline, asserting the automatic TilesetDownloadResumed{received==total} from the persisted ledger alone

**S5 (`9725dd2`) — tests written (10):**
- fe-terrain/tests/lazy_tiles_test.rs::a_lazy_source_serves_exactly_what_the_eager_import_did — runs over every elevation and satellite member of an archive written to disk. Each member starts cold (probe returns Miss and resident_bytes is 0 after open). After load_layer_blocking its bytes equal those from HexonArchive::import, and it is then resident. Baked splats load lazily and decode identically. A coordinate outside the archive is Absent and needs no read.
- fe-terrain/tests/lazy_tiles_test.rs::a_reinstalled_archive_is_reindexed_and_serves_the_new_bytes — unhappy path that can actually happen: HexonStore::install_tileset rewrites the same id in place while an old source is still in use. A cold read must re-index and return the new archive's bytes, never old offsets applied to the new file. Stale cached bytes must be dropped.
- fe-terrain/tests/lazy_tiles_test.rs::bevy_flows::registry_and_active_petal_share_one_source (render) — real assignment system with SharedTilesetRegistry. Checks Arc::ptr_eq between registry.source(id) and composite.hexon_sources()[0], both when the registry indexed the tileset at startup and on the miss path (a cold registry registers the source it opens).
- fe-terrain/tests/lazy_tiles_test.rs::bevy_flows::a_cold_tile_is_pending_then_spawns_never_failed (render) — headless app with the chain apply_terrain_assignments → update_terrain_lod → fetch_and_spawn_terrain_chunks over a real 5×5 Terrain-RGB archive. Frame one has no chunk and an empty FailedTiles. Within N frames the exact 3×3 ring spawns. origin_ele is grounded from the center tile (122 m) in both the config and the projection.
- fe-terrain/tests/lazy_tiles_test.rs::bevy_flows::tile_bytes_leave_with_the_exit_ring_and_return_on_reentry (render) — one pin per live chunk. Flying about 40 tiles away despawns the ring, after which pinned_len and resident_bytes are both 0. Flying back respawns the same ring with identical TerrainHeightField samples and identical resident bytes.
- fe-terrain/src/tiles/composite.rs::only_the_off_thread_loader_reads_the_disk_cache_and_never_in_offline_mode — table over Offline, Hybrid and Online. Pins NFR-4 (Offline never reads the disk cache) and NFR-2 (get_tile_sync never reads disk), and checks which modes resolve without I/O.
- fe-terrain/src/tiles/residency.rs — two scripted table tests of the pure ByteLru logic: eviction takes the least-recent unpinned entry and never a pinned one; release only drops bytes when the last pin goes; release_all_pins and clear_unpinned bookkeeping.
- fe-format/src/archive_index.rs — header_phase_then_member_reads_match_the_full_import (index reads equal the eager import, an absent member returns None) and a_replaced_file_is_reported_as_changed_not_misread (typed ArchiveChanged error).
- fe-terrain/src/tiles/hexon_source.rs::parse_tile_key_table — pure parser table: valid keys, too few or too many parts, non-numeric parts, u8 zoom overflow.
- fe-api/tests/hexon_tileset_test.rs::startup_indexed_tiles_stream_lazily_and_a_reinstall_serves_new_bytes — a tileset installed before startup is only indexed by load_all (0 resident bytes). The real authed REST routes serve the exact elevation and satellite bytes, an absent member returns 404, and a REST reinstall of the same id serves the new, differently-sized bytes. The existing round-trip test still covers 'the tile route returns the installed bytes'.

**S6 (`b9a85f3`) — tests written (8):**
- fe-database/src/handlers/petal_render_distance.rs: set_then_load_hierarchy_carries_the_value, clearing_restores_inherit, a_viewer_cannot_set_it, out_of_range_values_are_refused (table: NaN/-1/50001), an_editor_authored_replicated_petal_row_applies_and_a_viewer_authored_one_is_denied, an_inbound_out_of_range_value_is_denied, petal_settings_writes_land_in_the_op_log_before_the_row.
- fe-database/src/handlers/petal_terrain.rs: set_petal_terrain_replicates_and_a_peer_converges_on_it (real repl_tx capture + ApplyReplicatedRow on a second DB), an_inbound_petal_terrain_that_fails_the_shape_check_is_denied (table: not-an-object, non-string URI, ../ URI, oversize).
- fe-database/tests/petal_render_distance_test.rs (new, drives the real spawn_db_thread_with_sync_and_lifecycle dispatch loop): set_then_load_hierarchy_carries_the_value, clearing_restores_inherit, a_viewer_cannot_set_it, out_of_range_values_are_refused.
- fe-api/tests/petal_render_distance_api_test.rs (new, full router + real reply builders against Mem SurrealDB): rest_put_persists_render_distance_that_get_reads_back, rest_delete_clears_to_inherit, mcp_set_petal_render_distance_sets_then_clears, viewer_denied_and_foreign_petal_answer_uniformly_and_write_nothing, an_unparseable_value_is_refused_before_any_db_round_trip.
- fe-ui/src/settings.rs: effective_render_distance_prefers_session_over_petal_over_global (table-driven, replaces the old 2-arg test).
- fe-ui/src/verse_manager/spawn.rs: session_override_beats_petal_column_beats_global_and_clears_on_nav_change (replaces the old manifest-save-based test since manifest_save no longer writes the session override).
- fe-ui/src/actions/hexon.rs: manifest_save_persists_to_the_petal_column (asserts the exact DbCommand sent on the channel, the named flow test from the campaign plan).
- fe-database/src/handlers/crud.rs: importing_a_ktx2_model_is_refused_and_stores_nothing (AC-13 accountability directive: real GLB bytes with an image/ktx2 mimeType, through import_gltf_handler, asserting zero blobs/asset rows/node rows on refusal).

**S7 (`0692e14`) — tests written (5):**
- fe-terrain/src/tiles/streamed.rs::tests::bundle_for_and_covers_resolve_through_the_index — pure, no render feature needed
- fe-terrain/src/tiles/streamed.rs::tests::pin_and_release_evict_a_bundle_only_once_its_last_pin_drops — pure refcount test
- fe-terrain/src/tiles/streamed.rs::tests::a_petal_on_an_uninstalled_tileset_demands_only_ring_bundles (render-gated; plan's named S7 flow test) — demand set is a strict subset of the index's bundles
- fe-terrain/src/tiles/streamed.rs::tests::moving_the_camera_demands_new_bundles_and_releases_old_ones (render-gated; plan's named S7 flow test) — demands the new bundle and evicts the old one from memory on release
- fractalengine/src/terrain_bridge.rs::tests::answered_bundles_become_terrain_chunks (plan's named S7 flow test, adapted) — real crossbeam channel + background-thread decode of a real S1 bundle file; proves the decoded bytes become servable through the composite's resident (zero-I/O) tile path, the same path spawn_chunk consumes

**S8 (`a8a50c1`) — tests written (7):**
- fe-ui residency_ledger.rs (pure, table-driven): admission_is_ranked_nearest_first, forward_weighting_pulls_in_what_is_ahead_only, enter_exit_band_keeps_only_previously_admitted, cap_saturation_keeps_the_nearest_and_inside_r_first, invalid_inputs_admit_nothing, no_camera_admits_by_cap_in_tree_order, budget_or_halted_pressure_freezes_admission_but_keeps_the_ring, pressure_scale_table, polyline_distance_projects_onto_segments
- fe-ui residency_ledger.rs (Bevy App over the real ledger → respawn → apply_node_residency → materialize_cached_primitives → materialize_path_assets chain, MinimalPlugins + AssetPlugin, orbit camera moved per step): moving_camera_streams_nodes_in_and_out_with_hysteresis (200 nodes on a line. Live SpawnedNodeMarker ids are checked against the spec model {within r} ∪ {previously admitted within 1.5r} at every step. A ±0.2r oscillation must leave the Entity map unchanged (zero spawns or despawns), and flying back re-admits), the_selected_node_is_never_evicted, rotating_in_place_never_evicts_inside_r (same Entity for the inner ring across 12 headings; the far node straight ahead is admitted by forward weighting, the one behind is not), mesh_budget_exceeded_freezes_admission_without_evicting, gpu_pressure_shrinks_the_admitted_set (1.6 GiB → {47..53}; 1.8 GiB → no new admissions), a_far_stamp_group_is_evicted_and_restamped_on_return (also checks there is no restamp while resident and that pick data is dropped)
- fe-ui path_asset_materialize.rs: stamps_to_keep_truncates_the_farthest_and_keeps_stamp_order, pick_stamp_maps_a_truncated_pick_back_to_the_stamp_index, plus a residency-bit assertion in applied_gate_is_per_track_and_input_sensitive (existing gate tests updated for the new arg)
- fe-ui right_sidebar.rs: residency_readout_line_reports_every_counter
- fe-runtime diag15m.rs: pressure_is_the_largest_binding_not_the_sum, render_pressure_clones_share_one_cell
- fractalengine terrain_bridge.rs: tile_residency_mirror_clears_a_stale_readout; the existing S7 answered_bundles_become_terrain_chunks now also asserts ResidencyStats {tile_bundles: 1, tile_bytes ≥ 8} via the real mirror system
- fe-terrain streamed.rs: the existing moving_the_camera_demands_new_bundles_and_releases_old_ones now asserts streamed_resident_bundles/bytes go 1/>0 → 0/0 on exit-ring release

**S8b security review-and-fix (`4a61b19`) — must cover:**
- fe-sync lib: a_relayed_ad_cannot_replace_an_authenticated_root, tileset_ads_are_verse_bound_validated_and_capped, provider_map_prefers_authenticated_roots_and_ignores_relayed_claims_about_them, resolve_providers_prefers_explicit_override_and_dedupes, a_provider_address_keeps_only_trusted_relays, an_oversized_provider_address_is_refused_before_parsing
- fe-sync tests/tileset_transfer_test.rs (whole file, including the rewritten an_interrupted_download_resumes_after_restart_without_refetching and the iroh loopback over-cap assert in iroh_transport_round_trips_a_hash_seq_on_loopback)
- fe-runtime: safe_id::safe_path_id_table, blob_transport::an_oversized_blob_is_refused_and_never_stored plus the existing blob_transport tests (signature change)
- fe-format: a_smaller_import_budget_refuses_what_the_default_accepts
- fe-terrain: tests/tileset_bundles_test.rs (new a_peer_index_naming_another_tileset_never_overwrites_it; updated assemble_bundles call sites), store::device_and_traversing_ids_never_reach_the_filesystem, streamed::a_streamed_index_must_name_the_requested_tileset, streamed::a_streamed_bundle_over_the_decode_budget_is_refused (allocates about 130 MB), petal_binding::safe_cache_dir_table
- fe-database: petal_terrain tests (extended inbound deny table, a_local_terrain_write_peers_would_deny_is_refused), petal_render_distance tests unchanged
- fractalengine bin tests: a_download_naming_another_tileset_never_overwrites_it, a_failed_streamed_bundle_is_not_re_requested_every_frame, streamed_retry_delay_grows_and_caps, a_completed_download_is_assembled_and_installed, answered_bundles_become_terrain_chunks
- fe-api terrain_mutation_test / hexon_tileset_test / mcp_integration (the local SetPetalTerrain writer now refuses terrain that fails the shape check)
- fe-test-harness scenario 16 two_peer_tileset_stream (assemble now bound to TILESET_ID; resume needs the full-download marker that DownloadTileset writes)

**Final review-and-fix (`069f8ea`) — must cover:**
- cargo test -p fe-ui --lib verse_manager::residency_ledger (whole module, especially a_node_straight_ahead_keeps_a_hysteresis_band, rotating_in_place_never_evicts_inside_r, moving_camera_streams_nodes_in_and_out_with_hysteresis, enter_exit_band_keeps_only_previously_admitted)
- cargo test -p fe-ui --lib terrain_map::events (a_failed_download_shows_failed_not_downloading, closing_the_hexon_manager_does_not_stall_a_download)
- cargo test -p fe-sync --lib status::tests::undrained_tileset_buffers_stay_bounded
- cargo test -p fe-sync --lib sync_thread::tests (the_startup_pass_reports_downloads_and_hands_back_completed_ones, a_relayed_ad_cannot_replace_an_authenticated_root, tileset_ads_are_verse_bound_validated_and_capped, offline_thread_answers_tileset_commands_honestly, plus the rest of the module)
- cargo test -p fe-sync --test tileset_transfer_test (whole file, including the new a_download_completed_before_a_restart_is_handed_back_for_install, a_tag_rebuild_never_drops_a_tileset_published_meanwhile, and the iroh loopback catalog_from_tags test)
- cargo test -p fe-terrain --lib tiles::streamed (a_bundle_no_chunk_ever_pinned_is_evicted_once_idle, plus the render-gated demand tests: run with --features render or through the workspace)
- cargo test -p fractalengine --bin fractalengine terrain_bridge (a_bundle_that_fails_to_decode_is_not_refetched_every_frame, streamed_bundle_requests_stay_bounded_in_flight, only_a_requested_meta_reply_becomes_a_streamed_source, an_installed_download_completes_and_discards_its_cache, a_failed_streamed_bundle_is_not_re_requested_every_frame, answered_bundles_become_terrain_chunks, a_completed_download_is_assembled_and_installed, a_download_naming_another_tileset_never_overwrites_it)
- cargo test -p fractalengine-relay (a_headless_relay_publishes_its_seeding_tilesets_at_startup; the relay app now chains discard_tileset_events)
- fe-test-harness scenario 16 two_peer_tileset_stream (the startup pass now also emits TilesetDownloadComplete after Resumed{n/n}; the scenario must still pass on Resumed)
- Plus every item in the S1-S8b must-cover lists from the prior slice reports (unchanged by this commit)




---

## Signoff checklist

- [ ] All core suite steps executed and passing
- [ ] Feature-matrix gate green (fe-terrain fetch+render, fe-hexon all-features, fe-format no-default-features) and the A7 conformance tests confirmed present by name in the output (AC-19): `http_source_passes_conformance`, `in_memory_p2p_source_passes_conformance`, and the production P2P carrier `a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling` (root equality, `fe-hexon/tests/partial_fetch_p2p.rs:316-319`) — critic tick 5, AC-23
- [ ] W0/W0b/W0c/W0d must-cover lists reconciled and verified
- [ ] W1 PAS-P3P4 must-cover list appended and verified
- [ ] Zero test failures across all scopes
- [ ] No regressions in e2e scripts
- [ ] Packaged binaries (GUI + relay) launch and connect cleanly
- [ ] Campaign final sweep complete → ready to push

## W2 PAS-P5 (`4865a54..e7762d9`)

Generated 2026-10-14 from the W2 journal (`wf_4e779cc6-58c`).

**S1 (`db5883b`) — tests written (6):**
- fe-format/tests/integrity_flow.rs::sealed_tileset_verifies_and_names_its_publisher: export_tileset, then seal, then verify_archive under RequireSealed (with and without pin) and under VerifyIfSealed. Expects Sealed{did} equal to the signing key's DID.
- fe-format/tests/integrity_flow.rs::tampering_is_refused_per_class: one table-driven test with 10 rows, each asserting an exact IntegrityError. A flipped byte in a tile, entries.json, tileset_meta.json, members.json or an assets/{h} blob gives DigestMismatch. An appended uncovered member (the splat-append hazard) gives Uncovered. An unreferenced content-addressed blob gives AssetNotReferenced. A deleted committed member gives MissingMember. An edited manifest field without re-signing gives BadSignature. A re-seal with a different key while the publisher is pinned gives PublisherMismatch.
- fe-format/tests/integrity_flow.rs::legacy_unsealed_archive_passes_verify_if_sealed_and_fails_require_sealed: also checks that the legacy archive still imports.
- fe-format/tests/integrity_flow.rs::sealing_preserves_every_member_byte: import(seal(x)) equals import(x) for tiles, satellite tiles, assets, meta and entries. Every non-manifest member keeps its CRC-32, compressed size and compression method (proves the raw copy, covering R3). Re-sealing leaves exactly one members.json and still verifies.
- fe-format/tests/integrity_flow.rs::container_chunk_table_is_bound_to_its_chunk_members: a 3-chunk container of sealed chunks plus chunks/index.json (TilesetBundleIndex) verifies. Swapping chunk 1 for chunk 0 gives DigestMismatch{chunks/1.hexon}. A producer whose table disagrees with its chunks cannot seal.
- fe-format/src/integrity.rs::tests::seal_enforces_asset_hash_and_verifies_a_self_certifying_asset: a model whose asset is named by its blake3 seals and verifies. A model whose catalogued asset_hash does not match the blob is refused at seal with DigestMismatch{assets/<h>}. This is the A6 asset_hash check at the format layer.

**S1b (`44bc4e9`) — tests written (4):**
- fe-sync/src/blob_store.rs: default_path_under_data_dir_with_no_override, default_path_honors_fe_blob_dir_override, legacy_default_path_under_data_local_dir (ENV_GUARD-serialized)
- fe-hexon/src/registry.rs: default_path_under_data_dir_with_no_override, default_path_honors_fe_blob_dir_override (ENV_GUARD-serialized)
- fe-test-harness/tests/blob_store_cross_read.rs: default_path_agrees_between_fe_sync_and_fe_hexon (plan invariant: the two duplicated default_path() impls never re-diverge)
- fe-database/src/handlers/blob_migration.rs: migration_copies_referenced_blobs_once (plan flow test 'migration copies referenced blobs once', incl. legacy source untouched after two calls), two_installs_sharing_the_old_store_keep_independent_copies_after_migration (plan flow test 'two DBs sharing the old store: GC in one never removes the other's blob'), no_legacy_directory_is_a_clean_noop_and_still_marks_migrated

**S2 (`cbbd806`) — tests written (9):**
- fe-format/src/archive_index.rs :: header_phase_reads_no_tile_bytes
- fe-format/src/archive_index.rs :: lazy_member_read_refuses_only_the_tampered_member
- fe-format/src/archive_index.rs :: chunked_container_roundtrips_and_each_chunk_matches_its_signed_hash
- fe-format/src/archive.rs (import_integrity_tests) :: legacy_unsealed_archive_imports_as_unsealed
- fe-format/src/archive.rs (import_integrity_tests) :: sealed_archive_imports_and_reports_its_publisher
- fe-format/src/archive.rs (import_integrity_tests) :: tampered_sealed_archive_is_refused_at_import
- fe-format/src/partial.rs :: a_sealed_monolithic_header_verifies_under_its_publisher
- fe-format/src/partial.rs :: a_pin_mismatch_is_refused
- fe-format/src/partial.rs :: a_tampered_manifest_text_fails_signature_verification

**S3 (`36866aa`) — tests written (7):**
- publish_outcomes (table: sealed->200+searchable, unsigned->400, asset_hash!=blob->400 via post-seal tamper, same hexon_id different verified publisher->409, rejected version never written/indexed)
- meta_route_header_verifies_client_side_without_tile_bytes (4MiB+ tileset, meta response <64KiB, verify_header succeeds with no archive bytes)
- chunk_route_serves_bytes_matching_the_signed_index (3-chunk container: each chunk's blake3 matches the signed chunks/index.json digest + imports standalone; out-of-range seq->404; monolithic seq=0 is the whole file byte-for-byte, seq=1->404)
- member_route_refuses_names_not_in_the_archive (real member round-trips; traversal/absolute-looking/nonexistent paths all 404)
- an_html_member_is_served_as_an_octet_stream_attachment_with_nosniff (critic addition)
- entry_cap_picks_json_vs_binary (limits.rs unit test)
- drain_entry_capped_charges_the_budget_and_refuses_over_cap (limits.rs unit test)

**S4 (`3294210`) — tests written (5):**
- fe-hexon/tests/partial_fetch_http.rs::http_source_passes_conformance: an in-process registry on an ephemeral port serves the three sealed fixtures, and conformance::run through RemoteRegistryClient must pass every case. The test also asserts that named chunk, member, pin and unknown-uri cases ran, so a pass cannot be vacuous. Gated on remote+conformance.
- fe-hexon/tests/partial_fetch_http.rs::strategy_falls_back_from_a_tampering_source_to_an_honest_one: two real registries; one holds a tileset with one tile tampered after sealing. Its header is honest, but its whole archive fails verification. Asserts the returned bytes equal the honest archive, served_by is the honest registry, demoted() names the tampering one, is_demoted holds, and the next call goes straight to the honest source with no attempts.
- fe-hexon/src/registry.rs::legacy_install_refuses_missing_or_mismatched_blob: a table over missing and mismatched blobs. Asserts the exact error, that nothing is listed as installed, and that no blob was written to the store.
- fe-hexon/src/p2p/fetch.rs::pacing_delay_table: pure function over (bytes, rate, elapsed) to delay. Covers uncapped, zero rate, owed, partially owed, already slow enough, and overflow saturation.
- fe-hexon/src/p2p/fetch.rs::served_headers_are_bound_to_the_requested_uri_and_signed_layout: a table over a real sealed container header. Bare and versioned uri are accepted; another id and another version are refused as Substituted; a Monolithic layout claim over a signed container is refused. The legacy verify_fetched_manifest tests were deleted along with the function.

**S6 (`e86ec55`) — tests written (10):**
- fe-terrain/tests/hexon_integrity_flow.rs::tampered_sealed_tileset_install_is_refused_and_store_is_unchanged
- fe-terrain/tests/hexon_integrity_flow.rs::install_refuses_path_escaping_hexon_ids (table: ../x, a/b, a\\b, C:x, .., empty)
- fe-terrain/tests/hexon_integrity_flow.rs::legacy_unsealed_tileset_installs_flagged_unverified
- fe-terrain/tests/hexon_integrity_flow.rs::reinstall_keeps_an_older_readers_cached_bytes_and_new_opens_see_the_new_file
- fe-terrain/tests/hexon_integrity_flow.rs::chunked_container_install_serves_tiles_from_every_chunk
- fe-terrain/tests/hexon_integrity_flow.rs::assembled_p2p_download_installs_as_a_reseedable_sealed_container
- fe-terrain/src/tiles/hexon_source.rs::container_backed_source_serves_tiles_from_every_chunk
- fe-terrain/src/tiles/hexon_source.rs::container_backed_source_serves_baked_splats
- fe-terrain/src/tiles/composite.rs::add_hexon_source_refuses_past_the_cap
- fe-terrain/src/tiles/builder.rs::package_tiles_chunked_produces_a_container_that_verifies_sealed (fetch-feature-gated)

**S5 (`391eefd`) — tests written (10):**
- fe-hexon/tests/partial_fetch_p2p.rs::in_memory_p2p_source_passes_conformance: the A7 proof. It runs the same conformance::run as HTTP, publishing via publish_to_transport over InMemoryNetwork, and asserts the case names so the test cannot pass vacuously.
- fe-hexon/tests/partial_fetch_p2p.rs::poisoned_peer_store_is_refused: the transport itself returns Integrity for forged chunk bytes. With [poisoned, honest] sources, the strategy returns the honest bytes and the poisoned source is demoted. A forged child 0 fails the header with an integrity error.
- fe-hexon/tests/partial_fetch_p2p.rs::iroh_loopback_fetches_one_chunk_without_the_rest: two real endpoints and DocsStacks over IrohBlobTransport. It fetches the header plus chunk 1 of the 3-chunk sealed container, then checks read_local: chunk 1 is present, chunks 0 and 2 are absent. It skips if UDP binding is unavailable, like the existing loopback tests.
- fe-hexon/tests/partial_fetch_p2p.rs::a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling: install, then bundle_for_seeding, then TilesetTransfer::publish_files. Children[1..] must equal the signed chunks/index.json hashes; for the monolithic case the children are [HeaderDoc, whole archive]. child 0 is the HeaderDoc, the root equals publish_to_transport's, and a partial fetch from the seeder works.
- fe-hexon/tests/partial_fetch_p2p.rs::a_sealed_seed_downloads_installs_sealed_and_streams_checked_bundles: a sealed seed is downloaded through the real P3 transfer. The streamed source's BundleCheck passes the real bundle and refuses a forged one. Installing with a swapped chunk file is refused and the store stays empty, and a header naming another tileset is refused. A clean install reports verified=true and the stored file passes verify_archive(RequireSealed{pin}).
- fe-sync/src/event_outbox.rs::a_burst_from_spawned_tasks_never_blocks_the_loop_and_loses_nothing: 300 spawned tasks emit into a full bounded(256) channel. The loop still serves its shutdown channel, and all 300 events arrive in order after draining.
- fe-sync/src/event_outbox.rs::advisory_events_coalesce_while_state_events_queue: a newer relay health replaces the queued one in place, a repeated ad is dropped, both are counted, and state events keep their order.
- fe-sync/src/sync_thread.rs::a_command_burst_past_the_event_capacity_never_stalls_the_loop: the real spawn_sync_thread_with_transport with a virtual transport. 600 RequestChunk commands plus Shutdown go through a bounded(1) command channel while the main thread drains nothing; before the fix, the loop blocked on the 257th event and send_timeout failed. Afterwards all 600 ChunkFailed events and Stopped arrive in order.
- fractalengine/src/terrain_bridge.rs::a_never_answered_request_frees_its_slot_after_the_timeout_and_retries: driven through the real streamed systems. A meta request and a chunk request go out and hold their slots. expire_stalled_requests with a shifted clock (the only faked input) frees both into backoff; nothing is re-sent during the backoff, and both are re-sent after it.
- fe-sync/tests/tileset_transfer_test.rs::streamed_caches_past_the_disk_budget_evict_least_recently_used_first: the least-recently-used cache (mid) is evicted after old is re-read. An in-progress full download is never evicted. Startup trimming after a restart with a tighter budget evicts old and keeps new plus the full download.

**S7a (`a2147bb`) — tests written (12):**
- fe-sync/tests/tileset_transfer_test.rs::an_unconfirmed_download_never_fetches_a_chunk_even_across_a_restart
- fe-sync/tests/tileset_transfer_test.rs::a_confirmation_disagreeing_with_the_resolved_hashseq_is_refused
- fe-sync/tests/tileset_transfer_test.rs::a_published_tileset_downloads_verified_and_complete (extended: asserts AwaitingConfirmation before confirm)
- fe-sync/tests/tileset_transfer_test.rs::a_download_over_its_total_byte_cap_stops_without_completing (extended: confirms before download)
- fe-sync/src/sync_thread.rs::provider_map_prefers_authenticated_roots_and_ignores_relayed_claims_about_them (extended: asserts the TOFU pin candidate is the SAME chosen entry's publisher_did, never a different entry's)
- fe-sync/src/sync_thread.rs::tileset_ad_codec_and_authentication_table (extended: an older peer's wire frame with no publisher_did field still decodes, defaulting to empty)
- fractalengine/src/terrain_bridge.rs::seeder_with_an_unsigned_chunk_index_is_refused_before_any_chunk_request
- fractalengine/src/terrain_bridge.rs::chunk_not_in_the_signed_index_never_reaches_install
- fractalengine/src/terrain_bridge.rs::a_header_signed_by_a_publisher_other_than_the_pinned_one_is_refused
- fractalengine/src/terrain_bridge.rs::first_contact_tofu_pins_the_publisher_then_refuses_an_impostor
- fe-ui/src/terrain_map/events.rs::closing_the_hexon_manager_does_not_stall_a_download (extended: asserts Verifying before the first chunk, Downloading after)
- fe-test-harness two_peer_tileset_stream::run (extended: drives the real two-phase gate over loopback, confirm step added before awaiting TilesetDownloadComplete)

**S7b (`b56b877`) — tests written (3):**
- fractalengine/src/terrain_bridge.rs: install_from_registry_installs_a_sealed_container_end_to_end -- real in-process fe-hexon-registry (own OS thread + tokio runtime) serves a sealed 3-chunk container; queues HexonOp::InstallFromRegistry and drives it through drain_hexon_ops; asserts the exact progress sequence [(0,3),(1,3),(2,3),(3,3)], the terminal RegistryInstalled, and reads every chunk's tile back byte-identical through the real TilesetRegistry/HexonStore (not a mock)
- fractalengine/src/terrain_bridge.rs: install_from_registry_refuses_a_tampered_source_and_installs_nothing -- a sealed monolithic archive whose header verifies but whose tile bytes were flipped post-seal (same attack shape as fe-hexon's own tampering-source fixture); asserts RegistryFailed and an empty store afterward
- fe-ui/src/settings.rs: hexon_registry_url_resolve_precedence_table (env-wins/settings-fallback/blank-is-absent, 6 cases) + two new sanitize-table rows (blank->None, real URL gets trimmed) + hexon_registry_url added to saved_settings_survive_a_restart's round-trip

**S8 security review-and-fix (`d21a957`) — must cover:**
- cargo test --workspace (all PAS-P5 S1-S8 deferred tests)
- cargo test -p fe-hexon --all-features: http_source_passes_conformance and in_memory_p2p_source_passes_conformance must appear by name; also served_headers_are_bound_to_the_requested_uri_and_signed_layout
- cargo test -p fe-format: a_signed_header_served_under_another_uri_is_refused, a_layout_claim_that_contradicts_the_signed_chunk_index_is_refused, archive_size_is_advisory_and_never_reaches_the_verified_header, an_uncommitted_header_member_in_a_sealed_doc_is_refused, an_uncommitted_header_member_in_a_sealed_archive_is_refused_at_open, plus tests/integrity_flow.rs
- cargo test -p fe-terrain --features fetch,render: a_sealed_install_is_never_replaced_by_another_publisher_or_an_unsealed_archive (tests/hexon_integrity_flow.rs), container_splat_reads_are_bounded_by_the_streamed_bundle_budget
- cargo test -p fe-hexon-registry: concurrent_publishes_of_one_id_admit_exactly_one_publisher, a_hostile_member_name_cannot_shape_the_content_disposition_header, publish_outcomes, an_html_member_is_served_as_an_octet_stream_attachment_with_nosniff
- cargo test -p fractalengine (bin): a_header_signed_for_another_tileset_is_refused_and_pins_nothing, an_installed_tileset_pins_its_publisher_over_the_advertisers_hint, install_from_registry_stops_a_container_past_its_byte_budget, first_contact_tofu_pins_the_publisher_then_refuses_an_impostor, install_from_registry_installs_a_sealed_container_end_to_end
- cargo test -p fe-database blob_migration: a_crash_mid_migration_is_retried_without_loss_or_legacy_writes, a_corrupt_legacy_blob_is_not_copied, a_new_store_at_the_legacy_path_writes_nothing_into_it
- cargo test -p fe-sync (lib + tests/tileset_transfer_test.rs: confirm gate, resume_all skips unconfirmed)
- fe-test-harness two_peer_tileset_stream scenario

**Final review-and-fix (`e7762d9`) — must cover:**
- env CARGO_PROFILE_DEV_DEBUG=0 RUST_MIN_STACK=134217728 cargo test -j4 --workspace (all PAS-P5 S1-S8 + final-review deferred tests)
- cargo test -j4 -p fe-hexon --all-features — output must name http_source_passes_conformance and in_memory_p2p_source_passes_conformance (C10 non-vacuous), plus the_locator_stays_bounded_under_an_advertisement_storm, strategy_falls_back_from_a_tampering_source_to_an_honest_one, poisoned_peer_store_is_refused, iroh_loopback_fetches_one_chunk_without_the_rest, a_sealed_container_is_reseeded_from_its_signed_chunks_without_rebundling, legacy_install_refuses_missing_or_mismatched_blob, pacing_delay_table
- cargo test -j4 -p fe-sync --test tileset_transfer_test — a_streamed_cache_is_never_confirmed_into_a_full_download, an_unconfirmed_download_never_fetches_a_chunk_even_across_a_restart, a_confirmation_disagreeing_with_the_resolved_hashseq_is_refused
- cargo test -j4 -p fe-sync --lib — an_unconfirmed_full_download_is_discarded_at_startup_never_installed, the_startup_pass_reports_downloads_and_hands_back_completed_ones, event_outbox tests (a_burst_from_spawned_tasks_never_blocks_the_loop_and_loses_nothing, advisory_events_coalesce_while_state_events_queue), blob_store default_path tests
- cargo test -j4 -p fractalengine (bin) — install_flow_replies_are_verified_off_thread_and_untracked_ones_ignored, only_a_requested_meta_reply_becomes_a_streamed_source, a_never_answered_request_frees_its_slot_after_the_timeout_and_retries, first_contact_tofu_pins_the_publisher_then_refuses_an_impostor, a_header_signed_for_another_tileset_is_refused_and_pins_nothing, an_installed_tileset_pins_its_publisher_over_the_advertisers_hint, seeder_with_an_unsigned_chunk_index_is_refused_before_any_chunk_request, chunk_not_in_the_signed_index_never_reaches_install, install_from_registry_installs_a_sealed_container_end_to_end, install_from_registry_refuses_a_tampered_source_and_installs_nothing, install_from_registry_stops_a_container_past_its_byte_budget
- cargo test -j4 -p fe-ui terrain_map — closing_the_hexon_manager_does_not_stall_a_download plus the settings hexon_registry_url tables
- cargo test -j4 -p fe-database blob_migration asset_gc — migration_copies_referenced_blobs_once, two-DB GC test, a_crash_mid_migration_is_retried_without_loss_or_legacy_writes, a_corrupt_legacy_blob_is_not_copied, a_new_store_at_the_legacy_path_writes_nothing_into_it, plus the existing asset_gc tests (now on the doomed-subset query)
- cargo test -j4 -p fe-format (incl. tests/integrity_flow.rs and the S8 verify_header binding tests)
- cargo test -j4 -p fe-terrain --features fetch,render (tests/hexon_integrity_flow.rs, container_splat_reads_are_bounded_by_the_streamed_bundle_budget)
- cargo test -j4 -p fe-hexon-registry (publish_outcomes, concurrent_publishes_of_one_id_admit_exactly_one_publisher, nosniff/attachment route tests)
- cargo test -j4 -p fractalengine-test-harness: two_peer_tileset_stream scenario and tests/blob_store_cross_read.rs


## W2b fixups (`8d8eaf8..474ab9b`)

**Executor `4306cb2` — tests written:**
- fe-terrain/src/tiles/streamed.rs::an_unsealed_index_is_refused_once_a_sealed_publisher_pin_is_known
- fe-terrain/src/tiles/streamed.rs::a_streamed_header_signed_by_a_publisher_other_than_the_known_pin_is_refused (unit-level, via from_header_doc)
- fe-terrain/tests/hexon_integrity_flow.rs::concurrent_installs_of_one_id_by_two_publishers_leave_exactly_one_winner
- fractalengine/src/terrain_bridge.rs::a_streamed_header_signed_by_a_publisher_other_than_the_known_pin_is_refused (system-level, through drive_streamed_tilesets)
- fractalengine/src/terrain_bridge.rs::an_unsealed_streamed_meta_is_refused_once_a_sealed_publisher_is_known (system-level)
- fractalengine/src/terrain_bridge.rs::only_a_requested_meta_reply_becomes_a_streamed_source (modified to poll frames instead of asserting after one app.update())
- fractalengine/src/terrain_bridge.rs::a_local_hexon_install_runs_off_the_main_thread_and_reports_its_outcome
- fractalengine/src/terrain_bridge.rs::resolve_hexon_registry_token_table
- fractalengine/src/terrain_bridge.rs::install_from_registry_requires_the_configured_bearer_token
- fractalengine-relay/src/main.rs::derive_blob_dir_from_db_path_table
- fe-hexon-registry/tests/registry_test.rs::asset_and_download_routes_carry_nosniff_and_attachment_headers

**Review-and-fix `474ab9b` — must cover (2 vacuous F1 system tests were rewritten to drive verification):**
- fractalengine lib (terrain_bridge): a_streamed_header_signed_by_a_publisher_other_than_the_known_pin_is_refused, a_streamed_header_is_pinned_to_the_installed_publisher, an_unsealed_streamed_meta_is_refused_once_a_sealed_publisher_is_known, only_a_requested_meta_reply_becomes_a_streamed_source, a_local_hexon_install_runs_off_the_main_thread_and_reports_its_outcome, resolve_hexon_registry_token_table, install_from_registry_requires_the_configured_bearer_token, the expire_stalled_requests test (count still 2), and the full existing streamed/install-flow/registry suite
- fe-terrain lib (tiles::streamed): an_unsealed_index_is_refused_once_a_sealed_publisher_pin_is_known, a_streamed_header_signed_by_a_publisher_other_than_the_known_pin_is_refused, a_streamed_index_must_name_the_requested_tileset; fe-terrain lib (tiles::store) set_seeding/update_last_served tests (now lock-taking)
- fe-terrain/tests/hexon_integrity_flow.rs: concurrent_installs_of_one_id_by_two_publishers_leave_exactly_one_winner, installs_racing_seeding_toggles_lose_no_registry_update, plus the full file
- fractalengine-relay bin tests: derive_blob_dir_from_db_path_table (run on both Windows and Linux)
- fe-hexon-registry/tests/registry_test.rs: asset_and_download_routes_carry_nosniff_and_attachment_headers plus the full file
- fe-hexon/tests/partial_fetch_p2p.rs: full file (pin arg None call site)

## W3 CDL-W4 (append at W3 close)

> **Evidence preservation (critic AC-31):** W4-9's equivalence report `target/migration-evidence/*.json`
> is the R7 #3 CANON-DUAL-EMIT dev evidence and `target/` is gitignored — copy it to
> `.omc/research/cdl-w4-equivalence-<date>/` BEFORE any `target/` prune or `cargo clean`.

Generated 2026-10-14 from the W3 journal (`wf_644e287e-abf`), base `38dd271` → `381264e`. NO line is truncated.

**W4-1 (`41c466b`) — tests written (2):**
- each_dcl20_mutation_records_exactly_one_bypass_of_its_own_kind: drives CreateVerse -> CreateFractal -> CreatePetal -> ImportGltf (real non-KTX2 GLB fixture on disk) -> RenameNode -> DuplicateNode sequentially through the real DB thread + DbCommand/DbResult API; after each command asserts (a) exactly that command's own D_CL20_MUTATION_KINDS slot rose by 1 and the other five did not move (assert_only_this_kind_bypassed helper diffs a 6-slot snapshot), (b) the DbResult is the same success variant the pre-change handler returns (VerseCreated/FractalCreated/PetalCreated/GltfImported/NodeRenamed/NodeCreated), and (c) the op_log row count (read directly off the shared datastore handle via spawn_db_thread_shared, no second SurrealKV open) is unchanged across the command, proving the counter added no op-log row. Mutation check: deleting any one record_bypass_for call turns that command's own-kind-delta assertion from 1 to 0.
- a_rename_on_a_tombstoned_node_still_records_the_bypass_and_keeps_the_error_text (the one unhappy path kept, per the slice spec): creates a hierarchy + node, tombstones it (TombstoneNode -> NodeDeleted), then sends RenameNode on the now-tombstoned node; asserts the DbResult is Error containing the exact original text 'RenameNode matched no live node with node_id = {node_id}' (unchanged, since the diff is 6 insertions/0 deletions) AND that bypass_count_for("RenameNode") still rose by 1 — proving record_bypass_for fires as the literal first statement, before the tombstone-aware UPDATE that then matches zero rows.

**W4-2 (`665a795`) — tests written (7):**
- fe-database/tests/canon_log_materialization_test.rs::an_unknown_kind_candidate_is_quarantined_not_rejected: a validly signed operation of a kind the verifier has not registered gives UnknownKind and a Quarantine disposition, and appends 0 verified_op_log rows. The same bytes then admit after the 'build upgrade' (1 row), which shows a quarantine, not a blacklist.
- fe-database/tests/canon_log_materialization_test.rs::a_candidate_whose_chain_is_unavailable_is_quarantined_and_admits_once_it_arrives: the verifier first answers AuthorizationEvidenceUnavailable(CapabilityChainUnavailable), which gives Quarantined and 0 rows. Once the chain arrives, admit_and_append appends exactly 1 row with payload_facts None. Uses the real Surreal store.
- fe-canonical-log/src/materialize/errors.rs::every_admission_outcome_has_its_spec_disposition: the one table-driven unit test allowed here, covering every AdmissionOutcome variant (including all three gap kinds) and checking disposition(), is_reject() and is_quarantine().
- fe-canonical-log/src/materialize/errors.rs::only_the_three_evidence_gaps_wrap_into_authorization_evidence_unavailable: the constructor refuses Unauthorized, UnknownKind, MissingParent and FailedDecryption.
- fe-canonical-log/src/materialize/traits.rs::reduce_reads_its_own_payload_facts_and_stays_reference_only (A1 hand-back): a real AEAD-sealed payload under the §5.2 AAD goes through admit_candidate_with_facts, and the verifier extracts facts only after it authenticates. A fact-reading reduction then writes a reference row carrying only op_id and the extracted field, identically on replay. Without facts it returns ReferencedArtifactUnavailable naming the ciphertext hash, and admission without the ciphertext returns OpaquePayload as a quarantine. Reverting the reduce signature makes the fixture materializer uncompilable.
- fe-canonical-log/src/materialize/traits.rs::payload_facts_refuse_the_no_payload_form_and_non_canonical_plaintext
- compile_fail,E0639 doctest on VerifiedCandidate (an external struct literal is refused)

**W4-3 (`292bdf7`) — tests written (8):**
- compose/verifier.rs: a_signed_operation_with_a_live_chain_and_a_decryptable_payload_is_admitted. Runs a real chain, a real scope key and a real sealed envelope through admit_candidate_with_facts, then checks the payload rows: tampered supplied ciphertext gives InvalidPayload (reject), absent ciphertext gives OpaquePayload, and an absent key gives the PayloadKeyUnavailable gap.
- compose/verifier.rs: the_same_operation_is_refused_after_its_scope_epoch_is_bumped. The view at EPOCH returns Ok; at EPOCH+1 it returns UnauthorizedOperation with Reject disposition.
- compose/verifier.rs: a_candidate_the_verifier_cannot_yet_judge_is_quarantined_with_the_missing_evidence_named. No chain gives CapabilityChainUnavailable; a view with no epoch gives EpochStateUnavailable; an unregistered application kind 0x0200 gives UnknownKind. All three are asserted to be Quarantine (E-W4-1).
- compose/verifier.rs: a_payload_key_held_at_the_verse_opens_a_petal_scoped_operation (outward key resolution by key_id match).
- compose/lifecycle.rs: an_operation_by_a_retired_author_key_is_unauthorized. The rotation is admitted through the composed verifier and dispatched through LifecycleDispatcher; the follow-up by the old key with parents [rotation] and with [genesis, rotation] is UnauthorizedOperation (§4 rule 4, any parent). A concurrent op with parent [genesis] admits (§4 rule 5) and dispatches as NotLifecycle. Control: the same follow-up under NoLifecycle admits, so the refusal comes from the lineage.
- compose/wire.rs: a_wire_authorize_cannot_borrow_someone_elses_chain. The owner gets the exact VerifiedAuthorization; principal B presenting A's valid bytes gets CapabilityNotVerified (mutation check: reading the requester from the leaf audience would turn this green); a two-verb mask gives WrongShape key 2.
- compose/mod.rs: a_claim_naming_an_unbound_materializer_identifier_is_refused (claim and selection agree with each other on an id that is not the version's registry_id, giving MaterializerIdentityUnbound; the bound id is Ok). The existing binding test was updated to use registry ids, and its foreign-materializer case now varies the claim.
- materialize/identity.rs: the_registry_id_hashes_the_domain_and_the_hand_encoded_name_version_map (hand-encoded RFC 8949 bytes A2 00 6B 'scene-graph' 01 03 pin the provisional wire derivation); the_registry_id_separates_every_name_and_version_pair (table-driven). Tests are written and compile; none were run, per the user rule.

**W4-4 (`657b4cc`) — tests written (5):**
- compose::storage::tests::a_rewrite_of_an_existing_artifact_id_is_refused_by_the_byte_store_adapter (plan-named) — asserts the backing ContentByteStore's own put() is called exactly once for the first write and once for the idempotent identical rewrite, and NOT a third time for a conflicting rewrite, which the adapter must refuse before it ever delegates
- compose::storage::tests::crypto_shred_through_the_custody_store_makes_the_payload_unopenable (plan-named) — seals a real HashSeqNode via aead::seal + SealedArtifact::seal, opens it through SealedProofEnvironment before crypto_shred, then asserts Err(ScopeKeyUnavailable) after crypto_shred runs, exercising ScopeKeyStore+ShredLedger end to end
- compose::storage::tests::commit_submit_resubmitted_byte_identical_acks_already_present_not_a_second_append (plan-named) — submits the same signed envelope bytes twice through VerifyingCommitPipeline against a call-counting VerifiedLogStore double, asserting exactly one fresh append and one already-present hit
- retention::crypto_shred::tests::crypto_shredding_preserves_ciphertext_but_revokes_future_key_access (rewritten to use the real crypto::scope_key::InMemoryScopeKeyStore instead of the deleted test-local duplicate)
- retention::crypto_shred::tests::shredding_an_uncontrolled_key_fails_without_recording_a_disposition (rewritten, same reason)

**W4-5 (`dad195b`) — tests written (5):**
- golden_vector_envelope_admits_through_the_composed_verifier — admits a fresh envelope under the golden-vector signing identity through a self-issued capability chain, then proves a forged-signature byte is caught at ingress (InvalidEnvelope), never reaching authorization
- a_capability_gates_append_and_is_refused_after_a_scope_epoch_bump — same chain, same candidate: live epoch admits, epoch+1 view refuses with AdmissionOutcome::UnauthorizedOperation / AdmissionDisposition::Reject
- a_valid_claim_still_reports_unresolved_reachability_without_header_or_decrypt_authority — real sealed header segment/HashSeq node/manifest through SealedProofEnvironment+ByteStoreSealedArtifactStore: baseline Verified{1,0}; withheld header segment -> Unresolved; empty scope-key store -> Unresolved{ScopeKeyUnavailable}
- a_sealed_payload_round_trips_and_fails_closed_when_its_aad_envelope_is_mutated — a kind-1 payload opens and extracts facts under its own envelope, then the identical ciphertext (same committed hash/length) carried into a second, independently signed envelope whose branch_id differs is rejected as InvalidPayload (AEAD tag bound to the full §5.2 AAD, not just the hash)
- commit_submit_ack_delta_flows_over_the_composed_verifier — handle_commit_submit over VerifyingCommitPipeline (AcceptedPendingMaterialization), the same chain re-verified through ChainCapabilityVerifier's wire authorize, and is_delta_authorized_for_subscription/CommitDeltaBody::for_subscription proving a sibling petal's subscriber is refused (ScopeNotAuthorized) while a verse-wide subscriber is authorized

**W4-6 (`ca8fb74`) — tests written (9):**
- fe-canonical-log/src/crypto/device_enrolment.rs::tests::a_wrap_to_an_enrolled_device_opens_and_a_wrap_to_an_unenrolled_one_cannot_be_requested
- fe-canonical-log/src/crypto/device_enrolment.rs::tests::a_revoked_device_stays_revoked_when_a_stale_enrolment_arrives_later
- fe-canonical-log/src/crypto/device_enrolment.rs::tests::an_enrolment_signed_by_another_principal_does_not_verify
- fe-canonical-log/src/crypto/device_enrolment.rs::tests::malformed_artifacts_are_refused_at_the_verification_door (table-driven)
- fe-canonical-log/src/crypto/device_enrolment.rs::tests::same_class_artifacts_converge_on_one_stored_artifact_in_any_order
- fe-canonical-log/src/crypto/device_enrolment.rs::tests::the_unified_door_classifies_by_signature_domain_and_keeps_the_exact_bytes
- fe-canonical-log/src/crypto/device_enrolment.rs::tests::every_domain_separator_is_nul_terminated_ascii_and_none_prefixes_another
- fe-canonical-log/src/crypto/device_enrolment.rs doctests (2x compile_fail: Verified* struct literals refused outside the module)
- fe-canonical-log/tests/cross_spec_integration.rs::a_device_enrolment_gates_a_scope_key_wrap (plan W4-5 test 6)

**W4-7 (`7d935cb`) — tests written (5):**
- fe-database/tests/canon_log_composed_admission_test.rs::a_locally_signed_operation_admits_appends_and_replays_through_the_durable_verifier. Runs through the real DB thread's shared SurrealKV handle on an on-disk store. A kind-1 operation admits and is appended, with payload facts. An unregistered schema is quarantined as UnknownSchema and nothing is appended. After the DB thread is joined and reopened, the lifecycle replay sees 2 operations and the ciphertext is still in the artifact store. Re-submitting the same bytes gives AlreadyPresent, and a child operation admits.
- ::an_operation_after_its_authors_rotation_is_refused_after_restart. The author's rotation admits and is dispatched to SPEC-2 (Rotation / Admitted). A follow-up by the old key is refused as UnauthorizedOperation while the process is still running. After a restart, `lifecycle_replay.dispatched` re-derives the rotation from the log, `key_state` reports Retired, and the old-key operation is refused again. Control: an operation concurrent with the rotation still admits (SPEC-2 §4 rule 5), which shows the refusal comes from the lineage, not the chain.
- ::a_checkpoint_from_a_non_manager_signer_is_not_admitted. `AdmittedCheckpoint::admit` with the durable view. An Editor holding a valid Manager-issued append/checkpoint chain gets NotManagerPlus. The Manager naming an unheld chain, or the Editor's held chain, gets CapabilityDoesNotPermitCheckpoint. The Manager with its own chain is admitted.
- ::shredding_a_scope_key_turns_later_admission_into_quarantine_not_reject. An operation admits while the key is held. The key is shredded through the verifier's own custody. The next operation is Quarantined as AuthorizationEvidenceUnavailable (payload key), not Rejected, and nothing is appended. The keystore entry is gone. After a restart the custody state is Destroyed, re-keying is refused with IssuanceStopped, and admission still quarantines.
- Not run, per the user rule. The four tests compile under `cargo check --workspace --tests` and clippy `--all-targets`. They should be added to final-sweep-manifest.md §'W3 CDL-W4'; that bookkeeping belongs to the orchestrator.

**W4-8 (`43964ec`) — tests written (8):**
- fe-database/tests/device_enrolment_test.rs::a_local_enrolment_is_stored_signed_and_emitted_for_replication: real DB thread on an on-disk store with a keypair, secret store and replication channel. Sends EnrolLocalDevice and checks that: the principal is the node did:key; the device secret is persisted; the stored row verifies; the ReplicationEvent targets canon_device_enrolment; the published blob bytes equal the stored row; and the view answers Granted.
- fe-database/tests/device_enrolment_test.rs::revoking_the_local_device_refuses_it_and_rotates_to_a_fresh_key: enrol, then RevokeLocalDevice. The row is checked to be a revocation and the local secret to be deleted; re-enrolling yields a new key; the view answers Refused for the old key and Granted for the new one.
- fe-database/tests/device_enrolment_test.rs::an_enrolment_without_a_node_keypair_is_refused_not_placeholder_signed: with no keypair, the command gets a correlated RequestFailed, nothing is stored and nothing is published.
- fe-database/tests/device_enrolment_test.rs::a_peer_enrolment_is_admitted_only_from_its_own_principal: table-driven through the production apply_replicated_row_handler. Cases: own principal → Applied; a Manager relaying someone else's enrolment → Denied; named principal with bytes signed by another key → Denied; Viewer → Applied; pre-manifest relay → Denied (pins the branch-before-bootstrap ordering). Each case asserts the row is stored iff Applied. Also checks that the del marker → NotApplicable and leaves the held row untouched.
- fe-database/tests/device_enrolment_test.rs::a_revocation_survives_a_late_enrolment_replay: apply enrolment, revocation, then the enrolment again. Expected outcomes are Applied, AppliedTombstone, SkippedTombstoned; the stored row stays the revocation and the view answers Refused. The stale enrolment carries a later timestamp than the revocation, to show timestamps never decide.
- fe-database/tests/device_enrolment_test.rs::cdl_r2_delivery_acceptance: real epoch bump to 1, Manager and Editor role rows, the recipient's decrypt/op chain put through ChainStore, and enrolments applied via the inbound path, all read through DurableDeliveryView::load. Checks: enrolled device + Manager issuer + decrypt chain → issue_scope_key_wrap, then open_scope_key returns the scope key; Editor issuer → IssuerNotAuthorized{Refused}; unenrolled device → DeviceNotEnrolled{Unknown}; enrolled principal with no decrypt chain → RecipientNotAuthorized{Unknown}.
- fe-database/src/canon_log/device_enrolment.rs unit tests: a_row_whose_index_columns_lie_about_its_artifact_does_not_verify (table-driven over a forged revoked flag, a foreign DID, a foreign record id and non-base64 bytes) and admission_binds_the_author_to_the_principal_and_the_wire_key_to_the_record.
- fe-runtime/src/app.rs::device_enrolment_replies_reach_only_their_own_waiter: an uncorrelated echo never reaches the waiter, the enrol echo reaches its own entry, and a correlated RequestFailed reaches the revoke waiter.

**W4-9 (`7c31344`) — tests written (5):**
- fe-database/tests/migration_equivalence_harness.rs::transform_corpus_reaches_projection_parity_across_three_replays: the full corpus. Asserts compared_equal per transform at its own prefix; zero legacy_succeeded_without_candidate for UpdateNodeTransform; three-way replay roots identical, including after the restart from the durable stores and from a reversed input; that an arrival-order fold gives a DIFFERENT root for the concurrent pair; crash-recovery exactly-once (ledger, verified log, AlreadyPresent, edit_seq unchanged); that the uncovered tombstone surfaces as exactly one ValueMismatch(Tombstoned); and that blocks_promotion is still true because of the D-CL20 bypasses. Also checks the drift guard and writes the evidence JSON.
- fe-database/tests/migration_equivalence_harness.rs::a_lossy_transform_mapping_is_detected_as_a_difference: derives with a millimetre-rounding quantizer and asserts the comparator reports compared_different with a single ValueMismatch on the node's path.
- fe-canonical-log/tests/legacy_identifier_golden.rs::legacy_identifier_matches_the_canonical_log_cbor_encoder: table-driven across the RFC 8949 text-header boundaries (critic C10a).
- fe-sdk/src/legacy_identifier.rs unit tests: legacy_identifier_is_stable_and_collision_free_across_tables (plan-named; a mutation that drops the table tag turns it red); the_hashed_bytes_are_the_spec_preimage; a_non_ascii_id_is_refused_rather_than_hashed_non_canonically; text_headers_use_the_shortest_form_at_every_length_boundary.
- fe-database/src/migration/transform_mapping.rs unit tests: exact_quantization_scales_by_a_billion_and_refuses_what_cannot_be_an_i64 (table-driven); a_payload_round_trips_and_a_fifth_key_is_refused (SPEC-8 §4.2 rule 3, no previous-value field).

**Security review-and-fix (`e886539`) — must cover:**
- fe-database test binary device_enrolment_test: new a_principal_without_a_role_cannot_enrol_but_can_always_revoke, plus the existing a_peer_enrolment_is_admitted_only_from_its_own_principal and a_revocation_survives_a_late_enrolment_replay (the floor touches the admission path they drive)
- fe-database lib unit test canon_log::device_enrolment::tests::admission_binds_the_author_to_the_principal_and_the_wire_key_to_the_record (return type changed to SelfAuthoredRow)
- fe-database lib unit test canon_log::scope_key_custody::tests::a_corrupt_filed_secret_is_refused_without_quoting_any_of_its_characters (new test module)
- fe-canonical-log unit test compose::storage::tests::commit_submit_never_appends_bytes_the_verifier_did_not_judge (new), plus commit_submit_resubmitted_byte_identical_acks_already_present_not_a_second_append (its signed_sample_bytes helper was refactored)
- fe-canonical-log tests/cross_spec_integration.rs test 5 commit_submit_ack_delta_flows_over_the_composed_verifier (goes through VerifyingCommitPipeline::submit_candidate)
- fe-test-harness run (16/16 x2): enrolment rows ride the real replicated-row path that now consults resolve_role for post-manifest enrolments

**Correctness review-and-fix (`00a2a08`) — must cover:**
- fe-database test binary dcl20_bypass_counter_test: each_dcl20_mutation_records_exactly_one_bypass_of_its_own_kind, a_rename_on_a_tombstoned_node_still_records_the_bypass_and_keeps_the_error_text, and the new a_ktx2_refused_import_still_records_its_bypass (all now serialized through serial())
- fe-canonical-log unit test materialize::traits::tests::admission_never_hands_on_payload_facts_minted_for_another_operation (new), plus reduce_reads_its_own_payload_facts_and_stays_reference_only and the admit_candidate_* tests (they now run through the facts-binding check)
- fe-canonical-log unit test compose::storage::tests::commit_submit_never_acks_as_appended_what_admission_refused (new), plus commit_submit_never_appends_bytes_the_verifier_did_not_judge and commit_submit_resubmitted_byte_identical_acks_already_present_not_a_second_append
- fe-canonical-log tests/cross_spec_integration.rs: all 6 tests, especially commit_submit_ack_delta_flows_over_the_composed_verifier (new bumped-epoch rejection assertion)
- fe-canonical-log tests/legacy_identifier_golden.rs legacy_identifier_matches_the_canonical_log_cbor_encoder, and the fe-sdk legacy_identifier unit tests (4)
- fe-database test binaries migration_equivalence_harness (2 tests, highest risk, never executed), canon_log_materialization_test (an_unknown_kind_candidate_is_quarantined_not_rejected, a_candidate_whose_chain_is_unavailable_is_quarantined_and_admits_once_it_arrives), canon_log_composed_admission_test and device_enrolment_test: every one passes through admit_candidate_with_facts, which now refuses mismatched facts
- fe-database lib unit tests import_gltf_ktx2_guard_tests and migration::transform_mapping tests
- node --test docs/spec/canonical-log/operation-envelope-v1.test.mjs (no envelope codec change in
  W3; run as a regression guard, moved here from the stale reservation note — critic tick 7, AC-39)

## W3b fixups (`bccb654` + review `78eab0e`)

Reviewer fixed a seeding-toggle reorder race (FIFO mutation worker), publish-on-failed-write, and a GUI↔relay shared temp-path tear.

- fractalengine/src/terrain_bridge.rs: install_from_registry_threads_the_bearer_token_to_the_client
- fractalengine/src/terrain_bridge.rs: remove_and_seeding_ops_report_through_the_worker_channel
- fractalengine/src/terrain_bridge.rs: a_local_hexon_install_runs_off_the_main_thread_and_reports_its_outcome (amended with try_recv-empty assertions, both legs)
- install_from_registry_threads_the_bearer_token_to_the_client
- remove_and_seeding_ops_report_through_the_worker_channel
- a_local_hexon_install_runs_off_the_main_thread_and_reports_its_outcome
- fractalengine/src/terrain_bridge.rs: queued_store_mutations_apply_in_click_order (16 seeding toggles, one per frame, then a Remove; asserts the applied order matches the click order and the store ends empty. Mutation: going back to a thread per op makes 16 threads compete for the lock, so this very likely goes red, though not guaranteed)
- fractalengine/src/terrain_bridge.rs: a_seeding_enable_that_fails_to_persist_is_never_published (full flow: drain_hexon_ops → worker → drain_hexon_assemble_results → SyncCommand channel. Enable publishes, disable unpublishes, and after registry.json is corrupted an enable does not publish. Mutation: removing the result.is_err() gate makes it publish, so the test goes red)
- fe-terrain/src/tiles/store.rs: a_stale_registry_temp_file_never_blocks_a_save (a directory sits at the old fixed registry.json.tmp name; install and set_seeding still succeed. Mutation: reverting to the fixed temp name makes File::create fail, so the test goes red deterministically)
- queued_store_mutations_apply_in_click_order
- a_seeding_enable_that_fails_to_persist_is_never_published
- a_stale_registry_temp_file_never_blocks_a_save
- installs_racing_seeding_toggles_lose_no_registry_update
- test_set_seeding
- test_remove_tileset
- an_installed_download_completes_and_discards_its_cache

