---
type: Review
date: 2026-11-04
title: Divider v4 design review
---

# VIS-003 v4 independent design and source API review

Reviewer: `/root/panel_width_design_review`, 2026-11-04.
Verdict: **APPROVED for source integration. V3-D1 is corrected; no additional
blocking design/interaction or inspected public-API issue found.** Compilation,
authored assertions and live acceptance remain separate, pending root execution.

Applied `C:/Users/atooz/.gemini/UI-UX-guidance.md`, the approved divider design
and the preserved v3 changes-required review. Inspected the frozen isolated
`.omc/worktrees/qa-panel-width-preference` source, exact delta, report/receipt and
pinned egui/epaint0.33.3 source. Main fa955 and root-owned live QA37 were untouched.
Only this MAIN review artifact was written. No production source edit, Git
mutation, compiler/formatter/build/test, application/process/input/UIA/capture,
API/DB/store or network operation occurred.

## Exact approval anchors

All ten raw source hashes independently match the author's v4 receipt inside
the checkout. All three patch digests also independently match:

| Patch | Raw bytes | SHA256 |
| --- | ---: | --- |
| Combined v2-to-v4 | 39,970 | `50980329980c0b66e1e48884aaeb97215b3ca7065249f871a9e9ec27462dfba6` |
| Review fix v3-to-v4 | 18,557 | `37f5689dc3c5754694810bf7121dcfba75308f0e2495ba66aac8747a272f1f88` |
| Full v4 source | 62,781 | `6b726d6ed18c998ffad5dcccbf87e01e8e5bbf39c09bac330f4244ddf9dc3a7f` |

The unchanged baseline is `2bce88aff512a9998eb120ddbcde8b78b4f7aa84`.
Preserved v3 full patch is
`10aed262d777c8a3ff4fb3b98e2546e6c19ca6a4c67408855255207870048c22`;
its exact v2 delta is
`dc76a397888b1cf40f65a2c158ab017da16b0e6bb2eed26c7a23dd15d8444c9d`.
The earlier v3 review remains changes-required for that earlier source revision.

| Source | Verified raw SHA256 |
| --- | --- |
| `fe-ui/src/panels/AGENTS.md` | `54b61f1587af04103b5d3873c54b7fa43b72d39bcd26bdb8bc4f13498c41c017` |
| `fe-ui/src/panels/mod.rs` | `e5f323aa4a19dcce60ac207429622df3cf3af64a147320c99c05d67cb6723b48` |
| `fe-ui/src/panels/sidebar.rs` | `d37674624a0d9e0936737849d3633d1c1d07d301aa744cd6eb556705de9f8516` |
| `fe-ui/src/plugin.rs` | `778d46a83a8c7acc136ecacfa4c67b5170acf3d8f8141035f821f3b1567f88c0` |
| `fe-ui/src/ui_shell/AGENTS.md` | `ebf958c6ad8c47db522a01814a87b840cc12a69e7c47156b6326ed9f97c817cc` |
| `fe-ui/src/ui_shell/layout_policy.rs` | `d1a816a86e2266bb7a9394b6a93fe323b2e250b8fc3e03cc7d4cf224f4f8707b` |
| `fe-ui/src/ui_shell/left_sidebar.rs` | `93d16280c00eacba87d50404aca5671e9bc5a4de9aa368fce84d6221220b0cde` |
| `fe-ui/src/ui_shell/results_dock.rs` | `8e331af7f7a66723dde3c23bf7783e6190ac7a30d19c7c1a1ba19ba364eeb4f8` |
| `fe-ui/src/ui_shell/right_sidebar.rs` | `d87052bfcd787b36bc420732f4f492ad5d5bc45cbb705192c225ce276c2a1472` |
| `fe-ui/src/ui_shell/topbar.rs` | `d37307781d736472dc8e15a1def3a3b3d556df9f34bf119116b808b89b33a25e` |

## V3-D1 correction and interaction geometry

`results_dock.rs:205-224` now inspects actual preallocation available geometry
against the viewport. Each bordering side receives max(original8,checked gutter);
an edge without a neighboring side owner retains its native8point margin. The
top gutter, native zero-stroke frame, remaining margins, height bounds/default
and actual current-pass edge recording remain. This closes the concrete1x
two-pixel overlap and1.25x endpoint contact identified in v3 without guessing
open flags, preferred widths or target positions.

