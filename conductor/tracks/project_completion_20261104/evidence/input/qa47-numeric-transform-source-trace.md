---
type: SourceTrace
title: Numeric insertion clarification and input validation boundary
date: 2026-11-04
---

# QA47 numeric transform: source-only trace

Verdict: the full recorded Position draft was malformed, and the ordinary Inspector has a concrete validation gap that can silently apply its last valid converted value. QA47 does not demonstrate that a valid `2.500` draft was lost or that its backend transform failed. Enter is an alternative Apply trigger, not a required numeric commit step. No production edits or further input were performed.

Scope: actual QA47 stage `a16eab87032a41d012e6936776d8dcd9228b7a54`, run `20261104-162613-242`. The recorded stage manifest reports `source_dirty: true`; this is not a clean-commit provenance claim. Read-only scoped Git comparison against a16eab87 reports no current delta in the traced Inspector, sync, action, broadcast, Undo, CLI or input-bridge paths. Other agents' changes were preserved. Shared UI guidance applied.

## Decisive existing evidence

The raw UI tree is essential here; the narrow focused screenshot does not display the entire value:

| Artifact | Frame | Position (m) X, ID 3478998624401797978 |
| --- | ---: | --- |
| `qa/passes/47/owner-selected-ui.jsonl` | 3127 | `value: "0.000"`, not focused, enabled |
| `qa/passes/47/owner-transform-draft-ui.jsonl` | 9884 | **`value: "0.002.5000"`**, focused, enabled |
| `qa/passes/47/owner-after-transform-ui.jsonl` | 17184 | `value: "0.000"`, not focused, enabled |

Both actual PNGs were opened and visually inspected. Draft `162913-170-qa47-transform-draft.png` shows the caret and only the horizontally scrolled substring resembling `002.500`; this is consistent with the full invalid UI value, not evidence that the full draft is numeric. After `163017-946-qa47-transform-saved.png` shows zero and unchanged cubes. Its filename is not a save receipt.

`owner-edit-position-x.jsonl` records one click into the existing field, then exactly one `text_entry: "2.500"`. No selection, replacement, clearing or Enter command was present. `owner-scroll-apply-visible.jsonl` moves the pointer and scrolls -5; the draft UI thereafter still records the malformed full value. The Apply receipt names the real enabled `Apply node transform` button, ID 6082097978130472780, center (1020.078125,539.5), in its visible rectangle (1000,527,40.15625,25); it records one pointer click and `commands_applied: 1`. This is input application evidence, not an internal UiAction/DB receipt.

The before/after/late node payloads are byte-identical (775 bytes each, SHA256 `b093767cae08a78d35b239bb8428031ab82f2f9431e4f5c5026f7df429bef981`). The three histories are byte-identical (4165 bytes each, SHA256 `9c561b6e585142d9624a9efab699e8ed641b20b6fcfabc54a0914486ada4da9a`), with current_version 4. `owner-after-transform-state.jsonl` has the same selected node, zero ECS position, no notice and no exposed pending UI operation. The recorded input log slices contain no relevant transform error. These establish the unchanged result, not a failed database write.

## Exact production route

1. **Driver inserts text.** `tools/qa/fe.py:602–609` implements `type --into` as a focus click followed by text_entry; the CLI help at :1036–1038 promises a click, not replacement. `fractalengine/src/input_bridge.rs:410–449,529–541` turns a control-free text run into a character key consumed as egui text, with no select-all behavior. The pinned primary SDK, egui 0.33.3 `src/widgets/text_edit/builder.rs:974–983`, deletes only an existing selection, then inserts at the cursor. Its :658–677 retains a horizontal text offset to keep the caret visible. No DragValue is involved: `layout_policy.rs:966–968` constructs `TextEdit::singleline`.

2. **Displayed draft and Apply payload differ.** `fe-ui/src/panels/inspector.rs:596–625` renders the meter string (`pos_m`) and only writes the converted world string (`pos`) when `resp.changed()` AND the whole displayed string parses AND conversion succeeds. Failed parsing has no error branch. For `0.002.5000`, the prior valid `pos` remains. At :682–688 Position wires these exact two buffers. Raw Rotation/Scale at :631–651 edit the actual action buffers, so they do not have this particular dual-buffer gap.

3. **Apply does not revalidate displayed Position/Size.** `inspector.rs:710–717` queues the unparameterized ApplyNodeTransform action on a click; Enter/lost-focus at :623–625 also commits. The dispatcher `actions/mod.rs:1350–1364` checks the ordinary-single route, pending Undo and active gesture, then calls `actions::transform::apply`. Its parser (`actions/transform.rs:26–44`) parses `inspector.pos`, `rot`, and `scale`, **not `pos_m` or `size_m`**. Thus invalid displayed meter text can coexist with a fully valid old Apply payload. The advertised axis parse-error feedback in `actions/AGENTS.md:166–170` does not protect that displayed meter draft.

