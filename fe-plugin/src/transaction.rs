//! Atomic plugin transactions; see src/AGENTS.md §scene-host.

pub use fe_sdk::plugin_host::PendingOp;

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

    /// Commit all pending operations by sending them through the command channel.
    ///
    /// Returns the number of operations committed.
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
    #[error("Plugin command channel closed")]
    ChannelClosed,
    #[error("Plugin transaction failed: {0}")]
    Rejected(String),
}
