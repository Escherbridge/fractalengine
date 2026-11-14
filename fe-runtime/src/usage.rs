//! Per-request usage measurement seam: `UsageEvent` + `UsageSink` (see `src/AGENTS.md` §usage).

use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Lines queued for the writer thread; a full queue drops the event (counted) instead of blocking a request.
pub const USAGE_QUEUE_CAPACITY: usize = 4096;

/// One measured API request; carries route templates and counters only, never ids from the path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Caller DID; `None` for unauthenticated (public) routes.
    pub caller_did: Option<String>,
    /// The token's scope claim (e.g. "VERSE#v1"); `None` if unauthenticated.
    pub scope: Option<String>,
    /// The matched route TEMPLATE (e.g. "/api/v1/petals/{petal_id}/nodes"), never an expanded path.
    pub route_class: String,
    pub method: String,
    pub status: u16,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub duration_ms: u64,
    pub at_unix_ms: u64,
}

/// Receiver of usage events; implementations must not block the request path for long.
#[async_trait::async_trait]
pub trait UsageSink: Send + Sync {
    async fn record(&self, event: UsageEvent);
}

/// Sink that discards every event.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopUsageSink;

#[async_trait::async_trait]
impl UsageSink for NoopUsageSink {
    async fn record(&self, _event: UsageEvent) {}
}

/// Sink appending one JSON object per line to a file (append mode, never truncates) from one writer thread.
#[derive(Debug, Clone)]
pub struct JsonLinesUsageSink {
    writer: Arc<JsonLinesWriter>,
}

/// Bounded queue into the single writer thread; dropping the last handle drains the queue and joins it.
#[derive(Debug)]
struct JsonLinesWriter {
    queue: Option<crossbeam::channel::Sender<Vec<u8>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    dropped: AtomicU64,
}

impl JsonLinesUsageSink {
    /// Opens (creating if missing) `path` for appending and starts its writer thread.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let (queue, lines) = crossbeam::channel::bounded::<Vec<u8>>(USAGE_QUEUE_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("fe-usage-log".to_string())
            .spawn(move || {
                let mut failing = false;
                // One write_all per line from one thread keeps lines whole and in queue order.
                for line in lines {
                    match file.write_all(&line) {
                        Ok(()) if failing => {
                            failing = false;
                            tracing::info!("usage event writes recovered");
                        }
                        Ok(()) => {}
                        Err(error) if !failing => {
                            failing = true;
                            tracing::warn!(%error, "usage event write failed");
                        }
                        Err(_) => {}
                    }
                }
            })?;
        Ok(Self {
            writer: Arc::new(JsonLinesWriter {
                queue: Some(queue),
                thread: Some(thread),
                dropped: AtomicU64::new(0),
            }),
        })
    }

    /// Events dropped because the writer queue was full or the writer had stopped.
    pub fn dropped_events(&self) -> u64 {
        self.writer.dropped.load(Ordering::Relaxed)
    }
}

impl Drop for JsonLinesWriter {
    fn drop(&mut self) {
        // Closing the queue ends the writer loop after it drains what is queued.
        self.queue.take();
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::warn!("usage log writer thread panicked");
            }
        }
    }
}

#[async_trait::async_trait]
impl UsageSink for JsonLinesUsageSink {
    async fn record(&self, event: UsageEvent) {
        let mut line = match serde_json::to_vec(&event) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, "usage event serialization failed");
                return;
            }
        };
        line.push(b'\n');
        let Some(queue) = self.writer.queue.as_ref() else {
            return;
        };
        if queue.try_send(line).is_err() {
            let dropped = self.writer.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            // Warn at powers of two so a flood never becomes a log flood.
            if dropped.is_power_of_two() {
                tracing::warn!(dropped_total = dropped, "usage log queue full or closed; events dropped");
            }
        }
    }
}

/// Sink forwarding each event to every inner sink (one measurement point, many consumers).
#[derive(Clone)]
pub struct FanoutUsageSink(pub Vec<Arc<dyn UsageSink>>);

#[async_trait::async_trait]
impl UsageSink for FanoutUsageSink {
    async fn record(&self, event: UsageEvent) {
        for sink in &self.0 {
            sink.record(event.clone()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event(sequence: u64) -> UsageEvent {
        UsageEvent {
            caller_did: Some("did:key:zExample".to_string()),
            scope: Some("VERSE#v1".to_string()),
            route_class: "/api/v1/petals/{petal_id}/nodes".to_string(),
            method: "GET".to_string(),
            status: 200,
            bytes_in: sequence,
            bytes_out: sequence * 2,
            duration_ms: 3,
            at_unix_ms: 1_700_000_000_000 + sequence,
        }
    }

    fn read_events(path: &Path) -> Vec<UsageEvent> {
        std::fs::read_to_string(path)
            .expect("read usage log")
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line parses"))
            .collect()
    }

    fn temp_log_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fe_usage_{}_{}.jsonl", name, std::process::id()))
    }

    #[tokio::test]
    async fn json_lines_sink_appends_one_parseable_line_per_event_and_survives_reopen() {
        let path = temp_log_path("reopen");
        let _ = std::fs::remove_file(&path);

        let first = JsonLinesUsageSink::open(&path).expect("open");
        first.record(sample_event(1)).await;
        first.record(sample_event(2)).await;
        drop(first);

        let reopened = JsonLinesUsageSink::open(&path).expect("reopen");
        reopened.record(sample_event(3)).await;
        drop(reopened);

        let events = read_events(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(events, vec![sample_event(1), sample_event(2), sample_event(3)]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn json_lines_sink_is_safe_under_concurrent_record_calls() {
        const PER_TASK: u64 = 50;
        let path = temp_log_path("concurrent");
        let _ = std::fs::remove_file(&path);

        let sink = Arc::new(JsonLinesUsageSink::open(&path).expect("open"));
        let mut handles = Vec::new();
        for task in 0..2u64 {
            let sink = Arc::clone(&sink);
            handles.push(tokio::spawn(async move {
                for index in 0..PER_TASK {
                    sink.record(sample_event(task * 1000 + index)).await;
                }
            }));
        }
        for handle in handles {
            handle.await.expect("task");
        }
        assert_eq!(sink.dropped_events(), 0, "the queue holds every event of this burst");
        // Dropping the last handle joins the writer, so the file is complete when read.
        drop(sink);

        let events = read_events(&path);
        let _ = std::fs::remove_file(&path);
        assert_eq!(events.len(), (2 * PER_TASK) as usize);
    }
}
