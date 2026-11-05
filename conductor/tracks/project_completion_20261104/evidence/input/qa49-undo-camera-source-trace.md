---
type: SourceTrace
title: Undo and Redo camera collision source trace
date: 2026-11-04
---

# QA49 Undo/Redo camera drift: source-only trace

Finding: **Left Ctrl is simultaneously an editor command modifier and an unconditional camera-down key.** Ordinary Undo/Redo correctly replays the saved object transform, while the camera independently moves during the same held-modifier frames. Shift accelerates camera descent during Redo. This is a concrete input-ownership collision, not evidence of a failed history replay, stuck modifier or missing scene residency.

Reviewed actual QA49 artifacts from stage a16eab87, run `20261104-164547-795`, and current source paths with no scoped delta from a16eab87. No input, app/process/OS/native/API/DB operation, implementation execution, compiler/test/parser/formatter or Git mutation. Only this report is written. Root owns live runs and future author authorization. Shared UI guidance and renderer module notes applied.

## Actual evidence

The full raw Position X value is `2.500` in both `owner-draft-ui.jsonl` and `owner-draft-scrolled-ui.jsonl`. Unlike QA47, the driver focused once, sent Ctrl+A, then text_entry without refocusing. One Apply changes returned row X 0→2.5 and history version4→5 with one new full_transform. One Ctrl+Z returns X to0/version6; one Ctrl+Shift+Z returns X to2.5/version7, each appending one full_transform. The recorded input receipts report only those respective chord commands. This trace does not reclassify the successful object/history result as failed.

All five actual PNGs were separately opened and visually inspected; all five capture receipts have request `{}`, source `orbit`, distance5, yaw45°, pitch26.56232452392578°, settled=true, restored=false. Their metadata distinguishes the camera change from object movement:

| Captured state | Focus/target Y | Camera position Y |
| --- | ---: | ---: |
| Draft, after Apply, saved later | 0 | 2.2358551025390625 |
| After Undo | -0.7408829927444458 | 1.4949721097946167 |
| After Redo | -2.2526397705078125 | -0.01678466796875 |

Camera X/Z, distance, yaw, pitch, FOV and up remain equal across these receipts. Undo changes focus by about -0.740883; Redo changes it by a further -1.511757. The images agree: after Undo the scene shifts upward in the image; after Redo most grid/objects are outside the visible view. **Occlusion/framing is not a scene residency census.** These images do not prove assets were despawned/missing. The earlier three poses are exactly stable despite input and waiting, strengthening the association with the two command chords.

## Exact source cause

`fe-renderer/src/camera.rs:634–678` runs fly movement whenever `!egui_wants_keyboard`. At :663–664 it subtracts Vec3::Y when **KeyQ OR ControlLeft is held**. It never excludes Ctrl/Super/Alt command chords. At :668–672 ShiftLeft multiplies speed by2.5. Any movement clears target_focus and changes controller.focus by normalized movement × move_speed × delta_secs × sprint (:674–677). The default move_speed is10 (:103); actual per-frame delta times are not recorded, so the precise two observed numerical deltas must not be reconstructed or attributed to a fixed number of milliseconds.

At the same time, `fe-ui/src/node_manager/shortcuts.rs:48–74` independently decodes any Ctrl/Super plus just-pressed Z as Undo or, with Shift, Redo. It does not consume the shared ButtonInput state or claim the camera's keyboard branch. The UI keyboard-focus guard at :328–335 protects text editing, and :381–398 routes the admitted chord to checked editor_batch replay. `editor_batch/mod.rs:1258–1279` chooses the pending-safe batch/ordinary Undo owner; it does not direct the camera. Camera keyboard ownership is independent from its pointer-region gate (:539–560); being over the right panel does not prevent unowned keyboard fly movement. Only text keyboard focus prevents it, which explains why the earlier Ctrl+A inside the focused numeric TextEdit did not cause the same camera drift.

The input bridge is behaving according to its documented chord lifetime, not holding keys indefinitely. `fractalengine/src/input_bridge.rs:179–186` injects one planned frame before InputSystems and egui input collection. `plan_batch` (:464–574) creates separate modifier-prefix, letter-press, letter-release and modifier-release frames. `drive_synthetic_input` (:864–875) advances one per PreUpdate; `release_held` (:644–658) also releases held keys on terminal/abort cleanup.

For the exact receipts, the planned sequences are:

