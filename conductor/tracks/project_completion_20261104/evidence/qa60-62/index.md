---
type: EvidenceIndex
date: 2026-11-04
status: in_progress
title: QA60-62 painted icons, topbar breakpoint matrix and native observer packets
---

# Focused evidence and reviews

Raw packets remain at [QA60](../../../../../qa/passes/60/packet-manifest.json),
[QA61](../../../../../qa/passes/61/packet-manifest.json) and
[QA62](../../../../../qa/passes/62/packet-manifest.json). They are frozen; do not
rewrite them. File-only source-copy receipts identify exact original review bytes
preserved as `.txt`. This index is a context document, not a packet replacement.

- [QA60 independent visual review](./qa60-visual-review.txt) and [independent full evidence review](./qa60-full-evidence-review.txt): all 10 PNG screenshots on `9498869e` confirm crisp vector icons with zero missing-glyph squares (`□`) at `1x` and `2x`, clear selected tree row (`QA60 icon probe`) at both `1x` and `2x`, and `whole_targets_fit: true` on `grid_number` across all 9 matrix cases. At `1160x720@1x` and `1161x720@1x`, `Deselect object` (`right = 743.5`) overlaps `Toggle Maps section` (`55.5625px`) and `Toggle Settings section` (`9.25px` / `8.25px`; gap `-72.8125px` / `-71.8125px`) because `topbar.rs:742` switches from compact to expanded at `1160.0` instead of `>= 1248.0`. `QA-0075` remains open for that breakpoint overlap.
- [QA61 independent full review](./qa61-full-evidence-review.txt): separate-command launch exited and closed its Windows Job Object before `native observe` ran; controller truthfully returned `native_owned_boundary_unavailable` (`attempt_spent: null`, `0` workers spawned).
- [QA62 independent full review](./qa62-full-evidence-review.txt): single-session launch + two `native observe` calls on live `PID 127884` (`9498869e`) passed controller admission, bootstrap ledger verification and `Process` image/session/token checks, then cleanly exited (`helper_exit_code: 2`, `terminal_code: "worker_exited"`, `attempt_spent: false`) with structured `fe-native-worker-result-v1` `code: "desktop_refused"` because the host `exebox` sandbox runs worker threads on desktop `exebox-*` rather than `Default`.
- [QA59 divider eligibility source review](./qa59-divider-eligibility-source-review.txt) and [design review](./qa59-divider-eligibility-design-review.txt): independent approvals of the exact 5-file read-only `DividerEligibility` diagnostic candidate (`manifest.json` SHA-256 `5d596db66f3288b38aca4e6f3ca2c932c50d6a0a8326fd4b5e75ddc339756029`).
- [Proposed P2P/relay/query lab review](./p2p-relay-query-lab-review.txt): independent approval of [p2p-relay-query-lab.md](../../p2p-relay-query-lab.md) and its 3 exact `evidence/deployment-fixtures/` files.
- [QA60 freeze receipt](./qa60-packet-freeze-receipt.json), [QA61 freeze receipt](./qa61-packet-freeze-receipt.json) and [QA62 freeze receipt](./qa62-packet-freeze-receipt.json).
- [Independent Conductor publication review](./conductor-publication-review.txt): verifies `.gitattributes`, `evidence/qa60-62/`, `p2p-relay-query-lab.md`, `qa/findings.jsonl` (`QA-0075`), `checkpoint.md`, `metadata.json`, `plan.md` and frozen packets `qa/passes/60..62`.
- [Prior checkpoint raw bytes](./checkpoint-before-qa62.txt) and [archive receipt](./checkpoint-archive-receipt.json): preserve the original checkpoint bytes independently of normal context-markdown Git EOL rules.

No FULL coverage, final integrated sweep, native UIA positive acceptance, real-peer
asset/render/pick, retained restart, CI or tagged release acceptance is granted.
