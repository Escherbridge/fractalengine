---
type: EvidenceIndex
date: 2026-11-04
status: source_reviewed_live_pending
title: Topbar compact breakpoint raised to 1248 points
---

# Reviewed source correction

The [manifest](./candidate-manifest.json), [patch](./source.patch) and
[report](./source-report.txt) describe two changed files (`fe-ui/src/ui_shell/topbar.rs`
and `fe-ui/src/ui_shell/AGENTS.md`). Separate [correctness](./correctness-review.txt)
and [design/boundary](./design-review.txt) reviews approve the exact frozen bytes.
The [integration receipt](./integration-receipt.json) records all-before validation,
exact raw source copies and expected LF-normalized Git source hashes.

`COMPACT_TOPBAR_WIDTH` is set to `1248.0` logical points so the expanded left
cluster with `BrandIcon` and `Deselect object` (`right = 743.5`) and the expanded
right cluster (`width = 489.3125`, sum `1232.8125`) never overlap at `1160`–`1247`
points, while maintaining a single selection-independent breakpoint. The
`TitleBarHarness` test populates `BrandIcon` and checks `[1024.0, 1159.0, 1160.0,
1161.0, 1247.0, 1248.0, 1280.0, 1920.0]`. Matching build and live QA63 screenshot
verification are required before closing `QA-0075`.
