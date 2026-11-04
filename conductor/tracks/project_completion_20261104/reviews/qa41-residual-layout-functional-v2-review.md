---
type: Review
status: changes_requested
---

# QA41 residual layout v2: independent functional/UI/design review

Reviewer: `/root/residual_layout_review`, 2026-11-04.

**Verdict: CHANGES REQUESTED for one remaining calibration dependency, L3.** V2 repairs L1's missing actual chrome reservation and adds the actual outer camera/transform-to-egui edges requested by L2. The full current camera calibration path still permits a late vision FOV write after the camera's cached render projection has been computed. That dependency must be ordered before claiming matching current calibration. No other concrete blocker was found in the complete eighteen-file slice.

This is source-only re-review. No compiler, parser, formatter, test, application, OS/native/input/capture, API, DB, network or Git mutation was executed. Only this review artifact was written. The v1 and v2 source packets remain unchanged. Applied the shared UI guidance, local architecture and directory-documentation conventions.

## Exact complete scope

Checkout `.omc/worktrees/qa-residual-layout`, base `d8da3a46e9861e9191845ea283e37fb7e5ea3472`. Reviewed the complete eighteen-path source/doc scope, using the previously reviewed full v1 patch plus every v2 delta and the actual new-boundary diff against the same project base. The two newly scoped renderer files were assessed against their real base rather than treating their packet additions as newly invented source.

| V2 artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `manifest.json` | 4736 | `bb48736e290824bb0a58aada7b378a87d95893c9c5dc40513545a2b874f99725` |
| `source.patch` | 86876 | `0ea9391fb8119b17f25d064bb8e7e7b6f4b71b692aaf7e6b719881fddd4ddddb` |
| `report.md` | 9426 | `e538fb3b6b2d667ede02ee24acb1fddcdacf770b80a19191ae3f76c50ef8ccf8` |
| `delta-v1-to-v2.patch` | 50581 | `c921a3cbd42eb27e8a6f87ea1c3d4c90b1fd039ee4e78a6603b4038da40baa20` |
| `boundary-base-to-v2.patch` | 5196 | `f4df56648fe6e452230be56d059f8676db2011b721513b96a05d2ce9a21eb99e` |

All eighteen current raw file byte counts and hashes match the manifest and every retained `raw/` counterpart. Read-only Git diff inventory equals the manifest's eighteen paths exactly. Registered test declaration counts remain unchanged in each changed Rust file. No execution is inferred from that inventory.

Complete paths: `fe-renderer/src/{scene-pointer-design.md,scene_pointer.rs}`; `fe-terrain/src/{AGENTS.md,ruler.rs,ruler_plugin.rs}`; `fe-ui/src/{AGENTS.md,plugin.rs,native_accessibility/AGENTS.md,panels/AGENTS.md,panels/inspector.rs,panels/sidebar.rs,panels/tree_entry.rs,ui_shell/AGENTS.md,ui_shell/layout_policy.rs,ui_shell/right_sidebar.rs}`; `fractalengine/src/{AGENTS.md,main.rs,terrain_bridge.rs}`.

The five production UI files for tree/divider/compact navigation are byte-identical to v1: `panels/inspector.rs`, `panels/sidebar.rs`, `panels/tree_entry.rs`, `ui_shell/layout_policy.rs`, and `ui_shell/right_sidebar.rs`. Their prior full functional/design review therefore remains applicable. The associated panels and UI-shell documentation is likewise unchanged.

## Closed source findings

### L1 — actual hint reservation: source correction accepted

The real viewport still publishes its clipped header/browser/hint responses through the existing fixed-capacity ScenePointerChrome collector. The new `SceneChromeRects` value exposes only its bounded rectangle slice. `completed_exclusions_current_ui` copies context temp-state, releases its lock, then checks the completed pass and original window/viewport/screen/scale/navigation predicate. A pending build or mismatched pass refuses cosmetic publication.

