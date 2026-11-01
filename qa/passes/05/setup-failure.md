---
type: QA Investigation
pass: "05"
status: source-fixed-live-verification-pending
---

The first basic launch of stage ecee88e-20261101163416360 failed strict setup.
Owned run20261101-163444-703, root PID72992, used API port8878. The DB log
records SetNodeProperty persisting qa_value10 on node01M6CQEJM45MXMS1J6RFAFQRJT
at2026-11-01T23:34:54.526643Z, but the API returned request timed out.

Source inspection found the GUI dispatcher skips property reply forwarding
when the target is unselected. Strict setup was retained. The repair adds
optional request/reply correlation, exact API identity checks and early
API forwarding independent of UI selection, preserving cache hydration.

The failed launch ran its existing finally cleanup; root72992 was subsequently
absent and no app remains owned from this attempt. No successful fixture or
UI proof is claimed for this run. A new exact stage and successful strict
setup are required before closing QA-0022. Full tests/clippy remain deferred.
