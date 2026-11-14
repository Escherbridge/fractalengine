---
type: Track Plan
---

# Implementation Plan: Release CI

## Overview

Two-phase implementation. Phase 1 establishes the PR check workflow (fast feedback on every PR). Phase 2 builds the release pipeline (artifact generation on tags).

---

## Phase 1: PR Check Workflow

**Goal:** Every PR is verified to compile and pass tests on all 3 OS families.

---

### Task 1.1 — Create `.github/workflows/ci.yml` [x] (delivered as `build-artifacts.yml`; macOS job added 2026-07-15)

Workflow triggered on `pull_request` and `push` to `master`:

```yaml
strategy:
  matrix:
    os: [ubuntu-latest, windows-latest, macos-latest]
```

Steps per runner:
1. Checkout
2. Install Rust stable
3. Install system deps (Linux only: webkit2gtk, dbus, alsa, udev, etc.)
4. Setup sccache
5. Cache cargo registry
6. `cargo check -p fractalengine`
7. `cargo check -p fractalengine-relay`
8. `cargo test --workspace` with `FE_KEYSTORE_BACKEND=env`

**Verification:** Push to a feature branch, all 3 matrix jobs pass.

**Files:** new `.github/workflows/ci.yml`.

---

### Task 1.2 — Add sccache configuration [x] (superseded by rust-cache — see spec amendment 2026-07-15)

Use `mozilla-actions/sccache-action@v0.0.5` for GitHub Actions Cache backend.

Set workspace env:
```yaml
env:
  SCCACHE_GHA_ENABLED: "true"
  RUSTC_WRAPPER: "sccache"
```

Verify cache hits on second CI run by checking sccache stats in logs.

**Files:** `.github/workflows/ci.yml`.

---

### Task 1.3 — Add cargo registry caching [x] (via Swatinem/rust-cache@v2)

Use `actions/cache@v4` with:
```yaml
path: |
  ~/.cargo/registry/index/
  ~/.cargo/registry/cache/
  ~/.cargo/git/db/
key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
restore-keys: ${{ runner.os }}-cargo-
```

**Files:** `.github/workflows/ci.yml`.

---

### Phase 1 Checkpoint

- PR check runs on 3 OS families
- Caching reduces second-run build time by 50%+
- Test failures block merge

---

## Phase 2: Release Pipeline

**Goal:** Tag push produces 8 artifacts + Docker image, published as GitHub Release.

---

### Task 2.1 — Create `.github/workflows/release.yml` [x] (2026-07-15)

Workflow triggered on tag push `v[0-9]+.[0-9]+.[0-9]+*` and `workflow_dispatch`.

Define 8 parallel build jobs:

**GUI builds (native):**
- Linux x64: `ubuntu-latest`, install system deps, `cargo build --release -p fractalengine`, strip binary
- Windows x64: `windows-latest`, `cargo build --release -p fractalengine`
- Windows ARM64: `windows-latest`, `--target aarch64-pc-windows-msvc`, disable sccache
- macOS ARM64: `macos-latest`, native build
- macOS x64: `macos-latest`, `--target x86_64-apple-darwin`

**Headless builds (musl):**
- Linux musl x64: `ubuntu-latest`, `cargo-zigbuild --target x86_64-unknown-linux-musl -p fractalengine-relay`
- Linux musl ARM64: `ubuntu-latest`, `cargo-zigbuild --target aarch64-unknown-linux-musl -p fractalengine-relay`

**Assembly:**
- macOS Universal: depends on both macOS jobs, runs `lipo -create`

Each job: build, rename artifact, generate SHA256 checksum, upload artifact.

**Files:** new `.github/workflows/release.yml`.

---

### Task 2.2 — GitHub Release publishing job [x] (softprops/action-gh-release@v2 + SHA256SUMS)

Final job `needs` all 8 build jobs:
1. Download all artifacts
2. Flatten into `release-assets/` directory
3. Use `softprops/action-gh-release@v2` to create release
4. Include artifact table in release body
5. Set `prerelease` if tag contains `-`

**Files:** `.github/workflows/release.yml` (release job).

