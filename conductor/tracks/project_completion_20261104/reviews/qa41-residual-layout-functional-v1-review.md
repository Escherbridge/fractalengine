---
type: Review
status: changes_requested
---

# QA41 residual layout: independent functional/UI/design-pattern review v1

Reviewer: `/root/residual_layout_review`, 2026-11-04.

**Verdict: CHANGES REQUESTED.** The four correction directions are sound, and the tree/divider/compact-navigation changes have no additional concrete blocker from this review. Two ruler integration issues prevent approval of the complete frozen slice: it does not reserve the actual tool hint, and its outer schedule does not establish current camera/transform inputs.

This is source review and inspection of retained pre-change screenshots. No compiler, parser, formatter, test, application, OS/native/input, API, DB, network or Git mutation was run. Only this review artifact was written. Existing journey assertions are authored, not executed. Main and both frozen source packets were left unchanged. Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md` and local architecture/documentation conventions.

## Exact reviewed packet

Isolated checkout: `.omc/worktrees/qa-residual-layout`, base `d8da3a46e9861e9191845ea283e37fb7e5ea3472`.

| Artifact under that checkout's `.omc/research/qa41-residual-layout-source-v1/` | Bytes | SHA-256 |
| --- | ---: | --- |
| `manifest.json` | 4124 | `eb0c27033fa7684385a889bc221714583d0a5745ecc389171bb9653d0cfa4d34` |
| `source.patch` | 67707 | `3430a70004ae24c9597a6c8482de2f4ba0395c73d8f367c30c98b4a88e3bd526` |
| `report.md` | 9587 | `a9edccc880efc9a3dfa692ae9f6b230970aa77ad55fab99210f0870f34b66288` |

All 16 current source/doc files match their raw manifest byte counts and SHA-256 hashes and the retained `raw/` copies. Read-only Git diff inventory equals those 16 paths exactly. The entire changed production, journey and documentation patch was reviewed, along with the called existing viewport hint, sidebar scroll, tab/section transition, camera and output-schedule boundaries.

Registered `#[test]` declaration counts in changed Rust files are unchanged: ruler 24, Inspector 4, sidebar 1, plugin 19, right sidebar 13, terrain bridge 40; the other changed Rust files have zero. This is declaration inventory only and provides no execution credit.

The reviewed raw file anchors are the complete manifest `files` array, including:

| Path | SHA-256 |
| --- | --- |
| `fe-terrain/src/AGENTS.md` | `66cadbc5404b27c92da1217b620e31f18f3db71af833faf44d161cce639ddeac` |
| `fe-terrain/src/ruler.rs` | `d8393657019eed30100d5479a8bd3062e8494ee73c6e91ee42e66358b42de7e1` |
| `fe-terrain/src/ruler_plugin.rs` | `e0e0fb58f1d4f98c1dd666535961a2a42660b7d0362b298811046cae406d0f1c` |
| `fe-ui/src/AGENTS.md` | `b28e53d27a478341f72a14a386b51cf49e018fdd9617d94d0570070639473d99` |
| `fe-ui/src/native_accessibility/AGENTS.md` | `461265fe20f2dfb751653bed9480e9d3a689bca9de4adf03d5340d90981bb451` |
| `fe-ui/src/panels/AGENTS.md` | `e13f105e22cddc401481cebe42987316628cfd7cd67621c890ed26262b322e71` |
| `fe-ui/src/panels/inspector.rs` | `84afb22a54b6e0cf3979dba96f56fd6b1014f567891de6016702b17a2ceb47ca` |
| `fe-ui/src/panels/sidebar.rs` | `5500b60f711c3151e5797dfd9806abed35cb9fefc63cf7f636e735ced4667d41` |
| `fe-ui/src/panels/tree_entry.rs` | `bba088d228c770f076ac72283658ec896f799bb6c1fd72ba57ea84d7344aaabd` |
| `fe-ui/src/plugin.rs` | `cfb61fb450a46e66b013e3dd08f606a2c63f0c83aab16b02664b279b6ed901f9` |
| `fe-ui/src/ui_shell/AGENTS.md` | `e3de4db8d983622cd308bb1dc2c6a72d73911eaa9a1ee9e77285888838ff246c` |
| `fe-ui/src/ui_shell/layout_policy.rs` | `2034adc0ed2263f57e09b5a60ffb2489af240d94aefd614390cb675607500180` |
| `fe-ui/src/ui_shell/right_sidebar.rs` | `d1dbc34b8ab1be2880a429a3e4d891492b6482ec41450cc0f1fd3471c2544b3a` |
| `fractalengine/src/AGENTS.md` | `ec09cb2b388a7dd5349af7d88d9b191b337b4f60df5a908bd6fd265392025a98` |
| `fractalengine/src/main.rs` | `59fde1012f05ed557fa4797bb5fa279410666440e87a9e18e4d38b45dc15c91e` |
| `fractalengine/src/terrain_bridge.rs` | `7a33c1216821143e1b0be05ff7658e35fc3d1f0267dc77ee1639c2a41cd63fc7` |

