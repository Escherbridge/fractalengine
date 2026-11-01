//! Shared QA read bridge and bounded redacted diagnostics; see src/AGENTS.md §qa-debug.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::bridge::{bounded_bridge, BridgeReceiver, BridgeSender};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TreeFilter {
    pub name: Option<String>,
    pub role: Option<String>,
    #[serde(default)]
    pub interactive: bool,
}

impl TreeFilter {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.as_ref().is_some_and(|s| s.len() > 256)
            || self.role.as_ref().is_some_and(|s| s.len() > 64)
        {
            return Err("name/role filter exceeds its byte limit");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LogFilter {
    #[serde(default)]
    pub since: u64,
    pub level: Option<String>,
}

impl LogFilter {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.level.as_deref().is_some_and(|s| {
            !matches!(
                s.to_ascii_lowercase().as_str(),
                "trace" | "debug" | "info" | "warn" | "error"
            )
        }) {
            return Err("level must be trace, debug, info, warn or error");
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum QaOp {
    UiTree(TreeFilter),
    UiState,
    Logs(LogFilter),
}

#[derive(Clone, Debug, Serialize)]
pub struct QaFailure {
    pub code: &'static str,
    pub message: &'static str,
}

impl QaFailure {
    pub fn denied() -> Self {
        Self {
            code: "not_found_or_denied",
            message: "not found or not permitted",
        }
    }
}

pub type QaResult = Result<serde_json::Value, QaFailure>;
pub type QaSender = BridgeSender<QaOp, QaResult>;
pub type QaReceiver = BridgeReceiver<QaOp, QaResult>;

pub fn qa_bridge() -> (QaSender, QaReceiver) {
    bounded_bridge(32)
}

/// Redacts credentials before bounding output, including JWTs embedded in copyable labels.
pub fn redact(text: &str) -> String {
    let mut out = String::new();
    let mut bearer_next = false;
    let mut start = 0;
    while start < text.len() {
        let token_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
        let first = text[start..].chars().next().expect("nonempty suffix");
        if bearer_next && !first.is_whitespace() {
            let end = text[start..]
                .find(char::is_whitespace)
                .map_or(text.len(), |offset| start + offset);
            out.push_str("[REDACTED]");
            bearer_next = false;
            start = end;
            continue;
        }
        if !token_char(first) {
            out.push(first);
            start += first.len_utf8();
            continue;
        }
        let end = text[start..]
            .find(|c: char| !token_char(c))
            .map_or(text.len(), |offset| start + offset);
        let word = &text[start..end];
        let jwt = word.trim_matches('.').split('.').collect::<Vec<_>>();
        let credential = jwt.len() == 3
            && jwt.iter().all(|s| {
                s.len() >= 8
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            });
        if bearer_next || credential {
            out.push_str("[REDACTED]");
        } else {
            out.push_str(word);
        }
        bearer_next = word.eq_ignore_ascii_case("bearer");
        start = end;
    }
    if out.len() > 4096 {
        let mut end = 4096;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

#[derive(Clone, Debug, Serialize)]
pub struct LogEntry {
    pub seq: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Default, Debug)]
struct Ring {
    verse: Option<String>,
    generation: u64,
    next: u64,
    entries: VecDeque<LogEntry>,
}

#[derive(Clone, Default, Debug)]
pub struct QaLogRing(Arc<Mutex<Ring>>);

impl QaLogRing {
    pub fn set_verse(&self, verse: Option<&str>) {
        let mut ring = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if ring.verse.as_deref() != verse {
            ring.verse = verse.map(str::to_owned);
            ring.generation += 1;
            ring.entries.clear();
        }
    }

    pub fn push(&self, verse_id: Option<&str>, level: &str, target: &str, message: &str) {
        let message = redact(message);
        let mut ring = self.0.lock().unwrap_or_else(|p| p.into_inner());
        ring.next += 1;
        if ring.verse.is_none() {
            return;
        }
        let message = if verse_id.is_some() && verse_id == ring.verse.as_deref() {
            message
        } else {
            "[message withheld: event scope unavailable or outside active verse]".to_string()
        };
        let seq = ring.next;
        ring.entries.push_back(LogEntry {
            seq,
            level: level.to_ascii_lowercase(),
            target: redact(target),
            message,
        });
        while ring.entries.len() > 2000 {
            ring.entries.pop_front();
        }
    }

    pub fn read(&self, filter: &LogFilter) -> serde_json::Value {
        let ring = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let rank = |s: &str| match s {
            "error" => 4,
            "warn" => 3,
            "info" => 2,
            "debug" => 1,
            _ => 0,
        };
        let minimum = filter
            .level
            .as_ref()
            .map_or(0, |s| rank(&s.to_ascii_lowercase()));
        let entries = ring
            .entries
            .iter()
            .filter(|e| e.seq > filter.since && rank(&e.level) >= minimum)
            .collect::<Vec<_>>();
        serde_json::json!({ "entries": entries, "latest_seq": ring.next, "generation": ring.generation, "active_verse_id": ring.verse })
    }
}