---

### Task 2.3 — Install cross-compilation tools [x] (taiki-e/install-action cargo-zigbuild + mlugg/setup-zig; targets via dtolnay/rust-toolchain)

Add `taiki-e/install-action@v2` steps for:
- `cargo-zigbuild` (musl builds)
- `cross` (if ARM GNU targets are needed later)

Add `rustup target add` for:
- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `aarch64-pc-windows-msvc`
- `x86_64-apple-darwin` (on macOS ARM64 runner)

**Files:** `.github/workflows/release.yml`.

---

### Task 2.4 — Create Cross.toml [x] (2026-07-15)

Configure `cross-rs` for potential future ARM GNU targets:

```toml
[build.env]
passthrough = ["CARGO_TERM_COLOR", "RUST_LOG"]

[target.aarch64-unknown-linux-gnu]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-gnu:edge"
```

**Files:** new `Cross.toml` at repo root.

---

### Task 2.5 — Create Dockerfile for relay [x] (alpine runtime — blessed deviation, see spec amendment 2026-07-15)

Multi-stage build:

```dockerfile
FROM rust:alpine AS builder
RUN apk add --no-cache musl-dev perl make
WORKDIR /app
COPY . .
RUN cargo build --release -p fractalengine-relay --features headless

FROM scratch
COPY --from=builder /app/target/release/fractalengine-relay /fe-relay
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
VOLUME ["/data"]
EXPOSE 8765
ENTRYPOINT ["/fe-relay"]
```

**Verification:** `docker build -t fe-relay:test .` succeeds. `docker run --rm fe-relay:test` starts and binds port.

**Files:** new `docker/Dockerfile.relay`.

---

### Task 2.6 — Docker image CI job [x] (GHCR push in release.yml; `latest` only on stable tags)

Add to release workflow:
1. Build Docker image from `docker/Dockerfile.relay`
2. Tag as `ghcr.io/<owner>/fractalengine-relay:<tag>` and `:latest`
3. Push to GitHub Container Registry
4. Use `docker/login-action` + `docker/build-push-action`

**Files:** `.github/workflows/release.yml` (Docker job).

---

### Task 2.7 — Test release workflow [ ] (PENDING — requires live GitHub tag run) — AUDIT 2026-10-10: OPEN — `release.yml` triggers on `v*` tags only (`release.yml:9-11`) and no `v*` tag exists. The "8 jobs / macOS universal" checklist below is STALE: c272246 (2026-08-20) removed the macOS matrix + lipo job, so the expected artifact count is now lower; re-derive it from `release.yml` before tagging.

Create a test tag `v0.0.0-ci-test` to trigger the workflow. Verify:
- All 8 jobs complete
- Release is created as pre-release (tag contains `-`)
- All artifacts downloadable
- Checksums match
- Docker image pulls and runs

Delete test tag/release after verification.

**Files:** none (manual verification).

---

### Phase 2 Checkpoint

- Tag push produces GitHub Release with 8 artifacts + checksums
- Docker image published to GHCR
- macOS universal binary verified
- Pre-release flag works correctly

---

## Summary

| Phase | Delivers | Verification |
|-------|----------|-------------|
| 1 | PR check on 3 OS families | All matrix jobs green on test PR |
| 2 | Release pipeline: 8 artifacts + Docker | Test tag produces complete release |

---

## Phase 3: Code signing + first `v*` release

**Goal:** Tag-triggered artifacts carry a verifiable signature chain (Windows
Authenticode + a detached signature over the Linux artifacts / `SHA256SUMS`),
so a downloaded binary can be checked against SmartScreen/tampering before
the first real `v*` tag is cut. Distribution-trust scope only.

---

### Task 3.1 — Windows Authenticode signing [~] (wired 2026-11-13, provider choice open)

Status: optional signing step + verify wired in `build-artifacts.yml` and `release.yml`
(Azure Trusted Signing, gated on the `AZURE_TENANT_ID` secret; unsigned builds stay green with
a notice). Remaining: pick the provider, add secrets/variables, confirm a signed run. See `docs/release-signing.md`.

