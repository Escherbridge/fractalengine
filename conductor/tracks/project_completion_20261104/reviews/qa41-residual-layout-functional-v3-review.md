---
type: Review
status: source_approved
---

# QA41 residual layout v3: independent functional/UI/design review

Reviewer: `/root/residual_layout_review`, 2026-11-04.

**Verdict: SOURCE APPROVED for the complete nineteen-path residual layout slice. No remaining concrete source blocker was found.** V3 closes L3 by ordering the actual existing vision apply/advance chain before CameraUpdateSystems. The complete L1 chrome reservation, L2 outer egui ordering, hierarchy row sizing, divider geometry and compact Inspector corrections retain their previous source acceptance. This verdict does not grant execution, live visual, native capture, FULL, CI, release or overall goal-completion acceptance.

This was a separate independent re-review of the frozen packet and actual source. No compiler, parser, formatter, tests, application, OS/native/input/capture, API, DB, network or Git mutation was executed. Only this review artifact was written. Applied the shared UI guidance at `C:/Users/atooz/.gemini/UI-UX-guidance.md` and local architecture/documentation conventions. V1 and V2 review records and source packets are preserved.

## Exact scope and anchors

Checkout `.omc/worktrees/qa-residual-layout`, base `d8da3a46e9861e9191845ea283e37fb7e5ea3472`. Reviewed the complete slice using prior full review, independently verified raw-file identity, current relevant production boundaries, every V3 delta and the newly scoped vision file's actual project-base delta. The packet's addition of vision_bridge.rs was not mistaken for a new production file.

| V3 artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `manifest.json` | 4991 | `62c34c5ac10fa0faf536877e766d023034b6fe889e807077e9756889179db59c` |
| `source.patch` | 93069 | `611890ec0a3ab56f37556e6b7295b1ce93b32b49f24bd2614a7bbbbeec799d52` |
| `report.md` | 5995 | `fcaf682b4dfbed13c7a51e60e374c2a23061f54334d7f59e5eaa7045de2d465d` |
| `delta-v2-to-v3.patch` | 117468 | `9bca838767e42dc926043cb27dcb0b8d0c2b3916870e3037dc3d7252a3c902c7` |
| `vision-base-to-v3.patch` | 4183 | `75250c29044367608798dc0f8808bbd42f1b00b99660a041e7aa5906081443eb` |

All nineteen current raw file byte counts and hashes match the manifest, and all nineteen retained `raw/` copies match their manifest hashes. Read-only Git's actual changed-path inventory equals these nineteen paths. Seventeen V2 paths are byte-identical; the host directory guide is revised and vision_bridge.rs is newly included in packet scope. Against the actual project base, vision_bridge.rs has forty-one added lines, no removed lines: one production ordering line and forty lines extending the existing journey.

Complete scope:

- `fe-renderer/src/scene-pointer-design.md`
- `fe-renderer/src/scene_pointer.rs`
- `fe-terrain/src/AGENTS.md`
- `fe-terrain/src/ruler.rs`
- `fe-terrain/src/ruler_plugin.rs`
- `fe-ui/src/AGENTS.md`
- `fe-ui/src/native_accessibility/AGENTS.md`
- `fe-ui/src/panels/AGENTS.md`
- `fe-ui/src/panels/inspector.rs`
- `fe-ui/src/panels/sidebar.rs`
- `fe-ui/src/panels/tree_entry.rs`
- `fe-ui/src/plugin.rs`
- `fe-ui/src/ui_shell/AGENTS.md`
- `fe-ui/src/ui_shell/layout_policy.rs`
- `fe-ui/src/ui_shell/right_sidebar.rs`
- `fractalengine/src/AGENTS.md`
- `fractalengine/src/main.rs`
- `fractalengine/src/terrain_bridge.rs`
- `fractalengine/src/vision_bridge.rs`

V2 packet anchor hashes remain unchanged: manifest `bb48736e290824bb0a58aada7b378a87d95893c9c5dc40513545a2b874f99725`, full patch `0ea9391fb8119b17f25d064bb8e7e7b6f4b71b692aaf7e6b719881fddd4ddddb`, report `e538fb3b6b2d667ede02ee24acb1fddcdacf770b80a19191ae3f76c50ef8ccf8`.

## L3: actual projection writer ordering accepted

