//! The closed WASI policy for 2.0 guests; see src/wasm/AGENTS.md §wasi-policy.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use crate::LogLevel;

use super::ComponentError;

/// The one contract package a component may import from.
pub const CONTRACT_PACKAGE: &str = "fractalengine:plugin";
/// The contract major.minor this host binds; a newer minor's imports may not exist here.
pub const CONTRACT_MAJOR_MINOR: &str = "2.0.";

/// Interfaces of `fractalengine:plugin@2.0.0` a component may import.
const CONTRACT_IMPORTS: [&str; 11] = [
    "types", "context", "scene", "txn", "readings", "routes", "ingress", "egress", "bus", "kv", "log",
];

/// WASI 0.2 interfaces a component may import (virtualized); anything else is `forbidden-import`.
/// `wasi:clocks/monotonic-clock` is NOT here: its `subscribe-*` pollables sleep in real time
/// (forever for huge durations) whatever the frozen clock says, which would hang the plugin worker.
const WASI_IMPORTS: [&str; 19] = [
    "wasi:cli/environment",
    "wasi:cli/exit",
    "wasi:cli/stdin",
    "wasi:cli/stdout",
    "wasi:cli/stderr",
    "wasi:cli/terminal-input",
    "wasi:cli/terminal-output",
    "wasi:cli/terminal-stdin",
    "wasi:cli/terminal-stdout",
    "wasi:cli/terminal-stderr",
    "wasi:io/error",
    "wasi:io/streams",
    "wasi:io/poll",
    "wasi:clocks/wall-clock",
    "wasi:random/random",
    "wasi:random/insecure",
    "wasi:random/insecure-seed",
    "wasi:filesystem/types",
    "wasi:filesystem/preopens",
];

/// Judge one component import name (`ns:pkg/iface@version`) against the closed list.
pub fn check_import(name: &str) -> Result<(), ComponentError> {
    let (base, version) = match name.split_once('@') {
        Some((base, version)) => (base, Some(version)),
        None => (name, None),
    };
    if let Some(interface) = base
        .strip_prefix(CONTRACT_PACKAGE)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        if !version.is_some_and(|v| v.starts_with(CONTRACT_MAJOR_MINOR)) {
            return Err(ComponentError::UnsupportedContract(format!("component imports {name}")));
        }
        if CONTRACT_IMPORTS.contains(&interface) {
            return Ok(());
        }
    } else if WASI_IMPORTS.contains(&base) && version.is_some_and(|v| v.starts_with("0.2.")) {
        return Ok(());
    }
    Err(ComponentError::PackageInvalid(format!("forbidden-import {name}")))
}

// ---------------------------------------------------------------------------
// Time: frozen at the hook's injected-clock instant (never a clock source)
// ---------------------------------------------------------------------------

/// `wasi:clocks/wall-clock`, frozen at `context.now-ms`.
pub(crate) struct FrozenWallClock(pub(crate) Arc<AtomicU64>);

impl wasmtime_wasi::HostWallClock for FrozenWallClock {
    fn resolution(&self) -> Duration {
        Duration::from_millis(1)
    }
    fn now(&self) -> Duration {
        Duration::from_millis(self.0.load(Ordering::Acquire))
    }
}

/// `wasi:clocks/monotonic-clock`, frozen at `context.now-ms` (nanoseconds). Linked for
/// defense in depth only; `check_import` refuses components that import the interface.
pub(crate) struct FrozenMonotonicClock(pub(crate) Arc<AtomicU64>);

impl wasmtime_wasi::HostMonotonicClock for FrozenMonotonicClock {
    fn resolution(&self) -> u64 {
        1_000_000
    }
    fn now(&self) -> u64 {
        self.0.load(Ordering::Acquire).saturating_mul(1_000_000)
    }
}

// ---------------------------------------------------------------------------
// Randomness: deterministic per (installation, activation); NOT cryptographic
// ---------------------------------------------------------------------------

/// Bytes in each cycled random stream.
const RANDOM_STREAM_BYTES: usize = 128 * 1024;
/// Largest single `get-random-bytes` request.
const MAX_RANDOM_REQUEST: u64 = 64 * 1024;

/// A blake3-XOF stream keyed by installation, activation and purpose.
fn random_stream(installation_id: &str, activation: u64, purpose: &str) -> Vec<u8> {
    let mut hasher = blake3::Hasher::new_derive_key("fractalengine extension wasi random v1");
    hasher.update(installation_id.as_bytes());
    hasher.update(&activation.to_le_bytes());
    hasher.update(purpose.as_bytes());
    let mut stream = vec![0; RANDOM_STREAM_BYTES];
    hasher.finalize_xof().fill(&mut stream);
    stream
}

/// Build the per-instance WASI context: no args, env, preopens or network; frozen clocks;
/// seeded randomness; stdout/stderr routed into the instance log.
pub(crate) fn build_wasi_ctx(
    installation_id: &str,
    activation: u64,
    now_ms: Arc<AtomicU64>,
    log: Arc<Mutex<GuestLog>>,
) -> wasmtime_wasi::WasiCtx {
    let seed = random_stream(installation_id, activation, "insecure-seed");
    let mut seed_bytes = [0u8; 16];
    seed_bytes.copy_from_slice(&seed[..16]);
    let mut builder = wasmtime_wasi::WasiCtxBuilder::new();
    builder
        .stdout(GuestStdio { log: log.clone(), level: LogLevel::Info })
        .stderr(GuestStdio { log, level: LogLevel::Warn })
        .wall_clock(FrozenWallClock(now_ms.clone()))
        .monotonic_clock(FrozenMonotonicClock(now_ms))
        .secure_random(wasmtime_wasi::Deterministic::new(random_stream(installation_id, activation, "random")))
        .insecure_random(wasmtime_wasi::Deterministic::new(random_stream(installation_id, activation, "insecure")))
        .insecure_random_seed(u128::from_le_bytes(seed_bytes))
        .max_random_size(MAX_RANDOM_REQUEST)
        .allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false);
    builder.build()
}

