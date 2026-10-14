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
| **Harness scenario 16** | `cargo run -p fractalengine-test-harness` (real loopback iroh, all 15 scenarios ×2) | Terminal output: `15/15 ×2 scenarios pass` |
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
- fe-sync/tests/tileset_transfer_test.rs::a_published_tileset_downloads_verified_and_complete (:78): two services over one InMemoryNetwork. Every file's blake3 equals its HashSeq child and its bytes equal the source. The i
- fe-sync/tests/tileset_transfer_test.rs::a_tampered_bundle_is_rejected_and_never_persisted (:127): tamper(child2) gives failed == [(2, HashMismatch{expected, actual})]. No 2.bundle and no temp file is left behind, and the
- fe-sync/tests/tileset_transfer_test.rs::an_interrupted_download_resumes_after_restart_without_refetching (:181): seqs 0-1 are fetched, then the service is dropped. Alice withholds the root, the index and bundles 0-1, and
- fe-sync/tests/tileset_transfer_test.rs::cancel_discards_partial_state (:238): after cancel the dir and the download tag are gone, nothing is pending, fetch_chunk returns UnknownDownload, and a fresh start_download stores
- fe-sync/tests/tileset_transfer_test.rs::a_tileset_id_that_escapes_the_download_dir_is_refused (:283): the ids ../x, a/b, '', .., a\b and C:x each get UnsafeTilesetId from request_meta, fetch_chunk, download, cancel and p
- fe-sync/tests/tileset_transfer_test.rs::iroh_transport_round_trips_a_hash_seq_on_loopback (:340): two real DocsStacks with RelayConfig::Disabled. fetch_hash_seq over loopback returns the exact children, one child fetch m
- fe-sync/tests/tileset_transfer_test.rs::catalog_survives_a_stack_restart (:405): publish on a DocsStack, shut it down and respawn it on the same dir. A new service starts empty, and catalog_from_tags returns the identica
- fe-sync/src/tileset_transfer.rs::tests::tileset_id_guard_table: a pure table of accepted and refused ids, including the Windows device names and the length cap.
- fe-runtime/src/blob_transport.rs::tests: hash_seq_codec_round_trips_and_rejects_ragged_blobs; a_peer_fetches_what_another_published_and_hooks_change_what_is_served (checks that unverified bytes never enter the store); ta