At `fractalengine/src/vision_bridge.rs:303-308`, the actual shared production installer retains `(apply_vision_pose, advance_vision_job).chain()` and its existing before-TransformSystems::Propagate edge, and adds only `.before(bevy::camera::CameraUpdateSystems)`. This orders both existing writers, not a test substitute. `apply_vision_pose` reaches pose_job's window-mode transform and raw FOV write at lines 1353-1364. `advance_vision_job` reaches finish_job, which calls CameraSnapshot::restore at lines 1573-1576; that method restores raw perspective FOV at line 164. The failure path through apply_vision_pose also calls the same finish_job within the newly ordered chain.

Production main still calls the actual vision installer at line 766 and the actual ruler installer at line 916. The ruler installer at `terrain_bridge.rs:68-74` orders outer PostUpdate EndPass after both CameraUpdateSystems and TransformSystems::Propagate; its inner GeometryReady -> Bridge -> Draw relationship remains. Thus the late raw FOV write that blocked V2 now precedes camera computation, while propagated global pose and computed camera viewport remain ordered before the actual ruler pass.

The primary pinned local SDK was independently inspected again:

- `bevy_render-0.18.1/src/camera.rs:64-70` registers actual camera_system in CameraUpdateSystems in PostUpdate, before asset events and frustum update. Lines 343-347 include Projection change detection, and lines 380-387 update the projection and store its clip-from-view matrix.
- The rendered ExtractedView at that file's lines 548-551 reads `camera.clip_from_view()` and the propagated GlobalTransform. It does not recalculate the matrix from a later raw FOV.
- `bevy_transform-0.18.1/src/plugins.rs:33-42` puts actual transform propagation systems in TransformSystems::Propagate.
- `bevy_ui-0.18.1/src/lib.rs:140-150` chains CameraUpdateSystems before UI layout; its established PostLayout/transform relation remains.
- `bevy_egui-0.39.1/src/lib.rs:1198-1204` places the actual primary context pass loop and end-pass system inside EndPass. That actual loop subsequently runs the nested primary UI schedule.

The inspected graph therefore has vision apply/advance -> CameraUpdate -> relevant UI layout/transform propagation -> EndPass, with explicit additional vision -> propagation and camera -> EndPass edges. No inspected source edge requires the vision chain after CameraUpdate or EndPass, and no return-edge cycle was found. The new ordering is a small dependency correction at the real installer, consistent with Bevy's existing ordered pipeline. It needs no new state owner, snapshot authority, public set or copied writer implementation. Runtime schedule materialization remains a required execution check, not an inference from this inspection.

## Existing first-apply and first-restore journey accepted as source coverage

The existing registered `window_mode_locks_applies_the_pose_and_restores_the_snapshot` journey, `vision_bridge.rs:2085-2159`, retains its real vision harness and existing request, lock, screenshot-delivery and asynchronous completion paths. It adds the actual SDK CameraPlugin and TransformPlugin, their necessary message/resources, the real ruler installer and an EndPass observer. It changes the initial main FOV to 60 degrees, applies the actual default orbit pose at 45 degrees, and compares the raw current perspective matrix with the actual cached Camera::clip_from_view matrix in the first applied EndPass. It also checks propagated global pose there.

After the existing capture fixture drives finish_job, the journey locates the first return to the original 60-degree FOV and checks its cached matrix and restored global position. The all-recorded-frames equality assertion includes intervening EndPass frames. It retains the preexisting lock release, bit-identical camera snapshot and reported pose checks. Both FOV writers are the real production paths; only observation is added. The initial 60-degree assignment is fixture setup, not a substitute apply/restore writer.

No new test declaration is added. The newly scoped vision file has twenty-three registered test declarations; all prior slice counts remain unchanged: ruler twenty-four, Inspector four, sidebar one, UI plugin nineteen, right-sidebar thirteen, terrain bridge forty, and zero in the remaining scoped Rust files. Counting declarations is source inventory only. None was executed in this lane.

This journey uses an EndPass observer under actual production ordering, not the full EguiPlugin loop or OS/GPU renderer. Its existing tiny screenshot-delivery fixture supplies asynchronous completion and cannot establish live image acceptance. The real SDK loop membership is separate source evidence. The author's report and guide correctly state these limits.

## Complete unchanged slice re-review