`gardener_ui_system` clears both geometry and chrome before context access and captures the accessor only after the real `ScenePointerChrome::finish_pass`. The host bridge requires both current scene and current chrome, and the terrain resource copies at most four finite positive exclusions into the same pass input. Missing publication omits the HUD instead of reusing prior Area geometry or a whole-window fallback.

`RulerHudPlacement::footprint` starts with the bounded inset and moves the complete footprint above intersecting actual chrome, with four points of separation. Its search has at most five attempts for four exclusions. Moving upward strictly clears the selected intersecting top boundary; a different later blocker can move it farther, and failure to fit omits painting. Both the fixed chip and numeric bar call this same production placement, retaining the complete text layout/mesh and stroke/padding bounds plus scene clipping. Thus the v1 ruler/hint collision is no longer left unreserved.

The real `RulerHudInput::paint` dispatch is used by production and the expanded host journey. That journey renders the actual `viewport_petal_space` header and long Select hint, obtains actual completed chrome and calls the real numeric/fixed painter in the same pass. Its source assertions check complete footprint containment and disjointness across normal/narrow sizes, scales and Results states; they do not claim executed images. This is materially stronger coverage than v1's bare scene rectangle fixture.

### Pointer admission preservation: source boundary accepted

The added private ChromeSnapshot pass field is used only by the new cosmetic accessor. The existing `ChromeSnapshot::current`, `contains`, physical `accepts`, window matching and current-UI secondary pointer predicates have no new pass comparison or other changed condition. ScenePointerStamp equality is unchanged; ChromeSnapshot has no derived equality into which the added field could silently enter. `begin_pass` only records an additional value for cosmetic use.

Consequently normal Update input retains its prior completed-visible-geometry contract. The new read accessor does not mutate/invalidate the collector, grant input/native authority, add a new exclusion producer, relax any mismatch check or execute Context getters under the temp-data closure. The existing renderer journey retains its old pointer assertions and adds separate pending/completed cosmetic assertions.

### L2 — outer egui ordering: requested edges accepted, calibration completion qualified by L3

Production main now calls `terrain_bridge::install_ruler_hud`, and that actual installer orders PostUpdate's `EguiPostUpdateSet::EndPass` after both `CameraUpdateSystems` and `TransformSystems::Propagate`. The original inner GeometryReady -> Bridge -> Draw chain remains. The pinned actual egui pass loop belongs to EndPass, so this establishes the outer relationship missing in v1. The new inbound edges do not reverse ProcessOutput, Owner observation, native provider handoff/annotation or WindowChromeApply ordering. No return-edge cycle was found in the inspected host/SDK graph.

The existing settings/ruler journey now calls this real installer with the SDK render CameraPlugin and TransformPlugin, then observes computed viewport and propagated height from EndPass on initial, first resized/height-changed and custom-viewport frames. The observer is not the full egui loop; the report correctly separates that unexecuted fixture from the pinned real-loop registration evidence. It closes the original formula-only-fixture weakness. However, the real host has an additional raw FOV writer, described below, that the fixture does not exercise.

### Standalone contract clarification: accepted

The terrain guide now explicitly says that standalone TerrainPlugin integrators must publish their own same-pass scene and occupied chrome in Bridge and establish their outer camera/transform ordering. Otherwise both HUD variants intentionally remain absent even when show_scale_bar is true. This documents the compatibility behavior without restoring an unsafe editor fallback.

## Blocking finding

### [ORDERED PIPELINE / SINGLE PRODUCER DEPENDENCY] L3 — severity: medium

**Location:** unchanged but directly relevant production `fractalengine/src/vision_bridge.rs:295-308`, `:154-165`, `:1336-1364`, and `:1550-1576`; new consumer boundary `fractalengine/src/terrain_bridge.rs:68-75`; ruler read `fe-terrain/src/ruler_plugin.rs:153-182`.

