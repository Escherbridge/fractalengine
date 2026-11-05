---
type: EvidenceIndex
date: 2026-11-04
status: source_reviewed_live_pending
title: Font-independent native button icons
---

# Reviewed source correction

The [manifest](./candidate-manifest.json), [patch](./source.patch) and
[report](./source-report.txt) describe nine payloads: eight changed files and
one new shared ui_shell/icons.rs module. Separate [correctness](./correctness-review.txt)
and [design/boundary](./design-review.txt) reviews approve the exact frozen bytes.
The [integration receipt](./integration-receipt.json) records all-before validation,
exact raw source copies and expected LF-normalized Git source hashes. Raw support
copies stay exact; normal source blobs follow Git text rules.

A typed IconButton adapter uses the pinned public egui custom-atom seam and
returns the native Button response. Fixed thin vectors replace affected
font-dependent symbols in the toolbar, section rail, hierarchy visibility,
Deselect/token dismissal and stored-value disclosure. User property text stays
text. Existing IDs, names, roles, tooltips, disabled behavior, active fills,
action handlers, permission/selection ownership and confirmations remain.

The existing journey extension is authored but unexecuted. Source approval does
not close QA0075: matching build and actual screenshots must verify icon meaning,
state contrast and 1x/2x layouts, especially toolbar fit near1160 and1280 points
with Deselect and the right cluster present. No native observer/capture, peer,
retained restart, FULL, test sweep, CI or release acceptance is granted.
