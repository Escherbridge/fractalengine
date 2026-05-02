# Implementation Plan: Release CI

## Overview

Two-phase implementation. Phase 1 establishes the PR check workflow (fast feedback on every PR). Phase 2 builds the release pipeline (artifact generation on tags).

---

## Phase 1: PR Check Workflow

**Goal:** Every PR is verified to compile and pass tests on all 3 OS families.

---

### Task 1.1 — Create `.github/workflows/ci.yml`

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

### Task 1.2 — Add sccache configuration

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

### Task 1.3 — Add cargo registry caching

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

### Task 2.1 — Create `.github/workflows/release.yml`

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

### Task 2.2 — GitHub Release publishing job

Final job `needs` all 8 build jobs:
1. Download all artifacts
2. Flatten into `release-assets/` directory
3. Use `softprops/action-gh-release@v2` to create release
4. Include artifact table in release body
5. Set `prerelease` if tag contains `-`

**Files:** `.github/workflows/release.yml` (release job).

---

### Task 2.3 — Install cross-compilation tools

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

### Task 2.4 — Create Cross.toml

Configure `cross-rs` for potential future ARM GNU targets:

```toml
[build.env]
passthrough = ["CARGO_TERM_COLOR", "RUST_LOG"]

[target.aarch64-unknown-linux-gnu]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-gnu:edge"
```

**Files:** new `Cross.toml` at repo root.

---

### Task 2.5 — Create Dockerfile for relay

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

### Task 2.6 — Docker image CI job

Add to release workflow:
1. Build Docker image from `docker/Dockerfile.relay`
2. Tag as `ghcr.io/<owner>/fractalengine-relay:<tag>` and `:latest`
3. Push to GitHub Container Registry
4. Use `docker/login-action` + `docker/build-push-action`

**Files:** `.github/workflows/release.yml` (Docker job).

---

### Task 2.7 — Test release workflow

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
