---
type: Track Spec
title: UX QA Review — structured pass over shipped surfaces to seed the UX track
tags: [qa, ux, review, ux_qa_review_20260714]
timestamp: 2026-07-14T00:00:00Z
resource: ./metadata.json
---

# Specification: UX QA Review

**Track ID:** `ux_qa_review_20260714`
**Crates:** (review-only; findings feed a future implementation UX track)
**Alignment:** PLATFORM-KEEP · **P1** — a QA scaffold that assists the user's
own thorough review and produces the backlog for the dedicated UX track the
roadmap defers to. See `conductor/roadmap.md` (UX note).

## Vision / Why

The user will spec a dedicated UX-improvement track after a thorough QA review.
This track is the **QA harness for that review**: a structured checklist over
every surface shipped in the 2026-07 GPX/path/analytics work, plus a
findings log that becomes the UX track's backlog. It is a **review track**, not
an implementation track — the deliverable is a prioritized findings list, not
code.

## How to use this track

Walk each surface below in the running app; for each item, record: **works /
rough / broken**, plus a one-line note. Anything "rough/broken" becomes a
`findings.md` entry (severity + surface + repro + suggested fix). At the end,
the findings list is triaged into the future `ux_polish_*` implementation track.
Keep findings in `conductor/tracks/ux_qa_review_20260714/findings.md`.

## Review checklist — shipped surfaces (2026-07)

### A. Pen / path drawing (input_router, pen_autocreate, node_placement, glb_pick)
- [ ] Pen tool: click empty viewport with NO track selected → auto-creates a
      track + places first point + opens it in the Paths tab. Feels immediate?
- [ ] Subsequent pen clicks append cleanly; polyline/curve preview reads well.
- [ ] Ctrl+drag a point → raises/lowers height (Bevy Y). Sensitivity right?
- [ ] Numeric Height (Y) field per point — discoverable? precise?
- [ ] Single-point track renders a visible, selectable node (billboard quad).
- [ ] glb model picking: clicking a model's surface (not just origin) selects
      the right node. Precise enough?
- [ ] Click a track's rendered line → selects it AND opens it for editing.
- [ ] Marker drag / Shift-or-Alt+click annotate — modifiers discoverable?
- [ ] Router: no click "steals" (e.g. gimbal vs pen vs node-pick) feel wrong.

### B. Data icons (data_icons)
- [ ] Panel row glyphs (Paths tab track/point rows) legible + meaningful?
- [ ] 3D billboard markers face the camera as you orbit; readable at distance?
- [ ] Floating labels over points while editing — helpful or cluttered? Toggle wanted?
- [ ] Single-point node quad: is it too small / hard to click? (known concern —
      flat quad is a smaller hit target than the old sphere.)

### C. Track styling (track_styling)
- [ ] Color picker / thickness slider / visibility toggle in Paths tab — found easily?
- [ ] Changes apply live (no reload)? Thickness ribbon renders as expected?
- [ ] Default (unstyled) tracks look acceptable (now an unlit width-2 ribbon)?

### D. Path-asset stamp (path_asset_picker, hexon_path_asset)
- [ ] Tools panel reachable (toolbar "Tools" button)?
- [ ] Asset picker lists installed assets clearly; "Stamping onto: <track>" clear?
- [ ] Stamp along path → glb models appear at correct spacing/count/tangent?
- [ ] Spacing vs count modes both intuitive?

### E. Paths tab / GIS / general editing
- [ ] Track list: create / select / delete flow smooth?
- [ ] Selection concepts (viewport-select vs Paths-tab-edit) feel unified now?
- [ ] Inspector / annotation / GIS query panels — friction points?
- [ ] Overall: what feels slow, hidden, or surprising?

### F. Analytics-adjacent (forward-looking, note wishes for analytics_egress)
- [ ] Where would you *want* a "copy query / export" affordance to live?
- [ ] What spatial selections should be reportable (a petal, a track, a bbox)?

## Functional Requirements

- **FR-1:** Produce `findings.md` — a triaged list of UX issues found, each with
  surface, severity (blocker/major/minor/polish), repro, and a suggested fix.
- **FR-2:** From findings, propose the scope of a follow-on `ux_polish_*`
  implementation track (the user owns the final scoping decision).
- **FR-3:** Flag any **correctness** bugs found (not just UX) for immediate fix
  outside this review track.

## Constraints

- Review-only: no implementation in this track. No rustfmt, no quarantine.
- Findings feed the user-owned UX track; do not pre-empt that track's scope.
