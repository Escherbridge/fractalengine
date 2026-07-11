//! SurrealQL query submission action.

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

pub(crate) fn submit(db_sender: &DbCommandSender, sql: String) {
    if db_sender
        .0
        .send(DbCommand::RawQuery {
            sql,
            vars: std::collections::HashMap::new(),
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — RawQuery not dispatched");
    }
}
