---
type: Plan
date: 2026-11-01
status: in_progress
title: Rich data and history execution
---

# Execution

- [x] Source-grounded inventory and corrected dispatcher journal coverage.
- [x] User confirms both object replay and appended measurement charts.
- [x] RD1 typed editor source and independent static review; merged into main.
- [ ] RD1 integrated live typed value/draft/error/persistence proof.
- [~] RD2 bounded versioned descriptors and authoritative enforcement; source committed, review fixes applied.
- [~] RD3 atomic existing-journal mutations/baselines and scoped replay; Local-RBAC review correction in progress.
- [ ] Independent final backend security/design approval and integration compilation.
- [ ] RD4 full current-state materialization/receipt UI adapter; scoped history preview and measurement chart.
- [~] RD5 declarative scenario/driver, independent expected values and safe shared HTTP transport.
- [ ] Real Both scenario with type, version, measurement ordering, movement and restart evidence.
- [ ] Catalog reconciliation and source fingerprint review; changed-region and final full-coverage passes.
- [ ] Final integrated checks and original campaign CI/tagged artifact publication.

Current checkout ownership and immutable source commits are tracked in the campaign ledger and
`.omc/state/qa-chain-session.json`. Only the main owner runs Cargo or applications. Other authors remain in
isolated code-only checkouts. Review authors and executing authors remain separate; unresolved findings block
acceptance. Preserve `conductor/.conductor_session_log`.
