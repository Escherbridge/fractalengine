# Building FractalEngine

## Prerequisites (all platforms)

- **Rust 1.83+** (stable toolchain)
- **Git**

```bash
rustup toolchain install stable
rustup component add rustfmt clippy
```

## Windows

- **Visual Studio Build Tools 2022** with "Desktop development with C++" workload
- WebView2 runtime (pre-installed on Windows 10 1803+ and Windows 11)
- No additional system packages required

### Supported targets

| Target | Status |
|--------|--------|
| `x86_64-pc-windows-msvc` | Primary development target |
| `aarch64-pc-windows-msvc` | Supported (cross-compile from x64) |

```bash
# x86_64 (default)
cargo build --release -p fractalengine

# ARM64 (cross-compile)
rustup target add aarch64-pc-windows-msvc
cargo build --release -p fractalengine --target aarch64-pc-windows-msvc
```

## Linux (Ubuntu/Debian)

```bash
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
```

```bash
cargo build --release -p fractalengine
```

## Linux (Fedora/RHEL)

```bash
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

```bash
cargo build --release -p fractalengine
```

## macOS

- **Xcode 15+** (or Command Line Tools: `xcode-select --install`)
- No additional system packages required
- WKWebView is included with macOS

### Supported targets

| Target | Status |
|--------|--------|
| `aarch64-apple-darwin` | Apple Silicon (M1+) |
| `x86_64-apple-darwin` | Intel Mac |

```bash
cargo build --release -p fractalengine
```

## Running

```bash
# From build directory
./target/release/fractalengine

# With debug logging
RUST_LOG=debug cargo run -p fractalengine
```

## Testing

```bash
# Run all tests
cargo test --workspace

# Run with env-based keystore (for CI / headless)
FE_KEYSTORE_BACKEND=env cargo test --workspace
```

### Platform-specific test notes

- **Unix-only:** `blob_store_sets_restrictive_permissions` verifies 0o700 directory permissions
  (skipped on Windows via `#[cfg(unix)]`)
- **Windows-only:** `wry_backend_platform_types_compile` checks Win32 popup handle types
  (non-Windows variant checks `ParentHandle`)
- **Per-OS:** `stub_browser_command_exists` verifies the system browser launcher
  (`cmd` on Windows, `open` on macOS, `xdg-open` on Linux)
- **All platforms:** `data_local_dir_exists` confirms `dirs::data_local_dir()` returns `Some`

## Known Issues

- Cross-compilation from Windows to Linux/macOS requires platform-specific C toolchains
  (use native builds or CI for non-Windows targets)
- The `ring` crate requires a C compiler for all targets
- Linux builds require webkit2gtk **4.1** (not 4.0) for wry 0.54 compatibility
