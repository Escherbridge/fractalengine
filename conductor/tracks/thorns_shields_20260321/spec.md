---
type: Track Spec
---

# Track: Thorns and Shields — Security Hardening + Pre-Launch Documents

**Created:** 2026-03-21 (folder reconstructed 2026-07-14 during board-hygiene pass)
**Status:** Pending

## Scope

Security hardening and pre-launch security documents. The track existed only as a
tracks.md entry until 2026-07-14; this folder was reconstructed from the surviving
artifacts. Current state: docs/webview-threat-model.md, docs/security-checklist.md,
docs/unwrap-audit.md (says Status: PENDING and cites an .expect already removed
2026-04-30), scripts/audit.sh, and fuzz/targets/ are all thin Wave-1 scaffolds; the
audit was never run.

## Functional Requirements

- **FR-1**: Run + update the unwrap audit (docs/unwrap-audit.md is stale — it cites an .expect removed 2026-04-30).
- **FR-2**: Flesh out the webview threat model and security checklist against the current architecture.
- **FR-3**: Make the fuzz targets build and run in CI.