## Blocking findings

### [LAYOUT RESERVATION / OBSERVER] L1 — severity: medium

**Location:** `fe-terrain/src/ruler_plugin.rs:164-220`; `fe-ui/src/plugin.rs:451-453`; unchanged actual producer `fe-ui/src/viewport.rs:445-464`.

**Current:** `ViewportGeometry::overlay_current` supplies the divider-inset scene rectangle. The ruler's numeric and fixed-text producers fit only that rectangle, always anchoring at its bottom-left with a 16-point margin. The already-painted tool hint is bottom-centered at `rect.bottom() - 8`, has eight-point frame padding, and is not represented in this HUD input. Ruler painting is Foreground; the hint is a Middle Area. Scene containment alone therefore does not establish non-overlap with this existing chrome.

The retained QA41 1280x720 Circle scene has visual scene `(220,40,740,652)` and its visible hint occupies approximately `x=305..875, y=653..684` (the recorded label is `x=313..867.1875, y=661..676`). With v1's new scale-1 gutter, the ruler's inner left is about 250 and inner bottom 676. The retained camera height, 45-degree FOV, 720-pixel camera projection and scale 1 yield a 0.5-metre bar about 112 points long. Its line/shadow spans roughly `x=250..365`, with label/ticks in the same `y=650..676` band. Those ranges overlap the actual hint. The fixed unscaled chip is wider and follows the same anchor. This is a concrete source-derived collision, not a claimed screenshot from a new build.

**Suggestion:** publish the actual same-pass hint/excluded overlay geometry with the HUD placement input, using an existing bounded geometry seam or a small named cosmetic reservation value. Choose a complete free footprint above/beside the actual hint, or omit the HUD when no complete placement fits. Keep calibration derived from the camera independently of this placement. Do not use a guessed fixed hint height, a previous Area rectangle, or merely clip text underneath the hint.

**Example:**

```rust
// Before: only the outer scene is reserved.
paint_scale_bar(ctx, scene, spec);

// After, schematically: both reservations come from this completed UI pass.
let placement = hud.current(ctx)?;
let footprint = choose_ruler_footprint(placement.scene, placement.hint, size)?;
paint_ruler_at(ctx, footprint, spec);
```

**Impact:** closes the requested visual overlap fix through a real producer boundary. Extend the existing journey with the production hint and ruler in the same pass; check full paint-footprint disjointness at ordinary and narrow scales, including fixed unscaled text. A standalone rectangle-only ruler fixture cannot cover this interaction.

### [ORDERED PIPELINE] L2 — severity: medium

**Location:** `fractalengine/src/main.rs:915-922`; `fe-terrain/src/ruler_plugin.rs:113-159`.

**Current:** the added `GeometryReady -> Bridge -> Draw` ordering is inside `EguiPrimaryContextPass`, and correctly makes UI geometry current. The ruler also reads `GlobalTransform`, `Projection`, and `Camera::physical_viewport_size()`. No added or existing project edge orders the outer egui pass after Bevy's current camera computation or transform propagation.

Pinned primary local SDK source establishes the missing relationship:

- `bevy_egui-0.39.1/src/lib.rs:1201-1204` runs `run_egui_context_pass_loop_system` in PostUpdate's `EguiPostUpdateSet::EndPass`; `lib.rs:1916` runs the nested primary UI schedule there.
- `bevy_render-0.18.1/src/camera.rs:64-70` schedules the actual camera system in PostUpdate's `CameraUpdateSystems`, before asset events and frustum updates, with no egui edge. Its `camera_system` updates `Camera.computed.target_info` and projection from resize/scale/viewport changes.
- `bevy_camera-0.18.1/src/camera.rs:458` reads that computed target information when no explicit viewport overrides it.
- The project's scene-analysis collector orders itself after `TransformSystems::Propagate`, but that is not an edge for the independent egui pass or ruler.

Thus a legal schedule can draw the HUD using old physical viewport dimensions or old global camera height, then update them for rendering later in the same frame. Current context/pass/PPP stamping does not detect that mismatch. This is a source ordering gap; this review did not observe an actual runtime occurrence or claim it is a new historical failure.

