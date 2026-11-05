---
type: Review
title: Camera command ownership design review
date: 2026-11-04
---

# QA49 camera keyboard ownership: independent review

**APPROVE at functional/readability source scope; no concrete blocker.**
Reviewer `/root/legacy_helper_io_fix`, 2026-11-04, did not author this slice.
Only this report is written; no code, packet or Git change, implementation/parser/
compiler/test execution, app/OS/native/API/DB/network action.

Manifest SHA-256 `8001920495601fde46cbebf737271f8e94d82fd52c02af55cf7bcd90e63494c2`
and patch SHA-256 `57f003210bff97956490b3fe4f52ba62da41c82295bf9e4eb405322acbfe0d9e`
match. Both complete raw sizes/hashes and current source hashes match:
camera.rs55858/`136006c24d5faa6fa773872f44dad90c91fa31c7ae518acd1de5944b0b595ff2`,
AGENTS.md20846/`ec938c058d0fc09c8ca13a6692f29750406bc4338d26106824fedfddb473ca7f`.
Reviewed the full patch, actual helper/system/journey and directory rationale.

- **Ownership is enforced before direction mapping.** The private pure
  keyboard_fly_intent returns None for egui keyboard ownership or either Ctrl/
  Super side, regardless of a command letter. This includes modifier-only and
  Shift-Redo prefixes and already-held navigation keys. Shared input is borrowed
  immutably; editor command decoding/key lifetime is not consumed or changed.
  Removing ControlLeft descent resolves its dual role; Q remains descent.
- **The scope stays fly movement.** The system calls the helper at the old fly
  block and does not return early when ownership blocks intent. Existing mouse
  orbit/pan/zoom and pointer gating still run; whole-system capture run_if,
  easing and terrain constraints remain separate. Existing target_focus is
  canceled only by actual nonzero fly intent at this call site. Command ownership
  does not freeze a previously pending easing target or prevent other camera
  inputs; that is consistent with this narrow convention.
- **Plain navigation is preserved.** WASD axes, Q/E/Space, S first-press tool
  reservation, diagonal normalization and Left Shift2.5 speed are extracted
  without changing their arithmetic. Speed remains move_speed*delta_secs, then
  normalized direction*speed*sprint. Alt and right-Shift behavior are unchanged.
- **The boundary is readable and appropriately small.** One allocation-free
  helper centralizes ownership and mapping, leaving time and camera mutation in
  the system. Its direction/sprint tuple is immediately named at the single
  production call site; a new intent framework/type is unnecessary. Rationale
  belongs in the directory section, as implemented.

The existing registered capture-lock journey now invokes the production helper
through all four Ctrl/Super variants, Undo/Shift-Redo prefix/letter/release frames,
held-Q suppression/resumption, plain axes, text ownership, sprint, S first/held
frames and normalized diagonal. Local Bevy0.18.1 button_input.rs149-188,243-255
confirms press/release and clear semantics: clear advances transient flags while
preserving held keys, so these frames exercise the intended collision rather
than resetting it away. Existing capture run_if assertions remain; no new test
registration. They do not execute the full render-feature camera system or prove
OS input scheduling, editor Undo/Redo, mouse behavior or terrain rendering.

No repaired runtime or finding closure follows from this approval. Root still
owns reviewed build/integrated checks and fresh pose-stable Undo/Redo plus plain
navigation/Shift, focused text, mouse and capture acceptance. QA49's prior camera
drift evidence remains valid; no FULL, CI or release credit is supplied.
