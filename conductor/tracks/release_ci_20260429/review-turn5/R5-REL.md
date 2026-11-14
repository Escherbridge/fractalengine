---
type: Review
---

# R5-REL review (round 5, group REL)

Scope: REL5-01, REL5-02, REL5-03, REL5-04 (`git diff a85dc9ad` over their owned files, commits eb48430d, 1958882e, 91782e9a, 09d8d43b, ff6b6c8c, plus the first R5-REL pass 5fd96174). Second gate pass, run after REL5-03 re-anchored the footer again (09d8d43b). Prompted to refute; security review done (release secrets, publish gating, SECURITY.md truth).

## Findings

- RREL5-01 KEPT (correct as written) - .github/workflows/release.yml:283,326 - docker-relay (the only `push: true`, :311) and publish (the only release writer, softprops at :422) both gate on `startsWith(github.ref, 'refs/tags/') && github.event_name == 'push'`; a dispatch from any ref builds only; a UI re-run of a tag run keeps event `push`, so it still publishes. Header :10 says so.
- RREL5-02 KEPT (no leak) - .github/workflows/release.yml:359-379,401-417 - minisign key written under $RUNNER_TEMP with umask 077 and removed `if: always()`; tag and notes values reach shell via env, never `${{ }}` in script text; no step echoes a secret; Azure secrets only reach the signing action (:99-104,:176-181).
- RREL5-03 KEPT (GitHub permission model) - .github/workflows/release.yml:97-112 - a branch dispatch still runs Authenticode signing with the Azure secrets; only repo writers can dispatch and they can already change the workflow. Revisit when OIDC federation lands (docs/release-signing.md hardening notes).
- RREL5-04 KEPT (correct) - .github/release-notes-template.md:17 - checksum sentence points at the Verify section, which orders signature before `sha256sum -c`; the five placeholders are unchanged; docs/release-signing.md:112 states the tag-push-only rule.
- RREL5-05 KEPT (fixed in the first pass, still true) - README.md:144 - the sweep sentence names the three crates and the two fe-sync acceptance tests, matching build-artifacts.yml:396,430,448.
- RREL5-06 KEPT (fixed in the first pass, still true) - README.md:93-94 - "The rolling packages are unsigned alpha builds (a tagged release's notes say whether it is signed)".
- RREL5-07 KEPT (verified) - README.md:296-299 - TOOLS (fe-api/src/mcp/mod.rs:234-356) holds 54 `tool!` rows plus the `get_network_diagnostics` literal (:311-320) = 55; 19 carry `requires`, so 36 unconditional; the 13 desktop + 5 sim + 1 diagnostics breakdown sums to 55.
- RREL5-08 FIXED - BUILDING.md:167-180 - Testing section told contributors to run `cargo test --workspace`, contradicting README.md:158-160 (retired, the runner refuses it, tools/qa/test_batches.ps1:51), and claimed `FE_KEYSTORE_BACKEND=env` selects a keystore though no crate source reads that variable; replaced with the batch runner.
- RREL5-09 FIXED - README.md:209, docs/diagrams/01-system-overview.md:3 - "fe-relay does not start the plugin host" was unconditional; fractalengine-relay/src/main.rs:355-359 adds PluginHostPlugin under the opt-in `plugin-host` feature (default off), now stated.
- RREL5-10 FIXED - docs/diagrams.md:173 - footer read "added by install_with at :4553, inside install_with at :4424" (same function twice); reworded to name the add site inside `install_with` and to add the relay's feature-gated site.
- RREL5-11 KEPT (verified at HEAD) - docs/diagrams.md:173 - main.rs:144,303,419,485,806,567-569, plugin_bridge.rs:4424,4553, fe-plugin/src/lib.rs:263-264, replicated_row.rs:655,840, README.md:206-232,242-266 each open on the named construct.
- RREL5-12 KEPT (verified) - SECURITY.md:84-96 - admission wording (row's own scope, Manager+ for governance, cross-verse denied, manifest-first; residuals TOFU first manifest and relayed attribution) matches replicated_row.rs as checked in the first pass; no code under it changed since.
- RREL5-13 KEPT (verified) - conductor/tracks/oss_release_20260717/spec.md:51-107 - live `git grep -c` returns 51 hits in 28 files; every live file:line appears in the register (36 rows) or the TEST/doc lists (petal_snapshot_test.rs listed in compressed form at :105); 36+14+0+1=51.
- RREL5-14 KEPT (verified) - SECURITY.md:34-46 - 36 production write paths and the 51-hit totals equal the register, with the exact command, no backfill and the do-not-rely warning.
- RREL5-15 KEPT (verified) - conductor/tracks/oss_release_20260717/spec.md:181-186 - FR-8 amendment matches build-artifacts.yml:104,124 (fmt, cargo-deny; no clippy); `git grep -i clippy README.md BUILDING.md` is empty.
- RREL5-16 HANDED-OFF (I5-DOC) - fe-database/src/AGENTS.md:503 - still says "13 sites tracked by the hexon-p2p-commons research"; REL5-04's NEEDS line (point at the FR-2 register, 36 production sites) is unapplied. The hit at :502 stays, so the 51 count is unaffected.
- RREL5-17 HANDED-OFF (build-artifacts.yml owner, later round) - .github/workflows/build-artifacts.yml:391 - sets `FE_KEYSTORE_BACKEND: env`, which no crate reads; drop it or wire it.
- SEC-INBOX: no entry in .omc/state/turn4-security-inbox.md names a file this gate owns.
- Leak scan: no forbidden term in any added line of the owned files.

## Completeness

- REL5-01 COMPLETE .github/workflows/release.yml:10,283,326; .github/release-notes-template.md:17; docs/release-signing.md:112
- REL5-02 COMPLETE README.md:93-94,144,158-160,296-307; BUILDING.md:10,167-180 (Testing section fixed this gate); SECURITY.md:84-96
- REL5-03 COMPLETE docs/diagrams.md:173; docs/diagrams/01-system-overview.md:3 (relay plugin-host precision fixed this gate)
- REL5-04 COMPLETE conductor/tracks/oss_release_20260717/spec.md:51-107,181-186; SECURITY.md:34-46 (AGENTS.md:503 prose handed off, RREL5-16)

Verdict: PASS-WITH-FIXES