// ---------------------------------------------------------------------------
// Log: `log.write` and guest stdio share one per-instance budget
// ---------------------------------------------------------------------------

/// Longest log message; longer ones are cut at a char boundary.
pub const MAX_LOG_MESSAGE_BYTES: usize = 4 * 1024;
/// Messages per instance per second of the injected clock; the excess is dropped and counted.
pub const MAX_LOG_PER_SECOND: u32 = 50;

/// One instance's log sink: rate-limited, tagged with installation and package.
pub struct GuestLog {
    installation_id: String,
    package_id: String,
    now_ms: Arc<AtomicU64>,
    second: u64,
    written: u32,
    dropped: u64,
    /// Unterminated stdout / stderr bytes.
    pending: [Vec<u8>; 2],
}

impl GuestLog {
    pub(crate) fn new(installation_id: &str, package_id: &str, now_ms: Arc<AtomicU64>) -> Self {
        Self {
            installation_id: installation_id.to_string(),
            package_id: package_id.to_string(),
            now_ms,
            second: u64::MAX,
            written: 0,
            dropped: 0,
            pending: [Vec::new(), Vec::new()],
        }
    }

    /// Messages dropped by the rate cap so far.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Emit one message through `tracing`, or count it as dropped.
    pub fn write(&mut self, level: LogLevel, message: &str) {
        let second = self.now_ms.load(Ordering::Acquire) / 1_000;
        if second != self.second {
            self.second = second;
            self.written = 0;
        }
        if self.written >= MAX_LOG_PER_SECOND {
            self.dropped += 1;
            return;
        }
        self.written += 1;
        let message = truncate_utf8(message, MAX_LOG_MESSAGE_BYTES);
        let (installation, package) = (&self.installation_id, &self.package_id);
        match level {
            LogLevel::Debug => tracing::debug!(installation = %installation, package = %package, "{}", message),
            LogLevel::Info => tracing::info!(installation = %installation, package = %package, "{}", message),
            LogLevel::Warn => tracing::warn!(installation = %installation, package = %package, "{}", message),
            LogLevel::Error => tracing::error!(installation = %installation, package = %package, "{}", message),
        }
    }

    /// Line-buffer guest stdio; each complete (or over-long) line is one message.
    fn push_stdio(&mut self, level: LogLevel, bytes: &[u8]) {
        let slot = usize::from(level != LogLevel::Info);
        self.pending[slot].extend_from_slice(bytes);
        loop {
            let pending = &mut self.pending[slot];
            let cut = match pending.iter().position(|byte| *byte == b'\n') {
                Some(newline) => newline,
                None if pending.len() > MAX_LOG_MESSAGE_BYTES => MAX_LOG_MESSAGE_BYTES,
                None => break,
            };
            let line: Vec<u8> = pending.drain(..cut).collect();
            if pending.first() == Some(&b'\n') {
                pending.remove(0);
            }
            let text = String::from_utf8_lossy(&line).into_owned();
            self.write(level, text.trim_end_matches('\r'));
        }
    }
}

/// The longest prefix of `text` within `max` bytes that ends on a char boundary.
pub(crate) fn truncate_utf8(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Most bytes one stdio write may carry before the guest must re-check.
const STDIO_PERMIT: usize = 64 * 1024;

/// Guest stdout (info) or stderr (warn), feeding the instance log; never a terminal.
#[derive(Clone)]
pub(crate) struct GuestStdio {
    log: Arc<Mutex<GuestLog>>,
    level: LogLevel,
}

impl GuestStdio {
    fn push(&self, bytes: &[u8]) {
        if let Ok(mut log) = self.log.lock() {
            log.push_stdio(self.level, bytes);
        }
    }
}

impl wasmtime_wasi::cli::IsTerminal for GuestStdio {
    fn is_terminal(&self) -> bool {
        false
    }
}

impl wasmtime_wasi::cli::StdoutStream for GuestStdio {
    fn p2_stream(&self) -> Box<dyn wasmtime_wasi::p2::OutputStream> {
        Box::new(self.clone())
    }
    fn async_stream(&self) -> Box<dyn tokio::io::AsyncWrite + Send + Sync> {
        Box::new(self.clone())
    }
}

#[wasmtime_wasi::async_trait]
impl wasmtime_wasi::p2::Pollable for GuestStdio {
    async fn ready(&mut self) {}
}

#[wasmtime_wasi::async_trait]
impl wasmtime_wasi::p2::OutputStream for GuestStdio {
    fn write(&mut self, bytes: bytes::Bytes) -> wasmtime_wasi::p2::StreamResult<()> {
        self.push(&bytes);
        Ok(())
    }
    fn flush(&mut self) -> wasmtime_wasi::p2::StreamResult<()> {
        Ok(())
    }
    fn check_write(&mut self) -> wasmtime_wasi::p2::StreamResult<usize> {
        Ok(STDIO_PERMIT)
    }
}

impl tokio::io::AsyncWrite for GuestStdio {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        self.push(buf);
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