**S3 (`e5d8928`) — tests written (6):**
- fe-sync/src/sync_thread.rs::tests::tileset_ad_codec_and_authentication_table — table-driven: a ComputeEnvelope-shaped frame and garbage both fail decode_tileset_ad; a well-formed ad round-trips; tileset_ad_is_authenticat
- fe-sync/src/sync_thread.rs::tests::offline_thread_answers_tileset_commands_honestly — a virtual-transport sync thread (no TilesetTransfer) answers DownloadTileset with an honest offline TilesetMetaReceived (empty root_ha
- fe-sync/src/sync_thread.rs::tests::provider_map_sorts_authenticated_first_and_upgrades_in_place — TilesetProviderMap::record/candidates/root_for/bytes_for: authenticated providers sort first, an authenticated entry never
- fe-sync/src/sync_thread.rs::tests::resolve_providers_prefers_explicit_override_and_dedupes — explicit provider override is tried first and de-duplicated against known candidates; resolve_providers_and_root returns a name
- fe-sync/src/sync_thread.rs::tests::handle_advertise_tilesets_no_host_is_noop — updated for the new signature (no gossip host -> no-op, never panics).
- fe-sync/src/messages.rs — updated/added Debug+Clone/field-shape tests for RequestChunk, ChunkReceived, TilesetMetaReceived, plus new PublishTileset/DownloadTileset/TilesetDownloadComplete/TilesetPublished/TilesetDownload

**S4 (`3d89b60`) — tests written (4):**
- fe-ui/src/terrain_map/events.rs :: tests::closing_the_hexon_manager_does_not_stall_a_download — regression test: with ActiveDialog::None the whole session, TilesetMetaReceived/ChunkReceived/TilesetDownloadComplete still 
- fractalengine/src/terrain_bridge.rs :: tests::a_completed_download_is_assembled_and_installed — builds a real S1 bundle set (fe_terrain::tiles::bundle_tileset) on disk, runs the private assemble_and_install against a fre
- fractalengine-relay/src/main.rs :: tests::a_headless_relay_publishes_its_seeding_tilesets_at_startup — one seeding + one non-seeding installed tileset; publish_seeding_tilesets drains to exactly one PublishTileset (the s
- fe-test-harness/src/scenarios/two_peer_tileset_stream.rs :: run (scenario 16) — real two-peer loopback transport: alice bundles+publishes, bob joins the gossip topic, receives the real advertisement, downloads, assembles

**S5 (`9725dd2`) — tests written (10):**
- fe-terrain/tests/lazy_tiles_test.rs::a_lazy_source_serves_exactly_what_the_eager_import_did — runs over every elevation and satellite member of an archive written to disk. Each member starts cold (probe returns Miss and 
- fe-terrain/tests/lazy_tiles_test.rs::a_reinstalled_archive_is_reindexed_and_serves_the_new_bytes — unhappy path that can actually happen: HexonStore::install_tileset rewrites the same id in place while an old source is s
- fe-terrain/tests/lazy_tiles_test.rs::bevy_flows::registry_and_active_petal_share_one_source (render) — real assignment system with SharedTilesetRegistry. Checks Arc::ptr_eq between registry.source(id) and composite.hexon
- fe-terrain/tests/lazy_tiles_test.rs::bevy_flows::a_cold_tile_is_pending_then_spawns_never_failed (render) — headless app with the chain apply_terrain_assignments → update_terrain_lod → fetch_and_spawn_terrain_chunks over
- fe-terrain/tests/lazy_tiles_test.rs::bevy_flows::tile_bytes_leave_with_the_exit_ring_and_return_on_reentry (render) — one pin per live chunk. Flying about 40 tiles away despawns the ring, after which pinned_len and resid
- fe-terrain/src/tiles/composite.rs::only_the_off_thread_loader_reads_the_disk_cache_and_never_in_offline_mode — table over Offline, Hybrid and Online. Pins NFR-4 (Offline never reads the disk cache) and NFR-2 (get_tile_sy
- fe-terrain/src/tiles/residency.rs — two scripted table tests of the pure ByteLru logic: eviction takes the least-recent unpinned entry and never a pinned one; release only drops bytes when the last pin goes; release_all_
- fe-format/src/archive_index.rs — header_phase_then_member_reads_match_the_full_import (index reads equal the eager import, an absent member returns None) and a_replaced_file_is_reported_as_changed_not_misread (typed Arch
- fe-terrain/src/tiles/hexon_source.rs::parse_tile_key_table — pure parser table: valid keys, too few or too many parts, non-numeric parts, u8 zoom overflow.
- fe-api/tests/hexon_tileset_test.rs::startup_indexed_tiles_stream_lazily_and_a_reinstall_serves_new_bytes — a tileset installed before startup is only indexed by load_all (0 resident bytes). The real authed REST routes se

**S6 (`b9a85f3`) — tests written (8):**
- fe-database/src/handlers/petal_render_distance.rs: set_then_load_hierarchy_carries_the_value, clearing_restores_inherit, a_viewer_cannot_set_it, out_of_range_values_are_refused (table: NaN/-1/50001), an_editor_authored_r
- fe-database/src/handlers/petal_terrain.rs: set_petal_terrain_replicates_and_a_peer_converges_on_it (real repl_tx capture + ApplyReplicatedRow on a second DB), an_inbound_petal_terrain_that_fails_the_shape_check_is_denied
- fe-database/tests/petal_render_distance_test.rs (new, drives the real spawn_db_thread_with_sync_and_lifecycle dispatch loop): set_then_load_hierarchy_carries_the_value, clearing_restores_inherit, a_viewer_cannot_set_it, 
- fe-api/tests/petal_render_distance_api_test.rs (new, full router + real reply builders against Mem SurrealDB): rest_put_persists_render_distance_that_get_reads_back, rest_delete_clears_to_inherit, mcp_set_petal_render_di
- fe-ui/src/settings.rs: effective_render_distance_prefers_session_over_petal_over_global (table-driven, replaces the old 2-arg test).
- fe-ui/src/verse_manager/spawn.rs: session_override_beats_petal_column_beats_global_and_clears_on_nav_change (replaces the old manifest-save-based test since manifest_save no longer writes the session override).
- fe-ui/src/actions/hexon.rs: manifest_save_persists_to_the_petal_column (asserts the exact DbCommand sent on the channel, the named flow test from the campaign plan).
- fe-database/src/handlers/crud.rs: importing_a_ktx2_model_is_refused_and_stores_nothing (AC-13 accountability directive: real GLB bytes with an image/ktx2 mimeType, through import_gltf_handler, asserting zero blobs/asset 

**S7 (`0692e14`) — tests written (5):**
- fe-terrain/src/tiles/streamed.rs::tests::bundle_for_and_covers_resolve_through_the_index — pure, no render feature needed
- fe-terrain/src/tiles/streamed.rs::tests::pin_and_release_evict_a_bundle_only_once_its_last_pin_drops — pure refcount test
- fe-terrain/src/tiles/streamed.rs::tests::a_petal_on_an_uninstalled_tileset_demands_only_ring_bundles (render-gated; plan's named S7 flow test) — demand set is a strict subset of the index's bundles
- fe-terrain/src/tiles/streamed.rs::tests::moving_the_camera_demands_new_bundles_and_releases_old_ones (render-gated; plan's named S7 flow test) — demands the new bundle and evicts the old one from memory on release
- fractalengine/src/terrain_bridge.rs::tests::answered_bundles_become_terrain_chunks (plan's named S7 flow test, adapted) — real crossbeam channel + background-thread decode of a real S1 bundle file; proves the decoded byt

**S8 (`a8a50c1`) — tests written (7):**
- fe-ui residency_ledger.rs (pure, table-driven): admission_is_ranked_nearest_first, forward_weighting_pulls_in_what_is_ahead_only, enter_exit_band_keeps_only_previously_admitted, cap_saturation_keeps_the_nearest_and_insid
- fe-ui residency_ledger.rs (Bevy App over the real ledger → respawn → apply_node_residency → materialize_cached_primitives → materialize_path_assets chain, MinimalPlugins + AssetPlugin, orbit camera moved per step): movin
- fe-ui path_asset_materialize.rs: stamps_to_keep_truncates_the_farthest_and_keeps_stamp_order, pick_stamp_maps_a_truncated_pick_back_to_the_stamp_index, plus a residency-bit assertion in applied_gate_is_per_track_and_inpu
- fe-ui right_sidebar.rs: residency_readout_line_reports_every_counter
- fe-runtime diag15m.rs: pressure_is_the_largest_binding_not_the_sum, render_pressure_clones_share_one_cell
- fractalengine terrain_bridge.rs: tile_residency_mirror_clears_a_stale_readout; the existing S7 answered_bundles_become_terrain_chunks now also asserts ResidencyStats {tile_bundles: 1, tile_bytes ≥ 8} via the real mirror 
- fe-terrain streamed.rs: the existing moving_the_camera_demands_new_bundles_and_releases_old_ones now asserts streamed_resident_bundles/bytes go 1/>0 → 0/0 on exit-ring release

**S8b security review-and-fix (`4a61b19`) — must cover:**
- fe-sync lib: a_relayed_ad_cannot_replace_an_authenticated_root, tileset_ads_are_verse_bound_validated_and_capped, provider_map_prefers_authenticated_roots_and_ignores_relayed_claims_about_them, resolve_providers_prefers_explicit_override_and_dedupes, a_provide
- fe-sync tests/tileset_transfer_test.rs (whole file, including the rewritten an_interrupted_download_resumes_after_restart_without_refetching and the iroh loopback over-cap assert in iroh_transport_round_trips_a_hash_seq_on_loopback)
- fe-runtime: safe_id::safe_path_id_table, blob_transport::an_oversized_blob_is_refused_and_never_stored plus the existing blob_transport tests (signature change)
- fe-format: a_smaller_import_budget_refuses_what_the_default_accepts
- fe-terrain: tests/tileset_bundles_test.rs (new a_peer_index_naming_another_tileset_never_overwrites_it; updated assemble_bundles call sites), store::device_and_traversing_ids_never_reach_the_filesystem, streamed::a_streamed_index_must_name_the_requested_tilese
- fe-database: petal_terrain tests (extended inbound deny table, a_local_terrain_write_peers_would_deny_is_refused), petal_render_distance tests unchanged
- fractalengine bin tests: a_download_naming_another_tileset_never_overwrites_it, a_failed_streamed_bundle_is_not_re_requested_every_frame, streamed_retry_delay_grows_and_caps, a_completed_download_is_assembled_and_installed, answered_bundles_become_terrain_chun
- fe-api terrain_mutation_test / hexon_tileset_test / mcp_integration (the local SetPetalTerrain writer now refuses terrain that fails the shape check)
- fe-test-harness scenario 16 two_peer_tileset_stream (assemble now bound to TILESET_ID; resume needs the full-download marker that DownloadTileset writes)

**Final review-and-fix (`069f8ea`) — must cover:**
- cargo test -p fe-ui --lib verse_manager::residency_ledger (whole module, especially a_node_straight_ahead_keeps_a_hysteresis_band, rotating_in_place_never_evicts_inside_r, moving_camera_streams_nodes_in_and_out_with_hysteresis, enter_exit_band_keeps_only_previ
- cargo test -p fe-ui --lib terrain_map::events (a_failed_download_shows_failed_not_downloading, closing_the_hexon_manager_does_not_stall_a_download)
- cargo test -p fe-sync --lib status::tests::undrained_tileset_buffers_stay_bounded
- cargo test -p fe-sync --lib sync_thread::tests (the_startup_pass_reports_downloads_and_hands_back_completed_ones, a_relayed_ad_cannot_replace_an_authenticated_root, tileset_ads_are_verse_bound_validated_and_capped, offline_thread_answers_tileset_commands_hones
- cargo test -p fe-sync --test tileset_transfer_test (whole file, including the new a_download_completed_before_a_restart_is_handed_back_for_install, a_tag_rebuild_never_drops_a_tileset_published_meanwhile, and the iroh loopback catalog_from_tags test)
- cargo test -p fe-terrain --lib tiles::streamed (a_bundle_no_chunk_ever_pinned_is_evicted_once_idle, plus the render-gated demand tests: run with --features render or through the workspace)
- cargo test -p fractalengine --bin fractalengine terrain_bridge (a_bundle_that_fails_to_decode_is_not_refetched_every_frame, streamed_bundle_requests_stay_bounded_in_flight, only_a_requested_meta_reply_becomes_a_streamed_source, an_installed_download_completes_
- cargo test -p fractalengine-relay (a_headless_relay_publishes_its_seeding_tilesets_at_startup; the relay app now chains discard_tileset_events)
- fe-test-harness scenario 16 two_peer_tileset_stream (the startup pass now also emits TilesetDownloadComplete after Resumed{n/n}; the scenario must still pass on Resumed)
- Plus every item in the S1-S8b must-cover lists from the prior slice reports (unchanged by this commit)




---

## Signoff checklist

- [ ] All core suite steps executed and passing
- [ ] W0/W0b/W0c/W0d must-cover lists reconciled and verified
- [ ] W1 PAS-P3P4 must-cover list appended and verified
- [ ] Zero test failures across all scopes
- [ ] No regressions in e2e scripts
- [ ] Packaged binaries (GUI + relay) launch and connect cleanly
- [ ] Campaign final sweep complete → ready to push

