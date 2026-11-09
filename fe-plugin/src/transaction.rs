//! Atomic plugin transactions; see src/AGENTS.md §scene-host.

pub use fe_sdk::plugin_host::PendingOp;
use fe_sdk::plugin_host::{StagedReading, MAX_HOST_BYTES, MAX_STAGED_READINGS, MAX_TRANSACTION_OPS};

/// Bytes one window may hold before a flush; matches `fe_sdk::plugin_host::validate_commit`.
pub const MAX_WINDOW_BYTES: usize = MAX_HOST_BYTES * 8;

/// Counts and limits of a staged window (the `txn.window` import's answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowInfo {
    pub ops: usize,
    pub max_ops: usize,
    pub readings: usize,
    pub max_readings: usize,
    pub bytes: usize,
    pub max_bytes: usize,
}

/// A refused stage call; the window is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowError {
    #[error("staged window is full: {0}")]
    Full(&'static str),
    #[error("staged value refused: {0}")]
    Invalid(String),
}

/// The per-instance write window: staged until a tick flushes it as ONE commit.
#[derive(Debug, Clone)]
pub struct StagedWindow {
    ops: Vec<PendingOp>,
    readings: Vec<StagedReading>,
    bytes: usize,
    max_ops: usize,
    max_readings: usize,
}

impl StagedWindow {
    /// An empty window; limits are clamped to the host ceilings.
    pub fn new(max_ops: usize, max_readings: usize) -> Self {
        Self {
            ops: Vec::new(),
            readings: Vec::new(),
            bytes: 0,
            max_ops: max_ops.min(MAX_TRANSACTION_OPS),
            max_readings: max_readings.min(MAX_STAGED_READINGS),
        }
    }

    /// Stage one op after SDK validation; a full or invalid stage changes nothing.
    pub fn stage_op(&mut self, op: PendingOp) -> Result<(), WindowError> {
        if self.ops.len() >= self.max_ops {
            return Err(WindowError::Full("max_ops"));
        }
        fe_sdk::plugin_host::validate_transaction(std::slice::from_ref(&op)).map_err(WindowError::Invalid)?;
        let size = serde_json::to_vec(&op).map_err(|e| WindowError::Invalid(e.to_string()))?.len();
        if self.bytes + size > MAX_WINDOW_BYTES {
            return Err(WindowError::Full("bytes"));
        }
        self.bytes += size;
        self.ops.push(op);
        Ok(())
    }

    /// Stage one reading after SDK validation; a full or invalid stage changes nothing.
    pub fn stage_reading(&mut self, reading: StagedReading) -> Result<(), WindowError> {
        if self.readings.len() >= self.max_readings {
            return Err(WindowError::Full("max_readings"));
        }
        fe_sdk::plugin_host::validate_staged_readings(std::slice::from_ref(&reading)).map_err(WindowError::Invalid)?;
        let size = serde_json::to_vec(&reading).map_err(|e| WindowError::Invalid(e.to_string()))?.len();
        if self.bytes + size > MAX_WINDOW_BYTES {
            return Err(WindowError::Full("bytes"));
        }
        self.bytes += size;
        self.readings.push(reading);
        Ok(())
    }

    /// Current counts and limits.
    pub fn info(&self) -> WindowInfo {
        WindowInfo {
            ops: self.ops.len(),
            max_ops: self.max_ops,
            readings: self.readings.len(),
            max_readings: self.max_readings,
            bytes: self.bytes,
            max_bytes: MAX_WINDOW_BYTES,
        }
    }

    /// Whether nothing is staged (an empty window never flushes).
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.readings.is_empty()
    }

    /// Drop everything staged (the `txn.discard` import, and a fuel-exhausted tick).
    pub fn discard(&mut self) {
        self.ops.clear();
        self.readings.clear();
        self.bytes = 0;
    }

    /// Take the staged contents for a flush, leaving the window empty.
    pub fn take(&mut self) -> StagedFlush {
        self.bytes = 0;
        StagedFlush {
            ops: std::mem::take(&mut self.ops),
            readings: std::mem::take(&mut self.readings),
        }
    }
}

/// What one tick's flush carries: the whole window, committed atomically or not at all.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedFlush {
    pub ops: Vec<PendingOp>,
    pub readings: Vec<StagedReading>,
}

/// A host-supplied writer for a tick flush: ops and readings land in one atomic commit.
pub trait StagedCommitSink: Send {
    /// Return only after atomic persistence; a refusal changes nothing, a transport failure is unknown.
    fn commit_staged(
        &mut self,
        petal_id: &str,
        flush: &StagedFlush,
    ) -> Result<fe_sdk::plugin_host::TransactionReceipt, String>;
}

// ---------------------------------------------------------------------------
// Plugin transaction
// ---------------------------------------------------------------------------

/// Batches plugin operations for atomic commit.
///
/// Created via [`PluginContext::transaction()`](crate::context::PluginContext::transaction).
#[derive(Debug)]
pub struct PluginTransaction {
    context: crate::context::PluginContext,
    pending_ops: Vec<PendingOp>,
}

impl PluginTransaction {
    pub(crate) fn new(context: crate::context::PluginContext) -> Self {
        Self {
            context,
            pending_ops: Vec::new(),
        }
    }

    /// Queue a property-set operation.
    pub fn set_property(&mut self, node_id: &str, key: &str, value: serde_json::Value) {
        self.pending_ops.push(PendingOp::SetProperty {
            node_id: node_id.to_string(),
            key: key.to_string(),
            value,
        });
    }

    /// Queue a node-creation operation.
    pub fn create_node(&mut self, petal_id: &str, name: &str) {
        self.pending_ops.push(PendingOp::CreateNode {
            petal_id: petal_id.to_string(),
            name: name.to_string(),
        });
    }

    /// Queue a node-deletion operation.
    pub fn delete_node(&mut self, node_id: &str) {
        self.pending_ops.push(PendingOp::DeleteNode {
            node_id: node_id.to_string(),
        });
    }

    /// Number of pending operations.
    pub fn len(&self) -> usize {
        self.pending_ops.len()
    }

    /// Whether the transaction has any pending operations.
    pub fn is_empty(&self) -> bool {
        self.pending_ops.is_empty()
    }

    /// Wait for the injected sink's persisted receipt and return its committed count.
    pub fn commit(self) -> Result<usize, CommitError> {
        let count = self.pending_ops.len();
        if count == 0 {
            return Ok(0);
        }
        self.context
            .commit_ops(&self.pending_ops)
            .map(|receipt| receipt.op_count)
            .map_err(CommitError::Rejected)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CommitError {
    #[error("Plugin transaction failed: {0}")]
    Rejected(String),
}