4. **A last-valid zero payload has a silent no-op path.** `actions/transform.rs:82–97` writes the parsed transform and marks it committed even if equal. `node_manager/mod.rs:276–286` captures before and marks the flag. `node_manager/transform_broadcast.rs:35–43,77–105` consumes it and calls Undo. `undo.rs:582–617` compares before/after and returns Ok(false) before any DB command for matching finite transforms; broadcast :100 intentionally shows no notice for that result. This concrete source path explains zero history growth without a backend error if this Apply was dispatched with the retained zero payload. The evidence has no internal dispatch/buffer trace, so do not promote that conditional explanation into a directly observed internal receipt.

5. **Post-Apply reset does not prove per-frame edit erasure.** `node_manager/inspector_sync.rs:110–134` fills raw buffers only on selection or Changed<Transform>. `inspector.rs:501–534` similarly fills displayed buffers only on selection, scale change or Changed<Transform>; it is registered in PostSelection (`plugin.rs:1102–1117`). Applying even an equal value through mutable Transform can trigger that refill. The before-Apply draft demonstrably survived through the scroll. No source/evidence requires an unconditional per-frame synchronization bug or a scroll-induced draft reset.

## Minimal next action and author scope

No new application read is necessary to establish the malformed draft: the existing complete UI value resolves the screenshot ambiguity. If direct attribution of this individual Apply to the no-op branch is required, the missing observation is the displayed and converted drafts plus branch outcome at dispatch; existing input receipts and DB readbacks do not expose that internal state. Obtain it only through a separately authorized bounded diagnostic, without replaying this attempt or claiming a failed submission.

A legitimate future numeric editing case must replace/select the existing text using ordinary supported input, observe the **full** UI `value` equals the intended valid number, then issue one chosen commit gesture. Enter may be used as that one gesture; do not send Enter plus Apply, and do not press Enter into this malformed draft as a supposed repair. A click on Apply is already an intended valid route.

Confirmed CLI semantics for a **fresh separately authorized case**, not commands executed here: focus the freshly observed field, `fe.py --run <run> key ctrl+a`, then `fe.py --run <run> type 2.500` **without `--into`**, observe its complete UI value, then one `key Enter` OR one freshly located Apply click. `parse_combo` (`fe.py:537–555`) maps ctrl+a to ControlLeft/KeyA; `input_bridge.rs:503–525,556–574` retains the modifier through the key and releases it afterward. KeyA is allowed (the clipboard exclusion at :361 covers only C/V/X). Reusing `type --into` after select-all would click again and may collapse the selection. These are source-supported semantics, not a newly executed positive edit.

The narrow code fix should validate/convert every currently displayed Position/Size draft through one shared checked path before Apply or Enter can enqueue a transform, repeat admission at dispatch, and retain an invalid draft with a field-specific inline error. Never silently substitute its last valid converted value. Preserve raw Rotation/Scale behavior, map conversion, no-AABB scale fallback, selection/route/gesture/pending ownership, checked Undo/history admission and backend guards. This is a concrete UI validation defect even when malformed input arose from driver insertion; driver replacement semantics alone would not fix it. Meaningful source journey coverage should type malformed meter text into the actual widget, assert no action/DB write and retained error draft, correct it, then verify one explicit chosen commit uses the newly validated value. Existing widget journey `inspector.rs:3863–3915` types text and Enter but never asserts malformed meter input rejects or that emitted Apply uses the displayed numeric value; it does not cover this gap.

## Evidence anchors and limits

Relevant exact SHA256 anchors: draft UI `595084cd0db5230232254238c59017282eb5c01a68d3a07375523898d39a8ef3`; edit receipt `24261c1e61ca434a0bf21cb5e36db9e5670bbcc2b1f8639e9f3bb54ec72f5e32`; scroll receipt `aa8c9e5ffabb7850fb3eccc9f96dd1a744801b410d7cbb681c5ce223b16e462a`; Apply receipt `87e4a7ccf400f340b525ff6c1389a86829863dbab7bd5ebac2e70d73d15194fc`; after UI `24d454dc404fa7b01e8392fb91c6c4e212481d83fdb740258d0523d37516f486`. Actual draft PNG 362032 bytes, SHA256 `b880d5e30c119730438720d2adc89a662a2aaabc2fb1f25a2dcca1e53b88e121`; after PNG 361796 bytes, SHA256 `9dab7607f03150bd0edbc5a56b018a90f91330c4fd4ac453e5b2b970ed9d18d3`.

This report writes only itself. No new input/replay, implementation parser, tests, compiler, formatter, apps, process/OS inspection, API/DB/network request or Git mutation. Evidence JSON was read/hash-compared; primary SDK source was read locally. Source/evidence trace is not new runtime acceptance or a positive transform save result.
