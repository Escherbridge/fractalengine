---
type: plan
track: agent_vision_20261031
title: "Implementation plan: agent vision (default-on + dynamic capture + camera read-back) with agent_input_surface folded in"
created: 2026-10-31
resource: ./spec.md
computed_at_commit: 7ea181b
partitions_confidence: planned
---

# Implementation plan: agent_vision_20261031

> Citations computed at `7ea181b`. Partitions are `planned`. Before each slice, re-grep every "owns" path and symbol, because polish wave 1 is still editing fe-ui/fe-runtime/fe-database at this commit.

## 0. Sequencing (fixed)

- **When:** this program runs right after `polish_lane_20261031` **wave 1 closes** (W1-REGATE green) and **before polish wave 2** starts. Polish W1-S2 owns `fe-runtime/src/AGENTS.md`, so V1 may not start until W1 closes.
- **Order inside:** V1 → V2 → G1 → V3 → V4 → G2 → V5 → G3 → I1 → I2 → I3 → {I4 ∥ I5} → G4 → ADVOCATE → CRITIC → CLOSE → sweep. Every code slice is sequential, except I4 ∥ I5, which carries over the AIS plan's disjoint pair.
- **Cross-track order on shared files:** this track first, then `relay_operability_20261031`, then `plugin_host_revival_20261031`; `peer_networking_support_20261031` P14 bumps `TOOLS.len()` from 39. See §6.

## 1. Doctrine for every slice

- **Agents sketch, the orchestrator inks.** No slice runs cargo. The orchestrator runs one serial gate at G1-G4 (`-p` the crates touched since the last gate) and the full workspace sweep once at the end (`CARGO_PROFILE_DEV_DEBUG=0`, clippy `-D warnings`).
- **The gate is data.** Only V2 edits `operator_gate.rs` (adding `default_on`). After that, every surface adds a `const` (`CAMERA_STATE_GATE` in V2, `INPUT_GATE` in I2 with `default_on: false`) and never forks the chain.
- **Validate once.** `ValidCaptureSpec` is constructible only through `CaptureSpec::validate()`. Bevy-side code takes `ValidCaptureSpec` and never re-checks finiteness, so a NaN can never cross the bridge.
- **One capture helper.** `screenshot_bridge.rs` keeps sole ownership of the capture path. V5 generalizes it to `capture_png(commands, Screenshot, on_encoded)`, and I4 calls it.
- **Tests model real flows** (CLAUDE.md "Tests model real flows"):
  - REST/MCP go through `ApiHarness::spawn_with_screenshot` with a fake vision responder thread (renderer-free, the `screenshot_test.rs` precedent).
  - The math gets pure tables.
  - Bevy routing gets `World`/`run_system_once` tests (the `drain_once_with_window` precedent).
  - No tautologies: every assertion goes red if its branch inverts.
- **Docs:** terse doc-comments in code; the rationale goes in the directory `AGENTS.md` section each slice names.
- **No read-only reviewers:** ADVOCATE and CRITIC fix what they find, then record a verdict.

## 2. Folded agent_input_surface: amendments (its plan stays authoritative otherwise)

`conductor/tracks/agent_input_surface_20261031/plan.md` S1-S5 run here as **I1-I5**, unchanged except:

| AIS slice | Amendment (because V1-V5 landed first) |
|---|---|
| S1 → **I1** | None. `fe-runtime/src/input.rs` mirrors `screenshot.rs`; `fe-runtime/src/AGENTS.md` §input-bridge goes beside V1's §vision-bridge. |
| S2 → **I2** | `INPUT_GATE` sets **`default_on: false`** (the field V2 added). AIS tripwire "S2 must not edit `operator_gate.rs`" still holds. The `enabled_warning` must state that input + default-on screenshots = full GUI control by any Owner token. |
| S3 → **I3** | `TOOLS.len()` goes **38 → 39** (V2 took 37 → 38 for `get_camera`). The assert message appends "agent_input_surface added `inject_input`". `ToolSeam::Input` goes beside `ToolSeam::Screenshot`. |
| S4 → **I4** | `capture_after` calls V5's `capture_png(commands, Screenshot::primary_window(), …)`; no `pub(crate)` extraction is needed because V5 already made it crate-visible. `drain_input_requests` must **refuse** (reply error `camera_locked_by_capture`) while `CameraCaptureLock` exists, so synthetic input cannot fight a held pose. Wiring goes beside V5's vision wiring in `main.rs`. |
| S5 → **I5** | Add `inject_input_refused_while_a_posed_capture_holds_the_camera` (fake responder replies with that error and the API maps it to 409). Otherwise as written. |
| S6 | Absorbed into ADVOCATE + CRITIC below (the mutation-risk lens is kept as the CRITIC headline). |
| S7 | Absorbed into CLOSE. |

