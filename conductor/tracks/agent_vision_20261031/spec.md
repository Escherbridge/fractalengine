---
type: spec
title: "Agent vision: default-on screenshots, dynamic camera/geo/tile capture, camera read-back (+ agent_input_surface folded in)"
track: agent_vision_20261031
tags: [feature, fe-api, fe-runtime, fe-renderer, fe-terrain, fractalengine, agent-qa, security]
created: 2026-10-31
status: pending
resource: ./metadata.json
plan: ./plan.md
origin: "user directive 2026-10-31: enable screenshot support by default, make it dynamic (camera, tile coordinates, etc.), fold in the recent session's context"
depends_on: []
absorbs: [agent_input_surface_20261031]
---

# Specification: agent vision

**Track ID:** `agent_vision_20261031` · **Type:** feature · **Priority:** P1
**Crates:** `fe-runtime` (bridge contract), `fe-api` (gate, REST, MCP), `fe-renderer` (camera lock, pose math, streaming focus), `fe-terrain` (geo/tile framing, multi-focus streaming), `fractalengine` (Bevy capture orchestrator)
**Citations computed at:** `7ea181b`. Symbols are authoritative; re-grep line numbers before each slice.

## 1. User vision

> "look across the recent session and fold in their context into another workflow — enable screenshot support by default, allow it to be dynamic allowing the agent to pass in things like camera and tile coordinates etc." (user, 2026-10-31)

An agent driving the API should be able to (1) **read** where the camera is and what is open, (2) **look** anywhere: an exact camera, an orbit, a lat/lon, a slippy tile, a node, and (3) **act** on the GUI (the folded-in `agent_input_surface_20261031`). Together these form a closed QA loop: read pose → capture → act → capture.

## 2. Design decisions (ratified by the user directive; do not reopen)

