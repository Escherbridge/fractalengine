---
type: QA Pass Verdict
pass: "12"
date: 2026-11-02
focused_smoke: true
full_coverage: false
status: driver-comparison-failure
---

The corrected qualified device-format namespace was accepted. The scenario
created committed objects, installed schemas and reached the first derivation,
then refused to continue because raw journal after-state omitted tombstone while
the typed replay DTO serialized that absent optional field as null. This is
QA-0033, not evidence of a changed property or corrupted journal.

The failed receipts and partial dataset remain intact. Independently reviewed
5581381 defaults only the three absent optional top-level DTO fields on shallow
copies, retaining all other keys and existing comparisons. No writes were
repeated in this Petal. A fresh pass13 scope subsequently passed all38 functional
checks. This attempt earns no complete-scenario or convergence credit.
