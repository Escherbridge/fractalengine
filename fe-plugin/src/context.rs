//! Plugin execution context — the "world view" available to plugin code.
//!
//! Every callback on [`FractalExtension`](crate::FractalExtension) receives a
//! `&mut PluginContext` that provides scoped access to the engine through the
//! capability token and the command channel.

use crate::capability::{validate_access, CapabilityOperation};
use crossbeam::channel::Sender;
use fe_sdk::plugin_host::{NodeQuery, NodeReader, PendingOp, TransactionReceipt, TransactionSink};
use std::sync::{Arc, Mutex};

use crate::capability::CapabilityToken;
use crate::host_env::HostEnv;
use crate::transaction::{PluginTransaction, StagedCommitSink, StagedFlush};
use fe_sdk::manifest_v2::SceneWrite;
use fe_sdk::plugin_host::StagedReading;
use crate::PluginCommand;

/// Runtime context provided to plugin callbacks.
///
/// Wraps the crossbeam sender so plugins can issue commands back to the host
/// without holding a reference to the full engine. The optional [`HostEnv`]
/// carries the binary-injected storage/query trait objects (empty by default).
#[derive(Clone)]
pub struct PluginContext {
    pub(crate) session: crate::scene_host::PluginSession,
    /// The plugin's unique identifier.
    pub(crate) plugin_id: String,
    /// The petal scope this plugin instance is bound to.
    pub(crate) petal_id: String,
    /// The minted capability token controlling access.
    pub(crate) capabilities: CapabilityToken,
    /// Sender end of the plugin command channel.
    pub(crate) tx: Sender<PluginCommand>,
    /// Host-injected storage/query services (empty until the binary injects).
    pub(crate) host: HostEnv,
    reader: Option<Arc<Mutex<dyn NodeReader>>>,
    sink: Option<Arc<Mutex<dyn TransactionSink>>>,
    staged_sink: Option<Arc<Mutex<dyn StagedCommitSink>>>,
}

impl PluginContext {
    /// Create a new plugin context with no injected host services.
    pub fn new(
        plugin_id: String,
        petal_id: String,
        capabilities: CapabilityToken,
        tx: Sender<PluginCommand>,
    ) -> Self {
        Self {
            session: crate::scene_host::PluginSession::mint(),
            plugin_id,
            petal_id,
            capabilities,
            tx,
            host: HostEnv::new(),
            reader: None,
            sink: None,
            staged_sink: None,
        }
    }

    /// Attach a [`HostEnv`] carrying binary-injected storage/query services.
    pub fn with_host_env(mut self, host: HostEnv) -> Self {
        self.host = host;
        self
    }

    /// Carry the same host-only revocation lease into queued backend requests.
    pub fn revocation_lease(&self) -> fe_sdk::plugin_host::PluginLease {
        self.session.lease()
    }

    /// The host-injected storage/query services for this plugin.
    pub fn host(&self) -> &HostEnv {
        &self.host
    }

    /// Attach a trusted reader; plugins cannot supply this through script arguments.
    pub fn with_node_reader(mut self, reader: Arc<Mutex<dyn NodeReader>>) -> Self {
        self.reader = Some(reader);
        self
    }