| ID | Decision |
|---|---|
| **DD-1** | **Screenshots are on by default.** An unset `FE_API_SCREENSHOT_ALLOW` enables the surface; `FE_API_SCREENSHOT_ALLOW=0` disables it. The Owner floor and the rate limit stay. Headless and relay hosts stay disabled because they have no window. Elevated query and the input surface stay OFF by default, so `OperatorGate` gains a **per-gate default**. This overrides open-core.md S-AGENT-GUI's "both off by default". The row and RUNBOOK §3 were updated in this track's planning commit. |
| **DD-2** | **Dynamic capture:** `POST /api/v1/debug/screenshot` takes a JSON body. `GET` stays the plain current-window capture, byte-for-byte unchanged. The MCP `screenshot` tool takes the same parameters, and empty arguments keep today's behaviour. There are two modes: **`window`** (full UI; an optional pose moves the main camera, waits `settle_frames`, captures, then restores the pose by default) and **`scene`** (an off-screen render-to-texture camera at the requested pose and resolution, with no UI; the user's view is untouched and it works while the window is minimized). |
| **DD-3** | **Pose spec, EXACTLY ONE of:** `camera {position, target, up?, fov_deg?}` · `orbit {focus, yaw_deg, pitch_deg, distance}` · `geo {lat, lon, altitude_m?, heading_deg?, pitch_deg?}` · `tile {z, x, y}` (frame a Web-Mercator slippy tile through the active petal's terrain projection; typed refusal if there is no projection or the tile is outside the tileset) · `focus {node_id | petal_id, navigate?}` (frame the AABB; the active petal only unless `navigate: true`). Other parameters: `width`/`height` (clamped 64..4096), `settle_frames` (bounded), `restore` (window mode, default `true`). The response is the PNG plus the **resolved pose**: an `X-FE-Capture-Pose` header on REST, and a text part next to the image on MCP. |
| **DD-4** | **Validation is mandatory.** Every float must be finite (the smart_aim NaN crash class: RUNBOOK, `ui-semantics-audit-20260808`). Ranges are enforced. There must be exactly one pose spec. Refusals are typed 400s. Only one posed capture runs at a time. In window mode the camera controller is locked against user input while a capture holds the pose. |
| **DD-5** | **`GET /api/v1/debug/camera`** returns the current main-camera pose, the active verse/fractal/petal and the projection info, so an agent can work in a closed loop. |
| **DD-6** | **`agent_input_surface_20261031` is folded in and executed as written** (its spec §4-§6, D-AIS-1..7). It reuses `OperatorGate` with the gate default OFF and composes with screenshots for closed-loop QA. Its metadata is marked "merged into agent_vision_20261031". Its plan is referenced, not rewritten (amendments in plan.md §2). |
| **DD-7** | **Scene-mode tile streaming:** decided from the code in §5.4. The capture camera registers as an additional **streaming focus**. Waiting for residency alone is insufficient. |

## 3. Evidence: the surface as built (Lane B: `f55ca21`, `5405f4e`, `effb644`)

- `fe-api/src/operator_gate.rs::OperatorGate`: the gate is data with seven fields and no default concept. `opt_in_value_enables(value) -> value == Some("1")` hard-wires default-off. `admit` = `check_opted_in` → typed `require_role_level(Owner)` → `check_rate` (key `<prefix>:<did>`, the shared `query_guard::check_rate_limit` map).
- Gate consts: `fe-api/src/screenshot.rs::SCREENSHOT_GATE` (1/s, prefix `screenshot`) and `fe-api/src/lib.rs::ELEVATED_QUERY_GATE` (5/s). Both literals gain the new field.
- `fe-api/src/screenshot.rs::ScreenshotSurface { allowed, bridge, reply_timeout }`. `disabled()` is used by every test/headless `ApiState` literal (`assets.rs` ×5, `canonical_ws/router.rs`) and the harness default. **Keep `disabled()`/`new()` signatures** so none of those sites change.
- `fe-runtime/src/screenshot.rs`: `ScreenshotRequest { correlation_id, reply_tx }`, `SCREENSHOT_BRIDGE_CAPACITY = 4`.
- `fractalengine/src/screenshot_bridge.rs`: `capture_primary_window_png` (private; spawns `Screenshot::primary_window()` with a one-shot observer that encodes PNG on the compute pool), `capture_refusal` (0x0 minimized → immediate refusal), `drain_screenshot_requests` (coalesces every queued request into ONE capture). Wired at `fractalengine/src/main.rs` (`screenshot_bridge()` ~:415, `ApiConfig.screenshot` ~:546, systems ~:559-566). The GUI API binds `127.0.0.1:8765` (`main.rs` ~:515).
- MCP: `fe-api/src/mcp/mod.rs`: `ToolSeam::Screenshot` (advertised iff `state.screenshot.bridge.is_some()`), `operator_opt_in_refusal` (opt-in refused BEFORE `authorize`), the `screenshot` row (`Owner, Global, requires Screenshot, content`), and `TOOLS.len() == 37` with a self-documenting message. Handler: `mcp/tools.rs::screenshot` (opt-in + rate only, FR-5); schema `schemas::screenshot()` is `{}`.
- Tests: `fe-api/tests/screenshot_test.rs` (fake responder over `fractalengine_test_harness::api::ApiHarness::spawn_with_screenshot`); `screenshot_bridge.rs` World tests (`drain_once_with_window`).
- Docs: `fe-api/AGENTS.md` §operator-gate, §screenshot; `fractalengine/src/AGENTS.md` §screenshot; `fe-runtime/src/AGENTS.md` §screenshot-bridge.

**Camera** (`fe-renderer/src/camera.rs`):
- `OrbitCameraController { focus, distance, yaw, pitch, min/max_distance, min/max_pitch (±89°), target_distance, target_focus, easing_rate, … }`.
- `compute_orbit_transform(focus, distance, yaw, pitch)`: offset = `(d·cosp·siny, d·sinp, d·cosp·cosy)`, `looking_at(focus, Y)`.
- **`orbit_camera_system` recomputes `*transform` from controller state EVERY frame**, eases toward targets and ground-clamps pitch/height from `TerrainHeightField`. A raw `Transform` write is overwritten the next frame, so a held pose needs the system gated off (`run_if`).
- `deactivate_foreign_cameras` sets `is_active = false` on every `Added<Camera>` without `OrbitCameraController`, which **would kill a capture camera**.
- The FOV is `FRAC_PI_4`, near 0.01, and the far plane is scaled by `apply_camera_scale`.
- fe-ui already scopes its camera queries to `With<OrbitCameraController>` (`plugin.rs`, `viewport_labels.rs`, `node_manager/*`). `fe-ui/src/plugin.rs::apply_camera_focus` is the existing fly-to (it refuses effectively-hidden targets, D-14). `fe-ui/src/gimbal.rs::gimbal_center` holds the recursive subtree-AABB DFS idiom for glTF `SceneRoot`s.

**Geo → world** (`fe-terrain`):
- `projection.rs::Projection { origin_lat, origin_lon, origin_ele }` with `wgs84_to_local(lat, lon, ele) -> [x=easting, y=ele-origin_ele, z=northing]` (flat_projection, accurate to ~500 km) and its inverse `local_to_wgs84`.
- World = local × `TerrainConfig::effective_world_scale()` (`terrain_plugin.rs::spawn_chunk`: `scale_local(projection.wgs84_to_local(s_lat, nw_lon, 0.0), scale)`).
- **The live projection is `petal_binding::ActiveTileSource.projection`.** Its `origin_ele` is overwritten at runtime by center-tile grounding (`fetch_and_spawn_terrain_chunks`), so framing MUST read `ActiveTileSource`, not the persisted petal config.
- Tiles: `tiles/source.rs::TileCoord { x, y, zoom }` provides `to_lat_lon()` (NW corner) and `tiles_at_zoom`. The tile extent is NW(x,y) → NW(x+1,y+1) (`hexon_source.rs::covers`, `bundle.rs::bounds_for_tile_range`). Coverage comes from `CompositeTileSource::source_metas()` → `TilesetMeta.bounds = [min_lat, min_lon, max_lat, max_lon]` (root AGENTS.md §petal-map: origin = bounds center).

## 4. Scope / non-goals

**In scope:**
- The per-gate default on `OperatorGate`.
- The POST capture with 5 pose kinds × 2 modes.
- `GET /camera` plus the MCP `get_camera` tool.
- The camera lock and restore.
- The off-screen capture camera.
- Multi-focus terrain streaming.
- Narrowing every single-camera query to the main camera.
- The full agent_input_surface plan (as amended in plan.md §2).
- Docs and security review.

**Non-goals:**
- Changing GET's bytes or behaviour.
- Scope-confined captures. Owner-of-any-scope still passes; the critic evaluates this, see §7.
- Video/streaming captures.
- Orthographic projection.
- Per-layer hide/show. This is **deferred**: entity `Visibility` is shared by both cameras, so hiding a layer for a scene capture would flash it off in the user's view too. Doing it properly needs `RenderLayers` per terrain layer. Trigger to revisit: an agent QA flow that needs an isolated layer.
- Capturing on headless hosts.
- Any business framing (open-core.md HARD RULE).

## 5. Design

### 5.1 Default-on gate (`OperatorGate.default_on`)

`OperatorGate` gains `pub default_on: bool`. The parse becomes a pure, table-tested function `opt_in_value_enables(value, default_on)`:

| env value | default_on=false (elevated, input) | default_on=true (screenshot, camera) |
|---|---|---|
| unset or `""` | off | **on** |
| `1` | on | on |
| `0` | off | off |
| anything else (`true`, ` 1`, `yes`) | off + warning | **off + warning (fail closed)** |

`opted_in_from_env` WARNs loudly whenever the result is on, with a default-on variant: "enabled by default — set `FE_API_SCREENSHOT_ALLOW=0` to disable". The field stays on `ApiState`, so tests still flip it. `ScreenshotSurface::disabled()` remains the headless default; DD-1 changes only what the GUI binary reads from env.

### 5.2 Wire contract (`fe-runtime/src/vision.rs`, new; serde, pure validation)

A **new bounded bridge**, separate from the screenshot bridge so GET's path is untouched: `VisionRequest { correlation_id, op: VisionOp, reply_tx }`, `VisionOp::{Capture(ValidCaptureSpec), ReadCamera}`, `VisionReply { correlation_id, result: Result<VisionOutput, VisionFailure> }`, `vision_bridge()`, `VISION_BRIDGE_CAPACITY = 4`.

```jsonc
// POST /api/v1/debug/screenshot  (MCP `screenshot` arguments: identical object)
{
  "mode": "window" | "scene",            // default "window"
  "pose": {                               // optional; EXACTLY ONE key
    "camera": { "position": [x,y,z], "target": [x,y,z], "up": [x,y,z]?, "fov_deg": f? },
    "orbit":  { "focus": [x,y,z], "yaw_deg": f, "pitch_deg": f, "distance": f },
    "geo":    { "lat": f, "lon": f, "altitude_m": f?, "heading_deg": f?, "pitch_deg": f? },
    "tile":   { "z": u8, "x": u32, "y": u32 },
    "focus":  { "node_id": s } | { "petal_id": s }, "navigate": bool?   // navigate inside focus
  },
  "width": u32?, "height": u32?,          // scene mode only; 64..=4096; default 1280x720
  "settle_frames": u32?,                  // default 30 (window w/ pose) / 30 (scene); max 600
  "restore": bool?                        // window mode only; default true
}
```

`pose` is a serde externally-tagged enum, so two keys or zero keys are a structural refusal (`pose_must_have_exactly_one_kind`). `CaptureSpec::validate() -> Result<ValidCaptureSpec, VisionRefusal>` is the ONLY constructor of `ValidCaptureSpec`. That puts the "validated" newtype in the type system, and the Bevy side never sees an unvalidated spec.

**Static validation table (400, typed `code`):**

| Check | Code |
|---|---|
| any float NaN/±inf (positions, angles, distance, fov, lat/lon, altitude) | `non_finite` (+ field path) |
| zero or ≥2 pose kinds; `focus` with both/neither id | `pose_must_have_exactly_one_kind` |
| `width`/`height` outside 64..=4096 | `size_out_of_range` |
| `width`/`height` in window mode | `size_requires_scene_mode` |
| `restore` in scene mode; `navigate: true` in scene mode | `option_not_valid_in_mode` |
| `settle_frames > 600` | `settle_out_of_range` |
| `fov_deg` outside 1..=170 | `fov_out_of_range` |
| `orbit.distance` ≤ 0 or > 1e7; `orbit.pitch_deg` outside ±89 | `orbit_out_of_range` |
| `camera.position == target` (‖Δ‖ < 1e-6) or `up` zero / parallel to view dir | `degenerate_camera` |
| `geo.lat` outside ±85.0511 (Web-Mercator); `lon` outside ±180; `heading` outside 0..360; `pitch_deg` outside -90..=89; `altitude_m` outside -500..=1e6 | `geo_out_of_range` |
| `tile.z > 24`; `x` or `y` ≥ 2^z | `tile_out_of_range` |
| body > 4 KiB / unknown fields (`deny_unknown_fields`) | `malformed_body` |

**Dynamic refusals (Bevy side, 422, typed `code`):** `no_projection`, `tile_outside_tileset`, `geo_out_of_projection_range` (> 500 km from origin), `node_not_found`, `node_hidden` (D-14 shared picker rule), `not_active_petal` (without `navigate`), `nothing_to_frame` (empty petal with no tileset), `window_minimized` (window mode only), `no_window`.

**Other statuses:** 409 `capture_in_progress` (a posed capture already holds the camera); 403/429 from the gate; 503 bridge busy/host gone; 504 timeout. One `VisionError` enum maps each case to exactly one status, the same shape as `ScreenshotError::status()`.

**Resolved pose** (the `X-FE-Capture-Pose` header is compact ASCII-escaped JSON; the MCP text part is the same JSON):

```jsonc
{ "mode": "scene", "position": [..], "target": [..], "up": [..], "fov_deg": 45.0,
  "orbit": { "focus": [..], "yaw_deg": .., "pitch_deg": .., "distance": .. },
  "geo": { "lat": .., "lon": .., "altitude_m": .., "altitude_reference": "terrain"|"origin" } | null,
  "world_scale": .., "width": .., "height": .., "settle_frames_used": .., "settled": true,
  "tiles_pending": 0, "restored": true, "source": "tile" }
```

`settled: false` with `tiles_pending > 0` means the settle cap hit before residency; the capture is still returned and is honest about it.

### 5.3 Pose math (pure, table-tested)

- **orbit → pose:** degrees → radians → `compute_orbit_transform`. **camera → orbit (for `restore:false` and read-back):** new `orbit_from_look_at(position, target) -> (focus, distance, yaw, pitch)`, the exact inverse of `compute_orbit_transform` (yaw = `atan2(dx, dz)`, pitch = `asin(dy/d)`). Roll from a non-Y `up` cannot be expressed in orbit form, so it is dropped on `restore:false` and reported.
- **geo → pose** (`fe-terrain/src/camera_framing.rs`, f64, bevy-free):
  - Position = `wgs84_to_local(lat, lon, ground + altitude_m) × world_scale`, where ground comes from the resident `TerrainHeightField` (reference `terrain`). If there is none, the altitude is above the grounded origin elevation (reference `origin`).
  - Defaults: `altitude_m` 300, `heading_deg` 0 (= north = +z, clockwise toward +x east), `pitch_deg` -45.
  - The target is a point one unit along the view direction.
- **tile → pose:**
  - Bounds come from `TileCoord` NW(x,y)/NW(x+1,y+1).
  - Coverage is checked against the union of `source_metas()` bounds. A disjoint tile is refused with `tile_outside_tileset`. With zero metas (online/demo mode) the response reports `coverage: "unbounded"` and the capture proceeds.
  - The corners project to world and the camera frames top-down with north up (`up = +z`, looking −Y), at a distance that fits BOTH axes for the output aspect: `max(half_w/ tan(hfov/2), half_h/ tan(vfov/2)) × 1.1`.
  - A zoom outside the served range is allowed (framing is geometric) and is reported.
- **focus → pose:** the world AABB is the union over the node's subtree `Aabb`s (the same DFS idiom as `gimbal_center`); for `petal_id`, the union over that petal's `SpawnedNodeMarker` entities, falling back to the tileset bounds. The current view direction is kept, at distance = `radius / sin(fov/2) × 1.1`.

### 5.4 Scene-mode tile streaming: the decision (DD-7)

**Finding (code):** terrain loads ONLY around one camera.
- `fe-terrain/src/terrain_plugin.rs::fetch_and_spawn_terrain_chunks` centres its adaptive ring (`ring_offsets(radius)`, nearest-first) on `camera_query.single()` over `Query<(&GlobalTransform, &Projection), With<Camera>>`. The ring's desired zoom comes from that camera's height (`desired_zoom`).
- `update_terrain_lod` despawns every chunk farther than `despawn_r` from that same single camera, and evicts stale-zoom chunks against that camera's desired zoom.
- The streamed-tileset bundle demand (`TileBundleDemand`) is rebuilt from that ring every frame.

**Consequences of naively spawning a second `Camera`:**
1. `.single()` returns `Err` and both systems `return` early. **ALL terrain streaming and eviction stop silently.** The same failure hits `fe-renderer/src/grid.rs` and `axis_gizmo.rs` (`With<Camera3d>`), plus `fe-terrain/src/ruler_plugin.rs` and `splat/render.rs` (`With<Camera>`).
2. `deactivate_foreign_cameras` turns the capture camera off.
3. Even with those fixed, tiles at a remote scene pose never load, because nothing rings them, and the exit ring would evict them anyway.

"Settle until resident" alone would therefore wait forever.

**Decision:**
- (a) Narrow every single-camera query listed above to `With<OrbitCameraController>`. fe-ui already does this.
- (b) Add a `StreamingFocus` component (fe-renderer `camera.rs`). The orbit camera always carries it; the capture camera carries it for its job's lifetime.
- (c) `fetch_and_spawn_terrain_chunks` iterates foci, **the capture focus first** while one exists, each with its own height-derived zoom and ring.
- (d) `update_terrain_lod` despawns a chunk only when it is too far from **every** focus, and judges stale-zoom against the **nearest** focus's desired zoom.
- (e) A capture focus gets a bounded extra chunk allowance (`CAPTURE_CHUNK_ALLOWANCE`, ≤ `max_chunks / 2`), so a scene capture never evicts the user's view.
- (f) Scene settle = at least `settle_frames`, AND until the capture focus's ring reports zero `Pending` tiles. This is bounded by the 600-frame cap and a Bevy-side wall-clock cap (20 s); on the cap, `settled:false` is reported.
- (g) When the capture camera despawns, its focus disappears, and the next `update_terrain_lod` releases its chunks through the existing exit-ring eviction (no new eviction path).

Streamed bundles for the remote target are demanded automatically because the ring feeds `TileBundleDemand`. `fe-webview/src/petal_portal.rs::position_overlay_system` also calls `.single()` on all cameras, but its body is a TODO no-op, so it is noted and left alone.

### 5.5 Bevy capture orchestrator (`fractalengine/src/vision_bridge.rs`, new)

There is one `VisionJob` state machine resource, `Idle → Resolving → Posing → Settling{frames_left, deadline} → Capturing → Restoring → Idle`:

- **Busy:** a `Capture` op arriving while a job is active replies with the `capture_in_progress` failure. The API side also refuses early via a `try_acquire` on a 1-permit semaphore in `ScreenshotSurface` (409, bridge untouched). `ReadCamera` is answered immediately in any state, with `locked: true` while a job holds the pose.
- **Window mode:**
  - Refuse `window_minimized` via the existing `capture_refusal`.
  - Snapshot `OrbitCameraController` + `Transform` + `Projection` fov, then insert `CameraCaptureLock` (`orbit_camera_system` gets `.run_if(camera_unlocked)`, so user input cannot move it). Write the pose `Transform`/fov directly.
  - Settle, then capture with the existing primary-window path, which includes egui.
  - With `restore` (default) the snapshot is restored. Otherwise the controller is re-derived through `orbit_from_look_at` so the next user drag continues without a jump.
  - The lock is released in ALL exit paths, including the wall-clock teardown.
  - A plain GET arriving during a window job is **left queued** (`drain_screenshot_requests` no-ops while `CameraCaptureLock` exists), so GET never returns a half-posed frame. The 5 s GET timeout then applies.
- **Scene mode:**
  - Spawn a `Camera3d` with `CaptureCamera` + `StreamingFocus` markers, `RenderTarget::Image` (an `Image` of `width×height`, sRGB 8-bit, usages `RENDER_ATTACHMENT|COPY_SRC|TEXTURE_BINDING`), `order: -1`, and the same projection near/far/fov/tonemapping as the main camera. It gets no egui context, so there is no UI.
  - Settle per §5.4(f). Capture with `Screenshot::image(handle)` through the **generalized single capture helper** (`capture_png(commands, Screenshot, on_encoded)`; the current `capture_primary_window_png` becomes a thin call to it, keeping one owner).
  - Despawn the camera and the image.
  - Works with a minimized window; whether Windows keeps the Bevy loop running while minimized is checked in the in-app verify, and stated as a risk.
- **`focus` + `navigate:true`** (window mode only): `NavigationManager::navigate_to_verse/fractal/petal` using ids from the `VerseManager` tree, then settle until the petal's `SpawnedNodeMarker`s exist (bounded). Navigation is a user-visible side effect and **is not restored** (documented). `restore` covers the camera pose only.
- **Gizmos:** world-space gizmos (grid, axis, gimbal) render into every camera. If ONE `GizmoConfigStore` pass can put every gizmo group on a dedicated `RenderLayers` layer (with the main camera seeing layers 0+1 and the capture camera only 0), V5 does it; otherwise scene captures include gizmos and that is documented. Decided at implementation time against Bevy 0.18; it does not block the track.

### 5.6 `GET /api/v1/debug/camera` + MCP `get_camera`

The gate is `CAMERA_STATE_GATE`. It uses the **same env var and default** as screenshots (`FE_API_SCREENSHOT_ALLOW`, default on), its own rate key `camera`, and a 10/s budget, so the closed loop never burns the 1/s capture budget. The route goes over the vision bridge as a `ReadCamera` op. The reply is:

```jsonc
{ "pose": <resolved-pose shape, mode "window">, "locked": false,
  "active": { "verse_id", "verse_name", "fractal_id", "fractal_name", "petal_id" },
  "projection": { "origin_lat", "origin_lon", "origin_ele_grounded", "world_scale",
                  "tileset_bounds": [..] | null, "zoom_range": [min,max] | null } | null,
  "window": { "width", "height", "minimized" } }
```

The new MCP row is `get_camera` (`Owner, Global, requires Screenshot`). TOOLS goes 37 → 38 here, then 38 → 39 when the folded-in `inject_input` lands.

### 5.7 Gate order (unchanged contract)

- REST: opt-in → typed Owner → per-DID rate → static validation (400) → capture slot (409) → bridge `try_send` → reply with timeout. Validation runs AFTER the gate on purpose: an unauthenticated or un-opted caller learns nothing about the parameter grammar, and a refused caller never costs a parse. The rate budget is spent even on a 400, which is cheap and deliberate (it prevents validation probing).
- MCP: `operator_opt_in_refusal` → dispatcher Owner floor → handler `check_rate` → validate → same path.
- The reply timeout for posed captures is `5 s + settle wall-clock cap` (≤ 25 s), never a hang.

## 6. Acceptance criteria (observable, real flows)

- **AC-1 default-on:** the GUI started with `FE_API_SCREENSHOT_ALLOW` unset admits an Owner `GET`/`POST` screenshot. `=0` refuses it with the disabled message (403). `=true` refuses it and warns (fail closed). Elevated query is still refused with the env unset. This is proven by the gate table test plus the fake-responder flow.
- **AC-2 validation:** every row of §5.2's static table yields 400 with its `code`, and the fake bridge receives NOTHING.
- **AC-3 round trip:** a valid posed POST reaches the bridge as a `ValidCaptureSpec` equal to the request. The 200 carries the responder's PNG verbatim plus an `X-FE-Capture-Pose` header that parses as the responder's pose. MCP returns the image block plus a text block with the same JSON.
- **AC-4 single flight:** a second posed POST while the first is in flight gets 409 `capture_in_progress` without touching the bridge.
- **AC-5 framing math:** the tile/geo/orbit/look-at tables in plan V3 pass, including "the framed tile centre equals the chunk-anchor world position for the same lat/lon" (same projection, same scale).
- **AC-6 routing:** Bevy World tests show the following.
  - Scene mode spawns exactly one `CaptureCamera` with `StreamingFocus` and leaves the main camera's `Transform`/controller bit-identical.
  - Window mode holds the pose under `CameraCaptureLock`, then restores the snapshot exactly.
  - A minimized window refuses window mode at once (422) but still serves scene mode.
- **AC-7 streaming:** with a capture focus far from the main camera, chunks spawn at the capture target, and the main camera's chunks are not evicted. When the capture focus is removed, its chunks are released on the next LOD pass. With two `Camera` entities present, streaming does not stall (the regression the narrowing fixes).
- **AC-8 camera read-back:** `GET /camera` returns the active ids and the grounded `origin_ele` (not the persisted one), with `locked: true` during a window job.
- **AC-9 headless:** relay and the harness default return 503 "no window" on all three routes; MCP `tools/list` omits `screenshot` and `get_camera`.
- **AC-10 input surface:** agent_input_surface spec AC-1..AC-7 hold, with `TOOLS.len()` at 39 after its I3 slice.
- **AC-11 in-app (user-gated):** with the env unset, an agent does the following against a petal with a tileset, and the user confirms the three PNGs look right:
  1. `GET /camera`
  2. POST `tile` scene capture
  3. POST `geo` window capture with restore
  4. `GET /camera` again, showing the pose unchanged

## 7. Risks and security (feeds the CRITIC slice)

- **Default-on widens exposure.**
  - Any Owner token, of ANY scope, can now see pixels of whatever verse is open, and with posed scene captures can look at ANY location of the active petal without the user noticing.
  - Mitigants today: the GUI API binds `127.0.0.1` only (`main.rs`), there is the Owner floor, and the rate limit is 1/s.
  - The critic must verify there is no proxy or tunnel path that exposes `/api/v1/debug/*` off-host (relay, `fe-api` behind fe-relay, MCP over a forwarded port).
  - The critic decides whether default-on should additionally require the token's Owner scope to cover the **active verse** (the candidate fix: default-on admits only active-verse Owners, with `=1` restoring any-scope). Its verdict is recorded either way.
- **Scene captures are invisible to the user** (by design). Mitigation candidate for the critic: a transient status-bar indicator or toast "agent captured view" per capture, plus an INFO log line naming the token DID and pose.
- **Input injection** (folded track): it mutates state. agent_input_surface spec §7 applies verbatim; the critic treats it as the headline finding category. Composition risk: default-on screenshots plus opted-in input equals full remote control by any Owner token. The `FE_API_INPUT_ALLOW=1` startup WARN must say so.
- **NaN/degenerate math** crashing the frame loop. This is mitigated by `ValidCaptureSpec` (validate-once newtype), pure tables, and the `degenerate_camera` refusal. The critic must also grep every float that reaches `Transform`/`looking_at`.
- **Camera lock leak:** a panic or teardown that skips the restore leaves the user's camera frozen. All exits go through one `finish_job` path, and the wall-clock cap forces teardown.
- **Second-camera regressions** beyond the queries found (§5.4): the critic greps `single()` over camera queries workspace-wide after V4.
- **Minimized-window scene capture** may not render on Windows if the event loop throttles while minimized. This is verified in-app; if it fails, scene mode is documented "window must not be minimized" and refuses with `window_minimized`.
