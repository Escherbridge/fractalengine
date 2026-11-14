---
type: Review
---

# R5-REL review (round 5, group REL)

Scope: REL5-01, REL5-02, REL5-03, REL5-04 (`git diff a85dc9ad` over their owned files). Prompted to refute; security review done (release secrets, publish gating, SECURITY.md truth).

## Findings

- RREL5-01 KEPT (correct as written) - .github/workflows/release.yml:283,326 - docker-relay and publish both gate on `startsWith(github.ref, 'refs/tags/') && github.event_name == 'push'`; a dispatch from a tag ref now builds only; a UI re-run of a tag run keeps event `push`, so it still publishes. YAML parses.
- RREL5-02 KEPT (no leak) - .github/workflows/release.yml:357-376,401-417 - minisign key written under $RUNNER_TEMP with umask 077 and removed `if: always()`; notes values arrive via env and are sed-escaped; no step echoes a secret; Azure secrets only reach the pinned signing action.
- RREL5-03 KEPT (GitHub permission model) - .github/workflows/release.yml:97-112 - a branch dispatch still runs Authenticode signing with the Azure secrets; only repo writers can dispatch and they can already change the workflow. Revisit when OIDC federation lands (docs/release-signing.md hardening notes).
- RREL5-04 KEPT (correct) - .github/release-notes-template.md:17 - checksum sentence now points at the Verify section, which orders signature before `sha256sum -c`; the five placeholders are unchanged.
- RREL5-05 FIXED - README.md:144 - "scoped to three crates" omitted the two named fe-sync acceptance tests the Linux job also runs (build-artifacts.yml:430,448); sentence now names them.
- RREL5-06 FIXED - README.md:93 - "The packages are unsigned alpha builds" read as covering tagged releases, which sign when the secrets are set; now says the rolling packages, and that a tagged release's notes state its signing.
- RREL5-07 KEPT (verified) - README.md:296-299 - 55 TOOLS rows (fe-api/src/mcp/mod.rs:230-341); 19 carry `requires` (5 sim, 1 diagnostics, 2 screenshot, 5 QA, 2 extensions, 3 bindings, 1 input), so 36 unconditional matches the README.
- RREL5-08 FIXED - SECURITY.md:84-88 - "Editor+ at the verse scope" was imprecise: the gate checks the row's own scope (role_gate, replicated_row.rs:840) and needs Manager+ for governance, extension-policy and requirement changes (:957,1001,1080,1234,1274).
- RREL5-09 KEPT (verified) - SECURITY.md:89-96 - TOFU first manifest needing a did:key owner (replicated_row.rs:728-757), cross-verse references denied (:1025,1067,1107), relayed gossip by claim under compat (gossip_auth.rs:140-160) all match code.
- RREL5-10 FIXED - docs/diagrams.md:173 - footer claimed the plugin host is gated by the `plugin-host` feature; the thread always runs inert, and package admission needs the feature AND FE_PLUGIN_HOST_ALLOW=1 (main.rs:550-554). Added the install_with anchor (plugin_bridge.rs:4547).
- RREL5-11 KEPT (verified) - docs/diagrams.md:173 - main.rs:144,303,419,470,791, fe-plugin/src/lib.rs:263-264, replicated_row.rs:655,840, README.md:206-232,242-265 each open on the named construct; mermaid subgraph/alt/loop/opt blocks balance their `end`s in all eleven diagrams.
- RREL5-12 KEPT (verified) - conductor/tracks/oss_release_20260717/spec.md:51-107 - live grep returns 51 hits; all 36 table rows open on the stamp in the named function; TEST hits sit in cfg(test) items or tests/ (crud.rs:224, invite.rs:177, op_log.rs:368, role_manager.rs:368); 36+14+0+1=51.
- RREL5-13 KEPT (verified) - SECURITY.md:34-46 - "36 production write paths in fe-database" and the 51-hit totals equal the register; no backfill and "do not rely on op_log signatures" kept.
- RREL5-14 KEPT (cosmetic) - conductor/tracks/oss_release_20260717/spec.md:125 - pointer paragraph after the Requirement repeats the header's totals; harmless.
- RREL5-15 KEPT (verified) - conductor/tracks/oss_release_20260717/spec.md:181-186 - FR-8 amendment dated 2026-11-13 matches build-artifacts.yml:60-125 (fmt, cargo-deny, fuzz; no clippy).
- SEC-INBOX: no entry in .omc/state/turn4-security-inbox.md names a file this gate owns.
- Leak scan: no forbidden term in any added line of the owned files.

## Completeness

- REL5-01 COMPLETE .github/workflows/release.yml:10,283,326; .github/release-notes-template.md:17; docs/release-signing.md:112
- REL5-02 COMPLETE README.md:105-106,133-135,144,296-307; BUILDING.md:10; SECURITY.md:84-96 (precision fixed this gate)
- REL5-03 COMPLETE docs/diagrams.md:173 (plugin-host gating sentence fixed this gate)
- REL5-04 COMPLETE conductor/tracks/oss_release_20260717/spec.md:51-107,181-184; SECURITY.md:34-46

Verdict: PASS-WITH-FIXES
