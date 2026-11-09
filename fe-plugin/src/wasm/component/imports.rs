//! Host side of the 2.0 imports `context`, `scene`, `txn`, `readings`, `kv` and `log`;
//! see src/wasm/AGENTS.md §imports. `routes` lives in routes.rs; `ingress`, `egress`, `bus` in broker.rs.

use std::collections::BTreeMap;

use fe_sdk::plugin_host::{NodeQuery, PendingOp, StagedReading, StagedTimestamp};

use super::bindings::fractalengine::plugin::types::{Binding, HostError, Json, NodeId, Quat, Vec3};
use super::bindings::fractalengine::plugin::{context, kv, log as guest_log, readings, scene, txn, types};
use super::kv::{validate_kv_key, MAX_KV_KEY_BYTES, MAX_KV_LIST, MAX_KV_VALUE_BYTES};
use super::state::{context_error, denied, invalid, parse_json_arg, InstanceState};
use crate::transaction::WindowError;
use crate::LogLevel;

/// `[x, y, z]` as a WIT `vec3`.
pub(crate) fn vec3(value: [f64; 3]) -> Vec3 {
    Vec3 { x: value[0], y: value[1], z: value[2] }
}

fn array3(value: Vec3) -> [f64; 3] {
    [value.x, value.y, value.z]
}

fn array4(value: Quat) -> [f64; 4] {
    [value.x, value.y, value.z, value.w]
}

/// A context-projected SDK snapshot as the WIT record; properties serialize in key order.
fn wit_node(node: &crate::NodeSnapshot) -> types::NodeSnapshot {
    let properties: BTreeMap<&str, serde_json::Value> = node
        .properties
        .iter()
        .map(|(key, value)| (key.as_str(), fe_sdk::plugin_host::property_json(value)))
        .collect();
    types::NodeSnapshot {
        node_id: node.node_id.clone(),
        petal_id: node.petal_id.clone(),
        name: node.name.clone(),
        position: vec3(node.position),
        rotation: Quat {
            x: node.rotation[0],
            y: node.rotation[1],
            z: node.rotation[2],
            w: node.rotation[3],
        },
        scale: vec3(node.scale),
        properties: serde_json::to_string(&properties).unwrap_or_else(|_| "{}".into()),
    }
}

impl types::Host for InstanceState {}

impl context::Host for InstanceState {
    fn current_binding(&mut self) -> Binding {
        self.binding.clone()
    }

    fn now_ms(&mut self) -> u64 {
        self.now()
    }
}

impl InstanceState {
    fn require_scene_read(&self) -> Result<(), HostError> {
        self.lease_active()?;
        if self.grants().is_some_and(|grants| grants.scene_read) {
            Ok(())
        } else {
            Err(denied("scene.read"))
        }
    }
}

impl scene::Host for InstanceState {
    fn get_node(&mut self, id: NodeId) -> Result<Option<types::NodeSnapshot>, HostError> {
        self.require_scene_read()?;
        let node = self.context.get_node(&id).map_err(context_error)?;
        Ok(node.as_ref().map(wit_node))
    }

    fn query_nodes(&mut self, query: Json) -> Result<Vec<types::NodeSnapshot>, HostError> {
        self.require_scene_read()?;
        let query = NodeQuery::parse(&query).map_err(invalid)?;
        let nodes = self
            .context
            .query_nodes(self.context.petal_id(), &query)
            .map_err(context_error)?;
        Ok(nodes.iter().map(wit_node).collect())
    }
}

impl txn::Host for InstanceState {
    fn stage_set_property(&mut self, node: NodeId, key: String, value: Json) -> Result<(), HostError> {
        let value = parse_json_arg(&value)?;
        self.stage_op(PendingOp::SetProperty { node_id: node, key, value })
            .map(|_| ())
    }

    fn stage_set_transform(&mut self, node: NodeId, patch: txn::TransformPatch) -> Result<(), HostError> {
        self.stage_op(PendingOp::SetTransform {
            node_id: node,
            position: patch.position.map(array3),
            rotation: patch.rotation.map(array4),
            scale: patch.scale.map(array3),
        })
        .map(|_| ())
    }

    fn stage_create_node(&mut self, name: String) -> Result<u32, HostError> {
        let petal_id = self.context.petal_id().to_string();
        self.stage_op(PendingOp::CreateNode { petal_id, name })?
            .ok_or_else(|| HostError::Backend("create-node staged without an ordinal".into()))
    }

    fn stage_delete_node(&mut self, node: NodeId) -> Result<(), HostError> {
        self.stage_op(PendingOp::DeleteNode { node_id: node }).map(|_| ())
    }

