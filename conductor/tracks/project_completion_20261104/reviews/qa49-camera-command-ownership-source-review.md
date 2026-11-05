---
type: Review
title: Camera command ownership correctness review
date: 2026-11-04
---

# QA49 camera keyboard ownership: independent source review

**APPROVED AT SOURCE** for the exact two-path frozen slice against `71cd5f8e9f05bd39dbb1a61c17a95ca054dda5fe`. No correctness/design blocker was found. This is the authorized provisional command-ownership convention; root owns the separate decision ledger and actual rebuilt-camera acceptance. QA49's original camera drift is not relabeled repaired by this review.

Manifest SHA-256 **`8001920495601fde46cbebf737271f8e94d82fd52c02af55cf7bcd90e63494c2`** (1069 bytes); complete patch SHA-256 **`57f003210bff97956490b3fe4f52ba62da41c82295bf9e4eb405322acbfe0d9e`** (10897 bytes). Both frozen raw files match declared lengths/hashes and working bytes; manifest baseline Git blob identities independently match and current HEAD retains those source bases. Raw source identities govern approval, separately from Git newline representation.

| Path | Bytes | SHA-256 |
| --- | ---: | --- |
| `fe-renderer/src/camera.rs` | 55858 | `136006c24d5faa6fa773872f44dad90c91fa31c7ae518acd1de5944b0b595ff2` |
| `fe-renderer/src/AGENTS.md` | 20846 | `ec938c058d0fc09c8ca13a6692f29750406bc4338d26106824fedfddb473ca7f` |

The private pure `keyboard_fly_intent` is the actual production boundary, called by `orbit_camera_system`. Existing egui keyboard ownership or any held ControlLeft/ControlRight/SuperLeft/SuperRight returns no fly intent, including the modifier-only prefix before a shortcut letter and suppression of an already-held movement key. It reads immutable ButtonInput and consumes no editor key. CtrlLeft descent is removed; Q remains descent and E/Space ascent. Alt is unchanged.

For unclaimed keyboard input the extracted mapping and accumulation order are unchanged: yaw-projected W/S, D/A, ascent then Q descent, S ignored only on just-pressed frame, nonzero-vector normalization and Left Shift 2.5 multiplier. The live system retains `speed = move_speed * delta_secs`, followed by normalized direction times speed times sprint in the original association. Only nonzero actual fly intent clears `target_focus`. The helper allocates nothing and introduces no manager/resource/shortcut/history ownership.

No early return was added to the camera system. Mouse orbit/pan/zoom, UI pointer ownership, scene/chrome/gutter geometry, whole-system CameraCaptureLock run_if, easing, transform computation and terrain clamps are outside the changed production block and remain unchanged. In particular, reserving keyboard fly intent does not promise to stop an already active easing target or mouse gesture; pose-stable shortcut acceptance must start from an appropriate stable camera baseline. No shortcut injection or Undo/Redo mutation logic is altered.

The existing capture-lock test now invokes test-only temporal assertions on the actual production helper, then retains its original completed-chrome and real run_if checks. All **40** registered `#[test]` declarations remain. Installed Bevy0.18.1 ButtonInput `clear()` preserves held inputs while clearing edge flags, and `release()`/`reset_all()` support the authored sequences. Thus Ctrl/Super prefix, optional Shift prefix, Z press/release and modifier release represent distinct meaningful frames; held-Q suppression/resumption and plain axes, focused keyboard ownership, Shift speed, S first/held and diagonal checks exercise the correct helper. They are unexecuted source assertions, not the full render-feature camera system or live camera proof.

Directory rationale is additive and consistent with the narrow default: command modifiers reserve fly movement, user-facing Q remains descent, and broad input conventions remain provisional in the root-owned ledger. This review does not claim the user explicitly selected every convention or that help/runtime behavior was verified globally. Shared `C:/Users/atooz/.gemini/UI-UX-guidance.md` was applied.

Only this review was authored. Read-only source/Git/primary ButtonInput/hash inspection; no source/index/history mutation, implementation/parser/compiler/formatter/tests/build, apps/helpers, OS/process/native/input/capture, API/DB/network action. Native two-path source, evidence, decision ledger and session log are excluded and preserved.

Root may commit/build the exact approved camera source and perform appropriate integrated checks plus fresh stable-baseline Undo/Redo, both-side modifier-prefix, held movement release, plain navigation/Shift, focused text and capture behavior. Existing object/history successes and actual QA49 camera failure remain separate facts. No repaired runtime, finding closure, native, metric, peer, FULL, CI or release acceptance is granted.
