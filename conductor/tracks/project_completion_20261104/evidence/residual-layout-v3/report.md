---
type: ImplementationReport
status: independently_source_approved_execution_pending
---

# QA41 residual layout v3 source handoff

Status: L3 is source-authored and frozen for both independent re-reviews. No
source approval, compilation, test execution, runtime schedule materialization
or live acceptance is claimed. Checkout remains qa/residual-layout at base
d8da3a46e9861e9191845ea283e37fb7e5ea3472. No compiler, parser, formatter, tests,
apps, OS/native/input, API, DB, network or Git mutation occurred. Only source
inspection, read-only Git diff, raw copies and file hashes were used. Main and
the parallel native helper source remain untouched.

The requested finding is root qa41-residual-layout-functional-v2-review.md,
12724 bytes, SHA c5349395d9415170bd0f6889e80aa03f282dc3ec7892c6a85efd6883e6b34b5a.
L1 actual chrome reservation, L2 outer edges and the standalone publisher contract
were accepted there; this batch changes only the additional real FOV writer
dependency and its existing journey/documentation.

## Production correction and DAG

The actual existing vision installer now registers
(apply_vision_pose, advance_vision_job).chain() before CameraUpdateSystems as
well as retaining its existing TransformSystems::Propagate edge. This is one
additional production ordering line. The within-chain order and all authority,
lock, cancellation, pose and restore methods are untouched.

Both raw FOV writes occur in that actual chain: apply_vision_pose reaches
pose_job's initial window FOV write; advance_vision_job reaches finish_job and
CameraSnapshot::restore. They now precede the actual SDK camera_system that
caches clip_from_view from the current raw Projection. Render extraction later
uses Camera::clip_from_view, the same cached matrix. The ruler's raw perspective
angle and the rendered matrix therefore receive the same producer ordering.

Pinned source and existing host edges were inspected again. bevy_render 0.18.1
camera.rs:64-71 puts actual camera_system in PostUpdate CameraUpdateSystems,
before asset events/frusta; :380-386 computes the cached matrix, and :550
extracts that stored value. bevy_ui 0.18.1 lib.rs:140-150 orders UI layout after
camera updates and its PostLayout precedes transform propagation. The actual
vision chain has no after-camera, after-EndPass or output-set dependency.

The resulting source DAG is vision apply/advance -> CameraUpdate -> UI layout
-> TransformSystems::Propagate -> EndPass, with EndPass also explicitly after
CameraUpdate. Existing ProcessOutput, Owner observation, provider handoff and
annotation follow as before. WindowChromeApply remains after PostProcessOutput.
The new edge is inbound to CameraUpdate, with no inspected return edge to the
vision chain. This is a source inspection, not an executed schedule-cycle check;
root must still materialize/build the complete graph.

## Existing actual vision journey

No new test declaration was added. The existing registered
window_mode_locks_applies_the_pose_and_restores_the_snapshot journey uses its
existing harness, which installs the real vision bridge, request receiver and
job state machine. This extension also installs the SDK CameraPlugin and
TransformPlugin plus the real terrain_bridge::install_ruler_hud.

The main camera starts at 60 degrees, then the actual ordinary orbit capture
resolves/applies its default 45-degree FOV. The first applied EndPass checks raw
FOV, the actual Camera::clip_from_view cache, the current projection's SDK matrix
and propagated global pose. The existing screenshot-delivery fixture then drives
the actual asynchronous completion/finish_job restore path. The first restored
EndPass at 60 degrees must already have the matching cached matrix and restored
global position. All recorded EndPass frames must have raw/cache agreement.

The FOV change and restore are production writers, not a test-only updater or
copied writer-set label. The existing screenshot fixture is retained; it does
not produce real OS/GPU capture evidence. The observer sits in the real EndPass
set under the production installer relationship, rather than running a formula
without scheduling. The pinned actual egui pass loop's EndPass membership remains
the separate source evidence recorded in v2. These assertions are unexecuted.

## Preservation and packet scope

All seventeen prior source/doc files other than the host directory guide were
independently byte-compared with their frozen v2 raw files and remain identical.
This includes all pointer source/predicates, actual chrome accessor, ruler,
geometry bridge, tree, divider and compact navigation changes. No ninth native
producer, capture grant, pointer exception or Owner-order change was introduced.

The v2 frozen packet remains byte-identical:

- manifest bb48736e290824bb0a58aada7b378a87d95893c9c5dc40513545a2b874f99725;
- full patch 0ea9391fb8119b17f25d064bb8e7e7b6f4b71b692aaf7e6b719881fddd4ddddb;
- report e538fb3b6b2d667ede02ee24acb1fddcdacf770b80a19191ae3f76c50ef8ccf8.

V1 and all its raw files remain preserved as previously recorded. V3 includes
nineteen raw source/doc paths: v2's eighteen plus vision_bridge.rs. The complete
project-base patch covers the final integration scope. delta-v2-to-v3.patch
compares exact frozen packet raw trees, so vision_bridge.rs appears as an addition
to packet scope. vision-base-to-v3.patch separately provides its narrow actual
project-base delta for review; it is one production line plus forty lines in the
existing journey. No synthetic v2 raw anchor was invented for this new path.

## Remaining verification

Both independent re-reviews remain required. Root owns integrated compilation,
actual schedule materialization and the existing journey sweep, then first real
vision apply/restore, resize/orbit/focus and ruler-calibration checks with matching
screenshots. All previously recorded tree/divider/Inspector, actual Circle/Brush,
native/Owner and release gates remain outstanding acceptance work. No FULL, CI,
release, capture, numerical volume or overall goal-completion claim follows from
this source-only packet.
