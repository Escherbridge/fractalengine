---
type: Scenario
id: S08
gold: none
title: Polish visual checklist (polish-ux research "What to verify visually")
fixture: basic
source: .omc/research/polish-ux-20261031.md §"What to verify visually once the screenshot surface exists"
verified_in_pass: "01 (probe: items 2, 4, 11 partially)"
tags: [exploratory, polish, visual, rotation]
---

# S08 — polish visual checklist

Eighteen exploratory checks, rotated six per pass (README §Rotation). Each
item names the research finding ids it confirms or clears; a confirmed one
is filed with that id in its title so the polish lane can match it.

**Window sizes.** The research asks for 1280x800 and 1920x1080. The QA
window opens at 1280x720 and synthetic input cannot resize it; use
`F click 1195 20` (maximize) for the large size and record the actual
`F camera` → `window` size in the verdict. Scene-only shots
(`F shot --no-ui --width W --height H`) do not help here — the UI is the
subject.

**Role-dependent items (5, 6, 8 viewer parts).** Every verse this node can
open in a single-node run is owned (`F info` → roles all `owner`). Viewer
checks need a second node that invites this one — mark them
`NOT RUNNABLE (single-node fixture)` until a two-node fixture exists.

| # | what to set up (fe.py) | checkpoint: the screenshot must show | ids |
|---|---|---|---|
| 1 | Launch shot; `F click 236 52` ("◀ Verses") for the verse browser; 15 verses need 12 more S01 creates | Verse browser lists every verse, scrolls at 15, "1 fractal" (singular), Peer Discovery box not overlapping, role chip clear of the status bar | V-03, B-01 |
| 2 | Enter Petal A (S03 1-3), nothing selected | ID crumb and "◀ Back" do not overlap (probe 01: they do); hint line not clipped at 1280; hint readable over the grid; Inspector empty state worded for a first-time user | V-01, V-02, X-10, I-01 |
| 3 | In Petal A: `F key p`, `F key b`, `F key m`, shot after each; Brush → Polygon mode | Hint line fits for every tool; options arm has content; Brush+Polygon shows "0 point(s)" and a disabled Apply | V-02, O-01 |
| 4 | `F click 856 20` (Maps) with 3 installed maps | "Set petal map" and "Remove" columns visible at the default width, header/body aligned, rail present; on Close the viewport gets its width back (probe 01: it does not, QA-0003) | M-01, M-02 |
| 5 | Viewer role, petal with a map → "Set petal map"; shot now and 2 s later | NOT RUNNABLE (single-node fixture) | M-03, X-01 |
| 6 | Viewer role: topbar tools, node context menu, "Add proposal", Groups editor | NOT RUNNABLE (single-node fixture) | X-02 |
| 7 | `F click 172 55` (Join) → `F type "garbage"` → submit | An error stays on screen; the dialog does not just close | D-05 |
| 8 | Entity Settings (gear on a verse card) of a private verse | General tab radio is not "Public" for a private verse; Access tab copy; (viewer part NOT RUNNABLE) | D-01, D-02, D-03 |
| 9 | Volume report on a petal with no map after choosing a region; shot at 0 s and 10 s | Not "Computing…" forever | R-01 |
| 10 | S01 with 60-character names for verse/fractal/petal/node | Tree rows, cards, Maps rows, asset card ellipsize instead of overflowing | X-16 |
| 11 | In Petal A click the active verse row in the tree; before/after shots | Scene ejected? petal list collapsed? (probe 01: clicking a verse row collapses its subtree — L-01 confirmed in shape) | L-01 |
| 12 | `F click 900 650 --button right` on a node near the right/bottom edge | Context menu fully on screen; "Delete Track…" copy; "Tools panel" toast after "Edit Path" | X-15, D-12 |
| 13 | Two toasts within 1 s (the research's Copy Token → Copy curl pair writes the clipboard, which synthetic clicks are isolated from by design — INPUT-ADDENDUM 8 — so pair two other toasting actions, e.g. Save on the Portal URLs form twice) | Both toasts visible or queued, not overwritten | X-04 |
| 14 | Forced panel panic | NOT RUNNABLE (no debug hook) | B-02 |
| 15 | Launch shot | Frameless top bar: drag region, window buttons, app icon (see S07) | T-01 |
| 16 | Crop `TEXT_MUTED`/`TEXT_DIM` samples from any shot (hint line, "0 peers") | Contrast ≥ 4.5:1 body / 3:1 large; eye-toggle and "+" hit targets ≥ 24 px | X-10, X-11 |
| 17 | `F click 925 20` (Settings), `F scroll -20 1100 400` to the bottom | Timeseries explainer, "wu" units explained, jargon has tooltips | S-01, X-14 |
| 18 | `F click 1071 20` (Data), every tab | Tab fills at 360 px, Annotations not showing stale Query rows, point rows not raw coordinates | X-12, DW-01, DW-03 |