**Current:** the real vision installer registers `(apply_vision_pose, advance_vision_job).chain()` in PostUpdate before `TransformSystems::Propagate`, with no relation to `CameraUpdateSystems`. `apply_vision_pose` reaches `pose_job`, which writes `PerspectiveProjection.fov = pose.fov` for a window capture. `advance_vision_job` can reach `finish_job`, whose CameraSnapshot restore writes the previous raw FOV. Both writes can occur after the camera system but before the new ordered EndPass.

The pinned primary local SDK shows why this matters:

- `bevy_render-0.18.1/src/camera.rs:385` computes and stores `Camera.computed.clip_from_view` from the Projection inside `camera_system`.
- `bevy_camera-0.18.1/src/camera.rs:493-496` returns that stored matrix through `Camera::clip_from_view`.
- `bevy_render-0.18.1/src/camera.rs:550` places `camera.clip_from_view()` into the rendered ExtractedView; it does not recompute it from a later raw Projection there.
- The ruler reads the raw current `Projection::Perspective.fov`, not the cached render matrix.

Therefore this legal order remains: CameraUpdate computes the old FOV -> vision applies or restores a different raw FOV -> transform propagation updates pose -> EndPass draws the ruler. The ruler uses the new raw angle while the image uses the old cached projection for that frame. Ordering EndPass after both camera and transform producer sets does not order this intervening FOV writer before camera computation. The issue is a concrete source dependency, not a claim that a new runtime frame has been observed. Its writer being unchanged source does not waive the required current-calibration outcome.

**Suggestion:** order the existing production vision pose/job chain before `CameraUpdateSystems` as well as retaining its propagation edge. This covers both initial pose and restoration through their actual shared installer. Keep the same within-chain order, ownership, cancellation and restore behavior. No inspected edge requires this chain after EndPass or after CameraUpdate; combined with the pinned CameraUpdate -> UI layout -> transform propagation ordering, the additional writer -> CameraUpdate edge has no identified return-edge cycle. Root must still materialize/build/check the full schedule.

**Example:**

```rust
// Production vision installer, preserving its existing chain and transform edge.
(apply_vision_pose, advance_vision_job)
    .chain()
    .before(bevy::camera::CameraUpdateSystems)
    .before(TransformSystems::Propagate)
```

**Impact:** makes both camera consumers use the projection actually computed from the current vision write/restore for that rendered frame. Extend a meaningful existing camera/vision journey using the real vision installer and SDK CameraPlugin: start with one FOV, apply a different window-pose FOV, observe the first EndPass and cached render matrix, then restore and check the first restored frame. Retain actual screenshot/focus/capture acceptance. Do not replace the real writer with a test-only updater or treat a direct ruler formula call as the integration proof.

## Readability and remaining boundaries

The design continues to use appropriate small adapters and value objects: the renderer exposes bounded read-only geometry; UI owns its stamp; the host joins crates; terrain owns placement and painting. `RulerHudPlacement` is an appropriate named value for the scene/exclusion contract and avoids growing unrelated method parameters or introducing an unbounded inventory. The existing shared tab activation and manager methods remain preferable to a new state owner or generic menu framework. No additional speculative refactor is requested.

The eight mandatory native surface producers and finish remain unchanged. The ruler remains a separately audited fixed/numeric direct painter; no ninth producer is registered after finish, edited-point-label refusal remains, and the geometry accessor does not confer capture freshness or authorization. Camera crop and grid behavior remain unchanged. Scale math still separates actual physical viewport height from egui-point placement and retains its documented oblique-camera approximation limits.

Before integration approval, repair and independently review L3 with replacement complete anchors, preserving v1/v2 packets. After source approval, root still owns compilation, the existing integrated journeys and actual matching screenshots/input/calibration at the required sizes/scales, including first pose/restore/resize frames. No native capture, peer, retained-store, volume, FULL, CI, release or overall completion acceptance is granted here.
