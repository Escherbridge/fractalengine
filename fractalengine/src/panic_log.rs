//! Startup panic hook: chains onto the previous hook and appends payload +
//! backtrace to `data/panic.log`, so an in-pass egui panic (which otherwise
//! only surfaces as "Encountered a panic in system" + abort) leaves evidence
//! behind. std-only. See `src/AGENTS.md` §panic-hook.

use std::fmt::Write as _;
use std::io::Write as _;
use std::panic::PanicHookInfo;

const PANIC_LOG_PATH: &str = "data/panic.log";

/// Installs the panic logger, chaining the previous hook (default stderr
/// printer, or whatever was installed before this call) so existing
/// behavior — including Bevy's own panic reporting — is preserved.
pub(crate) fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        append_panic_entry(info);
        previous(info);
    }));
}

fn append_panic_entry(info: &PanicHookInfo<'_>) {
    let line = format_panic_line(info);
    if let Some(parent) = std::path::Path::new(PANIC_LOG_PATH).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(PANIC_LOG_PATH)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Extracts thread/location/payload/backtrace from a live `PanicHookInfo` and
/// builds the log line via the pure `format_panic_entry` below.
fn format_panic_line(info: &PanicHookInfo<'_>) -> String {
    let timestamp_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("<unnamed>");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "<unknown location>".to_string());
    let payload = panic_payload_string(info.payload());
    let backtrace = std::backtrace::Backtrace::force_capture();
    format_panic_entry(
        timestamp_secs,
        thread_name,
        &location,
        &payload,
        &backtrace.to_string(),
    )
}

/// `&str`/`String` payloads cover every `unwrap`/`expect`/`panic!`; anything
/// else logs a placeholder rather than silently dropping the entry.
fn panic_payload_string(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Pure line builder — one `data/panic.log` entry. Kept separate from
/// `format_panic_line` so the format is unit-testable without a real panic.
fn format_panic_entry(
    timestamp_secs: u64,
    thread_name: &str,
    location: &str,
    payload: &str,
    backtrace: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "[{timestamp_secs}] thread '{thread_name}' panicked at {location}: {payload}"
    );
    let _ = writeln!(out, "{backtrace}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_panic_entry_includes_every_field() {
        let line = format_panic_entry(
            1_700_000_000,
            "Main Thread",
            "src/foo.rs:10:5",
            "boom",
            "<backtrace>",
        );
        assert!(line.contains("1700000000"));
        assert!(line.contains("Main Thread"));
        assert!(line.contains("src/foo.rs:10:5"));
        assert!(line.contains("boom"));
        assert!(line.contains("<backtrace>"));
    }

    #[test]
    fn panic_payload_string_extracts_str_payload() {
        let s: &str = "boom &str";
        let boxed: Box<dyn std::any::Any + Send> = Box::new(s);
        assert_eq!(panic_payload_string(boxed.as_ref()), "boom &str");
    }

    #[test]
    fn panic_payload_string_extracts_string_payload() {
        let boxed: Box<dyn std::any::Any + Send> = Box::new(String::from("boom String"));
        assert_eq!(panic_payload_string(boxed.as_ref()), "boom String");
    }

    #[test]
    fn panic_payload_string_falls_back_for_non_string_payload() {
        let boxed: Box<dyn std::any::Any + Send> = Box::new(42i32);
        assert_eq!(
            panic_payload_string(boxed.as_ref()),
            "<non-string panic payload>"
        );
    }
}
