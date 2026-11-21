//! JSONL journal line and append (S2). Tokens and capabilities are never written.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Resolved request recorded for replay.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ResolvedOp {
    pub verb: String,
    pub via: String,
    pub tool: Option<String>,
    pub args: Option<Value>,
    pub method: Option<String>,
    pub path: Option<String>,
}

/// Instance summary recorded per line (no token).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JournalInstance {
    pub name: String,
    pub api_base: String,
    pub role: Option<String>,
}

/// One journal line (plan §2 "Journal line").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct JournalLine {
    pub v: u32,
    pub run: Option<String>,
    pub agent: Option<String>,
    pub seq: u64,
    /// RFC3339 wall time.
    pub t: String,
    /// Wall-clock Unix epoch milliseconds (same instant as `t`; not monotonic).
    pub unix_ms: u64,
    pub latency_ms: u64,
    pub argv: Vec<String>,
    pub resolved: ResolvedOp,
    pub ctx_before: Value,
    pub ctx_after: Value,
    pub instance: JournalInstance,
    pub exit: i32,
    pub code: String,
    /// blake3 hex digest of the printed envelope line.
    pub envelope_digest: String,
    pub created_ids: Vec<String>,
    /// Response data trimmed to 2 KB.
    pub data_small: Option<Value>,
    pub wait: Option<Value>,
    pub help: bool,
    pub note: Option<String>,
    pub cli_version: String,
    pub catalog_digest: String,
}

/// Appends one line to the journal file as a single write.
pub fn append_line(path: &Path, line: &JournalLine) -> std::io::Result<()> {
    let mut text = serde_json::to_string(line).map_err(std::io::Error::other)?;
    text.push('\n');
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(text.as_bytes())
}

/// Machine code of a budget refusal (exit 11).
pub const BUDGET_EXHAUSTED_CODE: &str = "budget-exhausted";
/// Machine code when the budget counter cannot be read, locked or written (exit 11, fail closed).
pub const BUDGET_UNAVAILABLE_CODE: &str = "budget-unavailable";
/// Longest wait for the counter lock before failing closed.
const COUNTER_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// One invocation's charge against the per-agent sidecar counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charge {
    /// This invocation's per-agent sequence number (budget refusals included).
    pub seq: u64,
    /// False when the budget was already spent (nothing was counted).
    pub admitted: bool,
}

/// `<journal>.<agent digest>.count`: the per-agent counter beside the journal (the agent id never names a file).
pub fn counter_path(journal: &Path, agent: Option<&str>) -> std::path::PathBuf {
    let digest = blake3::hash(agent.unwrap_or("").as_bytes()).to_hex();
    let name = journal.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_else(|| "journal".into());
    journal.with_file_name(format!("{name}.{}.count", &digest.as_str()[..16]))
}

/// Under an exclusive file lock: read `counted lines`, refuse when `counted >= max`, else count; always bump `lines`.
/// Any read, parse, lock or write failure is an error, which the caller treats as budget exhausted (fail closed).
pub fn charge(counter: &Path, max: Option<u64>) -> std::io::Result<Charge> {
    use std::io::{Read, Seek, SeekFrom};
    if let Some(parent) = counter.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(counter)?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) if started.elapsed() < COUNTER_LOCK_WAIT => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "budget counter stayed locked"));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let corrupt = || std::io::Error::new(std::io::ErrorKind::InvalidData, "budget counter is not `<counted> <lines>`");
    let (counted, lines) = match text.split_whitespace().collect::<Vec<_>>().as_slice() {
        [] => (0u64, 0u64),
        [counted, lines] => (counted.parse().map_err(|_| corrupt())?, lines.parse().map_err(|_| corrupt())?),
        _ => return Err(corrupt()),
    };
    let admitted = max.is_none_or(|max| counted < max);
    let (counted, lines) = (counted + u64::from(admitted), lines + 1);
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(format!("{counted} {lines}").as_bytes())?;
    file.unlock()?;
    Ok(Charge { seq: lines, admitted })
}

