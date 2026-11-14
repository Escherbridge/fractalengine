---
type: Review
track: relay_operability_20261031
gates: [R-NET-1, R-NET-2]
verdict: PASS-WITH-FIXES
---

# Relay operability: FR verdicts

Source: `review-turn4/R-NET-1.md` ("FR verdicts") and `review-turn4/R-NET-2.md`.

- FR-1/2 usage + metrics: PASS-WITH-FIXES (R1-16, R1-17, R1-19..R1-22); wiring pending I-API-1 (R1-24, R1-25).
- FR-3 JSON logs: PASS (module); binary wiring pending I-BIN-1 (R1-26).
- FR-4/5/6 docs: DEFERRED (R1-29).
- FR-7 relay advertise: PASS-WITH-FIXES (R1-01..R1-06, R1-10, R1-11); dial-site resolution and home gating HANDED-OFF (R1-07..R1-09, R1-14).
- FR-7 PASS-WITH-FIXES (R-NET-2: per-author quarantine, reach-checked pinned insert).

Round-5 follow-up (see `review-turn5/R5-NET-1.md`): NET5-01 (tombstone memo, R1-14) and NET5-02 (net-report feature, R6-05) are COMPLETE at HEAD. R1-29 (docs FR-4/5/6) is still open until the S9 sweep and the user-gated docs review.

Verdict: PASS-WITH-FIXES.
