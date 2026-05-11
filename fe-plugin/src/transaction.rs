//! Plugin transaction — batches multiple operations into a single atomic commit.
//!
//! A [`PluginTransaction`] collects [`PendingOp`]s and sends them through the
//! crossbeam channel on [`commit()`](PluginTransaction::commit). The host
//! applies them to the DB thread with the originating plugin_id for attribution.

use crossbeam::channel::Sender;
use serde::{Deserialize, Serialize};

use crate::capability::CapabilityToken;
use crate::PluginCommand;

// ---------------------------------------------------------------------------
// Pending operation
// ---------------------------------------------------------------------------

/// A single deferred operation within a plugin transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PendingOp {
    /// Set a property on an existing node.
    SetProperty {
        node_id: String,
        key: String,
        value: serde_json::Value,
    },
    /// Create a new node in a petal.
    CreateNode {
        petal_id: String,
        name: String,
    },
    /// Delete a node by ID.
    DeleteNode {
        node_id: String,
    },
}

// ---------------------------------------------------------------------------
// Plugin transaction
// ---------------------------------------------------------------------------

/// Batches plugin operations for atomic commit.
///
/// Created via [`PluginContext::transaction()`](crate::context::PluginContext::transaction).
#[derive(Debug)]
pub struct PluginTransaction {
    plugin_id: String,
    #[allow(dead_code)]
    scope: CapabilityToken,
    pending_ops: Vec<PendingOp>,
    tx: Sender<PluginCommand>,
}

impl PluginTransaction {
    pub(crate) fn new(
        plugin_id: String,
        scope: CapabilityToken,
        tx: Sender<PluginCommand>,
    ) -> Self {
        Self {
            plugin_id,
            scope,
            pending_ops: Vec::new(),
            tx,
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
        self.tx
            .send(PluginCommand::CommitTransaction {
                plugin_id: self.plugin_id,
                ops: self.pending_ops,
            })
            .map_err(|_| CommitError::ChannelClosed)?;
        Ok(count)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CommitError {
    #[error("Plugin command channel closed")]
    ChannelClosed,
}