## 3. Slice table

| # | Slice | Model | Owns (exact files) | Depends |
|---|---|---|---|---|
| 1 | **V1**: vision wire contract | sonnet | `fe-runtime/src/vision.rs` (new), `fe-runtime/src/lib.rs`, `fe-runtime/src/AGENTS.md` (§vision-bridge) | polish W1 closed |
| 2 | **V2**: default-on gate + REST POST/`/camera` + MCP | sonnet | `fe-api/src/operator_gate.rs`, `fe-api/src/lib.rs`, `fe-api/src/screenshot.rs`, `fe-api/src/vision.rs` (new), `fe-api/src/server.rs`, `fe-api/src/mcp/mod.rs`, `fe-api/src/mcp/tools.rs`, `fe-api/AGENTS.md` (§operator-gate, §screenshot, §camera-state), `fe-api/tests/vision_test.rs` (new) | V1 |
| G1 | gate: `-p fe-runtime -p fe-api` build + tests | orchestrator | n/a | V2 |
| 3 | **V3**: pose math + camera lock + markers | sonnet | `fe-renderer/src/camera.rs`, `fe-renderer/src/AGENTS.md` (§camera-capture), `fe-terrain/src/camera_framing.rs` (new), `fe-terrain/src/lib.rs`, `fe-terrain/src/AGENTS.md` (§camera-framing) | G1 |
| 4 | **V4**: multi-focus terrain streaming + main-camera query narrowing | opus | `fe-terrain/src/terrain_plugin.rs`, `fe-terrain/src/lod_ring.rs`, `fe-terrain/src/ruler_plugin.rs`, `fe-terrain/src/splat/render.rs`, `fe-renderer/src/grid.rs`, `fe-renderer/src/axis_gizmo.rs`, `fe-terrain/src/AGENTS.md` (§terrain_plugin, §tile-residency) | V3 |
| G2 | gate: `-p fe-renderer -p fe-terrain --features render` build + tests | orchestrator | n/a | V4 |
| 5 | **V5**: Bevy capture orchestrator + wiring | opus | `fractalengine/src/vision_bridge.rs` (new), `fractalengine/src/screenshot_bridge.rs`, `fractalengine/src/main.rs`, `fractalengine/src/AGENTS.md` (§screenshot, §vision), `fe-ui/src/gimbal.rs` (only to expose a `pub fn subtree_world_aabb` beside `gimbal_center`; no behaviour change) | G2 |
| G3 | gate: `-p fractalengine -p fe-ui` build + tests | orchestrator | n/a | V5 |
| 6 | **I1** (= AIS S1) | sonnet | `fe-runtime/src/input.rs` (new), `fe-runtime/src/lib.rs`, `fe-runtime/src/AGENTS.md` | G3 |
| 7 | **I2** (= AIS S2, amended) | sonnet | `fe-api/src/input.rs` (new) | I1 |
| 8 | **I3** (= AIS S3, amended) | sonnet | `fe-api/src/server.rs`, `fe-api/src/lib.rs`, `fe-api/src/mcp/mod.rs`, `fe-api/AGENTS.md` | I2 |
| 9a | **I4** (= AIS S4, amended) | sonnet | `fractalengine/src/input_bridge.rs` (new), `fractalengine/src/main.rs`, `fractalengine/src/AGENTS.md` | I3 |
| 9b | **I5** (= AIS S5, amended) | sonnet | `fe-api/tests/input_test.rs` (new) | I3 |
| G4 | gate: `-p fe-runtime -p fe-api -p fractalengine` | orchestrator | n/a | I4, I5 |
| 10 | **ADVOCATE**: review-and-fix | opus | `conductor/tracks/agent_vision_20261031/reviews/advocate.md` (new) + fix-only edits to any V/I file | G4 |
| 11 | **CRITIC**: review-and-fix incl. security-review depth | opus | `conductor/tracks/agent_vision_20261031/reviews/critic.md` (new) + fix-only edits to any V/I file | ADVOCATE |
| 12 | **CLOSE**: docs consolidation | haiku | `AGENTS.md` (root pointer), `conductor/tracks.md`, this `plan.md`, `metadata.json`, `conductor/tracks/agent_input_surface_20261031/plan.md` (ticks only) | CRITIC |
| sweep | full workspace test + clippy once; in-app verify (AC-11, user-gated) | orchestrator | n/a | CLOSE |

## 4. Slices in detail

