---
type: AcceptanceChecklist
date: 2026-11-04
status: pending_execution
title: Screenshot inspection for hierarchy and changed workflows
---

# Visual acceptance

Record actual stage/source identity, window dimensions, UI scale, navigation,
selection, open dialogs, image path and action result. Inspect the full image;
retain the uncropped original if a detail crop is used. Record failures before
fixing them. A passing locator or layout-check result does not close a visible
defect, and a screenshot alone does not establish persistence or authority.

## VIS-001 hierarchy cards

Baseline: [inspected 1280x720 screenshot](./evidence/hierarchy-card-wrapping-before.png),
scale1, Genesis Verse, no active Fractal/Petal or selected node. Petal count wraps
one character per line. A corrected screenshot must make the count readable in
its own line and preserve usable card name, Options and navigation.

- [ ] Review and integrate the isolated viewport layout correction.
- [ ] Build/seal current source; do not use a stale-stage override.
- [ ] At 1024x720, 1280x720 and 1600x900, inspect Verse, Fractal and Petal browse states.
- [ ] Repeat affected narrow states at UI scale1.25 and2; actions remain reachable through the intended scrolling/layout behavior.
- [ ] Inspect short and long names, singular/plural counts, empty lists, multiple card rows and the Create card.
- [ ] Verify breadcrumbs, title and first row do not overlap, truncate into neighboring controls or leave accidental giant gaps.
- [ ] Exercise card Open, Options and legitimate Create actions; menus and dialogs remain in bounds and preserve target identity/role behavior.
- [ ] Check the sidebar and right Inspector/Options still fit, distinguish selection and leave a usable viewport.
- [ ] Preserve before/after images, actual state and layout-check limitations; independently review the evidence before closing VIS-001.

## Every changed workflow

- [ ] No broken character wrapping, clipped labels/values/actions or overlapping panels.
- [ ] Readable type, contrast, selected/primary/partial state and chosen icon colors.
- [ ] Appropriate spacing/alignment and reachable keyboard/pointer targets.
- [ ] Loading, empty, error, disabled, confirmation and cancellation states are clear.
- [ ] Scrolling stays with the intended panel; controls do not move unexpectedly after selection or acknowledgement.
- [ ] Viewport/scene captures contain the expected geometry and selection indication; black/blank output is investigated.
- [ ] Actual behavior and data readbacks agree with the screenshot and the current role.

Apply the shared `C:/Users/atooz/.gemini/UI-UX-guidance.md`. New defects remain
tracked until corrected and verified; do not silently mark a required state
unavailable to obtain a clean pass.
