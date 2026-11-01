---
type: Review
date: 2026-11-01
commits: [572da0b, c002e9f, d1b6e57, eb0f263]
reviewers: [harness_security_review, harness_advocate]
scope: S4 confinement, ownership, design and focused live evidence
---

# S4 independent review

The scoped security recheck found all four prior findings corrected. The design recheck approved the final
interaction boundaries and the d1b6e57 dialog-replacement cleanup. Campaign-end test and lint execution is pending.

QA save/copy uses a secure sibling temporary file and atomic replacement; it does not validate then reopen an
unpinned destination. Path validation rejects ambiguous Windows/device/stream/traversal/reparse forms. Input-on
startup without a valid QA root fails closed instead of opening native dialogs. Queue admission validates the
entire candidate batch and capacity before mutating existing answers.

Read pins follow actual operations: GLTF retains its complete shared lease through the awaited DB import;
GPX/Hexon queued work owns DialogPath through reading; deferred exports own it until atomic writing. Manual GLTF
submission revalidates. There is no permanent provider cache. UiManager.replace_dialog releases abandoned form
ownership while an already dispatched import retains an independent lease.

One provider Strategy serves five call sites. TopbarState owns its private mailbox, refuses replacement and does
not let UI-scale requests erase chrome actions. OS/caption/debug close requests share confirmation. The client
uses one monotonic wait helper; REST uses one response formatter. No broader class hierarchy is needed.

Pass03 supports real Browse/cancel/refusal, persisted GLTF import, UI asset export/overwrite, path refusals,
three-size geometry, maximize/minimize/restore, ppp1.25 named clicking and close-confirm/cancel behavior. The
design reviewer independently inspected export and caption-confirmation PNGs and read both isolated files in
the owner context without changing ACLs. Both hashes matched:
`ED52F7192B8311D700AC0CE80644E3852CD01537E4D62241B9ACBA023DA3D54E`.

Remaining evidence: GPX/folder and normal native picker live consumption, separate OS close event, exact abandoned
dialog/concurrent-mailbox journeys, hostile concurrent replacement, and the campaign-end sweep. Existing path
refusals are not proof of an unlimited race-free filesystem guarantee. Captured root/child identities are gone;
the post-snapshot child limitation remains explicit. QA-0018/19 are open S6 defects, not a clean-layout claim.

Reviewers made no source edits or cargo/app runs in these passes.

VERDICT: APPROVE WITH DOCUMENTED CAMPAIGN VERIFICATION PENDING
