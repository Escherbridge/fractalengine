---
type: Review
date: 2026-11-02
reviewer: /root/integration_review
status: source-approved-with-driver-corrections-pending
---

# Independent integration source review

The independent reviewer inspected immutable changes without editing source or running Cargo,
tests, builds, or applications. These verdicts permit integration; they do not establish runtime acceptance.

| Change | Source verdict | Evidence and remaining acceptance |
| --- | --- | --- |
| Property grid `13be1e7`, `2960514` | Approve | Stored-value copying, scoped row IDs, two-step deletion, draft/conflict handling and column helpers have no identified correctness blocker. The narrow journey counts observed controls rather than clipped hit visibility. Fresh-stage horizontal scrolling and row actions at three window sizes remain required. |
| Atomic history cleanup `1ee9c24` | Approve | Only redundant unsupported `view_set` events are removed. Covered atomic property writes retain visibility changes and no longer poison `current_state_verified` through a duplicate unsupported event. |
| Terrain finite guard `9650d69` | Approve | Display and TSV share completeness checking for finite area, cut/fill/net, lattice step and per-item values. Integrated checks remain pending. |
| Rich-data driver through `ff7f3fd` | Changes requested | Two medium assurance gaps below must be corrected and independently rechecked before candidate acceptance. |

The driver checks that the selected calibration history fact contains an integer schema revision,
but does not compare it with an independently declared expected revision. The example expects revision 1.
The corrected assertion must bind that exact revision to the intended object and committed version.

The driver also discards the log response's latest sequence. The 2,000-record ring can evict a warning
after enough lower-level entries while returning an empty filtered warning list in the same generation.
Retain sequence bounds and mark possible retention loss unknown, instead of producing a clean candidate.

The reviewer found the bounded report output handling sound within this source review: authenticated
redirects are refused before following, mutations are not retried, embedded JSON escapes `<` and Unicode
line separators, and dynamic report text uses DOM text APIs. The unfinished hostile-input smoke, real
REST/MCP scenario, same-root restart, and native chart/history interactions remain unverified.

The driver author owns the two corrections in its isolated checkout. The main execution owner retains
exclusive Cargo/application ownership. Native committed-state materialization must be integrated before
the backend history stack is enabled in main, so API writes continue to update the live scene and Inspector.