### V1: vision wire contract (sonnet)

**Steps**
1. Create `fe-runtime/src/vision.rs` with:
   - `CaptureMode { Window, Scene }`
   - `PoseSpec` (externally-tagged enum `Camera{position,target,up,fov_deg}` / `Orbit{..}` / `Geo{..}` / `Tile{z,x,y}` / `Focus{target: FocusTarget, navigate}` with `FocusTarget::{Node(String), Petal(String)}`)
   - `CaptureSpec` (serde, `deny_unknown_fields`, every optional field `Option`)
   - `ValidCaptureSpec` (private fields, defaults applied)
   - `VisionRefusal { code: &'static str, field: Option<&'static str>, message: String }`
   - `ResolvedPose`, `CameraState`, `VisionOp`, `VisionRequest`, `VisionReply`, `VisionOutput::{Capture{png, pose}, Camera(CameraState)}`, `VisionFailure { status_class: Unresolvable|Busy|NoWindow|Failed, code, message }`
   - `vision_bridge()`, `VISION_BRIDGE_CAPACITY = 4`
   - constants `SIZE_MIN=64`, `SIZE_MAX=4096`, `SETTLE_MAX=600`, `DEFAULT_SETTLE=30`, `DEFAULT_SIZE=(1280,720)`, `FOV_RANGE=1..=170`, `MERCATOR_LAT_MAX=85.0511`, `TILE_Z_MAX=24`, `ALTITUDE_RANGE=-500..=1e6`.
2. Write `CaptureSpec::validate()` implementing spec §5.2's static table, in table order (the first failure wins and that order is documented).
3. Add `VisionRequest::reply(...)` mirroring `ScreenshotRequest::reply` (a late reply is dropped at debug level).
4. Add `pub mod vision;` to `lib.rs`, and §vision-bridge to `fe-runtime/src/AGENTS.md` (types, validate-once newtype, why the bridge is separate from the screenshot bridge).

**Acceptance**
- It is pure; there are no new crate deps (serde/tokio/crossbeam already in fe-runtime; confirm in `Cargo.toml`, and if serde is absent STOP and report rather than add).
- `ValidCaptureSpec` has no public constructor besides `validate`.

**Tests** (unit, in `vision.rs`)
- `capture_spec_validation_table`: one row per §5.2 static check, plus NaN/±inf in EACH float field (position.x, target.z, up.y, fov_deg, yaw/pitch/distance, lat, lon, altitude, heading). Each row asserts its `code` and `field`.
- `two_pose_keys_or_none_is_refused`: JSON-level, through `serde_json::from_str` + `validate`.
- `defaults_applied_by_mode`: window without pose → settle 0, restore true; scene → 1280×720, settle 30.
- `vision_bridge_is_bounded_at_its_capacity`.

### V2: default-on gate + REST + MCP (sonnet)

**Steps**
1. `operator_gate.rs`:
   - Add the `default_on: bool` field.
   - Change `opt_in_value_enables(value, default_on)` per spec §5.1.
   - Make `opted_in_from_env` WARN on an enabled default-on gate with the "set `=0` to disable" text, and warn and fail closed on unrecognized values.
   - Update the existing table test into a two-column table.
2. `lib.rs`: `ELEVATED_QUERY_GATE.default_on = false`; update the `ApiConfig.screenshot` doc line ("on unless `=0`").
3. `screenshot.rs`:
   - `SCREENSHOT_GATE.default_on = true`, with `enabled_warning` rewritten for default-on.
   - `ScreenshotSurface` gains `vision: Option<VisionRequestSender>` and `capture_slot: Arc<tokio::sync::Semaphore>` (1 permit), set through `with_vision(sender)`.
   - `disabled()`/`new()` keep their signatures (no edits in `assets.rs`/`canonical_ws/router.rs`/the harness).
   - `GET` is untouched.
4. `vision.rs` (new):
   - `CAMERA_STATE_GATE` (same env, `default_on: true`, prefix `camera`, 10/s).
   - `VisionError` → status mapping: 400/403/409/422/429/503/504.
   - `request_vision(state, op) -> Result<VisionOutput, VisionError>`, mirroring `request_capture`: try_send busy/disconnected, correlation check, timeout = 5 s + 20 s settle cap.
   - `post_screenshot`: gate → body → `validate` → `try_acquire` slot → bridge. The response is PNG + `X-FE-Capture-Pose` (compact JSON, `serde_json` with ASCII escaping) + `Cache-Control: no-store`.
   - `get_camera`: JSON `ApiResponse`.
   - Both are headless-safe (`vision: None` → 503 "no window").