/// Replaces capability-bearing values (tokens, invites, tickets, secrets) with a digest placeholder.
pub fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, inner)| {
                    let lowered = key.to_ascii_lowercase();
                    let secret = ["token", "invite", "secret", "password", "ticket", "credential"]
                        .iter()
                        .any(|needle| lowered.contains(needle));
                    let replacement = if secret && !inner.is_null() { redacted_placeholder(inner) } else { redact(inner) };
                    (key.clone(), replacement)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

fn redacted_placeholder(value: &Value) -> Value {
    let digest = blake3::hash(value.to_string().as_bytes()).to_hex();
    Value::String(format!("<redacted blake3:{}>", &digest.as_str()[..16]))
}

/// Longest string value (in chars) the journal keeps verbatim inside args.
pub const JOURNAL_MAX_STRING_CHARS: usize = 256;

/// [`redact`], then every string over [`JOURNAL_MAX_STRING_CHARS`] or carrying a loaded secret becomes a placeholder;
/// the structure is kept.
pub fn redact_with(value: &Value, secrets: &[String]) -> Value {
    redact_strings(&redact(value), secrets, Some(JOURNAL_MAX_STRING_CHARS))
}

/// Replaces strings carrying a loaded secret (and, with `max_chars`, longer strings) by a placeholder.
pub fn redact_strings(value: &Value, secrets: &[String], max_chars: Option<usize>) -> Value {
    match value {
        Value::String(text) if max_chars.is_some_and(|max| text.chars().count() > max) || crate::secrets::text_has_secret(text, secrets) => {
            redacted_placeholder(value)
        }
        Value::Object(map) => Value::Object(map.iter().map(|(key, inner)| (key.clone(), redact_strings(inner, secrets, max_chars))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(|item| redact_strings(item, secrets, max_chars)).collect()),
        other => other.clone(),
    }
}

/// One argv word for the journal: a JSON object/array word (an `--args-json` payload) keeps its structure with
/// [`redact_with`] applied; any other word over [`JOURNAL_MAX_STRING_CHARS`] or carrying a loaded secret is replaced.
pub fn redact_word(word: &str, secrets: &[String]) -> String {
    if let Ok(parsed @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(word) {
        return redact_with(&parsed, secrets).to_string();
    }
    if word.chars().count() > JOURNAL_MAX_STRING_CHARS || crate::secrets::text_has_secret(word, secrets) {
        redacted_placeholder(&Value::String(word.to_string())).as_str().unwrap_or_default().to_string()
    } else {
        word.to_string()
    }
}

/// Redacts and bounds response data to 2 KB for the journal.
pub fn data_small(data: &Value) -> Value {
    let redacted = redact(data);
    let size = redacted.to_string().len();
    if size <= DATA_SMALL_LIMIT_BYTES {
        redacted
    } else {
        serde_json::json!({ "truncated": true, "bytes": size })
    }
}

const DATA_SMALL_LIMIT_BYTES: usize = 2048;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_counter_never_overruns_across_threads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let counter = counter_path(&dir.path().join("journal.jsonl"), Some("agent-a"));
        let charges: Vec<Charge> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| (0..10).map(|_| charge(&counter, Some(25)).expect("charge")).collect::<Vec<_>>()))
                .collect();
            workers.into_iter().flat_map(|worker| worker.join().expect("worker")).collect()
        });
        assert_eq!(charges.iter().filter(|charge| charge.admitted).count(), 25, "exactly the cap is admitted");
        let mut seqs: Vec<u64> = charges.iter().map(|charge| charge.seq).collect();
        seqs.sort_unstable();
        assert_eq!(seqs, (1..=80).collect::<Vec<u64>>(), "every invocation gets a distinct seq");
        std::fs::write(&counter, "garbage").expect("corrupt");
        assert!(charge(&counter, Some(25)).is_err(), "an unreadable counter fails closed");
    }
}
