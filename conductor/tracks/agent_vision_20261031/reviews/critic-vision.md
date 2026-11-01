---
type: Review
title: Agent vision critic review — scope confinement of the whole view, capture-vs-view streaming budget, abandoned callers, audit
track: agent_vision_20261031
role: critic
run: vision
reviewed_commit: b5657c4
created: 2026-11-01
---

# Agent vision critic review (run "vision")

Lens: refute, don't confirm. Security-review depth on the default-on capture
surface (plan §CRITIC, spec §7, §SEC-ADDENDUM). The question was: can a token
scoped to verse B get verse A pixels or camera state, through any door
(REST GET/POST, MCP `screenshot`/`get_camera`), any pose (`focus`, `navigate`,
`geo`/`tile`), or any timing (a verse switch mid-capture, the settle wait, the
restore paths)? Fixes are applied in this pass, not proposed.

Provenance: an earlier critic attempt on this run was interrupted after it had
edited the tree and before it ran a gate or committed. Its uncommitted edits
(findings C1-C4) were read line by line, checked against the code they claim
to fix, gated, and adopted here. C5-C7 are new in this pass.

## Findings

| # | Sev | Finding | Disposition |
|---|-----|---------|-------------|
| C1 | HIGH | The scope gate checked only the nav cursor (`scope_covers_active_verse`). But the terrain binding outlives a verse switch: `navigate_to_verse` clears the petal, `load_petal_terrain_on_nav_change` sends no load for `None`, so `ActivePetalTerrain` / `ActiveTileSource` stay on the previous verse's petal. That verse's map, GPX tracks and overlays stay on screen until a petal of the new verse that has its own terrain is opened. Also, nodes despawn one frame late (deferred commands). Result: a verse-A Owner token, just after the user moved from C to A, could capture C's map (window or scene), pose a scene camera anywhere over it (which streams more of C), and read C's projection origin through `GET /camera`. | **Fixed.** `vision_bridge.rs` `ViewScope` (a `SystemParam`; the pure rule is `scope_covers_view`) is now the one gate. The scope must cover the active verse **and** every petal still rendered: the terrain binding while it has a config, and every `SpawnedNodeMarker`'s petal. An unknown petal denies. It applies at arrival and at frame time, on both bridges (`drain_vision_requests`, `advance_vision_job`, `drain_screenshot_requests`, `capture_pending_screenshots`). Tests: `a_view_still_rendering_an_uncovered_verse_is_denied` and `a_map_still_bound_from_an_uncovered_verse_refuses_plain_captures`. Both go red on the old gate. |
| C2 | MEDIUM | V4 streamed the capture focus **first**, against a shared total cap (`max_chunks + allowance`) and a shared per-frame spawn cap. A capture holding chunks pushed the total over `max_chunks`, and that paused the user view's ring fill. An agent looping scene captures could therefore stall the user's terrain while they moved. | **Fixed.** `terrain_plugin.rs` streams in `fetch_order` (the user's view first) and keeps separate budgets. A chunk belongs to the capture iff its anchor is nearest the capture focus. The view keeps `max_chunks`, the capture keeps its allowance, and the total stays bounded by `max_chunks + allowance`. Test: `a_capture_never_spends_the_user_views_chunk_budget` (red on the old shared cap). |
| C3 | MEDIUM | Abandoned callers. When a client disconnects or its reply timeout fires, the handler future drops and fe-api's single-flight permit is released, but the host still started or kept the job. That held the user's camera lock (up to 24 s) for a reply nobody reads. The next caller passed the API slot and met a host-side `capture_in_progress`. | **Fixed.** `reply_tx.is_closed()` requests are dropped at drain on both bridges. A running job whose caller left is torn down the next frame (`caller_gone`, which restores through `finish_job`). A plain request that was admitted and then held across a posed job's lock is dropped before capture (new this pass: `capture_pending_screenshots` filter). Tests: `an_abandoned_capture_never_holds_the_users_camera` and `an_abandoned_plain_request_takes_no_frame` (now also covers the held-across-lock case). |
| C4 | MEDIUM | `NavSnapshot` restore on failure, timeout or `restore: true` ran unconditionally. Navigation is not locked during a job, so a user who moved elsewhere mid-job (even to another verse) was yanked back to the pre-job petal. | **Fixed.** `NavSnapshot.navigated_to`: the snapshot is put back only while the cursor is still on the petal the job navigated to. Test: `a_user_who_navigated_away_mid_job_is_not_yanked_back`. |
| C5 | LOW | There was no audit trail for default-on captures (plan §CRITIC asks for an INFO line with DID, mode and pose kind). | **Fixed.** `fe-api` `vision::audit_capture` writes one INFO line (`did`, `scope`, `mode`, `pose`) from the shared cores `request_capture(state, &claims)` and `run_capture(state, &claims, spec)`, so neither door can skip it. fe-api AGENTS.md §screenshot "Audit". |
| C6 | LOW | `tile_out_of_range` named the field `pose.tile.x` for an out-of-range `y`. | **Fixed.** x and y are checked separately, each names its own field and value. The validation table gained a `pose.tile.y` row. |
| C7 | LOW | Stale docs: `SCREENSHOT_ALLOW_ENV`, `screenshot_allowed_from_env` and the module header still said "only `1` enables" / "deny-by-default". | **Fixed** (doc-only). |
| C8 | LOW | `409 capture_in_progress` is answered by fe-api's slot BEFORE the host's scope check. Any Owner token can learn that *some* posed capture is in flight. That leaks activity only: no pixels, no existence. | **Accepted.** Moving the slot behind a host round trip would cost a bridge hop per refusal for no pixel-level gain. |
| C9 | LOW | Scene captures are invisible on screen (by design). | **Deferred.** A per-capture toast would appear inside the very window captures an agent uses for UI QA (the user's stated loop). C5's audit line is the mitigation. Revisit if multi-tenant hosting lands. |
| C10 | INFO | A window capture includes UI chrome that lists other verses (UI-ADDENDUM residual). | **Accepted**, documented in fe-api §screenshot (use `include_ui: false` or `=0` on a shared node). |

## Refuted (checked, holds)

- **Off-host exposure.** The GUI API binds `127.0.0.1:8765` (`main.rs`). The relay wires `ScreenshotSurface::disabled()`, and every test or headless `ApiState` uses `disabled()`.
- **Opt-out.** `FE_API_SCREENSHOT_ALLOW=0` sets `allowed = false`, which closes REST GET/POST/camera (gate 1), the MCP dispatcher (`operator_opt_in_refusal`) and the MCP handler (`check_opted_in`). Garbage values fail closed (`per_gate_default_decides_the_unset_and_garbage_cases`).
- **Typed Owner floor.** REST uses `require_role_level(.., RoleLevel::Owner)`. MCP rows are `min_role: Owner` on both tools.
- **Rate limit per DID.** REST and MCP share the `screenshot:<did>` key (1/s) and the `camera:<did>` key (10/s).
- **Existence oracle.** A foreign target and a nonexistent `focus` target both resolve in `authorized_target`, in the same frame, to the same `denied()`. REST answers 404 with the same text, and MCP gives the same bare text (`coded_message` never prefixes the denial). A covered target in another verse gives a 422 only to a caller who covers that verse.
- **Cross-verse `navigate`.** It is refused (`cross_verse_navigate_refused`), and `navigate_to_petal` re-checks the verse.
- **NaN / overflow.** Every request float passes through `ValidCaptureSpec` (finite and range-checked). f64→f32 overflow is caught by `finite_vec3`, and an infinite yaw (an unbounded finite yaw cast to f32) is caught by `pose_transform`'s finiteness check (`degenerate_camera`). Nothing non-finite reaches `Transform` or `looking_at`.
- **Resource safety.** Every job exit goes through `finish_job`. It releases the lock, restores the camera and navigation, despawns the capture camera (and with it its `StreamingFocus`), removes the image, and despawns a pending `Screenshot` on failure. The 24 s job cap sits under fe-api's 25 s reply timeout.
- **Size DoS.** Size is limited to 64..=4096 and to scene mode only. The global single-flight slot plus 1/s per DID bounds GPU and readback memory to one 64 MB target at a time.
- **Second-camera regressions.** The workspace grep found every user-camera query narrowed to `OrbitCameraController`. The one remaining `Query<(&Camera, &GlobalTransform), Changed<..>>` (`fe-webview` `position_overlay_system`) is a no-op TODO whose `.single()` simply skips.

Not in this run: the input-injection surface (I1-I5) is not implemented yet, so plan §CRITIC item 2 is open for the run that lands it.

## Gate (this pass, all exit 0, foreground)

- `cargo check -j4 -p fe-runtime -p fe-api -p fe-renderer -p fe-terrain -p fe-ui -p fractalengine --tests`
- `cargo +1.99.0 clippy -j4` on the same crates, `--all-targets -- -D warnings`
- `cargo test -j4 --no-fail-fast -p fe-runtime -p fe-api -p fe-renderer -p fractalengine`: 676 passed, 0 failed, 1 ignored. This includes `vision_scope_test.rs` (3), `vision_test.rs` (9), `screenshot_test.rs` (4) and the fractalengine bin (124).
- `cargo test -j4 -p fe-terrain --features render --lib`: 339 passed (camera_framing 9, multi_focus 8 included).

VERDICT: PASS-WITH-FIXES (7)
