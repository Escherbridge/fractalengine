---
type: Review
track: oss_release_20260717
critic: C-REL
round: 4
date: 2026-11-13
---

# C-REL completeness: oss_release_20260717

Checked against the working tree on 2026-11-13 (HEAD 733a3256 plus uncommitted round-4 edits; README,
SECURITY.md, BUILDING.md, CONTRIBUTING.md, deny.toml and the workflows are unmodified in the tree). No
cargo. Scoper-excluded items are listed under their class, never as gaps.

## Spec requirements

| Requirement | Status | Evidence |
|---|---|---|
| FR-1 License (D-69 Apache-2.0 single) | complete | `LICENSE-APACHE` present, no `LICENSE-MIT`; `Cargo.toml:47-51` `[workspace.package] license = "Apache-2.0"`; every member in `Cargo.toml:2-33` carries `license.workspace = true`; `README.md:475-488`; `CONTRIBUTING.md:73-77`. |
| FR-2 Placeholder-signature register + disclosure | partial | Disclosure holds: `README.md:41` known limitation, `README.md:393` "not yet implemented", `SECURITY.md:36-47`. Register table drifted: a crate-scoped `git grep -c '"00"\.repeat(64)'` over `fe-*`, `fractalengine*`, `tools`, `extensions` returns 45 hits in 27 files today; the FR-2 table lists 22 and the 2026-11-13 re-run list in `spec.md` (FR-2, "Re-run 2026-11-13") is raw and unclassified. |
| FR-3 SurrealDB BUSL notice + deny | complete | `deny.toml:9-41` allow-list, BUSL only for the five `surrealdb*` crates `:30-37`; `THIRD-PARTY-LICENSES.md:11-37` (BUSL-1.1, Additional Use Grant); CI runs `cargo deny ... check advisories bans sources licenses` warn-only at `.github/workflows/build-artifacts.yml:119-125`. |
| FR-4 conductor/ public (D-70) + root .glb relocation | complete | `conductor/README.md:1-12` (OKF frontmatter, D-70 framing); no `*.glb` at repo root; `fractalengine/assets/samples/` holds `cameras.glb`, `info_panel.glb`, `README.md`; `fractalengine/assets/imported/` has no tracked files (the Duck residual is gone). |
| FR-5.1 Secrets re-scan | complete (pre-push re-run is part of 6.5) | Plan 6.1 record (gitleaks 2026-10-11). `.claude/settings.local.json`, which held the expired local JWTs, is no longer tracked (`git ls-files` empty); history is unchanged by policy. |
| FR-5.2 Decision-register review | blocked-user (6.2) | |
| FR-5.3 README claims audit | partial (scoper class) | REL-02 3a1eaf79: MSRV `README.md:104` = `Cargo.toml:51` = `BUILDING.md:5`; "26 workspace members" `README.md:339` = `Cargo.toml:2-33`; 54 MCP tools `README.md:296` = 54 `tool!(` in `fe-api/src/mcp/mod.rs:226-337`; seven threads `README.md:209-232`; T7 inert unless admitted, matching `fractalengine/src/main.rs:550-553`. One false claim left: `README.md:134-136` says CI's lint job runs clippy (`build-artifacts.yml:60-125` runs fmt, cargo-deny, fuzz). |
| FR-6 crates.io metadata | complete | Inheritance as FR-1; `fe-sdk/Cargo.toml:8`, `fe-plugin-test/Cargo.toml:8` descriptions; `fe-plugin-test/Cargo.toml:11` versioned path dep; dry run recorded in plan 4.2. Neither crate `include`s LICENSE-APACHE (plan 4.2 note); out of scope until D-53. |
| FR-7 SECURITY.md | complete except the contact toggle (blocked-user 5.4) | `SECURITY.md:3-8` GitHub private reporting, `:14` response expectations, `:23` supported versions, `:28-47` known limitations incl. the register; S-1 closure `:93` (4768c059). |
| FR-8 CI badge + lint gate | complete (clippy clause obsolete) | Badge `README.md:3`; lint job `build-artifacts.yml:60-125` (fmt + cargo-deny + fuzz, warn-only); clippy dropped by user decision 2026-11-09 (`CONTRIBUTING.md:30`), so FR-8's "clippy -D warnings" bullet is obsolete; release timeouts 150 at `release.yml:31,76,153,233,282`; scoped sweep documented `build-artifacts.yml:386-396`. |

## Plan tasks

