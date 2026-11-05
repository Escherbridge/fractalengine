---
type: Review
title: QA55 initial grid design review
date: 2026-11-04
---

# QA55 responsive property grid: independent design review

Verdict: **APPROVED for the exact frozen three-file source. No design, readability or reuse blocker found.** This is source approval only. Compilation, journey execution, screenshots of the corrected grid, permissions and actual edit/copy/delete acceptance remain parent-owned work. QA-0074 is not closed by this review.

Applied root AGENTS.md, fe-ui/src/panels/AGENTS.md and the shared C:/Users/atooz/.gemini/UI-UX-guidance.md. The author is a different agent. Only this report is written; source and other work are preserved.

## Exact reviewed source

Frozen manifest: `.omc/research/qa55-responsive-property-grid-v1/manifest.json`, 2119 bytes, SHA256 `ce29050a87e7bb93c031e9ba4759c411d90e946bd7872e4c1eac9c23bd9cc866`.
Frozen patch: 19675 bytes, SHA256 `d2dbc85d2f6273da06e8654a8edabb9a3ddc2ade46f7233e807da6d31cdc0473`.
Baseline: `22039d5add074d4df0e18f5847286606d95407ac`. All three baseline snapshot sizes/hashes match the manifest; all current sources match their frozen raw copies and hashes exactly.

| Path | Bytes | SHA256 |
| --- | ---: | --- |
| fe-ui/src/panels/inspector.rs | 282198 | `2e0127e60f3f8b137c0df2c7b9ef3be59dfbe60de4e29d0442df1d85a32728be` |
| fe-ui/src/panels/property_grid.rs | 6781 | `6bb19366a3cef4cca93ef81280cd1e20ba9db6952873d4d4d52cef8fe8af7907` |
| fe-ui/src/panels/AGENTS.md | 70958 | `6c2f3514d564b180f0c2f85fec447d1110a0a29517917855cd6d78442cd97594` |

The actual before image `qa/passes/55/screenshots/175750-365-qa55-alpha-property-grid.png` was inspected: the narrow Inspector shows clipped Value content and the horizontal-scroll hint, with row actions outside the visible body. It establishes the baseline defect, not acceptance of new source.

## Patterns used well

**Layout policy / Strategy:** property_grid.rs:20-75 computes the measured wide minimum once and selects one local geometry policy. The wide branch retains readable Key/Type/Value/Actions widths. The compact branch spends actual available width on Property and Value, then gives the action slots a separate wrapped row. This removes the forced wide floor without introducing a second property model, public configuration surface, dependency or general table framework. The private compact flag is adequate for these two closed layouts; a trait hierarchy or builder would add work without a concrete benefit.

**Shared behavior through a borrowed callback:** inspector.rs:1896-1939 defines one row_actions closure for both layout branches. Its Edit, exact stored-value Copy and canonical two-step Delete bodies retain the previous conditions and effects. Geometry helpers do not acquire database, selection or mutation ownership. Keeping the action callback local makes captured editor/actions ownership explicit and avoids duplicating confirmation or permission-related branches.

**Explicit allocation boundary:** property_grid.rs:104-164 reuses fixed_cell for content and reserved action slots. Summary cells reserve two control heights in compact mode; Edit/Copy and the complete Confirm/Cancel group retain measured widths and shared target height. Wrapping moves whole slots, rather than squeezing their targets into the value cell. The unchanged expansion seam uses the captured visible x-range for typed editors and stored compound details. Source intent is clear; actual target geometry still needs execution and rebuilt visual inspection at required widths/scales.

## State and interaction boundaries

The outer node/key push_id scope remains, and detail_id is still computed before responsive row containers. Typed drafts remain in the same key-addressed PropertyEditorState map. Preview/clipboard formatting, value parser, pending/readback lifecycle and external manager/action routing are unchanged by the patch. Full keys remain in hover text and contextual action names.

Do not equate those stable logical scopes with byte-identical leaf widget IDs across different container layouts. Compact and wide action containers may produce different leaf IDs. The unchanged widgets::confirm_button uses an explicit node/key arm identity, while native_accessibility::Confirmation::reconcile disarms when actual Confirm/Cancel IDs change. This preserves the existing safe refusal boundary; the patch does not promise that an armed confirmation survives a breakpoint. Dirty draft retention is a separate, explicit model guarantee.

## Authored evidence and remaining acceptance

The existing Inspector journey now checks complete normal and armed leaf rectangles against actual hit/child/table clips at zero horizontal offset for 260/320/640-point cases, rather than merely counting controls or moving to the right edge. Existing exact clipboard and real confirmation/editor actions remain in the journey. The added breakpoint transition checks the same dirty draft buffer before returning to editor-fit/cancel/no-write checks. This is useful behavioral coverage and adds no test registration; it has not been run here.

Root still needs a matching committed build, corrected narrow/wide/scaled screenshots, actual actions including wrapped Confirm/Cancel, compound expansion and dirty/pending behavior. This review establishes no executed test, visual, accessibility-delivery, persistence, peer, FULL, CI or release result. No implementation parser, formatter, compiler, tests, app/native/OS/API/DB/network action or Git mutation occurred. Read-only source and artifact inspection, hashing and the baseline image view were the only verification operations.
