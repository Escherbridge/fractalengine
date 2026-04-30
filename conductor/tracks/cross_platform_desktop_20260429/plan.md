# Implementation Plan: Cross-Platform Desktop

## Overview

Three-phase implementation. Phase 1 fixes build infrastructure so all targets compile. Phase 2 audits and tests platform-specific code. Phase 3 documents and verifies end-to-end.

Each task is ordered so that progress is immediately verifiable: fix the config, then compile, then test, then document.

**Documentation is continuous.** Each phase ends with a docs task. `BUILDING.md` grows incrementally — not written all at once at the end.

---

## Phase 1: Build Infrastructure

**Goal:** `cargo check -p fractalengine` succeeds on Linux, macOS, and Windows ARM64.

---

### Task 1.1 — Fix .cargo/config.toml for multi-target [x]

Current config is Windows-only:
```toml
[target.x86_64-pc-windows-msvc]
linker = "rust-lld.exe"
```

Update to support all targets:
```toml
[build]
jobs = 2

[target.x86_64-pc-windows-msvc]
linker = "rust-lld.exe"

[target.aarch64-pc-windows-msvc]
linker = "rust-lld.exe"

# Linux and macOS use default system linker — no override needed
```

**Verification:** `cargo check` still works on Windows. Config doesn't error on non-Windows hosts.

**Files:** `.cargo/config.toml`

---

### Task 1.2 — Verify Linux x86_64 compile [~] (needs native platform or CI)

On an Ubuntu system (or CI runner) with system deps installed:

```bash
sudo apt-get install -y pkg-config libdbus-1-dev libwebkit2gtk-4.1-dev \
  libasound2-dev libudev-dev libssl-dev libxcb-shape0-dev libxcb-xfixes0-dev libgtk-3-dev

cargo check -p fractalengine --target x86_64-unknown-linux-gnu
```

Fix any compile errors. Expected issues:
- Missing `use` imports behind `#[cfg(windows)]` that have no Linux counterpart
- Any Windows-assumed path separators in string literals

**Files:** any that fail to compile (likely none based on codebase audit, but verify).

---

### Task 1.3 — Verify macOS aarch64 compile [~] (needs native platform or CI)

On a macOS host with Xcode installed:

```bash
cargo check -p fractalengine --target aarch64-apple-darwin
```

Fix any compile errors. macOS is the closest to "just works" given Bevy + wry native support.

**Files:** any that fail to compile.

---

### Task 1.4 — Verify Windows ARM64 compile [~] (needs MSVC ARM64 toolchain or clang)

On a Windows host (x86_64 is fine — MSVC cross-compiles to ARM64):

```bash
rustup target add aarch64-pc-windows-msvc
cargo check -p fractalengine --target aarch64-pc-windows-msvc
```

Fix any compile errors. Expected: should work since all deps are pure Rust or have MSVC ARM64 support.

**Files:** any that fail to compile.

---

### Task 1.5 — Phase 1 docs: create BUILDING.md scaffold [x]

Create `BUILDING.md` with:
- Prerequisites per platform (Rust version, system deps for Linux, Xcode for macOS)
- `cargo build` command for each verified target
- Known issues found during Tasks 1.2-1.4

This file grows in each phase — Phase 2 adds testing info, Phase 3 adds the smoke test.

**Files:** new `BUILDING.md`.

---

### Phase 1 Checkpoint

- `cargo check -p fractalengine` succeeds on 4 targets: Windows x64, Windows ARM64, Linux x64, macOS ARM64
- `.cargo/config.toml` is multi-target clean
- `BUILDING.md` exists with per-platform build instructions

---

## Phase 2: Platform Code Audit and Testing

**Goal:** Every `#[cfg]` block is tested. Platform-specific behavior is verified.

---

### Task 2.1 — Audit all #[cfg] usage [x]

Grep the entire workspace for conditional compilation:

```bash
grep -rn '#\[cfg(' --include='*.rs'
```

For each occurrence, categorize:
- **A: Already correct** — both branches exist and are tested
- **B: Correct but untested** — needs a test on the target platform
- **C: Missing branch** — needs a fallback for other platforms

Document findings. Create subtasks for B and C items.

**Known sites from codebase audit:**
1. `fe-sync/src/blob_store.rs:30-41` — `#[cfg(unix)]` permissions (Category B)
2. `fe-webview/src/backends/wry.rs:18-217` — `#[cfg(target_os = "windows")]` Win32 FFI (Category A — already gated)
3. `fe-webview/src/backends/stub.rs:55-74` — per-OS browser launch (Category A)
4. `fe-webview/src/backends/wry.rs:285+` — Windows-specific popup handle fields (Category A)

**Files:** audit document (can be a checklist in this plan file or a separate `audit.md`).

---

### Task 2.2 — Test blob_store Unix permissions (TDD) [x]

Write a test for `fe-sync/src/blob_store.rs` that verifies:
- On Unix: blob store root directory has 0o700 permissions after initialization
- On non-Unix: initialization succeeds without error (permissions are a no-op)

```rust
#[cfg(unix)]
#[test]
fn blob_store_sets_restrictive_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let store = FsBlobStore::new(dir.path()).unwrap();
    let perms = std::fs::metadata(store.root()).unwrap().permissions();
    assert_eq!(perms.mode() & 0o777, 0o700);
}
```

