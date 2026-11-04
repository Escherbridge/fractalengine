---
type: Review
date: 2026-11-04
title: Batch axis UI borrow compiler correction review
---

# Batch transform axes: independent one-token source review

Reviewer `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for the one-token source correction** in `fe-ui/src/editor_batch/ui.rs:88`: `text_input(ui, value)` becomes `text_input(column, value)`. No remaining inspected borrow/API or binding issue was found. This is source approval, not a successful rebuild or test result.

## Exact change and failure evidence

Independently checked retained build log `conductor/tracks/project_completion_20261104/evidence/build-attempts/10194fc5-failed.log`: SHA-256 **`c3a52a1dce4389c975527818f8ba20339b41eb94c56659aab1c890b6650af6ba`**, matching its metadata. Its actual E0502 diagnostic identifies the outer `ui` immutable capture at text_input while `ui.columns(3, ...)` holds the mutable outer borrow. It is the **batch selection transform axes** helper, not an Inspector schema row. Metadata records build session 56692, exit 1, no stage and no app launch; no new build or process observation was made here.

Compared complete raw current source with the approved physical-target source snapshot, independently verifying that snapshot against its original source receipt. The entire current file equals that original file with exactly **one occurrence** of the token argument `ui` replaced by `column` at this call. All other bytes, including CRLF endings, are unchanged.

| Whole file revision | Bytes | SHA-256 |
|---|---:|---|
| Approved pre-correction snapshot | 21153 | `2d13235da6d8b09da66b901c06eaf7af3b2af15372acb6b4f517cdf6f2bac8d1` |
| Current corrected source | 21157 | `bc0ed735f53e9c0e1ce1d84372c18f51798be634beb157bafc9515d64729a163` |

No Git operation was used for this check; the raw snapshot/receipt and retained compiler diagnostic are the source comparison anchors.

## Adapter lifetime and column ownership

Actual `layout_policy::text_input<'a>(ui: &egui::Ui, text: &'a mut dyn TextBuffer) -> TextEdit<'a>` reads only `target_size(ui.ctx())` to configure min_size. Its output lifetime is tied to the mutable text buffer, **not** to the Ui reference. It stores no Ui, allocates no response/widget ID and emits no action during builder construction.

The supplied `column` is the exact child Ui already receiving add_sized and named, created by pinned egui 0.33.3 Ui::columns. Passing its short immutable reborrow removes the closure's capture of the outer Ui. The same-column read is consistent with the already present `column.available_width()` argument; after argument construction, add_sized owns the mutable child Ui interaction as usual. No Ui borrow is retained by the TextEdit builder.

Column available-width/max(target) sizing, target height, desired_width(0), char_limit(32), actual add_sized allocation and `Selection {label} {axis}` naming remain byte-identical. The three X/Y/Z buffers and iteration order are unchanged. Since the adapter only reads the same Context's physical target policy, this argument correction does not introduce new response IDs or reset native editing/focus state; widget allocation still occurs on the same child column.

The enclosing enabled gate, actual editability condition (`role.can_edit`, pending state and source limitation), parsing/finite checks, draft storage and SelectionBatch/CheckSelectionSave actions are unchanged. There is no authority or persistence change.

Root retains rebuild and integrated validation ownership; the failed build remains failed evidence until a later build succeeds. Only this review artifact was authored. No production or Git edit, build/compiler/formatter/test, app, OS/process/input/capture, API/DB/store or network operation occurred.
