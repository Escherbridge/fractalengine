# Track: Cross-Platform Desktop — Linux + macOS + Windows ARM64 GUI Builds

**Created:** 2026-04-29
**Status:** Draft
**Priority:** P1
**Depends on:** none (independent of headless relay)
**Blocks:** Release CI (must compile before CI can ship artifacts)

---

## Problem Statement

FractalEngine currently builds and runs only on Windows x86_64 (`x86_64-pc-windows-msvc`). The codebase has:

- `.cargo/config.toml` hardcoded to Windows MSVC linker (`rust-lld.exe`)
- `fe-webview/src/backends/wry.rs` with extensive Win32 FFI (already `#[cfg]`-gated, but untested on other platforms)
- `fe-sync/src/blob_store.rs` with `#[cfg(unix)]` permission code (untested)
- No Linux or macOS build verification
- System dependency requirements undocumented (webkit2gtk, dbus, alsa, udev for Linux)

Until every target platform compiles and passes tests, cross-platform support is aspirational. This track makes it real.

---

## Goals

1. `cargo build -p fractalengine` succeeds on Linux x86_64 (with documented system deps)
2. `cargo build -p fractalengine` succeeds on macOS aarch64 and x86_64
3. `cargo build -p fractalengine` succeeds on Windows aarch64
4. `cargo test --workspace` passes on all three OS families
5. `.cargo/config.toml` supports all targets without manual editing
6. Platform-specific code is audited, tested, and documented
7. No platform-specific `#[cfg]` code exists without a test covering both branches

## Non-Goals (this track)

- Headless / server binaries (covered by Headless Relay track)
- CI/CD pipeline (covered by Release CI track)
- Mobile targets (Android, iOS)
- WASM target
- Packaging (MSI, DMG, AppImage) — deferred

---

## Architecture

### Platform Dependency Matrix

| Dependency | Windows | Linux | macOS | Notes |
|------------|---------|-------|-------|-------|
| Bevy 0.18 | Works | Needs `libasound2-dev`, `libudev-dev` | Works | Vulkan or Metal backend |
| bevy_egui | Works | Same as Bevy | Works | |
| wry 0.54 | WebView2 | webkit2gtk-4.1 | WKWebView | Different native backends per OS |
| keyring 3 | Credential Manager | Secret Service (dbus) | Keychain | Needs `libdbus-1-dev` on Linux |
| rfd 0.15 | Win32 dialogs | GTK3 dialogs | Cocoa dialogs | Needs `libgtk-3-dev` on Linux |
| iroh/libp2p | Works | Works | Works | Pure Rust networking |
| SurrealDB | Works | Works | Works | Pure Rust storage |
| dirs 5 | Works | Works | Works | XDG on Linux, standard on others |

### Linux System Dependencies

```bash
# Ubuntu/Debian
sudo apt-get install -y \
  pkg-config \
  libdbus-1-dev \
  libwebkit2gtk-4.1-dev \
  libasound2-dev \
  libudev-dev \
  libssl-dev \
  libxcb-shape0-dev \
  libxcb-xfixes0-dev \
  libgtk-3-dev

# Fedora/RHEL
sudo dnf install -y \
  pkg-config \
  dbus-devel \
  webkit2gtk4.1-devel \
  alsa-lib-devel \
  systemd-devel \
  openssl-devel \
  libxcb-devel \
  gtk3-devel
```

### .cargo/config.toml Strategy

Replace the Windows-only config with a multi-target setup:

```toml
[build]
jobs = 2

[target.x86_64-pc-windows-msvc]
linker = "rust-lld.exe"

[target.aarch64-pc-windows-msvc]
linker = "rust-lld.exe"

# Linux and macOS use default linker (cc)
```

---

## Functional Requirements

### FR-1: Linux x86_64 GUI Build
`cargo build -p fractalengine --target x86_64-unknown-linux-gnu` succeeds with documented system dependencies installed. The binary launches, creates a window, and renders the Bevy viewport.

### FR-2: macOS GUI Build
`cargo build -p fractalengine --target aarch64-apple-darwin` succeeds on a macOS host with Xcode installed. Same for `x86_64-apple-darwin`. The binary launches with Metal rendering backend.

### FR-3: Windows ARM64 GUI Build
`cargo build -p fractalengine --target aarch64-pc-windows-msvc` succeeds. WebView2 works on ARM64 Windows.

### FR-4: Multi-Target .cargo/config.toml
Config file supports all targets without manual editing. Linker overrides are target-specific. `jobs` setting is shared.

### FR-5: Platform Code Audit
Every `#[cfg(target_os)]` and `#[cfg(unix/windows)]` block has a test or is verified to compile on all platforms. No dead code on any platform.

### FR-6: Platform-Specific Test Coverage
- `fe-sync/src/blob_store.rs` Unix permissions: test on Linux/macOS
- `fe-webview/src/backends/wry.rs` Win32 FFI: compile-test on Windows
- `fe-webview/src/backends/stub.rs` browser launch: verify command exists per OS
- `fe-identity/src/keychain.rs` keyring: verified on all 3 OS families (after SecretStore migration, tests use InMemoryBackend)

### FR-7: Developer Setup Documentation
`BUILDING.md` documents prerequisites per platform. Copy-paste ready for a fresh checkout.

---

## Testing Strategy

- **Compile gates:** `cargo check -p fractalengine` on all 3 OS families (Linux, macOS, Windows) in CI
- **Unit tests:** `cargo test --workspace` passes on all 3 OS families with `FE_KEYSTORE_BACKEND=env`
- **Platform cfg audit:** grep for `#[cfg(` and verify each has a complementary branch or is tested
- **Smoke test:** binary launches and exits cleanly on each platform (can be headless Bevy test)

---

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| webkit2gtk version mismatch on distros | Document minimum version (4.1), test on Ubuntu LTS |
| Bevy Metal backend issues on macOS | Bevy 0.18 supports Metal natively; well-tested upstream |
| Windows ARM64 WebView2 availability | WebView2 is pre-installed on Windows 11 ARM64 |
| wry macOS compile issues | wry 0.54 supports macOS natively via WKWebView |
| ring crate build differences | ring 0.17.x builds on all Tier 1 targets; verified upstream |