| Chord | Planned frames | Camera collision before final release |
| --- | --- | --- |
| Ctrl+Z | Ctrl down; Z down; Z up; Ctrl up | ControlLeft remains pressed during first three frames |
| Ctrl+Shift+Z | Ctrl down; Shift down; Z down; Z up; Ctrl+Shift up | Ctrl remains pressed during first four; Shift is also pressed during the last three of those |

Local pinned primary SDK `bevy_input-0.18.1/src/button_input.rs:149–158,172–175,252–255` confirms pressed state persists until release; clear resets only just-pressed/released. Thus even checking for Z alongside Ctrl would miss the **modifier-prefix** descent before Z exists. Ctrl+Z and camera-down can both succeed without any schedule race or unrecorded extra input. The successful Undo/Redo row/history transitions and pure vertical camera changes match this direct source path. There is no need to change input modifier release ordering to explain it.

## Minimal safe author plan, not implemented

1. Reserve command modifiers for editor/UI use in the camera's **keyboard fly branch**. Suppress that branch while either Ctrl or Super is held; reserve either Alt as well if maintaining the existing tool-shortcut convention. Cover left/right sides consistently. Keep the existing egui keyboard-focus gate and CameraCaptureLock run_if, pointer geometry/capture gates, easing and terrain safety. Gate movement only; do not return from the whole camera system and freeze normal easing/ground safety.
2. Remove the ControlLeft descent alias explicitly; retain Q as plain descent and E/Space as ascent, existing WASD behavior including S first-press handling, and Shift speed. An immediately active standalone Ctrl descent alias cannot coexist safely with a later Ctrl+Z chord: its prefix is indistinguishable from a standalone press. Reserving modifiers at source is smaller and more reliable than timing/debounce, special-casing Z, editing the injector, or introducing cross-crate action-consumption state.
3. Extend a meaningful production keyboard-policy test/journey through all actual prefix/letter/release phases for Ctrl+Z and Ctrl+Shift+Z, both command sides and Super variants, plus unmodified movement, Shift speed, text-owned input and capture lock. Keep that policy test attached to the production movement decision rather than duplicating it. Existing camera tests are mostly pure geometry; :1240–1267 checks capture run_if separately and does not exercise this modifier collision. Tests must await the sole integrated builder under the current workflow.
4. Parent-owned runtime acceptance should independently demonstrate valid save→Undo→Redo row/history transitions **and unchanged camera pose** under those chords, with plain navigation/Shift and UI text focus/capture behavior retained. No such repaired runtime acceptance is claimed here.

## Exact artifact anchors

Receipts, byte counts and SHA256:

- `owner-draft-shot.jsonl`:728, `a358f73bc3290b664ae4e635f89e503ef66fc3f4d26f7197af081348db577ea5`.
- `owner-after-shot.jsonl`:725, `21915933dffe3c3f12982cf33c5c7f6f8897ef2327f41485d3bcb975e6000289`.
- `owner-saved-later-shot.jsonl`:723, `411d5ca28769acd46411a59ae64d467ce9a8970f977e02ebecbbdb2b124a8a30`.
- `owner-undo-shot.jsonl`:754, `862cb4c26d4d08aa64a667e3110ea865be2cf0203fd3e546d145027b670f7892`.
- `owner-redo-shot.jsonl`:753, `449bb9e3eb1f3d345534f96f33bf6edf8dcfd0b0b7708cc2f2dab580f38ea96c`.

Actual Undo PNG:317234 bytes, SHA256 `d369d3753fc391f2b33375cea953cf9eab539f85edadb6994477d3997ad31ba0`. Redo PNG:131711 bytes, SHA256 `22049486e84fdee21bd3740512e3a53be2f76ed8dd4a1c7f6c3a6cfab8f108a7`. Returned history current versions4/5/6/7 have SHA256 respectively `1910302cf49243c7a53a544ff77228f9c98d7002f1e3df6137edb98010b0111e`, `2b9c160dfbdd53ddb109b135b840e0ae427c9dd5a45a63f4761199d71db4cb2a`, `8fcd1ecb16e9026f1ba874fded3c999ab6d4e871c8607940f843ff4c2adb5a02`, `8539b045705dd1fa8b7282207738bed8c0f25436c15fc3204d5d7feaa89796cf`. This trace adds no general evidence-packet/FULL, native, metric, peer or release approval.
