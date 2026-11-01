---
type: Review
date: 2026-11-01
commits: [afa3994, 0abe3d2, 367be14, ceb8cdc, 214045a]
reviewer: harness_advocate
scope: S6 layout policy, Results ownership and shared scene-input arbitration
---

# S6 independent review

The separate core recheck confirmed all eight requested corrections in 367be14: navigation invalidation before
reply adoption, matching header/body widths, compact budgets, Unicode-safe ellipsis, exact correlated replies,
total JSON ordering with deterministic ties, run-state reset and bounds-safe copy lookup.

The final source review through 214045a found one remaining divider-boundary defect. The interaction rectangle
insets each divider's egui grab radius, but inclusive Rect::contains admits the exact inner endpoint already owned
by egui's inclusive resize band. The source owner is applying strict containment and extending the existing
scale-two journey to the exact left, right and Results endpoints. Its new-stage live check is assigned to the
next terrain build; the preceding stage cannot establish behavior of the later correction.

The other final changes are statically sound. Camera and NodeManager share physical-pixel to egui-point conversion
and foreground-area arbitration; ray construction retains Bevy's logical cursor coordinates. ViewportRect remains
visual CentralPanel geometry. Gimbal hover reuses the arbiter. Shared actions enforce at least 24 points and 20
physical pixels. Query beacons derive from submitted-verse provenance, active-petal membership and inherited
visibility; NodeManager remains the selection authority.

The primary agent independently inspected final match-beacons, compact-edge and virtualized-200 PNGs. Blue
above-node beacons and the virtualized table were visible. The compact image also exposes a clipped Inspector
tab row, and the Owner chip wraps vertically even at ordinary size. These remain explicit follow-up visuals;
the bounded geometry matrix must not be described as exhaustive visual correctness. W3 owns role/status-chip
placement and S2 owns the remaining all-panel accessibility inventory.

Reported focused live evidence covers ordinary sidebar/Results-divider selection preservation, real viewport
pick/deselect, overlay selection preservation, wheel isolation, query sorting/errors/navigation and virtualization.
The final verdict, exact-boundary new-stage proof, exhaustive panel coverage and campaign-end checks remain pending.
No source edits, Cargo commands or app launches were performed by this reviewer.

Follow-up: `25310f1` implements strict containment and all four ppp2 endpoint journey cases. A separate bounded
security/correctness reviewer confirmed the fix and `e3a67e1` cleanup are statically clean. Cleanup pins validated
handles, attempts every Kill before waiting, uses one 15-second wait budget, disposes handles and reports failure.
One diagnostic-only limitation remains: simultaneous enumeration and process failures omit the enumeration reason.
The owner's final exact checks establish root/captured-child absence; a recycled unrelated PID was left untouched.
These follow-ups do not substitute for new-stage endpoint runtime proof or final campaign checks.

VERDICT: APPROVE WITH DOCUMENTED RUNTIME AND CAMPAIGN VERIFICATION PENDING