**Suggestion:** establish the actual outer schedule relationship to the camera computation and transform propagation, while retaining the current inner shell/bridge/draw chain and verifying no cycle with host input/output/capture systems. Alternatively consume a camera snapshot explicitly published by those real producer boundaries and refuse it unless current. Prefer the small existing-set ordering if valid over another unowned cache.

**Example:**

```rust
// Host-level PostUpdate relationship, subject to the full schedule DAG check:
app.configure_sets(
    PostUpdate,
    bevy_egui::EguiPostUpdateSet::EndPass
        .after(bevy::camera::CameraUpdateSystems)
        .after(bevy::transform::TransformSystems::Propagate),
);
```

**Impact:** makes the reported physical scale correspond to the camera projection and height rendered for that frame. Extend an existing actual App/registration journey or the later live matrix with resize/custom-viewport/height changes before drawing; check the first changed frame, not only settled direct calls to `scale_bar_for_camera`. The v1 direct painter/calibration fixture cannot establish host schedule ordering.

## Integration contract clarification — nonblocking

`RulerHudInput` now defaults to unavailable and the real draw requires a publication. A standalone `TerrainPlugin` caller that does not install a publisher will get no numeric ruler or unscaled chip, despite `RulerSettings::show_scale_bar` being true. That is consistent with the approved no-whole-window-fallback direction, and the only current production caller found in this workspace is the GUI main that installs the bridge. This review does not require reintroducing an unsafe editor fallback. Add an explicit sentence to the terrain directory contract: standalone integrators must publish their own same-pass owned anchor in `Bridge`, otherwise HUD omission is intentional. The current prose strongly implies this but does not state the compatibility behavior directly.

## Positive design and boundary checks

- **Explicit row allocation:** the minimum four-slot row and three gaps are reserved before native children; the name Button has its own native minimum. The existing both-axis hierarchy scroll owns overflow. Stable entry IDs, separate eye/Options callbacks, full names/tooltips, native context menus, selection accents and authorization boundaries remain intact. No competing whole-row action was introduced.
- **Shared geometry policy:** side strips round physical endpoints outward, validate final span without QA epsilon, and share conservative radius/clearance/gutter with camera and overlays. The existing Results scope restores its SDK metric through RAII. Held-input, context/scale invalidation and release ownership were not bypassed.
- **Shared activation command:** `INSPECTOR_TABS` and `activate_inspector_tab` serve both visual presentations. `set_active_tab` still owns secret/request cancellation; API activation still resets page, populates scope and sends the original query. Section menus retain the existing toggle semantics and popup marker. Deselect still calls NodeManager. Compact navigation does not add a permission path or duplicate selection state.
- **Body ownership:** pending batch controls move into the existing body; the main `right_section_scroll_Inspector` identity is retained. Compact navigation has a separate bounded fallback scroll and keeps its body budget based on actual remaining height. No new arbitrary scroll minimum inflates content through the footer.
- **Crate adapter:** the host joins UI `ViewportGeometry` and terrain-owned `RulerHudInput` without a fe-ui to fe-terrain dependency. Both currentness checks reject stale contexts/passes/scales. Camera crop and world-grid behavior remain unchanged. Metric conversion correctly separates physical camera pixels from egui points and retains the existing oblique-camera approximation limits.
- **Native contract:** no ninth mandatory surface producer is marked after finish. The changed native documentation accurately keeps the ruler as a separately audited fixed/numeric direct painter; edited-point label refusal and Owner read-before-native-handoff ordering are not relaxed.

No speculative builder, strategy hierarchy, generic menu facade or state-manager rewrite is recommended. The existing manager and activation seams are suitable; complete their geometry and schedule contracts.

## Screenshots and remaining execution

Independently inspected actual retained QA41 `owner-circle-armed-shot.png`, `owner-physical-quarter-shot.png`, and `owner-narrow-double-shot.png`. They establish the existing ruler/footer overlap, deep-name truncation and cramped high-scale Inspector, and the actual hint's placement. They are pre-change images and provide no v1 after-image acceptance.

After both source blockers are repaired and independently re-reviewed, root still owns integration, compilation, the final integrated journeys and matching live checks. Required live work includes actual tree clipping/hscroll/neighbor actions, divider endpoints/camera behavior, compact section/tab dispatch and draft/privacy/keyboard behavior, both Inspector scroll ends, and ruler placement/calibration on panel, Results, window, scale, navigation and camera changes. No native capture, peer delivery, numerical volume, FULL, CI, release or goal-completion acceptance follows from this review.