Results and the existing left/right panel bodies also intersect inherited clip
with their actual inner content rectangle before rendering widgets. This is a
necessary part of the geometry fix: pinned ScrollArea expands content clip by
the normal three-point clip_rect_margin and then intersects the parent clip.
A margin alone would let scrolled targets reclaim some gutter. The explicit
parent clip now preserves the physical gap for table cells, headers, nested
scroll areas and scrollbars. Native popup layers retain their independent clip
ownership. No layout-cost stroke, outer width adjustment, new resize controller
or preference write provides the clearance.

The production changes since v3 are these bounded clipping/margin corrections
plus removing an unused mut on the public divider Ui. The topbar delta only
converts two authored endpoint checks from egui Vec2 to the required Bevy Vec2;
it does not change production routing geometry or expected outcomes. The
corrections remain at the existing interaction owners and are readable without
a new global layout manager or speculative abstraction.

## Public API and authored journey review

The extended existing journey now constructs64rows and24columns using real
ResultsDockState::complete and the production native Results renderer. It
requires overflow on both axes and visible first-column sort/data controls.
The visible hit calculation uses each control's hit rectangle intersected with
its actual clip, then checks X and Y clearance. This addresses the empty-v3
fixture's failure to reach the table.

The cfg(test)-only table geometry accessor copies the actual public
ScrollAreaOutput inner_rect, content_size and state.offset; it does not fabricate
scrollbar geometry or add a production automation API. The hover probe uses
public Context::interaction_snapshot and its public hovered IDs, followed by
public read_response. Pinned0.33.3 confirms these APIs and confirms native
scrollbars advertise click-and-drag with their real interaction rectangles.
The probe filters the actual hovered scrollbar shape/sense and then requires
a real pointer drag to change the corresponding stored table offset. It derives
no SDK scrollbar/resize ID and does not treat a Frame Response as a resize.

Both-side, left-only, right-only and no-side populated profiles are authored at
0.75/1/1.25/2x, alongside the retained same-Context scale restoration sequence.
They include assertions for native8point margins on edges without an owner,
expanded/hovered scrollbar clearance and real offset changes. This is meaningful
authored coverage of V3-D1. It has NOT been executed, and source/API inspection
does not establish that every runtime assertion or hover probe will pass.

## Previously reviewed contracts retained

- DividerMetrics remains one finite value object: stable5point baseline,
  radius=max(5,10/scale), checked ceil(radius+1/scale) gutter, refreshed every
  pass before the old one-time style return. Complete strips are>=20physical
  pixels by construction; invalid/unrepresentable geometry fails closed.
- App-owned side responses retain actual displayed outer edges, normal
  Background hit testing, stable public IDs, thin visuals and actual clipping.
  Native Results alone receives the temporary SDK radius under RAII restoration.
  Other SDK owners keep their existing radius; no portal/modal permission gate
  or broad native action route changes.
- Left/right zero-stroke inner budgets remain220/320 by default. The hierarchy
  floor subtracts the actual gutter and existing scroll reserve, and the deep
  action row wraps. Real both-axis overflow remains available.
- The central interactive overlay child is inset and clipped; its parent actual
  CentralPanel response remains the visual ViewportRect and camera projection
  budget. The actual same-pass Results edge handles close-in-render correctly.
  Plugin routing reads explicit metrics after the scoped SDK radius restores.
  Camera and NodeManager retain shared strict endpoint ownership.
- Width preview/commit logic remains the reviewed v2 state machine: actual
  displayed baseline, real visible delta, release-only commit, stationary/cap
  preference preservation and cancellation on Context/scale/geometry/pass loss.
  First current close-pending eligibility still precedes left prepare, right
  close gating remains, and held-input suppression/focus/Escape/modal/animation/
  portal/quarantine behavior has not been bypassed.
- No checker exemption, fake AX Focus/numeric action, claimed keyboard resize,
  A1/A2 source change, helper grant or raw action injection was introduced.

## Remaining execution and live gates

This approval unblocks source integration and the root-owned integrated sweep;
it does not certify a clean build or passing tests. Actual exported target
rectangles must still be measured after clipping and overlapping-widget hit
testing. Exercise Options/eye/rail/table/scrollbar/header/Browse-all/Back edge
clicks, not only centers; inspect default/constrained/deep/long-name/Results
screenshots and perform high-scale horizontal-scroll-and-invoke.

Retain prior fa955 restoration/selection/camera evidence and recheck affected
default/custom widths, stationary/cap-only motion, geometry/context changes,
first close/modal transition, animation and held release. Actual minimize while
dragging and restore remains a live event/lifecycle gate; source inspection did
not exercise it. Pre-existing6point Results column splitters remain outside this
side/top-divider correction and may legitimately trigger the unchanged checker.
Do not label incomplete layout coverage or this source approval a full QA pass.
Preserve the failed QA36 packet and root's frozen QA37 evidence.
