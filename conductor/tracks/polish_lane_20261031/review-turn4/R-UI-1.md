---
type: Review
track: polish_lane_20261031
gate: R-UI-1
reviewed: [UI-01, UI-02, UI-10, UI-11]
base: 7af0a144
date: 2026-11-13
---

# R-UI-1 adversarial review (group UI)

SEC-INBOX: no entry in `.omc/state/turn4-security-inbox.md` names a file this gate owns; nothing to close.

F1 FIXED - fractalengine/src/screenshot_bridge.rs:182 - UI-11 only deferred F12 when the API queue was already full; `capture_screenshot_on_f12` and `drain_screenshot_requests` are unordered in `Update`, so F12 first + API drained later still spawned two `Screenshot`s that frame. `capture_pending_screenshots` now holds the queue while a `Screenshot` was added this frame (`Added<Screenshot>, Without<Capturing>`), so both orders give one capture per frame; journey `f12_waits_a_frame_while_an_api_capture_is_pending` extended with the opposite order.
F2 FIXED - fe-ui/src/panels/volume_report_panel.rs:763 - WrongPetal copy "This selection belongs to another petal. Select again here." was false: the panel only accepts replies for the petal it is on, so WrongPetal means fe-terrain has not bound this petal's map yet. Copy is now "Waiting for this petal's map to load."; the panel already re-requests on `PetalTerrainLoaded`, which is the same message that binds the map (assignment applies before the answer in the chained `Update`).
F3 KEPT (clearing would loop) - fe-ui/src/panels/volume_report_panel.rs:743 - UI-01 task said WrongPetal should `VolumeReportState::clear`; clearing makes `sync_volume_report_request` re-send next frame and ping-pong every frame while the map is unbound, so showing a waiting state is the correct reading of the requirement.
F4 FIXED - fe-ui/src/panels/volume_report_panel.rs:750 - NoMap sentence lacked its final period.
F5 FIXED - fe-ui/src/ui_shell/right_sidebar.rs:117 - doc comment named a non-existent `sync_heatmap_overlay_toggle`; the mirror runs in `update_residency_readout`.
F6 KEPT (no hot-path cost) - fe-ui/src/ui_shell/right_sidebar.rs:150 - UI-02 mirrors the checkbox through `RightSidebarState.heatmap_overlay` inside `update_residency_readout` instead of threading `ResMut<HeatmapOverlayToggle>` into the render bundle; it is allocation-free, writes only on change and avoids the 16-param limit. One-frame latency is invisible.
F7 DEFERRED (fe-ui gets a read of layer visibility) - fe-ui/src/panels/terrain_tools_panel.rs:275 - "show only while the ProposalOverlay layer is visible" is not met: the checkbox shows whenever proposals exist. Layer state lives in fe-terrain `LayerStack`, which fe-ui cannot read (NFR-5); needs a fe-renderer seam mirror.
F8 KEPT (journey already exists) - fe-terrain/src/terrain_plugin.rs:3355 - UI-02 added no test; the existing heatmap journeys already flip `HeatmapOverlayToggle` through the resource (:3355, :3635) and the type move is a re-export, so the paths they test are unchanged.
F9 FIXED - fe-ui/src/ui_shell/right_sidebar.rs:930 - UI-10 residue: "Serve terrain only from installed hexons" used "hexon" as the artifact noun (ui_ux.md section 9); now "installed maps". Registry hover example host changed to maps.example.com.
F10 FIXED - fe-ui/src/dialogs/hexon_manager.rs:134 - hint "package name or name@version" disagreed with the field's accessible name "Registry map ID and optional version"; now "map ID or ID@version".
F11 KEPT (scope stated in test) - fe-ui/src/ui_shell/right_sidebar.rs:3019 - `user_copy_has_no_dev_jargon` covers only the Settings section; other sections have no headless harness in this module. Mutation check holds: restoring the old :856 hover text fails both assertions.
F12 KEPT (correct order) - fe-terrain/src/terrain_plugin.rs:2542 - an unbound petal (`active.petal_id == None`) answers WrongPetal, not NoMap; with F2's copy that reads as "waiting", which is the true state before the first assignment.
F13 KEPT (no spam) - fe-terrain/src/terrain_plugin.rs:2549 - unavailable replies are written once per incoming request and never retained; the extended `request_for_inactive_petal_is_dropped` proves no per-frame repeat. Allocation only on a request.
F14 KEPT (bounded) - fractalengine/src/screenshot_bridge.rs:268 - a continuous stream of API captures every frame could postpone a held F12 indefinitely; agents poll at most every few seconds, so starvation is theoretical.
F15 KEPT - leak discipline: no forbidden term in any added line of the reviewed diffs or these fixes; no clippy, no cargo, no swallowed sends (F12 `try_send` still warns).

Verdict: PASS-WITH-FIXES
