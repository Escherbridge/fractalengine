---
type: Implementation Plan
title: "Implementation Plan: OSS Release — phased pre-push checklist"
tags: [oss_release_20260717]
resource: ./spec.md
---

# Implementation Plan: OSS Release Checklist

## Overview

Phases 1–4 are agent-executable now. Phase 5 is the ratification wall — every item is
**BLOCKED-ON-USER** and nothing in Phase 6 may run until Phase 5 is fully resolved.
Ordering follows the audit's blocking analysis: legal scaffolding verification →
truth alignment → community/CI → metadata → ratification → preflight + push.

## Phase 1 — Verify the 2026-07-17 scaffolding (agent)

- [ ] 1.1 `LICENSE-MIT` + `LICENSE-APACHE` present at root with correct canonical texts.
- [ ] 1.2 Root `[workspace.package]` carries `license = "MIT OR Apache-2.0"`,
      `repository`, `edition`, `version`; all member crates inherit via
      `license.workspace = true` (spec FR-6).
- [ ] 1.3 `deny.toml` allowlist correct (MIT/Apache-2.0/BSD/ISC/MPL-2.0 + BUSL
      exception scoped to `surrealdb*` only); `cargo deny check licenses` wired into
      `build-artifacts.yml` (spec FR-3).
- [ ] 1.4 `THIRD-PARTY-LICENSES.md` names SurrealDB BUSL-1.1 + the Additional Use
      Grant terms explicitly; note the regeneration command used.
- [ ] 1.5 Re-grep `"00".repeat(64)` and reconcile against the spec FR-2 register
      (13 sites, all fe-database, as of 2026-07-17).

## Phase 2 — Truth alignment in shipped docs (agent)

- [ ] 2.1 README claims audit (spec FR-5.3): spatial-analytics positioning per
      roadmap, MSRV unified with BUILDING.md (1.83+), thread topology corrected to 7
      (add plugin host to the diagram), honest status section (REL-06).
- [ ] 2.2 README security claims amended per spec FR-2 path (b): op-log signing
      stated as not-yet-implemented — unless Phase 5.3 resolves to path (a).
- [ ] 2.3 SECURITY.md known-limitations section lists the unsigned-op-log register
      and any open RBAC gaps (spec FR-7).
- [ ] 2.4 BUILDING.md: add RUST_MIN_STACK=134217728 / surrealdb-core deep-recursion
      gotcha (+ poisoned-rmeta `cargo clean -p` note); fix the GHCR org pull URL
      (REL-09).
- [ ] 2.5 README license section: dual-license text + contribution-licensing sentence
      (lands with Phase 5.1 ratification; draft may be staged behind it).

## Phase 3 — Community health + CI gates (agent)

- [x] 3.1 CONTRIBUTING.md (build via BUILDING.md, pre-commit trio
      `cargo fmt && cargo clippy -- -D warnings && cargo test`, licensing-of-
      contributions note) (REL-07). (done 2026-07-17)
- [x] 3.2 CODE_OF_CONDUCT.md (Contributor Covenant 2.1) (REL-07). (done 2026-07-17)
- [x] 3.3 `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.yml` + PR template
      referencing the test policy (REL-07). (done 2026-07-17)
- [x] 3.4 Fast lint job (fmt --check + clippy -D warnings, Linux-only) in
      `build-artifacts.yml` (spec FR-8). (done 2026-07-17)
- [x] 3.5 `release.yml` build-job timeouts 120→150 (parity with 43655f9).
      (done 2026-07-17)
- [x] 3.6 build-artifacts badge in README; document the scoped 3-crate test sweep in
      the workflow (widening is D-66 — do not silently change scope).
      (done 2026-07-17)
- [x] 3.7a Docs polish, done parts: `#![warn(missing_docs)]` on fe-sdk;
      docs/unwrap-audit.md marked STALE (REL-11). (done 2026-07-17)
- [ ] 3.7b Docs polish, remaining: fix root AGENTS.md SQLite→SurrealDB line
      (currently AGENTS.md:20); fill fe-sdk missing-docs gaps (REL-11).

## Phase 4 — crates.io metadata verification (agent)

- [ ] 4.1 Per-crate `description` for fe-sdk + fe-plugin-test at minimum (spec FR-6).
- [ ] 4.2 `cargo publish --dry-run` green for fe-sdk and fe-plugin-test.
      (Granted-command exception must be explicit in the executing session; otherwise
      defer to the end-of-session sweep.)
- [ ] 4.3 Root .glb strays (`camera.glb` 14-byte placeholder, `duck.glb`,
      `info_panel.glb`) relocated under assets/ or examples/; camera.glb replaced.

## Phase 5 — Ratification wall (every item BLOCKED-ON-USER)

- [ ] 5.1 **BLOCKED-ON-USER — D-69:** ratify (or override) MIT OR Apache-2.0 dual
      license. No public push, tag, or publish before this lands.
- [ ] 5.2 **BLOCKED-ON-USER — D-70:** conductor/ (+ research/) public vs private.
      If public: conductor/README.md framing + register marked draft/unratified.
      If private: split to a sibling repo before push.
- [ ] 5.3 **BLOCKED-ON-USER — REL-03 path choice:** implement real op-log signing
      (D5-1-adjacent, its own track if chosen) vs ship with disclosure (Phase 2.2/2.3
      text stands). Default posture staged: disclosure.
- [ ] 5.4 **BLOCKED-ON-USER — SECURITY.md contact:** the real private reporting
      channel (GitHub private vulnerability reporting and/or security@ address).
- [ ] 5.5 **BLOCKED-ON-USER — D-66:** ratify the scoped CI test sweep or order the
      widening before the badge goes public.
- [ ] 5.6 **BLOCKED-ON-USER — D-53:** publishing gate (repo settings, tokens, live
      tag) — required for the eventual push/release but executed by the user.

## Phase 6 — Pre-push preflight + push (agent prepares, user pushes)

- [ ] 6.1 Secrets re-scan across full history; record the tool + date in this plan.
- [ ] 6.2 Decision-register review: no public doc states an unratified default as
      settled; Phase 5 items all carry dated resolutions.
- [ ] 6.3 Final README claims audit re-run (spec FR-5.3) against the tree as it will
      push.
- [ ] 6.4 Final `"00".repeat(64)` re-grep matches the disclosed register.
- [ ] 6.5 **BLOCKED-ON-USER:** the push itself (first public push is a user action).
