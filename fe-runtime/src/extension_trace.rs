//! Bounded in-memory trace buffer for extension lifecycle, hook, command,
//! ingress, egress, and decode events exposed on `GET /api/v1/debug/extensions-trace`.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::prelude::Resource;
use serde::Serialize;

/// Default ring capacity kept in memory per host.
pub const DEFAULT_TRACE_CAPACITY: usize = 512;
const MAX_DETAIL_CHARS: usize = 256;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn sanitize_text(input: &str, max_chars: usize) -> String {
    input
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max_chars)
        .collect()
}

/// Input recorded into [`ExtensionTraceBuffer`].
#[derive(Debug, Clone)]
pub struct ExtensionTraceInput {
    pub kind: String,
    pub package_id: String,
    pub petal_id: Option<String>,
    pub hook: Option<String>,
    pub key: Option<String>,
    pub outcome: String,
    pub detail: Option<String>,
    pub duration_ms: Option<u64>,
}

/// Sequenced trace event emitted by `GET /api/v1/debug/extensions-trace`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtensionTraceEvent {
    pub seq: u64,
    pub timestamp_ms: u64,
    pub kind: String,
    pub package_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub petal_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Bounded trace snapshot returned to diagnostics endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtensionTraceSnapshot {
    pub total_recorded: u64,
    pub buffered: usize,
    pub events: Vec<ExtensionTraceEvent>,
}

#[derive(Debug)]
struct Inner {
    next_seq: u64,
    capacity: usize,
    events: VecDeque<ExtensionTraceEvent>,
}

/// Cloneable, thread-safe extension trace buffer shared across plugin host and API.
#[derive(Resource, Clone, Debug)]
pub struct ExtensionTraceBuffer(Arc<Mutex<Inner>>);

impl Default for ExtensionTraceBuffer {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_TRACE_CAPACITY)
    }
}

impl ExtensionTraceBuffer {
    /// Create a trace buffer retaining up to `capacity` recent events (minimum 1).
    pub fn with_capacity(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self(Arc::new(Mutex::new(Inner {
            next_seq: 0,
            capacity: cap,
            events: VecDeque::with_capacity(cap.min(64)),
        })))
    }

    /// Record one sanitized event into the bounded ring.
    pub fn record(&self, input: ExtensionTraceInput) {
        let mut guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        guard.next_seq = guard.next_seq.saturating_add(1);
        let seq = guard.next_seq;
        if guard.events.len() >= guard.capacity {
            guard.events.pop_front();
        }
        guard.events.push_back(ExtensionTraceEvent {
            seq,
            timestamp_ms: now_ms(),
            kind: sanitize_text(&input.kind, 64),
            package_id: sanitize_text(&input.package_id, 128),
            petal_id: input.petal_id.map(|v| sanitize_text(&v, 64)),
            hook: input.hook.map(|v| sanitize_text(&v, 64)),
            key: input.key.map(|v| sanitize_text(&v, 128)),
            outcome: sanitize_text(&input.outcome, 64),
            detail: input.detail.map(|v| sanitize_text(&v, MAX_DETAIL_CHARS)),
            duration_ms: input.duration_ms,
        });
    }

    /// Snapshot matching events (chronological order, truncated to the most recent `limit`).
    pub fn snapshot(
        &self,
        package_id: Option<&str>,
        petal_id: Option<&str>,
        limit: usize,
    ) -> ExtensionTraceSnapshot {
        let guard = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let cap = limit.clamp(1, guard.capacity);
        let mut matched: Vec<ExtensionTraceEvent> = guard
            .events
            .iter()
            .rev()
            .filter(|ev| package_id.is_none_or(|p| ev.package_id == p))
            .filter(|ev| petal_id.is_none_or(|p| ev.petal_id.as_deref() == Some(p)))
            .take(cap)
            .cloned()
            .collect();
        matched.reverse();
        ExtensionTraceSnapshot {
            total_recorded: guard.next_seq,
            buffered: guard.events.len(),
            events: matched,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_evicts_oldest_and_filters_by_package_and_petal() {
        let buf = ExtensionTraceBuffer::with_capacity(3);
        for (i, pkg) in ["a", "b", "a", "c"].into_iter().enumerate() {
            buf.record(ExtensionTraceInput {
                kind: "hook".into(),
                package_id: pkg.into(),
                petal_id: Some(format!("p{i}")),
                hook: Some("scene.create".into()),
                key: None,
                outcome: "allow".into(),
                detail: Some("ok\nline".into()),
                duration_ms: Some(1),
            });
        }
        let all = buf.snapshot(None, None, 10);
        assert_eq!(all.total_recorded, 4);
        assert_eq!(all.buffered, 3);
        assert_eq!(
            all.events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
        assert_eq!(all.events[0].detail.as_deref(), Some("ok line"));

        let filtered = buf.snapshot(Some("a"), Some("p2"), 10);
        assert_eq!(filtered.events.len(), 1);
        assert_eq!(filtered.events[0].seq, 3);
    }
}