Sign `fractalengine.exe` and `fe-relay.exe` in every Windows job
(`windows-x86_64`, `windows-arm64`) in `release.yml` before packaging.

**Open decision:** certificate path.
- **OV cert**: cheapest, but SmartScreen reputation starts at zero and
  accumulates slowly per-cert; a hardware-token-bound OV cert is awkward to
  drive from a GitHub Actions runner on every release.
- **EV cert**: immediate SmartScreen reputation, but requires a hardware
  token (USB HSM or cloud HSM) — the heaviest CI operational cost.
- **Azure Trusted Signing**: Microsoft's CI-native signing service, no
  physical token, REST/CLI-driven (fits GitHub Actions directly), builds
  SmartScreen reputation faster than OV. Eligibility requires a verified
  Azure account + a minimum business-registration age (or an open-source
  attestation path) — confirm eligibility before committing.
- **Recommended default: Azure Trusted Signing, if eligibility is confirmed.**
  *Reversal cost: Medium* — switching signing providers later needs a new
  cert/identity; it does not invalidate already-signed historical releases,
  but any doc that tells users how to verify a binary needs updating.

**Files:** `.github/workflows/release.yml` (Windows jobs).

---

### Task 3.2 — Linux artifact signing [x] (code done 2026-11-13: one minisign signature over SHA256SUMS covers every archive, `release.yml` step "Sign SHA256SUMS (minisign)" at :356-374; no CI run claimed)

Produce a detached signature over the Linux artifacts (or over
`SHA256SUMS`, see Task 3.3) so `.tar.gz` downloads get the same trust chain
as the signed Windows archives.

**Open decision:** tool.
- **minisign**: single static binary, tiny, one-key model — simple to script
  in CI and simple for a user to verify with one command.
- **cosign / sigstore**: keyless signing via GitHub Actions' own OIDC
  identity, transparency-log-backed (Rekor) — stronger provenance (ties the
  signature to "this exact CI run"), heavier verification UX for an end user.
- **Recommended default: minisign** — matches a project whose current
  audience downloads a desktop app rather than consumes a supply-chain-
  attested build; cosign/sigstore is the natural upgrade once there's
  demand for SLSA-style provenance. *Reversal cost: Low* — both can coexist
  (sign with both), so adding cosign later doesn't require removing minisign.

**Files:** `.github/workflows/release.yml` (`publish` job, new signing step).

---

### Task 3.3 — Sign `SHA256SUMS` [x] (code done 2026-11-13: `release.yml` "Sign SHA256SUMS (minisign)" :356-374 and "Verify SHA256SUMS signature" :393-395; the public key `docs/release-signing.pub` is NOT committed yet, so the first verified run is user-gated under 3.7)

`SHA256SUMS` is already generated (`publish` job, `sha256sum * >
SHA256SUMS`) but is itself unsigned — anyone who can replace a release
asset can also replace the checksum file. Add a detached signature over
`SHA256SUMS` using whichever tool Task 3.2 selects and publish it alongside
(`SHA256SUMS.minisig` or `SHA256SUMS.sig`).

**Verification:** a downloaded `SHA256SUMS` plus its signature file verifies
against a published public key before the checksums it contains are trusted.

**Files:** `.github/workflows/release.yml` (`publish` job).

---

### Task 3.4 — Release notes template [x] (done 2026-11-13: `.github/release-notes-template.md` rendered by the "Render release notes" step in `release.yml:399-416`; a leftover `{{PLACEHOLDER}}` fails the job)

Replace the inline `body:` block in the `publish` job (currently a
hand-written downloads table) with a template file (e.g.
`.github/release-notes-template.md`), so release notes stay consistent
across tags and can include signing-verification instructions (Tasks
3.1-3.3) without editing workflow YAML per release.

**Files:** new `.github/release-notes-template.md`; `.github/workflows/release.yml`.

---

### Task 3.5 — SmartScreen reputation note (docs) [x] (done 2026-11-13: `BUILDING.md:35-37` "### SmartScreen" and `.github/release-notes-template.md:54-57` "## Windows SmartScreen")