The tree correction retains stable `tree_entry` kind/id scopes, authoritative selection state, accessible names/selected descriptions, role gates and existing navigation/command callbacks. It reserves the complete icon/name/visibility/options row after indentation before allocating controls. Four target slots and three gaps provide the name control's minimum instead of allowing it to consume leftover space after sibling overflow. The hierarchy enables both scroll axes, so deeper rows may exceed the panel without overlapping sibling controls. It introduces no competing whole-row click owner. Actual deep-row accessibility and horizontal scrolling still require execution.

DividerMetrics remains one shared value for actual hit geometry, interior gutter and matching scene exclusion. Upward GUI quantization plus physical margin addresses the 19.999992-pixel deficit; outward physical endpoints are calculated in f64 and the resulting f32 rectangle must still satisfy the full physical minimum and clearance. Invalid geometry refuses rather than weakening the checker. The RAII native Results scope restores the prior metric, preserving its native owner and held-drag behavior. Camera-region and cosmetic insets remain aligned with the same policy. No camera crop or grid behavior is introduced.

Compact right-sidebar and Inspector navigation still derive from the actual available width/visible height and shared body budget. Native section/tab menus reuse RightSidebarState and activate_inspector_tab; they introduce no selection, tab, draft or permission authority. Existing token request/secret lifecycle and scoped pagination remain inside shared activation. Deselect still calls NodeManager. Batch controls remain in scrolling bodies, and section scroll identity remains stable. The previous source acceptance for drafts, selection, permissions, native controls and body budget continues to apply because these production files are byte-identical to V2.

The actual chrome path remains bounded and same-pass: UI clears stale geometry, finishes the real collector, captures its completed exclusions, and the host publishes only when both current scene and current chrome are available. The terrain value owns a bounded copy. RulerHudPlacement moves the complete numeric/fixed footprint above actual intersecting exclusions with separation, or omits it when it cannot fit. Both painters use complete text layout/mesh and stroke/padding bounds and scene clipping. The actual hint reservation that closed L1 has not regressed. No guessed hint width or old-frame fallback was reintroduced.

Physical camera viewport height still calibrates meters per physical pixel, while placement and preferred visual width use egui points with explicit scale conversion. It remains an oblique ground-plane approximation with documented limits. V3 fixes the raw/cache temporal dependency; it does not claim exact terrain-following measurement. Standalone TerrainPlugin's publish-required HUD contract remains explicitly documented: no publisher means no HUD, even if show_scale_bar is true.

## Pointer, native and design boundaries

The new ChromeSnapshot pass stamp remains used only by completed_exclusions_current_ui. The old physical pointer admission/current-window/current-UI predicates and ScenePointerStamp equality are unchanged; normal Update still uses its previous completed visible geometry. Context temp-data access releases its lock before Context getters, and the cosmetic accessor neither mutates admission state nor grants input or capture authority.

The eight mandatory native producer completions and their finish boundary remain unchanged. Fixed/numeric ruler painting remains a separately audited direct painter, not a ninth producer after finish. No native producer credit, edited-point-label refusal exception, Owner floor reduction, freshness grant or provider ordering change is introduced. Existing EndPass -> ProcessOutput -> Owner/provider handoff/annotation and later window chrome ordering are preserved.

Readability and extensibility are appropriate at the shared seams: DividerMetrics centralizes related geometry; shared section/tab activation keeps lifecycle effects in one place; SceneChromeRects is a bounded read-only value; the host adapter preserves crate direction; RulerHudPlacement owns one placement policy; and actual installers own scheduling dependencies. These established value-object, facade, RAII and ordered-pipeline patterns are used without adding speculative generic menus, builders or competing state machines. No additional design-pattern refactor is requested.

## Execution gates retained

Root still owns integration boundary review, compilation, complete runtime schedule materialization and the existing integrated journey sweep. Then actual screenshots/input must verify the four observed residual defects across required sizes/scales, Results states, hierarchy depth and Inspector navigation/body states. First real vision apply/restore and resize frames must show matching ruler calibration and actual painted scene/hint separation, alongside existing orbit/focus/Circle/Brush workflows. Retained prechange screenshots remain failure evidence and do not become V3 after-images.

Native/Owner, peer, retained-store, rich data/history, volume/material/intersection, FULL, CI and downloadable tagged-release gates are unaffected and remain pending wherever recorded elsewhere. This source review supplies no runtime or release credit for them.
