---
type: Review
track: release_ci_20260429
critic: C-REL
round: 4
date: 2026-11-13
---

# C-REL completeness: release_ci_20260429

Checked against the working tree on 2026-11-13 (HEAD 733a3256 plus uncommitted round-4 edits; none of
the files cited below is modified in the tree). No cargo, no workflow run. Statuses: complete / partial /
missing. Scoper-excluded items are listed under their class, never as gaps.

## Spec requirements

| Requirement | Status | Evidence |
|---|---|---|
| FR-1 PR check: build + test on Linux, macOS, Windows | partial | `.github/workflows/build-artifacts.yml:21-25` (push/PR on main). Linux builds both bins `:371` and runs the scoped sweep `:386-396` (`FE_KEYSTORE_BACKEND: env` `:391`; scope blessed by the 2026-07-15 amendment). Windows builds only, no tests (`:468-545`). macOS removed 2026-08-19 by the owner (`:547-552`). The spec has no amendment recording the macOS removal or the Windows build-only job. |
| FR-2 Release on `v*`: parallel targets, checksums, release table, prerelease on `-` | complete (macOS part: see FR-5) | `release.yml:12-15` trigger; Linux `:30-56`, Windows x64 `:75-94`, Windows ARM64 `:152-172`, musl relay x2 `:230-263`; `SHA256SUMS` `:344-354`; template-rendered notes `:399-416`; `prerelease: ${{ contains(github.ref_name, '-') }}` `:425`. Per-artifact `.sha256` files were replaced by one `SHA256SUMS` manifest. |
| FR-3 Build caching | complete | Blessed rust-cache deviation (spec amendment 2026-07-15); `Swatinem/rust-cache@v2` at `release.yml:51,89,167,258` (per-job `shared-key`, e.g. `:260`) and `build-artifacts.yml:91,365,483`. |
| FR-4 Docker relay image to GHCR, env-configurable | complete | `docker/Dockerfile.relay:2` builder, `:10` alpine runtime (blessed deviation), `:15-23` `FE_DB_PATH`, `FE_BIND_ADDR`, `FE_KEYSTORE_BACKEND`; GHCR push `release.yml:279-312`, `latest` only on stable tags `:304`. |
| FR-5 macOS universal binary via `lipo` | missing (owner removal) | Removed 2026-08-19 at the owner's direction, `release.yml:225-227`, `build-artifacts.yml:547-552`. The spec still lists it as a goal and Target Artifact Matrix rows 4-6. |
| FR-6 Cross.toml for cross-rs | complete | `Cross.toml:4-8` (passthrough incl. `RUST_LOG`, `CARGO_TERM_COLOR`; `edge` image for aarch64 GNU). |
| Non-goal turned Phase 3: distribution signing | partial (Windows provider is blocked-user) | See Tasks 3.1-3.3. |

## Plan tasks

| Task | Status | Evidence |
|---|---|---|
| 1.1 PR check workflow | complete (as `build-artifacts.yml`; macOS since removed) | `build-artifacts.yml:21-25`, `:336-396`, `:468-545` |
| 1.2 sccache | complete (superseded by rust-cache, amendment 2026-07-15) | `build-artifacts.yml:91` |
| 1.3 Registry caching | complete | `build-artifacts.yml:365` |
| Phase 1 checkpoint "3 OS families" | partial | Two families (Linux, Windows); see FR-1. |
| 2.1 `release.yml` | complete | `release.yml:12-275` |
| 2.2 Release publishing job | complete | `release.yml:318-425` (`softprops/action-gh-release@v2` `:422`) |
| 2.3 Cross tools | complete | `release.yml:249-255` (cargo-zigbuild via install-action, setup-zig) |
| 2.4 Cross.toml | complete | `Cross.toml:1-8` |
| 2.5 Relay Dockerfile | complete | `docker/Dockerfile.relay:1-24` |
| 2.6 Docker image CI job | complete | `release.yml:279-312` |
| 2.7 Test release workflow end to end | blocked-user (scoper) | Needs a live `v*` tag; none exists. |
| 3.1 Windows Authenticode | partial; remainder blocked-user (scoper) | Optional sign step wired, gated on the Azure secret: `release.yml:78-128` and `:155-204` (unsigned path emits `::notice::` `:128`, `:204`); `build-artifacts.yml:473-503` (never on PRs `:473`). Provider choice, secrets and a signed run are the user's. |
| 3.2 Linux artifact signing | complete | One minisign signature over `SHA256SUMS` covers every archive: `release.yml:356-374`; `docs/release-signing.md:60` states the coverage. R-REL-1 RR1-08 KEPT. |
| 3.3 Sign `SHA256SUMS` | complete (code); first verified run blocked-user | Sign `release.yml:358-374`, key file under `$RUNNER_TEMP` deleted by `if: always()` `:377-378`, unsigned warning once the public key is committed `:382-389`, verify `:393-395`. `docs/release-signing.pub` is not committed (user generates the key pair, `docs/release-signing.md:83`). |
| 3.4 Release notes template | complete (one residual line, below) | `.github/release-notes-template.md:1-57`, rendered at `release.yml:399-416`; leftover placeholders fail the job (R-REL-1 RR1-06). |
| 3.5 SmartScreen note | complete | `BUILDING.md:35-37`; `.github/release-notes-template.md:54-57` |
| 3.6 BUILDING.md macOS reconcile | complete | `BUILDING.md:76-82` ("Buildable locally; not built or tested in CI") |
| 3.7 Cut `v0.1.0` | blocked-user (scoper) | User push. `release.yml:14` `v*` also matches the `v0.1.0-qa.*` tag that the §9 q43 default puts first, and `:425` publishes it as a prerelease. |