5. `server.rs`: `.route("/api/v1/debug/screenshot", get(..).post(..))` and `/api/v1/debug/camera`.
6. `mcp/mod.rs`:
   - Rewrite the `screenshot` row description to name the pose kinds.
   - New row `get_camera` (`Owner, Global, requires Screenshot`); `operator_opt_in_refusal` uses `CAMERA_STATE_GATE` for it.
   - `TOOLS.len()` 37 → 38 with the message appended "agent_vision added `get_camera`".
7. `mcp/tools.rs`:
   - `screenshot` handler: empty args → existing `request_capture` path (byte-identical behaviour); otherwise parse → validate → `request_vision`, returning `[image, text(pose JSON)]`.
   - `get_camera` handler.
   - `schemas::screenshot()` gains the full optional property schema; add `schemas::get_camera()`.
8. `fe-api/AGENTS.md`: rewrite §screenshot (default-on, POST, modes, pose grammar table, status table, curl/MCP examples), amend §operator-gate (per-gate default table), add §camera-state.

**Acceptance**
- GET path diff = zero lines in `get_screenshot`/`gated_capture`/`request_capture`.
- Every route refuses in gate order before validation.

**Tests**
- Unit in `operator_gate.rs`: `per_gate_default_decides_the_unset_and_garbage_cases` (spec §5.1 table, both columns).
- `fe-api/tests/vision_test.rs` (fake vision responder thread that records every `VisionRequest` it receives):
  - `posed_capture_round_trips_png_and_pose_header` (AC-3)
  - `invalid_bodies_are_400_with_their_code_and_never_reach_the_bridge`: table over 5 representative rows, including a NaN encoded as `1e999` → serde inf → `non_finite` (AC-2)
  - `second_posed_capture_while_one_is_in_flight_is_409` (the responder holds the first reply) (AC-4)
  - `host_refusal_maps_to_422_with_its_code` (`tile_outside_tileset`)
  - `camera_state_round_trips_and_does_not_spend_the_capture_budget` (a capture, then `/camera` within the same second → both 200)
  - `disabled_surface_refuses_post_and_camera_before_the_bridge`
  - `headless_host_reports_no_window_and_hides_both_mcp_rows` (AC-9)
  - `mcp_screenshot_with_pose_returns_image_and_pose_text`
  - `mcp_screenshot_without_args_still_uses_the_plain_bridge` (proves GET-equivalence over MCP)
- Existing `screenshot_test.rs` and `mcp::tests::table_rows_are_well_formed` (now 38) stay green unmodified, apart from the count.

### V3: pose math + camera lock + markers (sonnet)

**Steps**
1. `fe-renderer/src/camera.rs`:
   - `pub fn orbit_from_look_at(position: Vec3, target: Vec3) -> Option<(Vec3, f32, f32, f32)>`: the exact inverse of `compute_orbit_transform`; `None` on degenerate or non-finite input.
   - `pub fn pose_transform(position, target, up) -> Option<Transform>`: `None` when up is parallel to the view direction.
   - `#[derive(Resource)] pub struct CameraCaptureLock;` plus `pub fn camera_unlocked(lock: Option<Res<CameraCaptureLock>>) -> bool`; register `orbit_camera_system.run_if(camera_unlocked)`.
   - `#[derive(Component)] pub struct CaptureCamera;` and `#[derive(Component)] pub struct StreamingFocus;`.
   - `spawn_orbit_camera` adds `StreamingFocus`; `deactivate_foreign_cameras` adds `Without<CaptureCamera>`.
2. `fe-terrain/src/camera_framing.rs` (new, f64, bevy-free, no `render` gate):
   - `tile_bounds(z,x,y) -> [min_lat,min_lon,max_lat,max_lon]`
   - `bounds_intersect`
   - `coverage_check(tile_bounds, &[metas bounds]) -> Coverage::{Inside, Outside, Unbounded}`
   - `frame_bounds_top_down(projection, world_scale, bounds, ground_y, fov_v_rad, aspect) -> FramedPose { position, target, up }`
   - `geo_pose(projection, world_scale, lat, lon, ground_or_origin_m, altitude_m, heading_deg, pitch_deg) -> FramedPose`
   - `within_projection_range(projection, lat, lon) -> bool` (≤ 500 km)
   - `pub mod camera_framing;` in `lib.rs`.
3. Docs: `fe-renderer/src/AGENTS.md` §camera-capture (lock, markers, why `run_if` and not an early return: the `Transform` is recomputed every frame); `fe-terrain/src/AGENTS.md` §camera-framing (axis convention x=east, z=north, y=up; world = local × scale; the live grounded projection rule).

