# Track: Release CI — Cross-Compilation Pipeline, Artifact Publishing, Docker Image

**Created:** 2026-04-29
**Status:** Draft
**Priority:** P2
**Depends on:** Cross-Platform Desktop (GUI targets compile), Headless Relay (relay binary exists)
**Blocks:** none (all tracks can develop locally without CI)

---

## Problem Statement

FractalEngine has no CI/CD pipeline. Builds happen only on the developer's Windows machine. There is no automated:

- Compilation verification across platforms
- Test execution on non-Windows OS
- Release artifact generation
- Docker image publishing
- Checksum/signature generation for distributed binaries

This track establishes a GitHub Actions pipeline that validates every PR and produces release artifacts for 8 build targets.

---

## Goals

1. PR check workflow: compile + test on 3 OS families (Linux, macOS, Windows)
2. Release workflow: build 8 artifacts on tag push, publish GitHub Release
3. Docker image: musl static relay binary in `FROM scratch` container, pushed to GHCR
4. Caching: sccache + cargo registry cache for fast builds
5. macOS universal binary via `lipo`

## Non-Goals (this track)

- Code signing (Apple notarization, Windows Authenticode) — deferred
- Packaging (MSI, DMG, AppImage, .deb) — deferred
- Nightly builds — deferred
- Performance benchmarks in CI — deferred

---

## Target Artifact Matrix

| Job | Runner | Target | Type | Tool | Artifact |
|-----|--------|--------|------|------|----------|
| 1 | ubuntu-latest | x86_64-unknown-linux-gnu | GUI | native | `fractalengine-linux-x86_64` |
| 2 | windows-latest | x86_64-pc-windows-msvc | GUI | native | `fractalengine-windows-x86_64.exe` |
| 3 | windows-latest | aarch64-pc-windows-msvc | GUI | MSVC cross | `fractalengine-windows-aarch64.exe` |
| 4 | macos-latest | aarch64-apple-darwin | GUI | native | (input to universal) |
| 5 | macos-latest | x86_64-apple-darwin | GUI | cross from M1 | (input to universal) |
| 6 | macos-latest | — | GUI | lipo | `fractalengine-macos-universal` |
| 7 | ubuntu-latest | x86_64-unknown-linux-musl | Headless | cargo-zigbuild | `fe-relay-linux-musl-x86_64` |
| 8 | ubuntu-latest | aarch64-unknown-linux-musl | Headless | cargo-zigbuild | `fe-relay-linux-musl-aarch64` |

Each artifact accompanied by `.sha256` checksum file.

---

## Functional Requirements

### FR-1: PR Check Workflow
On every PR, run:
- `cargo check -p fractalengine` (GUI)
- `cargo check -p fractalengine-relay` (headless)
- `cargo test --workspace` with `FE_KEYSTORE_BACKEND=env`
- Matrix: ubuntu-latest, windows-latest, macos-latest

### FR-2: Release Workflow
On tag push `v*`:
- Build all 8 targets in parallel
- Produce SHA256 checksums
- Create GitHub Release with artifacts table
- Mark pre-release if tag contains `-` (e.g., `v0.3.0-rc1`)

### FR-3: Build Caching
- sccache with GitHub Actions Cache backend (all jobs except cross-rs Docker jobs)
- Cargo registry cache keyed by `Cargo.lock` hash
- Separate cache keys per target to avoid cross-contamination

### FR-4: Docker Image
- Multi-stage `Dockerfile`: `rust:alpine` builder -> `FROM scratch` runtime
- Published to `ghcr.io/<owner>/fractalengine-relay:<tag>`
- Configurable via env vars: `FE_DB_PATH`, `FE_BIND_ADDR`, `FE_KEYSTORE_BACKEND`, `FE_SECRET_*`

### FR-5: macOS Universal Binary
- Build aarch64 and x86_64 separately
- Combine via `lipo -create` in a dependent job
- Verify with `file` command (should show "Mach-O universal binary")

### FR-6: Cross.toml for cross-rs
Configure `cross-rs` for any ARM GNU targets:
- Pass through `RUST_LOG`, `CARGO_TERM_COLOR`
- Use `edge` image for aarch64-unknown-linux-gnu

---

## Testing Strategy

- **Workflow syntax:** validate YAML with `actionlint` locally before push
- **Caching:** verify cache hits on second run (check sccache stats in log)
- **Artifacts:** download from draft release, verify checksum matches, verify binary runs
- **Docker:** `docker run --rm fractalengine-relay:test /fe-relay --help` exits 0

---

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| sccache doesn't work inside cross-rs Docker | Disable `RUSTC_WRAPPER` for cross jobs |
| macOS runner architecture changes | Pin `macos-latest` and verify both targets build |
| GitHub Actions runner resource limits | Keep `jobs = 2` for Rust, use `--release` only in release workflow |
| Cargo.lock drift between OS-specific builds | Single `Cargo.lock` committed to repo, all builds use it |
| Long build times (~15min per target) | All 8 targets build in parallel; total wall time = longest single build |
