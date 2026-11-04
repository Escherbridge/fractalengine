---
type: Review
date: 2026-11-04
title: Native navigation foundation source review
---

# NAV-1A-R1 v2: independent functional and security source re-review

Reviewer: `/root/completion_scope_review`, 2026-11-04.

**APPROVED for integration of the NAV-1A foundation only. NAV-1A-R1 is corrected.** No remaining blocker was found in this narrow correction. This does not approve NAV-1B or the helper/operator phases, and it supplies no build, test or runtime acceptance credit.

Reviewed frozen `.omc/worktrees/qa-native-navigation-helper`, unchanged base/HEAD `fa9555632380977c41fe89ac984fcb491c47295a`, the v2 report and hashes, the exact additional patch and preserved original source. The additional patch SHA256 independently matches `2073183fdd0d2159bdd97ffcbd98d86d8351981ff33469d78de86b77430e0f1f`. All 14 current contribution files match the v2 raw byte/hash manifest. Original v1 manifest SHA256 remains `7b2ee924a7f0657660d6cde8d875e0cc444dc01a4eec52987f82301d7b7c7664`.

No production/Git mutation, compiler/build/formatter/test, app, process/OS/native input, API/DB/store or network operation occurred. Only this main review artifact was written. Existing source approvals and limitations are inherited from `qa-native-navigation-nav1a-source-review.md`, except its single R1 blocker, which is corrected here.

## Actual production correction

`fe-runtime/src/app.rs::PendingApiRequests::try_deliver` now checks `DbResult::NativePetalViewRead { .. }` and returns false before Error, RequestFailed, family dispatch or the legacy uncorrelated FIFO fallback. The exclusion applies to both successful and unavailable native outcomes without relying on a future UI pump, a ReplyKind omission, an HTTP-route assumption or ordering. No other result-routing behavior changes.

This fixes the exact reviewed failure: a native proof cannot deliver to or consume an unrelated API waiter through the generic None-family path. The result remains on the ordinary UI result stream for its future private owner; declining it at this router adds no action or permission. No new public route, ReplyKind, generic authority framework or native input path is introduced. The short branch uses the existing enum by reference and is source/type coherent with the surrounding by-value routing.

## Meaningful existing-journey extension

The existing `result_without_a_waiter_is_not_delivered_and_leaves_waiters_untouched` journey now creates a real legacy FIFO entry via `enqueue_for(DbCommand::RenameEntity)`, explicitly confirms that command is unmapped, and offers success plus unavailable NativePetalViewRead results to the actual production router. Each must return false, leave the FIFO length at one and leave the actual oneshot receiver Empty, rather than Closed or delivered. It then delivers and checks that waiter's genuine EntityRenamed response. The separate Ping receiver also remains Empty until the actual Pong arrives.

This directly exercises the previously vulnerable fallback instead of merely placing a mapped Ping waiter in a different queue. It does not claim RenameEntity has a new HTTP endpoint. The original 22 registered tests in app.rs remain 22; no declaration was added. The authored assertions were read, not executed, and do not establish a passing test result.

## Preservation checks

All 13 original v1 file snapshots independently match their original byte lengths and SHA256 values. The original v1 report and manifest are byte-identical to their preserved copies. Twelve original contribution files are still byte-exact in the corrected checkout; the remaining original file, runtime AGENTS.md, retains the entire original raw body as a prefix with only the documented additive routing note. The added app.rs snapshot matches its original HEAD after ordinary CRLF normalization. The extra patch contains only that router branch, the existing-journey assertions and the additive runtime rationale. Read-only git diff --check passes.

Therefore the previously reviewed DB-local Viewer/ancestry/name transaction, minimal source-fence DTO, private 16-target/one-flight owner, immutable original one-second deadline, nonblocking backpressure and late-result/current-time checks are unchanged. Affected Rust test declarations total 63 only because app.rs's existing 22 declarations are newly included in the enumerated file set; this is not an increase in registered tests.

## Foundation-only approval

NAV-1A can proceed to parent-owned integration while preserving unrelated main contributions. Actual integration, compilation and final execution remain separate gates. The code still installs no owner/result pump, positive navigation profile, sidebar target tag, typed Snapshot/Delivery/semantic-click binding, final callback guard or operator helper.

NAV-1B must wire the original immutable proof and fresh-clock predicate through actual render completion, annotation, Last, PreUpdate, begin_pass and final Petal callback before deselection/navigation. NAV-2/3 must implement the reviewed original-provider/element/pattern ownership, watcher before discovery, durable pre-worker reservation and pre-Invoke intent, all-outcome process latch and uncertain worker-lifetime handling. All-Verse bootstrap, selected Inspector/node/group support, GLB/render evidence and WGC freshness/capture remain pending. This correction closes the R1 source blocker, not a recipient navigation or campaign QA gate.
