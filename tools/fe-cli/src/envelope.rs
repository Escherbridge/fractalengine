//! Stable `--json` envelope, exit-code table and the shared CLI error type (S2).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Process exit codes (plan §1 "Exit codes").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ExitCode {
    Ok = 0,
    Internal = 1,
    Refused = 2,
    Transport = 3,
    RoleDenied = 4,
    NotFound = 5,
    Usage = 6,
    Retryable = 7,
    UnknownOutcome = 8,
    WaitExpired = 9,
    ServerFault = 10,
    BudgetExhausted = 11,
    /// `fe replay`: a replayed step answered with a different exit code than the journal recorded.
    ReplayDiverged = 12,
}

impl ExitCode {
    /// Numeric process exit code.
    pub fn number(self) -> i32 {
        self as u8 as i32
    }
}

/// Error carried between slices before it becomes an envelope.
#[derive(Debug, Clone)]
pub struct CliError {
    pub exit: ExitCode,
    /// Stable machine code, e.g. `role-denied`, `not-loopback`.
    pub code: String,
    pub message: String,
}

impl CliError {
    /// Builds an error from its three parts.
    pub fn new(exit: ExitCode, code: &str, message: impl Into<String>) -> Self {
        Self { exit, code: code.to_string(), message: message.into() }
    }
}

/// Extension/hook refusal detail.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Refusal {
    pub package_id: Option<String>,
    pub reason: Option<String>,
}

/// What was invoked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OpInfo {
    pub verb: String,
    /// `mcp`, `rest` or `local`.
    pub via: String,
    pub tool: Option<String>,
    pub method: Option<String>,
    pub path: Option<String>,
}

/// Resolved context echoed in the envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct EnvelopeContext {
    pub instance: Option<String>,
    pub verse_id: Option<String>,
    pub fractal_id: Option<String>,
    pub petal_id: Option<String>,
}

/// `--json` output, schema version 1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub v: u32,
    pub ok: bool,
    pub exit: i32,
    pub code: String,
    pub message: String,
    pub data: Option<Value>,
    pub retry_after_ms: Option<u64>,
    pub refusal: Refusal,
    pub op: OpInfo,
    pub context: EnvelopeContext,
    /// CLI-generated ULID for journal correlation.
    pub cid: String,
    pub latency_ms: u64,
    pub wait: Option<Value>,
}

impl Envelope {
    /// Serializes to one compact JSON line.
    pub fn to_json_line(&self) -> String {
        serde_json::to_string(self).expect("envelope serializes")
    }
}

/// Milliseconds since the Unix epoch (0 if the clock is before it).
pub fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Current UTC time as RFC3339 with millisecond precision (`2026-11-20T10:00:00.123Z`).
pub fn now_rfc3339() -> String {
    format_rfc3339(unix_millis_now())
}

/// Formats Unix milliseconds as RFC3339 UTC.
pub fn format_rfc3339(unix_millis: u64) -> String {
    let seconds = (unix_millis / 1000) as i64;
    let millis = unix_millis % 1000;
    let (year, month, day) = civil_from_days(seconds.div_euclid(86_400));
    let in_day = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        in_day / 3600,
        (in_day % 3600) / 60,
        in_day % 60
    )
}

/// Parses an RFC3339 timestamp (`Z` or `+HH:MM` offset, optional fraction) to Unix seconds.
pub fn rfc3339_to_unix(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.is_ascii() {
        return None;
    }
    let separators_ok = bytes[4] == b'-'
        && bytes[7] == b'-'
        && matches!(bytes[10], b'T' | b't' | b' ')
        && bytes[13] == b':'
        && bytes[16] == b':';
    if !separators_ok {
        return None;
    }
    let number = |range: std::ops::Range<usize>| -> Option<i64> { text.get(range)?.parse::<i64>().ok() };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = text.get(19..)?;
    if let Some(fraction) = rest.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        rest = &fraction[digits..];
    }
    let offset_seconds = if rest.eq_ignore_ascii_case("z") {
        0
    } else {
        let sign = match *rest.as_bytes().first()? {
            b'+' => 1,
            b'-' => -1,
            _ => return None,
        };
        let offset_hours: i64 = rest.get(1..3)?.parse().ok()?;
        let offset_minutes: i64 = rest.get(4..6)?.parse().ok()?;
        sign * (offset_hours * 3600 + offset_minutes * 60)
    };
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset_seconds)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Hinnant's civil-days algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = (if year >= 0 { year } else { year - 399 }) / 400;
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Inverse of `days_from_civil`: (year, month, day) for days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = (if shifted >= 0 { shifted } else { shifted - 146_096 }) / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}
