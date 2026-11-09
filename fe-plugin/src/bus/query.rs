//! `host/query` subscription state: change-triggered re-runs behind a floor (bus.md §2.3, OD-3).

use std::collections::HashSet;

use crossbeam::channel::{Receiver, TryRecvError};
use serde_json::{Map, Value};

use super::canonical_json;
use super::sources::{PreparedQuery, QueryRequest, QueryRows};
use super::HostError;

/// One query subscription's pacing: dirty flag, one run in flight, floor since the last start, last hash.
pub(super) struct QueryState {
    pub(super) request: QueryRequest,
    pub(super) prepared: PreparedQuery,
    tables: HashSet<String>,
    min_interval_ms: u64,
    /// Guests get at most 1 MiB per snapshot; external clients only REST's caps.
    event_cap: Option<usize>,
    dirty: bool,
    launching: bool,
    running: Option<Receiver<Result<QueryRows, HostError>>>,
    last_started_ms: Option<u64>,
    last_hash: Option<blake3::Hash>,
}

impl QueryState {
    /// Starts dirty, so the first poll runs it ("run now -> snapshot").
    pub(super) fn new(request: QueryRequest, prepared: PreparedQuery, min_interval_ms: u64, event_cap: Option<usize>) -> Self {
        let tables = prepared.tables.iter().map(|table| table.to_ascii_lowercase()).collect();
        Self {
            request,
            prepared,
            tables,
            min_interval_ms,
            event_cap,
            dirty: true,
            launching: false,
            running: None,
            last_started_ms: None,
            last_hash: None,
        }
    }

    /// Whether a write to `table` (lowercase) touches this query.
    pub(super) fn reads(&self, table: &str) -> bool {
        self.tables.contains(table)
    }

    /// Every FROM table, lowercase.
    pub(super) fn tables(&self) -> impl Iterator<Item = &str> {
        self.tables.iter().map(String::as_str)
    }

    /// Many writes set one flag.
    pub(super) fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Dirty, idle, not paused, and the floor since the last start has passed.
    pub(super) fn due(&self, now_ms: u64, paused: bool) -> bool {
        self.dirty
            && !paused
            && !self.launching
            && self.running.is_none()
            && self.last_started_ms.is_none_or(|started| now_ms >= started.saturating_add(self.min_interval_ms))
    }

    /// Claim the run before the rate budget is asked (outside the lock); returns the previous start to restore.
    pub(super) fn begin_launch(&mut self, now_ms: u64) -> Option<u64> {
        self.launching = true;
        self.dirty = false;
        std::mem::replace(&mut self.last_started_ms, Some(now_ms))
    }

    /// The run is in flight.
    pub(super) fn launched(&mut self, reply: Receiver<Result<QueryRows, HostError>>) {
        self.launching = false;
        self.running = Some(reply);
    }

    /// The identity's rate budget was spent: stay dirty and keep the old floor.
    pub(super) fn launch_deferred(&mut self, previous_start: Option<u64>) {
        self.launching = false;
        self.dirty = true;
        self.last_started_ms = previous_start;
    }

    /// A finished run, if one answered (a vanished runner counts as a backend failure).
    pub(super) fn collect(&mut self) -> Option<Result<QueryRows, HostError>> {
        let answer = match self.running.as_ref()?.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(HostError::Backend("query runner vanished".into())),
        };
        self.running = None;
        Some(answer)
    }

    /// The snapshot data to deliver, or `None` when the result hash is unchanged.
    pub(super) fn snapshot_if_changed(&mut self, rows: &QueryRows) -> Option<String> {
        let mut hasher = blake3::Hasher::new();
        for row in &rows.rows {
            hasher.update(canonical_json(row).as_bytes());
            hasher.update(b"\n");
        }
        hasher.update(&[u8::from(rows.truncated)]);
        let hash = hasher.finalize();
        if self.last_hash == Some(hash) {
            return None;
        }
        self.last_hash = Some(hash);
        Some(snapshot_data(&rows.rows, rows.truncated, self.event_cap))
    }
}

/// `{"data": rows, "row_count": n, "truncated": bool}`, rows cut at the boundary that keeps it under `cap`.
fn snapshot_data(rows: &[Value], truncated: bool, cap: Option<usize>) -> String {
    let render = |kept: &[Value], cut: bool| {
        let mut data = Map::new();
        data.insert("data".into(), Value::Array(kept.to_vec()));
        data.insert("row_count".into(), Value::from(kept.len()));
        data.insert("truncated".into(), Value::Bool(cut));
        Value::Object(data).to_string()
    };
    let full = render(rows, truncated);
    let Some(cap) = cap else { return full };
    if full.len() <= cap {
        return full;
    }
    // Envelope overhead is small and fixed; keep whole rows until the next one would cross the cap.
    let budget = cap.saturating_sub(64);
    let mut used = 0usize;
    let mut kept = 0usize;
    for row in rows {
        let size = row.to_string().len() + 1;
        if used + size > budget {
            break;
        }
        used += size;
        kept += 1;
    }
    render(&rows[..kept], true)
}
