---
type: Review
title: QA55 initial grid correction requested
date: 2026-11-04
---

# QA55 responsive property grid v1 independent source review

Verdict: **changes requested for one concrete narrow-layout boundary**. The responsive direction and shared callbacks are sound, but the frozen implementation does not yet keep the destructive controls within every supported narrow Inspector body. This is source review only; no compiler, parser, formatter, test, app, input, API, DB, network operation, or Git mutation was run. No source or frozen artifact was edited. The root and panels guidance and `C:/Users/atooz/.gemini/UI-UX-guidance.md` were applied. QA0074 remains open; no live acceptance is given.

## Exact reviewed scope

Baseline recorded by the frozen packet: `22039d5add074d4df0e18f5847286606d95407ac`.

- Manifest: 2,119 bytes, SHA256 `ce29050a87e7bb93c031e9ba4759c411d90e946bd7872e4c1eac9c23bd9cc866`.
- Source patch: 19,675 bytes, SHA256 `d2dbc85d2f6273da06e8654a8edabb9a3ddc2ade46f7233e807da6d31cdc0473`.
- `fe-ui/src/panels/inspector.rs`: 282,198 bytes, SHA256 `2e0127e60f3f8b137c0df2c7b9ef3be59dfbe60de4e29d0442df1d85a32728be`.
- `fe-ui/src/panels/property_grid.rs`: 6,781 bytes, SHA256 `6bb19366a3cef4cca93ef81280cd1e20ba9db6952873d4d4d52cef8fe8af7907`.
- `fe-ui/src/panels/AGENTS.md`: 70,958 bytes, SHA256 `6c2f3514d564b180f0c2f85fec447d1110a0a29517917855cd6d78442cd97594`.

All three frozen raw files and baseline snapshots match their manifest sizes/hashes; all three current working source files match the frozen raw files exactly. The patch and all production/test hunks were read. Pinned egui 0.33.3 source was consulted for the actual layout APIs.

## GRID-B1: wrapping cannot split an oversized destructive slot

`property_grid.rs:37-42` keeps the third action slot's minimum at the full measured Confirm + 8-point gap + Cancel pair. `actions_row` at lines 128-134 wraps the three *slots*, and `action` at lines 137-153 still allocates each full fixed slot. It neither bounds that slot by the available body width nor provides a vertical arrangement for its children. The canonical `widgets.rs:238-251` confirmation helper itself uses an ordinary nonwrapping `ui.horizontal` for Confirm and Cancel. Even the unarmed Delete allocation therefore reserves the full pair width.

This is a reachable application boundary, not an arbitrary zero-width input. `layout_policy.rs:863-865` permits the right sidebar when at least 120 logical points remain; `right_sidebar.rs:1717-1724` then allows its exact width down to that remaining width. The divider gutter, Inspector's 8-point inner margins (`inspector.rs:91-93`), vertical scroll reservation, and collapsing-section indentation reduce the actual property-table body further. The prior captured high-scale narrow layout already demonstrated a roughly 52-point Inspector body. There is no source invariant making every property body at least the measured confirmation-pair width.

At any body width smaller than `action_widths[2]`, `horizontal_wrapped` can only move that slot to a new line; it cannot shrink/split it. `fixed_cell` still requests the oversized exact allocation. Pinned `egui-0.33.3/src/ui.rs:1383-1388,1399-1410,2686-2710` confirms that these APIs wrap whole allocations and do not return an allocation smaller than the requested size. The resulting row either enlarges horizontal content again or is clipped by the ancestor; the pair cannot be wholly visible at zero horizontal offset. Removing the hint does not resolve this case. Consequently the new documentation's unqualified claim that row actions require no horizontal scrolling is not yet supported.

Minimal repair: make the compact destructive presentation adapt when its measured pair is wider than the real body, so each actual Confirm/Cancel target fits without requiring horizontal travel, while preserving the canonical confirmation ownership/reconciliation and delete callback. A vertical pair or an equally clear bounded presentation is sufficient; do not weaken confirmation or silently truncate away a control. Preserve the existing wide path. Extend the existing journey with the actual narrow body (for example a 52-point body, or the shell harness that produces it), below/above the measured pair breakpoint, and assert whole actual Delete/Confirm/Cancel rectangles intersected with hit/child/table clips at zero horizontal offset. The current 260/320/640 screen-width cases do not cover this boundary.

## Preserved behavior and positive observations

The geometry change is otherwise appropriately local. Wide mode retains the previous measured column widths and action reservations. Compact mode combines key/type, gives the value its own bounded cell, and renders one shared `row_actions` closure below the content. All callbacks were moved without changing their effects: Edit inserts the same typed `PropertyDraft`; Copy uses the same exact `clipboard_value` function and toast; Delete uses the same explicit `(delete-property, node_id, key)` confirmation salt and returns the same deferred delete action. Loading/editing/pending guards remain byte-equivalent in those callback bodies. No capability, loading result, pending/unknown write, save, mutation enqueue, parser, schema, or integer-handling code is changed.

The existing node/key `push_id`, detail-state key, draft map, expanded editor, stored JSON path, accessible action names, full-key hover, clipboard formatting and scalar preview logic are retained. The shared closure's mutable borrows finish before the subsequent draft lookup; the immutable kind closure only captures the stored value. Passing `&mut row_actions` through the existing `impl FnOnce` helpers is consistent with Rust's closure traits. No additional concrete borrow/API compile blocker was identified by inspection; compilation remains unproved.

The authored extension to the existing journey is meaningful: it observes actual leaf response rectangles/hits/clips, exercises real pointer arm/cancel/edit paths, checks stable content width and zero offset, and crosses the column breakpoint with an exact dirty draft before cancellation without a write. Its existing model readback remains explicitly a simulation. No new test declaration is introduced. These checks were read, not executed, and do not prove persisted DB history, schema enforcement, or GUI clipboard behavior in the running app.

After the narrow-slot correction, integrated compilation and the planned single final sweep remain required. Matching live verification should cover the saved QA55 default 1280 layout, the actual high-scale narrow body, whole Edit/Copy/Delete and Confirm/Cancel targets, complex-value expansion, retained drafts across resizing, disabled/loading/pending states, and actual edit/copy/delete outcomes. Until then neither QA0074 nor broader UI acceptance is complete.
