---
type: Track Spec
title: Consolidated Wave Retrospectives
tags: [chore, retro, wave_retros_20260710]
timestamp: 2026-07-10T00:00:00Z
resource: ./metadata.json
---

# Specification: Consolidated Wave Retrospectives

**Track ID:** `wave_retros_20260710`
**Type:** Chore (documentation)
**Precedent:** `code_review_retro_20260701` (a single system-wide review session
captured as spec.md + plan.md).

## Overview

This track produces one **consolidated** retrospective document
(`retro.md`) covering every completed wave, rather than a separate track
folder per wave. Consolidation was chosen over one-retro-per-wave because:

1. The waves are tightly interdependent (Wave 2's 3D editor pipeline sits on
   Wave 1's channel architecture; Wave 3's plugin system sits on Wave 2's
   scene graph bridge) — a single narrative captures the drift and lessons
   more usefully than N disconnected documents.
2. The 2026-07-10 conductor/tracks.md reconciliation pass (see git history
   around this commit) is itself the evidence base for this retro — it
   verified every track against the codebase in one pass, so writing the
   retro as one document mirrors how the evidence was gathered.
3. `code_review_retro_20260701`'s own plan.md already covers much of Wave
   1-2's retro ground; this doc extends it rather than duplicating it,
   folding in what the 2026-07-10 pass additionally found (metadata drift,
   spec-vs-code divergence on several tracks, a real bug in the conductor
   plugin's SessionStart hook).

See [./retro.md](./retro.md) for the actual retrospective content.

## Out of Scope

- Re-litigating tracks already covered in detail by `code_review_retro_20260701`'s
  plan.md (architecture review checklist, pain points) — this doc references
  that one rather than repeating it.
- Any code changes — this is a documentation-only track.