    fn window(&mut self) -> txn::WindowInfo {
        let info = self.window.lock().map(|window| window.info());
        match info {
            Ok(info) => txn::WindowInfo {
                ops: info.ops as u32,
                max_ops: info.max_ops as u32,
                readings: info.readings as u32,
                max_readings: info.max_readings as u32,
                bytes: info.bytes as u32,
                max_bytes: info.max_bytes as u32,
            },
            Err(_) => txn::WindowInfo { ops: 0, max_ops: 0, readings: 0, max_readings: 0, bytes: 0, max_bytes: 0 },
        }
    }

    fn discard(&mut self) {
        self.discard_window();
    }
}

impl readings::Host for InstanceState {
    fn stage(&mut self, batch: Vec<readings::Reading>) -> Result<(), HostError> {
        self.lease_active()?;
        if !self.grants().is_some_and(|grants| grants.readings_class.is_some()) {
            return Err(denied("readings"));
        }
        if batch.is_empty() {
            return Ok(());
        }
        // Refuse an over-budget batch before converting or authorizing any of it.
        let room = self
            .window
            .lock()
            .map(|window| window.info())
            .map(|info| info.max_readings.saturating_sub(info.readings))
            .map_err(|_| HostError::Backend("staged window lock poisoned".into()))?;
        if batch.len() > room {
            return Err(HostError::BudgetExceeded("staged window is full: max_readings".into()));
        }
        let staged: Vec<StagedReading> = batch
            .into_iter()
            .map(|reading| StagedReading {
                node_id: reading.anchor,
                metric: reading.metric,
                value: reading.value,
                units: reading.units,
                recorded_at: reading.recorded_at.map(|time| match time {
                    readings::Timestamp::Rfc3339(text) => StagedTimestamp::Rfc3339(text),
                    readings::Timestamp::EpochMs(ms) => StagedTimestamp::EpochMs(ms),
                }),
            })
            .collect();
        for reading in &staged {
            self.context.authorize_reading(reading).map_err(context_error)?;
        }
        // All-or-nothing: stage into a copy, swap only when every reading fits.
        self.with_window(|window, _| {
            let info = window.info();
            if info.readings + staged.len() > info.max_readings {
                return Err(WindowError::Full("max_readings"));
            }
            let mut trial = window.clone();
            for reading in staged {
                trial.stage_reading(reading)?;
            }
            *window = trial;
            Ok(())
        })
    }
}

impl InstanceState {
    fn kv_slot(&mut self) -> Result<&mut super::kv::KvSlot, HostError> {
        self.kv.as_mut().ok_or_else(|| denied("kv"))
    }
}

impl kv::Host for InstanceState {
    fn get(&mut self, key: String) -> Result<Option<Json>, HostError> {
        validate_kv_key(&key).map_err(invalid)?;
        self.kv_slot()?.backend().get(&key).map_err(HostError::Backend)
    }

    fn set(&mut self, key: String, value: Json) -> Result<(), HostError> {
        validate_kv_key(&key).map_err(invalid)?;
        if value.len() > MAX_KV_VALUE_BYTES {
            return Err(invalid(format!("kv value exceeds {MAX_KV_VALUE_BYTES} bytes")));
        }
        parse_json_arg(&value)?;
        let slot = self.kv_slot()?;
        if !slot.fits(&key, &value).map_err(HostError::Backend)? {
            return Err(HostError::BudgetExceeded("kv.max_bytes".into()));
        }
        slot.backend_mut().set(&key, value).map_err(HostError::Backend)
    }

    fn delete(&mut self, key: String) -> Result<(), HostError> {
        validate_kv_key(&key).map_err(invalid)?;
        self.kv_slot()?.backend_mut().delete(&key).map_err(HostError::Backend)
    }

    fn list_keys(&mut self, prefix: String, limit: u32) -> Result<Vec<String>, HostError> {
        if !(1..=MAX_KV_LIST).contains(&limit) {
            return Err(invalid(format!("list-keys limit must be 1..={MAX_KV_LIST}")));
        }
        if prefix.len() > MAX_KV_KEY_BYTES {
            return Err(invalid(format!("kv prefix exceeds {MAX_KV_KEY_BYTES} bytes")));
        }
        self.kv_slot()?
            .backend()
            .keys(&prefix, limit as usize)
            .map_err(HostError::Backend)
    }
}

impl guest_log::Host for InstanceState {
    fn write(&mut self, level: guest_log::Level, message: String) {
        let level = match level {
            guest_log::Level::Debug => LogLevel::Debug,
            guest_log::Level::Info => LogLevel::Info,
            guest_log::Level::Warn => LogLevel::Warn,
            guest_log::Level::Error => LogLevel::Error,
        };
        if let Ok(mut log) = self.log.lock() {
            log.write(level, &message);
        }
    }
}