## Round-4 items for this group

| Item | Status | Evidence |
|---|---|---|
| REL-01 signing chain + template | complete | Commits a66198c5, c392c1ab; rows 3.2-3.4 above. |
| REL-02 shipped-docs truth pass | partial | 3a1eaf79. MSRV agrees: `Cargo.toml:51` `rust-version = "1.96"`, `README.md:104`, `BUILDING.md:5`. Crate count agrees: `README.md:339` "26 workspace members" = `Cargo.toml:2-33`. MCP count agrees: `README.md:296` "54" = 54 `tool!(` entries in `fe-api/src/mcp/mod.rs:226-337`. macOS `README.md:93-94`, SmartScreen `BUILDING.md:35-37`. Missed: `README.md:134-136` says CI's lint job runs `cargo clippy ... -D warnings`, but the lint job runs fmt, cargo-deny and fuzz only (`build-artifacts.yml:60-125`) and `CONTRIBUTING.md:30` says clippy is in no gate. |
| REL-03 diagram redraw | complete (one anchor is loose) | 695d4714: `docs/diagrams.md:27`, `docs/diagrams/01-system-overview.md:3,43`. The T7 anchor `docs/diagrams.md:173` cites `fractalengine/src/plugin_bridge.rs:4510`, which is the `fe-ext-net` runtime; the plugin host thread is spawned at `fe-plugin/src/lib.rs:264`. |
| REL-04 REL bookkeeping | complete | 560fc932 ticked 3.2-3.6 with file:line; Status Notes entry 2026-11-13 in `plan.md`. |
| REL-05 project_completion reconcile | complete | 560fc932: P4 clippy lines struck OBSOLETE, `cargo test --workspace` line SUPERSEDED, QA-0071/QA-0070 SOURCE STATE notes (`project_completion_20261104/plan.md`). Residual: `plan.md:93` still calls `extensions/iot-bridge` a separate workspace, but it is a member (`Cargo.toml:29-30`). |
| REL-10 / REL-11 / REL-12 | see `crate_consolidation_r2_20260718/completeness-turn4.md` | |

## Gate verdicts and HANDED-OFF delivery

- R-REL-1 (`review-turn4/R-REL-1.md`): PASS-WITH-FIXES. Fixes are in place: RR1-01 `release.yml:382-389`
  (warning once the public key is committed); RR1-02 and RR1-03 `.github/release-notes-template.md:25-38`.
- RR1-10 HANDED-OFF (track bookkeeping): reached. T-REL ticked 3.2, 3.3, 3.4 (560fc932).
- RR1-09 DEFERRED (a dispatch on a tag ref publishes): still open, trigger unchanged; `release.yml:10` header
  is accurate only for branch dispatches.
- SEC-INBOX: no entry names a file of this track (`.omc/state/turn4-security-inbox.md` grepped for release,
  workflow, minisign, README, BUILDING, SECURITY.md).

## §9 defaults vs code

- q41 minisign with `MINISIGN_SECRET_KEY` and the public key at `docs/release-signing.pub`: matches
  `release.yml:358-395`.
- q42 Azure Trusted Signing wired, unsigned builds stay green: matches `release.yml:99`, `:126-128`, `:202-204`.
- q43 `-qa` tag first: the workflow accepts it (`:14`) and marks it prerelease (`:425`); no code decides the
  order.
- q44 keep the CI sweep scoped "and say so in README (already stated)": the scope is stated in the workflow
  (`build-artifacts.yml:386-387`) but NOT in README (no CI test-scope sentence; `README.md:134-160`). The
  default's premise is false.

## Excluded by the scoper

- blocked-user: 3.1 remainder (provider choice, secrets, a confirmed signed run); 2.7 end-to-end tag run;
  3.7 cut `v0.1.0`; Windows code signing.

## HANDED-OFF (owner = next round)

- HANDED-OFF (REL docs, next round): `README.md:134-136` drop `cargo clippy` from the "CI's lint job runs
  these" block (user decision 2026-11-09); optionally drop the clippy rows at `README.md:106`, `BUILDING.md:10`.
- HANDED-OFF (REL docs, next round): add one README sentence that CI's PR test sweep is scoped to
  `fe-ui`, `fe-terrain`, `fractalengine` (`build-artifacts.yml:396`), which q44's default assumes exists.
- HANDED-OFF (REL docs, next round): `.github/release-notes-template.md:17` still says "Verify downloads with
  `sha256sum -c`" before the signature step; point it at "## Verify" instead (RR1-02 ordering residual).
- HANDED-OFF (REL bookkeeping, next round): spec amendment recording the 2026-08-19 macOS removal (FR-1 matrix,
  FR-5, artifact matrix rows 4-6, Phase 1/2 checkpoints) and the Windows build-only PR job, with the
  owner-direction citation (c272246).
- HANDED-OFF (REL-03 owner, next round): `docs/diagrams.md:173` T7 anchor -> `fe-plugin/src/lib.rs:264`.
- HANDED-OFF (REL bookkeeping, next round): `project_completion_20261104/plan.md:93` "Separate
  extensions/iot-bridge workspace" is stale; it is a workspace member.
- HANDED-OFF (I-CONDUCTOR, next round): `conductor/tracks.md:412-414` release_ci board line ("9 done / 1 open")
  predates Phase 3; refresh to 3.2-3.6 done, 3.1 partial, 2.7 and 3.7 user-gated.

Verdict: PARTIAL - every agent task is implemented and gated; open items are user-gated (2.7, 3.1, 3.7), two README truth gaps and spec bookkeeping for the macOS removal.
