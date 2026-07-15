---
type: QA Findings
title: UX QA Review — findings log
tags: [qa, ux, findings, ux_qa_review_20260714]
timestamp: 2026-07-14T00:00:00Z
resource: ./metadata.json
---

# UX QA Review — Findings

Log issues found during the review here. One entry per finding. When the review
is done, triage these into the follow-on `ux_polish_*` implementation track.

**Format per finding:**

```
### [SEVERITY] Surface — one-line title
- **Surface:** (pen / icons / styling / stamp / paths-gis / analytics)
- **Repro:** how to reproduce
- **Expected:** what should happen
- **Actual:** what happens
- **Suggested fix:** (optional)
- **Severity:** blocker | major | minor | polish
```

Severity guide: **blocker** = can't complete the task · **major** = works but
painful/confusing · **minor** = small friction · **polish** = cosmetic/nice-to-have.

---

## Findings

_(none yet — add as you review)_

---

## Candidate UX-track scope (fill after review)

Once findings are logged, summarize the themes here → this becomes the proposed
scope for the user-owned `ux_polish_*` track.

---

## Outstanding decisions feeding the UX track (2026-07-15)

UX/product-surface entries mirrored from the session-end decision register. Authoritative copy (full context, defaults, ratification checklist): [`../outstanding_decisions_20260715/spec.md`](../outstanding_decisions_20260715/spec.md). Resolve there; these lines are pointers, not a second log.

- D-28 (register): copy-for-BI card implemented in the GIS panel's Export tab (`gis_panel.rs` → `egress_card.rs`); API base URL is an editable field defaulting to localhost:8765 — ratify placement.
- D-29 (register): the hands-on GUI review + ux_polish FR-2 scoping are user-owned — schedule the pass.
- D-30 (register): may an agent pre-seed this findings.md with static-analysis candidates, or is the log purely user-authored?
- D-31 (register): where does the analytics-egress affordance live, and which spatial selections (petal / track / bbox) are reportable? Feeds analytics_egress Phase 4 / checklist §F.
- D-32 (register): inspector scope under repositioning — is FR-3 petal/fractal/verse inspection still wanted; cut first FR-4 delivery to a read/write role list?
- D-33 (register): inspector tabs folded to shipped {Properties, ApiAccess, Query} + new Access tab (wave default; active_tab persists across selections) — still open: multi-selection behavior, panel resizability.
- D-34 (register): confirm BIM FR-5 wall-binding is superseded by GPX rip-walls; amend spec.
- D-35 (register): is sign-first-then-promote petal-wide primitive materialization (async property fetch on cold cache) acceptable v1 behavior?
- D-36 (register): path-asset picker end state — shipped asset-node/blob:// picker vs hexon-ref semantics (spec OQ1, decide during/after FR-6); FR-1a still gated on lifting the ImportGltf quarantine.
- D-37 (register): sidebar-toggle button removal + pre-LogPlugin logging style — proceeding on plan defaults; veto now if wanted.
- D-38 (register): click-to-place surface semantics — Y=0 plane vs terrain-surface snap (interacts with node_placement_z_axis follow-up).
- D-64 (register): "Get shareable link" ships as a copyable curl command, not an in-app call — follow-up is a fe-ui→fe-api client seam wiring the button to POST /api/v1/query/share.
- D-65 (register): a node with both asset_path and a primitive descriptor renders the GLTF (asset wins) — flag if primitives should win.

(The single-point-node click-target concern is already on this track's checklist §B.)