**Acceptance**
- No allocation or behaviour change in `orbit_camera_system` when unlocked.
- `camera_framing.rs` compiles without the `render` feature.

**Tests**
- fe-renderer:
  - `orbit_from_look_at_inverts_compute_orbit_transform` (table: yaw ∈ {0, π/4, π, -3π/4}, pitch ∈ {-1.55, 0, 0.46, 1.55}, distance ∈ {0.05, 8.66, 1e5}; asserts the round trip within tolerance)
  - `degenerate_look_at_and_parallel_up_return_none` (also NaN input → None)
  - `capture_lock_stops_the_orbit_system_from_rewriting_the_transform`: an App with the system scheduled, the lock inserted, a manual Transform write that survives `app.update()`. If `EguiContexts` prevents headless App construction, test `camera_unlocked` + the schedule's `run_if` wiring through `App::world().schedule` introspection instead, and say so in the AGENTS.md.
  - `deactivate_foreign_cameras_spares_capture_cameras`: World test with a `CaptureCamera` and a plain foreign camera; only the foreign one ends `is_active == false`.
- fe-terrain `camera_framing`:
  - `tile_bounds_match_the_slippy_formula` (z0 whole world ±85.0511; z1 (0,0) NW quadrant; a z11 known tile)
  - `framed_tile_centre_is_the_chunk_anchor_space_point` (the tile's centre lat/lon through `wgs84_to_local × scale` equals the frame target x/z: the same function `spawn_chunk` uses)
  - `frame_fits_both_axes_for_wide_and_tall_aspects`
  - `coverage_outside_inside_unbounded_table`
  - `geo_heading_zero_looks_north_and_ninety_looks_east` (target.z > position.z; target.x > position.x)
  - `geo_position_round_trips_through_local_to_wgs84`
  - `geo_beyond_500km_is_out_of_range`

### V4: multi-focus streaming + query narrowing (opus)

**Steps**
1. Narrow to the main camera:
   - `grid.rs`, `axis_gizmo.rs`: `With<Camera3d>` → `With<OrbitCameraController>`.
   - `ruler_plugin.rs` (both systems), `splat/render.rs`: `With<Camera>` → `With<OrbitCameraController>` (fe-terrain already depends on fe-renderer under `render`).
2. `terrain_plugin.rs`:
   - `fetch_and_spawn_terrain_chunks` queries `(GlobalTransform, Projection, Has<CaptureCamera>)` with `With<StreamingFocus>`, ordering capture foci first.
   - Each focus computes its own lat/lon/zoom/ring.
   - The chunk budget is `max_chunks` for the main focus plus `CAPTURE_CHUNK_ALLOWANCE` (≤ `max_chunks/2`) while a capture focus exists.
   - Grounding happens once, from the main focus only (the existing behaviour).
   - The per-focus pending count is published in a new `CaptureFocusResidency { pending: usize }` resource (read by V5's settle).
3. `update_terrain_lod`: despawn iff too far from every focus (each with its own `despawn_r`); stale-zoom is judged against the nearest focus's desired zoom; the height-field upkeep is unchanged.
4. `lod_ring.rs`: pure helpers `nearest_focus_index(chunk_pos, &foci)` and `chunk_retained_by_any_focus(...)`.
5. `fe-terrain/src/AGENTS.md`: amend §terrain_plugin and §tile-residency with the multi-focus rule and the allowance; note the `petal_portal.rs` `.single()` as a known benign site.

**Acceptance**
- With only the orbit camera present, behaviour and per-frame allocation are identical to today (one focus → same ring, same order).
- Hot-path discipline: no per-frame `Vec` of foci beyond a small fixed array (≤ 2 foci; capture jobs are single-flight).

**Tests**
- `lod_ring` unit tables: `chunk_retained_by_any_focus_table`, `nearest_focus_decides_stale_zoom_table`.
- Flow tests in `terrain_plugin.rs`'s test module, following the existing streamed-tileset flow precedent (`tiles/streamed.rs::moving_the_camera_demands_new_bundles_and_releases_old_ones`):
  - `two_camera_entities_do_not_stall_streaming` (the regression: an orbit camera plus a non-focus extra camera, and chunks still spawn)
  - `capture_focus_spawns_chunks_at_its_remote_target`
  - `main_view_chunks_survive_while_a_capture_focus_streams_elsewhere`
  - `removing_the_capture_focus_releases_its_chunks_on_the_next_lod_pass`
  - `capture_pending_count_reaches_zero_when_its_ring_is_resident`

### V5: Bevy capture orchestrator + wiring (opus)

**Steps**
1. `screenshot_bridge.rs`:
   - Generalize `capture_primary_window_png` into `pub(crate) fn capture_png(commands, screenshot: Screenshot, on_encoded)`; keep `capture_primary_window_png` as a one-line wrapper (so F12 and the GET path are unchanged).
   - `drain_screenshot_requests` returns early **without draining** while `CameraCaptureLock` exists (plus a test).
2. `vision_bridge.rs` (new):
   - The `VisionBridge(VisionRequestReceiver)` resource.
   - `VisionJob` state machine (spec §5.5) with a single `finish_job` exit, which releases the lock, restores or re-derives the controller, despawns the capture camera and image, and replies.
   - Resolvers: orbit/camera direct; geo/tile via `fe_terrain::camera_framing` + `ActiveTileSource` (live grounded projection) + `TerrainHeightField`; focus via `SpawnedNodeMarker` + `fe_ui::gimbal::subtree_world_aabb` + `fe_ui::visibility::effective_visibility` (refuse hidden).
   - Navigate-then-settle for `focus.navigate` (window only).
   - Settle counts frames and reads `CaptureFocusResidency` for scene mode; the wall-clock cap is 20 s via `Time<Real>`.
   - `ReadCamera` answered immediately from the controller, `NavigationManager`, `ActiveTileSource`, `ActivePetalTerrain` and the primary `Window`.
   - Busy → `capture_in_progress`.
   - Scene camera as spec §5.5; gizmo `RenderLayers` exclusion only if one `GizmoConfigStore` pass suffices (record which in AGENTS.md).
3. `main.rs`: `vision_bridge()`, `ScreenshotSurface::new(..).with_vision(vision_tx)`, `VisionBridge` resource plus systems (resolver/settle in `Update` after the camera and terrain systems; the capture/finish in `PostUpdate`); update the comment "off unless `FE_API_SCREENSHOT_ALLOW=1`" to "on unless `=0`".
4. `fe-ui/src/gimbal.rs`: add `pub fn subtree_world_aabb(entity, &Query<&GlobalTransform>, &Query<&Aabb>, &Query<&Children>) -> Option<(Vec3, Vec3)>`, reusing `find_in_subtree`'s walk (no behaviour change to `gimbal_center`).
5. `fractalengine/src/AGENTS.md`: amend §screenshot (capture_png, GET deferral during a lock) and add §vision (state machine diagram, restore and teardown guarantees, the navigation-not-restored caveat, minimized behaviour).

**Acceptance**
- Spec AC-6 and AC-8.
- The no-job frame path allocates nothing: `try_recv` on an empty channel, then return.

**Tests** (`vision_bridge.rs` World/App tests, renderer-free; captures are asserted as queued `Screenshot` entities like `drain_once_with_window`)
- `scene_mode_spawns_one_capture_camera_and_leaves_the_main_camera_untouched` (routing; asserts the `CaptureCamera` + `StreamingFocus` components, the `RenderTarget::Image` size, and that the main `Transform`/controller are bit-identical)
- `window_mode_locks_applies_the_pose_and_restores_the_snapshot`
- `window_mode_restore_false_rederives_the_orbit_controller`
- `minimized_window_refuses_window_mode_at_once_but_serves_scene_mode`
- `a_second_capture_while_a_job_is_active_replies_capture_in_progress`
- `plain_get_requests_stay_queued_while_a_posed_job_holds_the_camera`
- `wall_clock_cap_tears_down_and_restores_the_camera`
- `tile_pose_without_a_projection_refuses_no_projection`
- `tile_pose_outside_every_tileset_refuses`
- `focus_on_a_hidden_node_refuses_node_hidden`
- `focus_on_another_petal_without_navigate_refuses`
- `read_camera_reports_active_ids_and_the_grounded_origin`
- `read_camera_reports_locked_during_a_window_job`

### I1-I5: agent_input_surface (sonnet)

Run as written in `../agent_input_surface_20261031/plan.md` §2-§3 (steps, owns and the named flow tests), with the §2 amendments above. The tests are AIS's named list plus I5's added `inject_input_refused_while_a_posed_capture_holds_the_camera`.

### ADVOCATE: review-and-fix (opus, fresh context)

Argue FOR the feature and make it actually usable by an agent.
- Is the pose grammar discoverable from the MCP schema alone?
- Are the refusal messages actionable (each names the field and the valid range)?
- Does the closed loop `get_camera → screenshot(pose) → inject_input(capture_after)` work in three calls?
- Is `settled:false` actionable?
- Do GET, F12 and the folded input surface still behave?

Fix findings and record the verdict in `reviews/advocate.md`. Run `/code-review high` with `conductor-okf:code-styleguides` loaded.

### CRITIC: review-and-fix with security-review depth (opus, fresh context, refute-not-confirm)

The headline categories, in order:
1. **Default-on exposure.**
   - Any Owner token, any scope, sees any open verse; scene captures look anywhere silently.
   - Verify the loopback-only bind holds on every path (search for any proxy, forward or relay route to `/api/v1/debug/*` or `/mcp`).
   - Decide (and implement if YES) active-verse Owner-scope confinement for the default-on case, and a per-capture user-visible indicator plus an INFO audit log line (DID, mode, pose kind).
2. **Input injection** (AIS spec §7): mutation posture; `INPUT_GATE` default-off proven; the combined default-on screenshot + input = remote-control warning text; the lock interaction (I4 refusal).
3. **NaN and degenerate input:** grep every float reaching `Transform`, `looking_at` and `Projection` from the bridge, and prove it only comes from `ValidCaptureSpec` or resolver math with finite inputs.
4. **Lock and teardown leaks:** panic paths, despawned window, petal switch mid-job, verse replica close mid-job.
5. **Second-camera regressions:** run a workspace-wide grep for `.single()` over camera queries after V4.
6. **DoS:** 4096² scene renders at 1/s (GPU memory and time); confirm the size cap, single-flight and timeout bound it; lower `SIZE_MAX` if a 4096² capture exceeds the reply timeout on reference hardware.

Use `/security-review` plus `/code-review high`. Fix findings and record the verdict in `reviews/critic.md`. A phase with no verdict is unreviewed.

### CLOSE (haiku)

- Root `AGENTS.md`: a one-line pointer under the existing seams list to fe-api §screenshot/§camera-state and fractalengine §vision.
- `conductor/tracks.md`: tick and status.
- Ticks in this plan and the AIS plan; `metadata.json` progress.

## 5. Task checklist

- [ ] V1: vision wire contract
- [ ] V2: default-on gate + POST/camera REST + MCP (`TOOLS` 38)
- [ ] G1
- [ ] V3: pose math + camera lock + markers
- [ ] V4: multi-focus streaming + query narrowing
- [ ] G2
- [ ] V5: Bevy capture orchestrator + wiring
- [ ] G3
- [ ] I1, I2, I3 (`TOOLS` 39), I4 ∥ I5
- [ ] G4
- [ ] ADVOCATE (verdict recorded)
- [ ] CRITIC (verdict recorded, security depth)
- [ ] CLOSE
- [ ] Full sweep (once)
- [ ] In-app verify AC-11 (user-gated), including minimized-window scene capture
- [ ] Retro + archive (with agent_input_surface_20261031)

## 6. File overlaps (for sequencing)

**With polish_lane_20261031 waves 2-6.** This track lands first; the polish slice rebases and re-greps its citations.

| File | Here | Polish slice |
|---|---|---|
| `fe-terrain/src/terrain_plugin.rs` | V4 | **W4-S3** |
| `fe-terrain/src/AGENTS.md` | V3, V4 | **W4-S3** |
| `fe-renderer/src/AGENTS.md` | V3 | **W4-S3** |
| `fe-api/AGENTS.md` | V2, I3 | **W5-S1** |
| `fe-runtime/src/AGENTS.md` | V1, I1 | **W5-S1** (and W1-S2, which must close first) |

- `fe-api/src/query_guard.rs` (W5-S1) is not edited here, but `OperatorGate::check_rate` calls it, so W5-S1's reviewer must keep `check_rate_limit`'s signature.
- No polish wave owns `fe-ui/src/gimbal.rs` (V5); W4-S2 owns the sibling `node_manager/gimbal_interaction.rs`, which calls into `gimbal.rs`, so the change is additive only.
- `fe-runtime/src/messages.rs` (W3-S4, W4-S4, W5-S1) is NOT touched; the vision and input bridges are new modules.

**With the other open-core tracks.** This track goes first on every shared file.

| Track | Shared files |
|---|---|
| `relay_operability_20261031` | S1: `fe-runtime/src/{lib.rs,AGENTS.md}` (V1, I1). S2/S6b: `fe-api/src/server.rs`, `fe-api/AGENTS.md` (V2, I3). S2/S3: `fe-api/src/lib.rs` (V2, I3). S4/S5: `fractalengine/src/main.rs` (V5, I4). |
| `plugin_host_revival_20261031` | S1: `fractalengine/src/main.rs` (V5, I4), `fe-api/src/server.rs` (V2, I3), `fractalengine/src/AGENTS.md` (V5, I4). S7: `fractalengine/src/AGENTS.md`. Its fe-terrain files (`extension.rs`, `api_ext.rs`) do NOT overlap. |
| `hexon_entitlements_20261031` | No code overlap. Root `AGENTS.md` and `conductor/tracks.md` are append-only for everyone (the last writer rebases). |
| `peer_networking_support_20261031` | P11: `fractalengine/src/main.rs`. P14: `fe-api/src/{server.rs,lib.rs,mcp/mod.rs,mcp/tools.rs}`, `fe-api/AGENTS.md`. **P14 bumps `TOOLS.len()` from 39.** The AIS note "the only track that edits `mcp/mod.rs`" is stale. |

## 7. Tripwires

- `ScreenshotSurface::disabled()` stays the default for the relay, the harness and every test `ApiState`. DD-1 changes only the GUI's env read.
- GET `/api/v1/debug/screenshot`, F12 and `capture_refusal` behaviour are byte-for-byte unchanged.
- `OperatorGate` is edited once (V2, adding `default_on`). After that, only new consts are added; `INPUT_GATE.default_on == false` and `ELEVATED_QUERY_GATE.default_on == false` are asserted in tests.
- No float from a request reaches the ECS except through `ValidCaptureSpec`.
- `CameraCaptureLock` is removed on EVERY job exit (a `finish_job` single exit, plus a wall-clock cap).
- After V4, no system that means "the user's camera" queries `With<Camera>`/`With<Camera3d>` with `.single()`.
- Agents never run cargo; the orchestrator gates.

## 8. Commit strategy

One commit per slice after its gate is green: `feat(fe-runtime): V1 vision wire contract`, …, `fix(...)` for the review fixes, `docs(conductor): ...` for CLOSE. Trailer per the repo rule. **No push**; pushes are held for the user.

## §SEC-ADDENDUM (binding, orchestrator 2026-10-31 — from the background security review of V2's commit)

Applies to V5, the ADVOCATE and the CRITIC. A HIGH finding is not "done" until all of these hold:

1. **Scope confinement (keeps the user's default-on decision, closes cross-scope exfil).** Every capture (REST GET/POST
   screenshot, MCP screenshot) and every camera read (GET /api/v1/debug/camera, MCP read) is refused unless the caller's
   token scope COVERS THE ACTIVE VERSE at the moment of capture. V5's orchestrator therefore reports the active
   verse/fractal/petal ids to the API side (in the request reply or a pre-check query), and fe-api resolves coverage with
   the same scope helpers the petal-scoped endpoints use. A verse-B Owner token can never receive verse-A pixels.
2. **Focus targets.** `focus {node_id | petal_id}` is resolved to its verse BEFORE run_capture; the token scope must
   cover it. `navigate: true` additionally requires that coverage AND must never switch the user's view to a verse the
   token doesn't cover. Unresolvable ids → typed 404/422, never "capture whatever is on screen".
3. **One check, both doors.** REST and MCP call one shared authorization function (no MCP-only gap). Flow tests through
   ApiHarness: verse-B token refused for an active-verse-A capture; cross-verse focus refused; navigate across verses
   refused; covered scope succeeds — for REST and MCP.
4. Default-on stays (user directive, RUNBOOK §3 2026-10-31); document in fe-api/AGENTS.md §screenshot that default-on is
   bounded by (1).

## §UI-ADDENDUM (binding, user directive 2026-11-01: "make sure the camera can also capture the UI elements — the tool itself should expose the option")

Applies to V5 (authorized to extend V1/V2's fe-runtime/src/vision.rs, fe-api/src/vision.rs and fe-api/src/mcp/tools.rs for
this — those slices are done, no concurrent owner), the ADVOCATE and the CRITIC.
1. Add an explicit `include_ui: bool` to CaptureSpec, REST POST body and the MCP `screenshot` tool. Default `true`.
   `include_ui: true` ⇒ window capture (full egui UI; a pose moves the main camera, settles, captures, restores);
   `include_ui: false` ⇒ scene capture (off-screen, 3D only). `mode` stays for compatibility; if both are given and
   disagree → typed 400 `include_ui_conflicts_with_mode` (fail closed, no silent precedence). Table test the matrix.
2. The MCP tool's JSON schema + description must SAY it in plain words: "include_ui (default true): include the app's
   UI panels in the image; false renders only the 3D scene off-screen without moving the user's view". Same text in
   fe-api/AGENTS.md §screenshot and the REST docs.
3. Response metadata reports `include_ui` (and resolved mode) alongside the resolved pose.
4. Width/height stay scene-only (the window capture is the window's size) — the validation message must say so.
