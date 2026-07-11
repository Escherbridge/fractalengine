//! Node custom-property action handling (load / set / delete).

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

pub(crate) fn load(db_sender: &DbCommandSender, node_id: String) {
    if db_sender
        .0
        .send(DbCommand::GetNodeProperties { node_id })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — GetNodeProperties not dispatched");
    }
}

pub(crate) fn set(db_sender: &DbCommandSender, node_id: String, key: String, value: serde_json::Value) {
    if db_sender
        .0
        .send(DbCommand::SetNodeProperty { node_id, key, value })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — SetNodeProperty not dispatched");
    }
}

pub(crate) fn delete(db_sender: &DbCommandSender, node_id: String, key: String) {
    if db_sender
        .0
        .send(DbCommand::DeleteNodeProperty { node_id, key })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — DeleteNodeProperty not dispatched");
    }
}
