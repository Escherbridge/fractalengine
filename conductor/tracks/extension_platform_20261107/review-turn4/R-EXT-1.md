---
type: Review
track: extension_platform_20261107
gate: R-EXT-1
reviews: [EXT-08, EXT-14, EXT-15, EXT-16, EXT-17]
base: 7af0a144
---

# R-EXT-1 adversarial review (round 4)

Security inbox: no entry names a file in this gate's owns (grepped 2026-11-13).

- F1 FIXED - fractalengine/src/terrain_bridge.rs:1054 - EXT-16 added `time: Res<Time>` to `drain_hexon_assemble_results`, but two existing journeys (`a_seeding_enable_that_fails_to_persist_is_never_published` through `app_for_install_from_registry`, and `an_installed_download_completes_and_discards_its_cache`) build an App with no `Time`; Bevy 0.18 fails param validation and panics. `Time` is now initialised in the shared helper (:2858) and in the download journey (:4531); the redundant per-test init was dropped.
- F2 FIXED - fractalengine/src/window_chrome.rs:119,152 - `sync_window_title` ran `format!` every frame either manager was change-flagged (any `ResMut` deref marks it). It now writes into a reused scratch buffer (`WindowTitleBuffers`) and swaps on change; `window_title` stays the pure, tested wrapper.
- F3 FIXED - tools/qa/docs_link_check.py:23 - every backticked `a/b.ext` was treated as a repo path, so archive members (`plugin/extension.json`, 15+ lines), crate-relative names (fe-database `src/AGENTS.md`) and `fixtures/basic.json` would fail the current docs. A backticked path now counts only when its first segment is a real top-level repo directory (or `.`/`..`); markdown link targets stay strict.
- F4 FIXED - tools/qa/docs_link_check.py:68 - link text was re-scanned as a backticked path (abbreviated labels gave false failures); only the link target is checked now.
- F5 FIXED - tools/qa/docs_link_check.py:86 - a shorter inner fence closed a longer outer fence; closing now follows CommonMark (same character, at least as long, no info string).
- F6 HANDED-OFF (I-BIN-1) - fractalengine/src/main.rs - `window_chrome::sync_window_title` is not registered yet (pinned seam, plan §4); add `app.add_systems(Update, window_chrome::sync_window_title)` next to `set_window_icon` (main.rs:839).
- F7 HANDED-OFF (I-BATCH-1) - conductor/test-batches.md - Misc needs the non-cargo line `python tools/qa/docs_link_check.py --root .`; the `window_chrome::` (R6) and `terrain_bridge::` (R4) filters already reach EXT-14/EXT-16 tests.
- F8 HANDED-OFF (I-DOC-API, I-DOC-CORE) - AGENTS lines still missing at HEAD: fractalengine/src/AGENTS.md §window-chrome (FE_PEER_LABEL, title on change) and §tileset-p2p (LocalInstallFailed notice), tools/qa/AGENTS.md §docs-link-check (incl. the top-level-directory rule from F3), tools/qa/sim/AGENTS.md scenario list (fleet-terrain), fe-terrain/src/AGENTS.md ruler (Background order, QA-0176).
- F9 HANDED-OFF (docs owner of docs/extensions, I-DOC-API) - docs/extensions/extensions-ui.md:3 - public doc embeds a personal absolute home-directory path to the UI guidance file; replace with a neutral reference (the guidance is not in the repo).
- F10 KEPT (verified) - fe-terrain/src/ruler_plugin.rs:222,252 - Background order is correct in egui 0.33.3: the background area is registered in `area_order` each pass (context.rs:498), and `GraphicLayers::drain` (layers.rs:219-264) paints non-area layers of an order after the area-ordered ones, so the ruler draws above the central panel fill and below every Middle window.
- F11 KEPT (local-only UI) - fractalengine/src/terrain_bridge.rs:1090 - on a disk error the notice reason can carry the store's temp path (store.rs write_synced_and_rename context); the subject is the file name only as required, and the reason is the user's own machine state needed to diagnose.
- F12 KEPT (verified) - fe-terrain/src/tiles/store.rs:196 - QA-0173 evidence shows `HexonArchive::import` passes a legacy `terrain` manifest through to this match, so the readable message is reached; other types keep the Debug name.
- F13 KEPT (verified byte-identical) - tools/qa/sim/scenarios/fleet-terrain.json - inline base64 GPX equals tools/qa/sim/assets/roads/road-{a,b,c}.gpx; route names resolve via fleet-sim `ids_by_name` to the track nodes, `gpx_points` is `ROUTE_GEOMETRY_KEY`, `loop` maps to `MoverSpec.looped`. HANDED-OFF (CLOCK-12): a multipart GPX driver op so the files become the single source.
- F14 DEFERRED (trigger: prober gains a terrain-height probe and a range predicate) - tools/qa/sim/scenarios/fleet-terrain.json - tileset-bounds and Y-within-2 m are not asserted; agreement on every peer is (gap stated in the description).
- F15 DEFERRED (trigger: docs adopt reference-style or angle-bracket links) - tools/qa/docs_link_check.py - `[x]: path` and `<path>` targets are not checked.

Verdict: PASS-WITH-FIXES
