//! Empty-node creation dispatch (viewport context-menu "Add Empty Node").

use fe_runtime::app::DbCommandSender;
use fe_runtime::messages::DbCommand;

/// Send `CreateNode` for an empty node at `position` in `petal_id`.
pub(crate) fn create_at(db_sender: &DbCommandSender, petal_id: String, position: [f32; 3]) {
    if db_sender
        .0
        .send(DbCommand::CreateNode {
            petal_id,
            name: "Empty Node".to_string(),
            position,
            correlation_id: None,
        })
        .is_err()
    {
        bevy::log::warn!("db_sender channel closed — CreateNode not sent");
    }
}
