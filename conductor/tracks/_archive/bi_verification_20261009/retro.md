---
type: retro
track: bi_verification_20261009
title: "Retro: BI Egress Verification"
created: 2026-10-31
---

## Summary

**What shipped**: Consolidated record track for M4 BI-egress verification. The live-relay + real-DuckDB e2e proof (scripts/bi-egress-verify.ps1), readings-export acceptance, persistent share-signer keystore, docs/bi-egress.md, and the adversarial-review hardening (FAIL → fix → STILL-FAILING → remediation).

**Verification**:
- A22: scripts/bi-egress-verify.ps1 (live relay + DuckDB 1.5.6): 31/31 → 48/48 → 49/49 across M4 review saga.
- A23/A24: Readings export + persistent share-signer (@F11 73f4777); batched anchor join, latlon via petal Projection, nullable geometry.
- A25: docs/bi-egress.md (@F12 4fde52b); every command sourced from verified script output, DuckDB leg live-verified.
- M4 verdict: FAIL @F10 → fix pass @6f23aa7 (DEC-C17/C18) → re-review STILL-FAILING (node_log side channel HIGH) → remediation + grammar audit @236ae8e (DEC-C19/C20) → gate green.

**Hardening in M4 review**:
- GeoParquet crs metadata corrected to spec-legal null (bd46845, DEC-C16).
- Scope-filter injection bypass closed.
- Correlation_id + ETag/If-Range + SurrealQL TIMEOUT added.
- node_log removed from ALLOWED_TABLES (side channel).
- Export/share surface hardened beyond original scope.

**Left open / handed off**:
- Re-run scripts/bi-egress-verify.ps1 after future export/share changes (the standing re-runnable artifact).
- Implementation history lives in analytics_egress_20260714/metadata.json (now closed).
- Known limitations (ledgered): WKB local-frame axes, f32 anchor quantization, db_reader gap on /query + share fmt=json.

**Lessons**:
- Live e2e with real deps (DuckDB 1.5.6 on real relay) catches subtle bugs (CRS metadata encoding, node_log side channel) that unit tests miss.
- Staged review workflow (FAIL → fix → re-review) surfaces new findings; gate must stay open long enough for fixing.
- Signed share URLs proved robust: verification shapes converge across rounds without design churn.

**Status**: Done (record). Created at F18 mission close as one of three initiative tracks. Implementation commits: 73f4777, 7418a16, 911730d, bd46845, 4fde52b, f65aaa2, 277edea, 6f23aa7, 236ae8e.
