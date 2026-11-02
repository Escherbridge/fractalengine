---
type: QA Pass Verdict
pass: "09"
date: 2026-11-01
focused_smoke: true
full_coverage: false
status: boot_failure_fixed_source_pending_recheck
---

The combined production GUI and offline helpers staged successfully fromb190d25, executable SHA2560340D002452E181F8E921AAD015FDBA6F423C5236B23A776D2FD5D6ABC656C10. The same saved QA root was resumed without reseeding. Database initialization refused the new `field_def.default_val TYPE any FLEXIBLE` declaration before authenticated readiness. QA-0031 records the S1 boot failure; no new tree/grid/report UI was exercised and no coverage credit is claimed.

Root31640 was captured atstart_ticks639291906930001911 on this exact staged executable. The launcher reported failed readiness, no fresh child snapshot because root had gone, and cleanup completed for captured identities. Independent Get-Process confirmed root31640 absent. This does not certify children beyond the unavailable snapshot. The saved root remains20261101-191920-658.

The schema correction6a1d288 is independently reviewed against the pinned parser/coercion source. It declares the explicit optional JSON union including object, preserving NONE/null, numeric representations and flexible children. A fresh stage is building from867e01d, which also includes separately reviewed plugin and dialog safety slices. Actual boot, roundtrip and UI acceptance remain pending.

Earlier launcher invocations refused a relative Run argument, then an absolute Stage interpreted as a stage name, then failed sandbox removal of the launcher-managed token file; none launched a process. Authorized restart under ordinary user permissions then reached the recorded boot failure. These setup attempts are not application feature evidence.
