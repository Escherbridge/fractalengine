//! Bounded terminal drains; see AGENTS.md.

use super::{Status, Supervisor};
use bevy::prelude::*;
use fe_runtime::messages::DbResult;

pub(super) fn drain_terminal_channels(
    world: &mut World,
    supervisor: &mut Supervisor,
    status: &mut Status,
) {
    let results: Vec<_> = world
        .get_resource::<fe_runtime::app::DbResultReceiver>()
        .and_then(|receiver| {
            receiver
                .0
                .lock()
                .ok()
                .map(|rx| rx.try_iter().take(fe_runtime::CHANNEL_BUFFER).collect())
        })
        .unwrap_or_default();
    for result in results {
        fe_ui::maintenance::observe_terminal_db_result(world, &result);
        if let DbResult::TerminalDevelopmentReset {
            request_id,
            cleared,
            error,
        } = result
        {
            if supervisor.clear_sent
                && status.request_id.as_deref() == Some(request_id.as_str())
                && supervisor.receipt.is_none()
            {
                status.cleared = Some(cleared);
                supervisor.receipt = Some((cleared, error));
            }
        }
    }
    macro_rules! drain_locked {
        ($ty:ty) => {
            if let Some(receiver) = world.get_resource::<$ty>() {
                if let Ok(rx) = receiver.0.lock() {
                    for _ in 0..fe_runtime::CHANNEL_BUFFER {
                        if rx.try_recv().is_err() {
                            break;
                        }
                    }
                }
            }
        };
    }
    drain_locked!(fe_runtime::app::NetworkEventReceiver);
    drain_locked!(fe_runtime::app::LifecycleEventReceiver);
    drain_locked!(fe_runtime::app::ApiCommandReceiver);
    drain_locked!(fe_sync::SyncEventReceiverRes);
    drain_locked!(fe_plugin::PluginResultReceiver);
    if let Some(receiver) = world.get_resource::<crate::SceneChangeReceiver>() {
        for _ in 0..fe_runtime::CHANNEL_BUFFER {
            if receiver.0.try_recv().is_err() {
                break;
            }
        }
    }
    if let Some(receiver) = world.get_resource::<fe_runtime::app::InboundTransformReceiver>() {
        for _ in 0..fe_runtime::CHANNEL_BUFFER {
            if receiver.0.try_recv().is_err() {
                break;
            }
        }
    }
}
