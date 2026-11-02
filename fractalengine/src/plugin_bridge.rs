//! Production plugin services over the typed DB channel; see src/AGENTS.md.

use bevy::prelude::App;
use crossbeam::channel::Sender;
use fe_plugin::scene_host::{PluginContexts, SceneBackendFactory};
use fe_plugin::{NodeQuery, NodeReader, NodeSnapshot, TransactionReceipt, TransactionSink};
use fe_runtime::messages::DbCommand;
use fe_runtime::plugin_host::{PluginAdmission, PluginRead, PluginReply};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
struct DbSceneBackend {
    sender: Sender<DbCommand>,
    admission: PluginAdmission,
}

impl DbSceneBackend {
    fn request(
        &self,
        read: Option<PluginRead>,
        ops: &[fe_plugin::transaction::PendingOp],
    ) -> Result<PluginReply, String> {
        let (reply, receiver) = crossbeam::channel::bounded(1);
        let is_write = read.is_none();
        let command = match read {
            Some(request) => DbCommand::PluginRead {
                admission: self.admission.clone(),
                request,
                reply,
            },
            None => DbCommand::PluginCommit {
                admission: self.admission.clone(),
                ops: ops.to_vec(),
                reply,
            },
        };
        self.sender
            .send_timeout(command, Duration::from_secs(1))
            .map_err(|_| "plugin DB command was not enqueued".to_string())?;
        receiver
            .recv_timeout(Duration::from_secs(15))
            .map_err(|_| {
                if is_write {
                    "plugin commit outcome is unknown; reconcile before retrying".to_string()
                } else {
                    "plugin DB read reply timed out or disconnected".to_string()
                }
            })?
    }

    fn require_petal(&self, petal_id: &str) -> Result<(), String> {
        if petal_id != self.admission.petal_id() {
            return Err("plugin backend is bound to another petal".into());
        }
        Ok(())
    }
}

impl NodeReader for DbSceneBackend {
    fn petal_scope(&self, petal_id: &str) -> Result<String, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::PetalScope), &[])? {
            PluginReply::Scope(scope) => Ok(scope),
            _ => Err("unrelated plugin scope reply".into()),
        }
    }
    fn get_node(&self, petal_id: &str, node_id: &str) -> Result<Option<NodeSnapshot>, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::Node(node_id.into())), &[])? {
            PluginReply::Node(node) => Ok(node),
            _ => Err("unrelated plugin node reply".into()),
        }
    }
    fn query_nodes(&self, petal_id: &str, query: &NodeQuery) -> Result<Vec<NodeSnapshot>, String> {
        self.require_petal(petal_id)?;
        match self.request(Some(PluginRead::Nodes(query.clone())), &[])? {
            PluginReply::Nodes(nodes) => Ok(nodes),
            _ => Err("unrelated plugin query reply".into()),
        }
    }
}

impl TransactionSink for DbSceneBackend {
    fn commit(
        &mut self,
        petal_id: &str,
        ops: &[fe_plugin::transaction::PendingOp],
    ) -> Result<TransactionReceipt, String> {
        self.require_petal(petal_id)?;
        match self.request(None, ops)? {
            PluginReply::Committed(receipt) => Ok(receipt),
            _ => Err("unrelated plugin commit reply; outcome requires reconciliation".into()),
        }
    }
}

struct DbSceneFactory(Sender<DbCommand>);

impl SceneBackendFactory for DbSceneFactory {
    fn bind(
        &self,
        context: fe_plugin::context::PluginContext,
    ) -> Result<fe_plugin::context::PluginContext, String> {
        let token = context.capabilities();
        let admission = PluginAdmission::new(
            context.plugin_id().into(),
            context.petal_id().into(),
            token.capabilities.clone(),
            token.property_scope.clone(),
            token.verse_scope.clone(),
            token.petal_scope.clone(),
        )?;
        let mut backend = DbSceneBackend {
            sender: self.0.clone(),
            admission,
        };
        let scope = backend.petal_scope(context.petal_id())?;
        backend.admission = backend.admission.bind_scope(scope)?;
        let backend = Arc::new(Mutex::new(backend));
        Ok(context
            .with_node_reader(backend.clone())
            .with_transaction_sink(backend))
    }
}

pub(crate) fn install(app: &mut App) {
    let sender = app
        .world()
        .resource::<fe_runtime::app::DbCommandSender>()
        .0
        .clone();
    app.insert_resource(PluginContexts::default().with_factory(Arc::new(DbSceneFactory(sender))));
    app.add_plugins(fe_plugin::PluginHostPlugin);
}