| Task | Status | Evidence |
|---|---|---|
| 1.1 LICENSE-APACHE | complete | root file present |
| 1.2 workspace license metadata | complete | `Cargo.toml:47-51` |
| 1.3 deny allow-list + CI | complete | `deny.toml:9-41`; `build-artifacts.yml:119-125` (now includes `licenses`) |
| 1.4 THIRD-PARTY-LICENSES | complete | `THIRD-PARTY-LICENSES.md:11-37` |
| 1.5 register reconcile (2026-10-11) | complete as of its date; superseded by 6.4 | |
| 2.1 README claims | complete | see FR-5.3 |
| 2.2 README signing disclosure | complete | `README.md:41`, `:393` |
| 2.3 SECURITY known limitations | complete | `SECURITY.md:28-47` |
| 2.4 BUILDING RUST_MIN_STACK + GHCR URL | complete | `BUILDING.md:135`, `:190` |
| 2.5 README license section | complete | `README.md:475-488` |
| 3.1 CONTRIBUTING | complete (gate text updated) | `CONTRIBUTING.md:24-30` (fmt + `cargo check -p`; no clippy) |
| 3.2 CODE_OF_CONDUCT | complete | `CODE_OF_CONDUCT.md:1`, `:118-119` (Covenant 2.1) |
| 3.3 Issue/PR templates | complete | `.github/ISSUE_TEMPLATE/{bug_report,feature_request}.yml`, `.github/PULL_REQUEST_TEMPLATE.md` |
| 3.4 Fast lint job | complete (clippy obsolete) | `build-artifacts.yml:60-125` |
| 3.5 release timeouts 150 | complete | `release.yml:31,76,153,233,282` |
| 3.6 badge + scoped-sweep note | complete | `README.md:3`; `build-artifacts.yml:386-387` |
| 3.7a / 3.7b docs polish | complete | `fe-sdk/src/lib.rs:6` `#![warn(missing_docs)]`; `AGENTS.md:25` SurrealDB |
| 4.1 descriptions | complete | `fe-sdk/Cargo.toml:8`, `fe-plugin-test/Cargo.toml:8` |
| 4.2 publish dry run | complete (recorded 2026-10-11, not re-run) | plan 4.2 record |
| 4.3 root .glb strays | complete | see FR-4 |
| 5.1 D-69 / 5.2 D-70 | complete | plan dated resolutions |
| 6.1 secrets re-scan | complete | see FR-5.1 |
| 6.4 register re-grep matches disclosure | partial | 45 hits vs the 22-row table (FR-2). The plan note "SECURITY.md:43 still says 22" is stale: `SECURITY.md:42-47` now states both counts and calls the figure unstable. |

## Round-4 items for this track

- REL-02 (truth pass): partial, see FR-5.3 and the HANDED-OFF lines.
- REL-04 (T-REL bookkeeping): complete; 560fc932 added the 6.3 and 6.4 notes and the FR-2 "Re-run 2026-11-13" list.
- §9 q44 (keep the CI sweep scoped, "already stated" in README): the workflow states it
  (`build-artifacts.yml:386-387`), README does not; HANDED-OFF below.
- No R- gate covered REL-02; SEC-INBOX has no entry naming README, SECURITY.md, BUILDING.md or this track.

## Excluded by the scoper

- blocked-user: 5.3 op-log signing vs disclosure; 5.4 SECURITY.md contact toggle; 5.5 D-66 CI test scope;
  5.6 D-53 publishing gate; 6.2 decision-register review; 6.5 first public push.
- partial: 6.3 final README claims audit (stays open until the user-timed pre-push pass).

## HANDED-OFF (owner = next round)

- HANDED-OFF (REL next round, opus): classify the 45 placeholder-signature hits (production write path vs
  `#[cfg(test)]`/`tests/`), rebuild the FR-2 table with current file:line, then tick 6.4.
- HANDED-OFF (REL docs, next round): `README.md:134-136` remove `cargo clippy` from the "CI's lint job runs
  these" block; add one sentence that CI's PR test sweep is scoped (`build-artifacts.yml:396`) per q44.
- HANDED-OFF (REL bookkeeping, next round): spec FR-8 amendment that clippy left every gate on 2026-11-09;
  plan 3.1/3.4 notes the same; plan 6.3 note needs the REL-02 claim-by-claim list pasted in (T-REL NEEDS).
- HANDED-OFF (I-CONDUCTOR, next round): `metadata.json` description/notes still say dual MIT OR Apache and
  "13 sites"; `conductor/tracks.md:400-402` count "17 done / 15 open" is stale.

Verdict: PARTIAL - all agent-executable phases are done; open items are the unclassified 45-site register (6.4), one README CI claim, and six user-gated items.
