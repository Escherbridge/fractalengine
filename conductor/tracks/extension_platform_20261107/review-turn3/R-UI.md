---
type: review
track: extension_platform_20261107
gate: R-UI
turn: 3
reviews: [S-UI, I-SLOTS, I-UI]
date: 2026-11-13
---

# R-UI - turn 3 UI gate (store flows, names, states, slots, inspector writes)

Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md` and `docs/extensions/extensions-ui.md`. No cargo run (code-first turn).
The GUI QA path for refine is `fe-ui/src/extensions/AGENTS.md` section gui-qa-path.

## Verdicts

- R-UI3-01 FIXED - `extensions/mod.rs` `draw_context_slot` pre-filtered context entries with an `id`/`name`-only node, so every property-keyed `when` (fleet-sim `start-this`, `when: fleet.config`) was dropped before `draw_context_menu_items` layered the cache; it now filters with `slots::with_cached_properties`.
- R-UI3-02 FIXED - `ExtensionSlotState::set_node_properties` had no caller anywhere, so the I-SLOTS property cache was always empty; new `mirror_inspected_node_properties` (PostUpdate, `ExtensionSlotsPlugin`) files the inspector's loaded bag under the primary selection.
- R-UI3-03 FIXED - a refused detail document (`BrowseEvent::Failed` from `load_detail`) cleared the Browse results and Details showed a reason-less `Details are unavailable`; `BrowseState::detail_requested`/`detail_error` route it to Details (`Details are unavailable: <reason>`), results stay; J9 extended.
- R-UI3-04 FIXED - Browse showed the empty state `No extension sources are configured` while the first `Refresh` was in flight; it now reads `Loading extensions` until the sources arrive.
- R-UI3-05 FIXED - a licensed Browse row read `Licensed`, the same word Details uses for a held licence; the row now reads `Licence required`.
- R-UI3-06 KEPT - steer: `actions/asset.rs::apply_overrides` already submits `CommitStampOverrides` through `submit_scene_write` (asset.rs:397-401); no change needed.
- R-UI3-07 HANDED-OFF (verse_manager owner / R-Z) - steer: `verse_manager/db_results/nodes.rs:66-84` sends the earthwork contract `SetNodeProperty` keys straight to `db_sender.0`; not in R-UI's partition, NEEDS line filed.
- R-UI3-08 KEPT - inspector writes are gated: outside tests `panels/` sends only API-token, field-definition and read commands (inspector.rs:203,1463,1502,2359-2433); scene edits queue `UiAction`s handled through `submit_scene_write`; `transform_broadcast.rs` submits through `SceneWriteDoor` itself, no proxy gate left.
- R-UI3-09 HANDED-OFF (I-BIN / R-BIN) - `UiManager::portal_bundle_nodes` has no feeder in `fractalengine/src`, so a resolved `portal.open` binding always shows `bundle not on this peer` and the inspector never offers `Open bound bundle`.
- R-UI3-10 HANDED-OFF (R-POR / I-MAIN) - no production code writes `BrowserCommand::OpenBoundBundle`; the inspector offer mounts the repository node via `OpenBundle`. Confirm that is the intended bound-open path or add the routing.
- R-UI3-11 HANDED-OFF (D-EXT) - `docs/extensions/extensions-ui.md`: names list lacks `Back to extension results`, `Load more extensions`, `Reviewed <id>`; Store flows step 2 says `Get licence` requests the token, but it opens the licence page and `Check licence` requests and verifies the token.
- R-UI3-12 KEPT - Browse draws no inline line in Idle/Loading: the panel spinner row already names the task (`Searching...`, `Loading more...`); a second line would repeat it.
- R-UI3-13 KEPT - `Get licence` is absent (not disabled) when the registry lists no licence page: there is no action to disable; `Check licence` stays.
- R-UI3-14 KEPT - a kill-listed package keeps its `Reviewed` badge: review is a fact about the bytes; the red `Unavailable` line and the absence of every action carry the state.
- R-UI3-15 KEPT - stable names verified against extensions-ui.md: Installed, Browse, Store details, Details, Permissions, consent (`Enable|Update <id>`, `Cancel extension consent`, `Override kill list for <id>`) and bindings names match the doc and J9.
- R-UI3-16 DEFERRED - the context-menu property cache covers only nodes inspected at least once (max 256). Trigger: a package whose `when` must match never-inspected nodes; then feed it from every `NodePropertiesLoaded` in the active petal (verse_manager).
- R-UI3-17 DEFERRED - no journey drives `draw_context_slot` or the mirror system (both need the lent egui host + a Bevy App). Trigger: refine pass; extend the slot journey with an App-level leg if the R11 batch gains an App harness.