Document, in `BUILDING.md` and/or the release notes template, that early
Windows releases may still show a SmartScreen warning even when signed —
reputation accumulates per certificate/publisher and isn't instant, Azure
Trusted Signing or not. State plainly that "More info -> Run anyway" (the
unsigned-case text already in `scripts/package_release.py`'s `RUN_GUI`
QUICKSTART fragment) remains expected for the first several releases, so
users and support don't mistake it for tampering.

**Files:** `BUILDING.md`; `.github/release-notes-template.md`.

---

### Task 3.6 — Reconcile `BUILDING.md`'s macOS section (doc fix) [x] (done 2026-11-13: `BUILDING.md:76-82` marks macOS "Buildable locally; not built or tested in CI")

`BUILDING.md`'s `## macOS` section still fully documents macOS as a
supported build target, but the macOS GUI matrix and the `lipo`
universal-binary job were removed from `release.yml` and
`build-artifacts.yml` on 2026-08-19 (owner direction, commit `c272246` —
`release.yml` carries its own comment recording the removal). `BUILDING.md`
was never updated to match, so it currently promises CI coverage that does
not exist. Reconcile by marking macOS as "buildable locally, not
CI-verified" rather than removing the section outright, unless macOS
support is meant to be dropped entirely.

**Files:** `BUILDING.md` (`## macOS` section).

---

### Task 3.7 — Cut `v0.1.0` [ ] (USER-GATED — do not push the tag unilaterally)

Push the first real `v*` tag once Tasks 3.1-3.6 are green on a
`workflow_dispatch` dry run (builds without publishing, per `release.yml`'s
own `on:` block) — this finally closes Task 2.7's long-open "requires a
live GitHub tag run" blocker, with a real version instead of the
`v0.0.0-ci-test` placeholder Task 2.7 originally proposed.

**Verification:** all jobs green; release published as non-prerelease (tag
has no `-` suffix); every artifact's signature verifies; `SHA256SUMS`'
signature verifies; the Docker image pulls from GHCR at both `v0.1.0` and
`latest`.

**Files:** none (tag push + manual verification). Requires explicit user
authorization before pushing the tag.

---

### Phase 3 Checkpoint

- Every Windows artifact is Authenticode-signed per the ratified cert choice.
- Every Linux artifact (or its `SHA256SUMS`) carries a verifiable detached signature.
- `SHA256SUMS` itself is signed.
- Release notes are template-driven and document the verification steps.
- `BUILDING.md`'s macOS section matches CI reality.
- `v0.1.0` is cut only on explicit user go-ahead.

---

## Status Notes

- **2026-07-15 — CI run 29390918844 (build-artifacts.yml, push on main):**
  - Linux job **FAILED** in "Build release binaries" (exit 101): `glib-sys v0.18.1`
    build script — `pkg-config` could not find `glib-2.0` (`Package 'glib-2.0' ...
    not found`). Root cause: workflow installed only Bevy system deps; the GUI
    binary also links wry/rfd/keyring (webkit2gtk/gtk3/glib/dbus). NOT a Rust
    source failure. Fixed 2026-07-15 by extending the apt dependency list in
    `build-artifacts.yml` (and mirrored in `release.yml`).
  - Windows job: still `in_progress` at time of check (no verdict).
  - macOS job: did not exist yet; added 2026-07-15.
- **2026-07-15:** Task 2.7 end-to-end tag verification pending (requires live
  GitHub run). Track stays in_progress until a test tag produces a complete
  release (8 artifacts + SHA256SUMS + GHCR image).
- **2026-11-13 — REL bookkeeping (T-REL):** Tasks 3.2-3.6 ticked after a HEAD check (see each heading for file:line). Task 3.1 stays `[~]` (provider choice and secrets are user actions). Tasks 2.7 and 3.7 stay open: they need a live `v*` tag, which the user cuts. Gate verdict for the signing change: `conductor/tracks/release_ci_20260429/review-turn4/R-REL-1.md` (PASS-WITH-FIXES, RR1-01..RR1-11). No CI run is claimed by this entry.