**Files:** `fe-sync/src/blob_store.rs` (test module).

---

### Task 2.3 — Test wry backend platform compilation [x]

Add compile-time verification tests:

```rust
#[test]
fn wry_backend_compiles() {
    // This test existing and compiling on each platform proves
    // the #[cfg] gates are correct. No runtime assertion needed.
    #[cfg(target_os = "windows")]
    {
        // Win32 FFI types exist
        let _ = std::mem::size_of::<super::win32::Hwnd>();
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Non-Windows path compiles without Win32 types
        let _ = std::mem::size_of::<super::ParentHandle>();
    }
}
```

**Files:** `fe-webview/src/backends/wry.rs` (test module).

---

### Task 2.4 — Test stub backend browser launch commands [x]

Verify the browser-open commands are valid on each platform:

```rust
#[test]
fn stub_browser_command_exists() {
    #[cfg(target_os = "windows")]
    assert!(std::process::Command::new("cmd").arg("/C").arg("echo test").output().is_ok());

    #[cfg(target_os = "macos")]
    assert!(std::process::Command::new("open").arg("--version").output().is_ok());

    #[cfg(target_os = "linux")]
    assert!(std::process::Command::new("which").arg("xdg-open").output().is_ok());
}
```

**Files:** `fe-webview/src/backends/stub.rs` (test module).

---

### Task 2.5 — Verify dirs crate paths on each platform [x]

Write a test that `dirs::data_local_dir()` returns `Some` on all platforms (it should, but verify):

```rust
#[test]
fn data_local_dir_exists() {
    assert!(dirs::data_local_dir().is_some(), "dirs::data_local_dir() returned None on this platform");
}
```

This runs in CI on all 3 OS families and catches any platform where the directory resolution fails.

**Files:** `fe-sync/src/blob_store.rs` or `fe-sync/src/offline.rs` (test module).

---

### Task 2.6 — Audit hardcoded paths for platform safety [x]

Grep for string literals that look like paths:

```bash
grep -rn '"data/' --include='*.rs'
grep -rn '"fractalengine/' --include='*.rs'
grep -rn '\\\\' --include='*.rs'  # backslashes
```

Verify all use `std::path::Path` / `PathBuf` (not string concatenation). The current code uses `Path::new()` which handles separators correctly — confirm no regressions.

**Files:** audit only (no changes expected based on prior review).

---

### Task 2.7 — Phase 2 docs: update BUILDING.md with testing info [x]

Add to `BUILDING.md`:
- "Running tests" section: `cargo test --workspace`
- Platform-specific test notes (e.g., blob store permission tests only run on Unix)
- Any test skips or known platform-specific behaviors discovered in this phase

**Files:** `BUILDING.md`.

---

### Phase 2 Checkpoint

- Every `#[cfg]` block has a covering test on each platform
- Blob store permissions verified on Unix
- WebView backend compilation verified per-platform
- Path handling audited — all use `std::path`
- `BUILDING.md` updated with testing instructions

---

## Phase 3: Documentation and Verification

**Goal:** A developer can clone the repo and build on any supported platform.

---

### Task 3.1 — Create BUILDING.md [x]

Document build prerequisites per platform:

```markdown
# Building FractalEngine

## Prerequisites (all platforms)
- Rust 1.83+ (stable)
- Git

## Windows
- Visual Studio Build Tools 2022 (MSVC toolchain)
- No additional dependencies

## Linux (Ubuntu/Debian)
sudo apt-get install -y pkg-config libdbus-1-dev ...

## Linux (Fedora/RHEL)
sudo dnf install -y pkg-config dbus-devel ...

## macOS
- Xcode 15+ (or Command Line Tools)
- No additional dependencies

## Building
cargo build --release -p fractalengine

## Running
./target/release/fractalengine
```

**Files:** new `BUILDING.md` at repo root.

---

### Task 3.2 — Full workspace test on all platforms [x] (Windows verified)

Run `cargo test --workspace` on:
- Ubuntu latest (x86_64)
- macOS latest (aarch64)
- Windows latest (x86_64)

With `FE_KEYSTORE_BACKEND=env` to avoid OS keychain issues in CI.

Fix any test failures. Document any platform-specific test skips.

**Files:** any failing tests.

---

### Task 3.3 — Smoke test: binary launches on each platform

On each platform, verify:
1. Binary starts without crash
2. Window appears (or exits cleanly with appropriate error if no display)
3. Bevy initializes (check for "fractalengine" in log output)
4. Binary responds to `Ctrl+C` shutdown

This can be manual for now — automated GUI testing is out of scope.

**Files:** none (manual verification).

---

### Phase 3 Checkpoint

- BUILDING.md complete with copy-paste instructions
- `cargo test --workspace` passes on all 3 OS families
- Binary launches successfully on all platforms
- Developer can go from clone to running binary on any supported OS

---

## Summary

| Phase | Delivers | Verification |
|-------|----------|-------------|
| 1 | Multi-target build infra | `cargo check` succeeds on 4 targets |
| 2 | Platform code audit + tests | Every `#[cfg]` block has test coverage |
| 3 | BUILDING.md + full verification | Clone-to-run works on all platforms |
