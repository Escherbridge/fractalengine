---
type: Review
date: 2026-11-04
title: Hierarchy card layout independent source review
source_artifact_sha256: 611af1e5042fee434abadea919e19fade6d2a24d6e5757dbe218b6178304b389
---

# VIS-001 hierarchy-card layout: independent source/design review

Reviewer: `/root/legacy_helper_io_review`, 2026-11-04.

**APPROVED for source integration.** No blocking source/design finding in this
bounded correction. VIS-001 still requires a parent-owned production build and
fresh live layout/interaction evidence before closure.

## Reviewed source and guidance

Read and applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`: readable responsive
structure, clear actions, existing conventions, semantic controls, small
architecture and preservation of permission/recovery behavior. Inspected the
actual retained baseline PNG
`conductor/tracks/project_completion_20261104/evidence/hierarchy-card-wrapping-before.png`.
It visibly places the Genesis Fractal count in a narrow vertical character column
beside its title/Options controls. The browser title also has the old fixed gap
below its breadcrumb. No new screenshot or measured improvement is claimed.

Isolated checkout `.omc/worktrees/qa-hierarchy-card-layout`, baseline
`1d3297de4c36496c05476f5bf178180bc53a4565`. Only the two intended tracked files
were dirty. SHA256 of captured
`git diff --no-ext-diff --no-color -- fe-ui/src/viewport.rs fe-ui/src/AGENTS.md`:
`614f26aff0d29ba6e274a7e46f43bd112ee837cc47ddd7b292e43a3ef1c7e6c9`.

| Raw working file | SHA256 |
| --- | --- |
| `fe-ui/src/viewport.rs` | `c1f6254b0254a20fb8804acd2dcb98b011522ecd4c6ae4fdebe2f8ce88955c9b` |
| `fe-ui/src/AGENTS.md` | `c1cfb4fc4396aa64a677d19918dfd17e12623f4189b238b0557fbde527600a40` |

`git diff --check` for those paths returned 0. Test-attribute declarations in
viewport.rs remain 0 before and after; the existing test-only render journey was
extended rather than adding another test declaration.

## Source reasoning

The shared `browser_card` allocates a child with explicit top-down layout before
calling Frame::show. Pinned egui 0.33.3 `containers/frame.rs:392-407` creates its
content child without overriding the inherited layout. The prior card frame was
inside a horizontal Grid cell, so its row and subsequent label inherited that
flow. The new explicit vertical boundary puts the count below the name/Options
row while retaining the Grid cell as one card. The extraction is readable and
does not introduce state or a competing layout framework.

Width accounting now subtracts `frame.total_margin().sum().x`. The pinned
Frame::total_margin implementation includes inner margin, stroke width on both
sides and outer margin, so the frame's total width agrees with the computed Grid
column width instead of exceeding it by the stroke. The name reserves the actual
26px Options width plus current item spacing. Names and counts use explicit
truncate mode, overriding the shared scroll adapter's wrap mode; hover text
retains complete values. The actual Options control keeps its existing 26x28
target. The 80px minimum card gives the content row usable width at current
spacing; unusually narrow containers can scroll horizontally instead of forcing
that target into the remaining sliver. Both axes retain the existing bounded,
named scroll adapter and stable browser/Grid keys.

Pinned `ui.rs:2496-2520` shows scope_builder's response comes from the actual
child min_rect. `browser_body` therefore starts the title/cards below the rendered
wrapped breadcrumb height, rather than relying on the former 60/62px reservation.
The early return after breadcrumb navigation still prevents the old branch from
being rendered. Breadcrumb button widths are derived from text and button
padding, bounded to the existing 32-140px range and available container width;
the 26px height, full accessible names, full identity hover and route methods are
retained. Overflow and extreme compact cases still need actual live verification.

All three hierarchy levels call the same card renderer. Their card construction,
index/ID mapping, Open actions, target-bound EntityDetails context, Create parent
IDs, Manager advisory checks and Join action remain semantically unchanged.
No role grant, DB command, stored authorization rule, hierarchy mutation or
navigation-manager contract changes in the reviewed diff. Opening Options remains
available for the existing unresolved/read-only role path, with permissions
resolved by the target dialog/backend rather than the active navigation role.

New-item labels truncate while preserving their existing full accessible names
and disabled-role explanation. Moving the empty-state text inside the scroll
keeps it within the same bounded content flow. The additional directory-level
documentation describes these layout choices without adding verbose inline
rationale.

## Authored coverage and remaining acceptance

The existing `browser_access_render_journey` adds 180x480 and 360x480 to the three
original sizes and long Fractal/Petal names. It inspects actual painted child
count text for one row, horizontal containment and placement below a real Options
control. Existing actual pointer-driven Options, bounded scroll and empty Join
checks remain. These are authored assertions only and were not executed here.

The additions cover zero/singular counts, not a separately authored large-number
case. They also do not directly exercise a container narrower than the 80px card
minimum or all runtime font/scale choices. Those limits do not block this source
correction, but must remain explicit in the final layout matrix. Before VIS-001
closure, capture the original 1280x720 state plus narrow/scaled states from the
new matching stage, verify all three browser levels, actual Open/Options/Create
and ancestor navigation, full values through hover/accessibility, responsive
column changes and the horizontal fallback. Run the existing journey in the
campaign's integrated sweep.

This review performed read-only source/pinned SDK/Git inspection and baseline
image viewing, then wrote this review artifact. No compiler, Cargo, formatter,
tests, app, OS process inspection/input, network, API/DB/store access,
production-source edit or Git mutation was performed.