    /// Attach a trusted atomic sink bound to this plugin's admitted capability limits.
    pub fn with_transaction_sink(mut self, sink: Arc<Mutex<dyn TransactionSink>>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// Attach the trusted sink a tick flush commits through (ops and readings in one command).
    pub fn with_staged_sink(mut self, sink: Arc<Mutex<dyn StagedCommitSink>>) -> Self {
        self.staged_sink = Some(sink);
        self
    }

    fn require_capability(&self, capability: &str) -> Result<(), String> {
        if !self.session.is_active() {
            return Err("plugin instance is no longer admitted".into());
        }
        if self.capabilities.plugin_id != self.plugin_id
            || !self.capabilities.has_capability(capability)
        {
            return Err(format!("plugin capability denied: {capability}"));
        }
        Ok(())
    }

    fn authorize_petal(&self, petal_id: &str) -> Result<String, String> {
        if petal_id != self.petal_id {
            return Err("plugin is bound to another petal".into());
        }
        let reader = self
            .reader
            .as_ref()
            .ok_or("plugin node reader is unavailable")?;
        let scope = reader
            .lock()
            .map_err(|_| "plugin reader lock poisoned")?
            .petal_scope(petal_id)?;
        let parts = fe_policy::parse_scope(&scope).map_err(|error| error.to_string())?;
        if parts.petal_id.as_deref() != Some(petal_id) {
            return Err("reader returned an unrelated petal scope".into());
        }
        validate_access(
            &self.capabilities,
            &CapabilityOperation::PetalAccess {
                scope: scope.clone(),
            },
        )
        .map_err(|error| error.to_string())?;
        validate_access(
            &self.capabilities,
            &CapabilityOperation::VerseAccess {
                scope: format!("VERSE#{}", parts.verse_id),
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(scope)
    }

    fn project_node(&self, mut node: crate::NodeSnapshot) -> Result<crate::NodeSnapshot, String> {
        if node.petal_id != self.petal_id {
            return Err("reader returned an unrelated node".into());
        }
        node.properties.retain(|key, _| {
            validate_access(
                &self.capabilities,
                &CapabilityOperation::PropertyAccess { key: key.clone() },
            )
            .is_ok()
        });
        if serde_json::to_vec(&node)
            .map_err(|error| error.to_string())?
            .len()
            > fe_sdk::plugin_host::MAX_HOST_BYTES
        {
            return Err("node snapshot exceeds 65536 bytes".into());
        }
        Ok(node)
    }

    /// Read a live, scoped, capability-filtered authoritative node snapshot.
    pub fn get_node(&self, node_id: &str) -> Result<Option<crate::NodeSnapshot>, String> {
        self.require_capability(fe_sdk::CAP_STORAGE_READ)?;
        fe_sdk::storage::validate_node_id(node_id).map_err(|error| error.to_string())?;
        let before = self.authorize_petal(&self.petal_id)?;
        let node = self
            .reader
            .as_ref()
            .unwrap()
            .lock()
            .map_err(|_| "plugin reader lock poisoned")?
            .get_node(&self.petal_id, node_id)?;
        if before != self.authorize_petal(&self.petal_id)? {
            return Err("plugin petal ancestry changed during read".into());
        }
        node.map(|node| {
            if node.node_id != node_id {
                return Err("reader returned an unrelated node".into());
            }
            self.project_node(node)
        })
        .transpose()
    }

    /// Query current nodes through a bounded structured predicate.
    pub fn query_nodes(
        &self,
        petal_id: &str,
        query: &NodeQuery,
    ) -> Result<Vec<crate::NodeSnapshot>, String> {
        self.require_capability(fe_sdk::CAP_QUERY_SELECT)?;
        self.require_capability(fe_sdk::CAP_STORAGE_READ)?;
        query.validate()?;
        if let Some(predicate) = &query.property {
            validate_access(
                &self.capabilities,
                &CapabilityOperation::PropertyAccess {
                    key: predicate.key.clone(),
                },
            )
            .map_err(|error| error.to_string())?;
        }
        let before = self.authorize_petal(petal_id)?;
        let nodes = self
            .reader
            .as_ref()
            .unwrap()
            .lock()
            .map_err(|_| "plugin reader lock poisoned")?
            .query_nodes(petal_id, query)?;
        if nodes.len() > query.limit || nodes.iter().any(|node| !query.matches(node)) {
            return Err("reader violated the bounded query contract".into());
        }
        if before != self.authorize_petal(petal_id)? {
            return Err("plugin petal ancestry changed during read".into());
        }
        let nodes: Vec<_> = nodes
            .into_iter()
            .map(|node| self.project_node(node))
            .collect::<Result<_, _>>()?;
        if serde_json::to_vec(&nodes)
            .map_err(|error| error.to_string())?
            .len()
            > fe_sdk::plugin_host::MAX_HOST_BYTES
        {
            return Err("node page exceeds 65536 bytes".into());
        }
        Ok(nodes)
    }

    /// Check one op against the token: write kind, property scope and bound petal.
    pub fn authorize_op(&self, op: &PendingOp) -> Result<(), String> {
        let (kind, property) = match op {
            PendingOp::SetProperty { key, .. } => (SceneWrite::SetProperty, Some(key)),
            PendingOp::CreateNode { petal_id, .. } => {
                if petal_id != &self.petal_id {
                    return Err("plugin creation targets another petal".into());
                }
                (SceneWrite::CreateNode, None)
            }
            PendingOp::DeleteNode { .. } => (SceneWrite::DeleteNode, None),
            PendingOp::SetTransform { .. } => (SceneWrite::SetTransform, None),
        };
        if !self.capabilities.allows_scene_write(kind) {
            return Err(format!("plugin capability denied: scene write {kind:?}"));
        }
        if let Some(key) = property {
            validate_access(&self.capabilities, &CapabilityOperation::PropertyAccess { key: key.clone() })
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// Check one staged reading: metric inside the grant and a host-fixed source class.
    pub fn authorize_reading(&self, reading: &StagedReading) -> Result<(), String> {
        if !self.session.is_active() {
            return Err("plugin instance is no longer admitted".into());
        }
        if !self.capabilities.allows_reading(&reading.metric) {
            return Err(format!("plugin capability denied: reading metric {}", reading.metric));
        }
        Ok(())
    }

    /// Commit a whole tick window as one atomic command and return its persisted receipt.
    pub fn commit_staged(&self, flush: &StagedFlush) -> Result<TransactionReceipt, String> {
        if !self.session.is_active() {
            return Err("plugin instance is no longer admitted".into());
        }
        if !flush.ops.is_empty() {
            self.require_capability(fe_sdk::CAP_STORAGE_WRITE)?;
        }
        fe_sdk::plugin_host::validate_commit(&flush.ops, &flush.readings)?;
        self.authorize_petal(&self.petal_id)?;
        for op in &flush.ops {
            self.authorize_op(op)?;
        }
        for reading in &flush.readings {
            self.authorize_reading(reading)?;
        }
        let receipt = match &self.staged_sink {
            Some(sink) => sink
                .lock()
                .map_err(|_| "plugin sink lock poisoned")?
                .commit_staged(&self.petal_id, flush)?,
            None if flush.readings.is_empty() => return self.commit_ops(&flush.ops),
            None => return Err("plugin staged sink is unavailable".into()),
        };
        let created = flush.ops.iter().filter(|op| matches!(op, PendingOp::CreateNode { .. })).count();
        if receipt.op_count != flush.ops.len() || receipt.created_node_ids.len() != created {
            return Err("plugin sink returned an invalid commit receipt; outcome requires reconciliation".into());
        }
        Ok(receipt)
    }

    /// Commit a complete admitted batch and return its persisted receipt.
    pub fn commit_ops(&self, ops: &[PendingOp]) -> Result<TransactionReceipt, String> {
        self.require_capability(fe_sdk::CAP_STORAGE_WRITE)?;
        fe_sdk::plugin_host::validate_transaction(ops)?;
        self.authorize_petal(&self.petal_id)?;
        for op in ops {
            self.authorize_op(op)?;
        }
        let receipt = self
            .sink
            .as_ref()
            .ok_or("plugin transaction sink is unavailable")?
            .lock()
            .map_err(|_| "plugin sink lock poisoned")?
            .commit(&self.petal_id, ops)?;
        if receipt.op_count != ops.len()
            || receipt.created_node_ids.len()
                != ops
                    .iter()
                    .filter(|op| matches!(op, PendingOp::CreateNode { .. }))
                    .count()
        {
            return Err(
                "plugin sink returned an invalid commit receipt; outcome requires reconciliation"
                    .into(),
            );
        }
        Ok(receipt)
    }

    /// Begin a new transaction for batching operations.
    pub fn transaction(&self) -> PluginTransaction {
        PluginTransaction::new(self.clone())
    }

    /// The petal this plugin instance is scoped to.
    pub fn petal_id(&self) -> &str {
        &self.petal_id
    }

    /// The plugin's unique identifier.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// The capability token controlling this plugin's access.
    pub fn capabilities(&self) -> &CapabilityToken {
        &self.capabilities
    }

    /// Send a raw command to the plugin host.
    pub fn send_command(
        &self,
        cmd: PluginCommand,
    ) -> Result<(), crossbeam::channel::SendError<PluginCommand>> {
        self.tx.send(PluginCommand::Bound {
            session: self.session.clone(),
            command: Box::new(cmd),
        })
    }
}

impl std::fmt::Debug for PluginContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginContext")
            .field("plugin_id", &self.plugin_id)
            .field("petal_id", &self.petal_id)
            .field("capabilities", &self.capabilities)
            .field("host", &self.host)
            .finish_non_exhaustive()
    }
}
