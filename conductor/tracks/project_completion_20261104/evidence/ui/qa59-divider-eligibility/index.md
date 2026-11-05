---
type: EvidenceIndex
date: 2026-11-04
status: source_reviewed_live_pending
title: Read-only divider eligibility diagnostic for layout-check
---

# Reviewed source diagnostic

The [manifest](./candidate-manifest.json) and [patch](./source.patch) describe five
changed payloads (`fe-ui/src/ui_shell/layout_policy.rs`,
`fe-ui/src/ui_shell/right_sidebar.rs`, `fractalengine/src/qa_debug_bridge.rs`,
`fe-ui/src/ui_shell/AGENTS.md` and `fractalengine/src/AGENTS.md`). Separate
[correctness](./correctness-review.txt) and [design/boundary](./design-review.txt)
reviews approve the exact frozen bytes. The [integration receipt](./integration-receipt.json) and [independent integration review](./integration-review.txt)
records all-before validation, exact raw source copies and expected LF-normalized
Git source hashes.

`DividerGates` and `DividerEligibility` factor existing registration-time splitter
gates into a pure value object without adding a second gesture owner or relaxing
any modal, popup, focus, Escape, close-pending or quarantine check. `layout-check`
exports `divider_diagnostic_version: 1` and at most two pass-local `dividers`
records (`id`, `side`, `pass`, `eligible`, `response_enabled`, `blocker` and the
nine boolean gate inputs).

The width-journey additions are authored and unexecuted. Source integration does
not establish QA59's historical failed guard or live `520`/`52` resize acceptance;
a matching build and live pass are required.
