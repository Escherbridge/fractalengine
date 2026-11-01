---
type: Review
date: 2026-11-01
commits: [80177b0, 89be757]
reviewers: [harness_advocate, nav_clipboard_executor]
scope: independent static security and robustness review
---

# QA harness safety independent review

The independent review of 80177b0 required two follow-ups: a descendant PID could be recycled between Toolhelp
snapshot and identity lookup, and the QA in-memory secret store lost non-node keys across restarts. Both were
implemented in 89be757 and checked by a different reviewer. No further required defects were found statically.

The reviewed implementation explicitly builds the GUI binary, fingerprints actual build inputs, validates stage
hashes, checks held root process identity, and records verified descendants for cleanup after root exit. The
snapshot cutoff rejects identities born after capture began. Cleaner retention protects runs/stages with recorded
survivors, confines literal deletion paths and retains reparse-containing candidates.

QA startup avoids the normal OS keystore, uses run-local node identity and RunLocalBackend for arbitrary secrets,
redirects settings/blob/P2P/hexon storage, skips real-user legacy migration, and copies assets rather than linking
the writable repository tree. Secret creation supplies an owner-only DACL at CreateFileW creation and keeps the
same handle through writing; pinned ancestor handles reject reparse boundaries. Secret-store set/delete serialize
within one backend instance, and account names are encoded keys rather than filesystem paths.

Runtime verification is still required: Windows file permissions and parent pinning, same-run restart persistence,
current-source launch, screenshots/input, run-local storage, and exact recorded-process cleanup. Cross-process
writers to one QA secret store are not part of the supported one-app-per-run contract. Children born after the
last recorded snapshot cannot be claimed absent after a root crash without additional evidence.

The reviewers made no edits, launched no apps and ran no cargo/tests during these independent passes.

VERDICT: STATIC-CLEAN AFTER FOLLOW-UP; RUNTIME-PENDING
