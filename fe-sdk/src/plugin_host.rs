//! Bounded scene host contracts; see src/AGENTS.md §plugin-host.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{NodeSnapshot, PropertyValue};

/// Maximum scene operations admitted in one atomic transaction.
pub const MAX_TRANSACTION_OPS: usize = 64;
/// Maximum encoded transaction or returned snapshot bytes.
pub const MAX_HOST_BYTES: usize = 65_536;
/// Maximum nodes returned by one structured query.
pub const MAX_NODE_QUERY_ROWS: usize = 200;

/// Shared host-only revocation lease; never reconstructed from guest input.
#[derive(Debug, Clone)]
pub struct PluginLease(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Default for PluginLease {
    fn default() -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            true,
        )))
    }
}

impl PluginLease {
    /// Refuse work that has not crossed the backend commit admission boundary.
    pub fn revoke(&self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
    /// Check the same flag across queued requests and direct contexts.
    pub fn require_active(&self) -> Result<(), String> {
        if self.0.load(std::sync::atomic::Ordering::Acquire) {
            Ok(())
        } else {
            Err("plugin instance is no longer admitted".into())
        }
    }
}

/// One deferred operation; the sink commits the entire list or changes nothing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PendingOp {
    /// Replace one node property.
    SetProperty {
        /// Existing node in the admitted petal.
        node_id: String,
        /// Admitted property key to replace.
        key: String,
        /// Complete replacement value, preserving its JSON type.
        value: Value,
    },
    /// Create a data node in the bound petal.
    CreateNode {
        /// Exact petal bound to the admitted instance.
        petal_id: String,
        /// Display name for the new data node.
        name: String,
    },
    /// Soft-delete an existing node through its lifecycle path.
    DeleteNode {
        /// Existing plain node admitted for lifecycle deletion.
        node_id: String,
    },
    /// Patch a node transform; absent fields keep the stored value at commit.
    SetTransform {
        /// Existing node in the admitted petal.
        node_id: String,
        /// New world position, if changed.
        #[serde(default)]
        position: Option<[f64; 3]>,
        /// New rotation quaternion `[x, y, z, w]`, if changed.
        #[serde(default)]
        rotation: Option<[f64; 4]>,
        /// New scale, if changed.
        #[serde(default)]
        scale: Option<[f64; 3]>,
    },
}

/// Maximum readings staged in one tick window.
pub const MAX_STAGED_READINGS: usize = 1_000;
/// Maximum metric-name length in bytes.
pub const MAX_METRIC_LEN: usize = 128;
/// Maximum units-string length in bytes.
pub const MAX_UNITS_LEN: usize = 64;

/// A sensor time on a staged reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagedTimestamp {
    /// RFC-3339 text, parsed at the DB door.
    Rfc3339(String),
    /// Milliseconds since the Unix epoch.
    EpochMs(i64),
}

/// One reading staged by an extension; source class and DID are host-stamped, never guest input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StagedReading {
    /// Anchor node; checked against the bound petal at flush.
    pub node_id: String,
    /// Metric name, inside the manifest's declared metrics.
    pub metric: String,
    /// Finite measurement.
    pub value: f64,
    /// Units label, may be empty.
    #[serde(default)]
    pub units: String,
    /// Sensor time; the flushing tick's clock when absent.
    #[serde(default)]
    pub recorded_at: Option<StagedTimestamp>,
    /// Host-only decode provenance; never read from guest input (R-X-29).
    #[serde(default, skip_deserializing)]
    pub provenance: Option<ReadingProvenance>,
}

/// Host-stamped origin of a decoded reading: the device payload and the decoder source class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadingProvenance {
    /// The `device_payload` row this reading was decoded from.
    pub payload_id: String,
    /// The class the reading lands under (the decoder class).
    pub source_class: crate::source_class::SourceClass,
}

/// An exact property predicate, compared without numeric coercion.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyEquals {
    /// Property key to inspect.
    pub key: String,
    /// Required JSON value.
    pub value: Value,
}

/// A bounded structured query, never a SQL or script expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeQuery {
    /// Optional exact display name.
    pub name: Option<String>,
    /// Optional exact property predicate.
    pub property: Option<PropertyEquals>,
    /// Maximum rows, in deterministic node-ID order.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

impl Default for NodeQuery {
    fn default() -> Self {
        Self {
            name: None,
            property: None,
            limit: default_limit(),
        }
    }
}

impl NodeQuery {
    /// Parse the structured vocabulary; `all` and an empty string preserve legacy calls.
    pub fn parse(filter: &str) -> Result<Self, String> {
        if filter.len() > 4096 {
            return Err("node filter exceeds 4096 bytes".into());
        }
        let query: Self = if matches!(filter.trim(), "" | "all") {
            Self::default()
        } else {
            serde_json::from_str(filter).map_err(|error| format!("invalid node filter: {error}"))?
        };
        query.validate()?;
        Ok(query)
    }

    /// Validate query bounds before the reader runs.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_NODE_QUERY_ROWS).contains(&self.limit) {
            return Err("node query limit must be 1..200".into());
        }
        if self.name.as_ref().is_some_and(|name| name.len() > 1024) {
            return Err("node query name exceeds 1024 bytes".into());
        }
        if let Some(property) = &self.property {
            crate::storage::validate_key(&property.key).map_err(|error| error.to_string())?;
        }
        if serde_json::to_vec(self)
            .map_err(|error| error.to_string())?
            .len()
            > 4096
        {
            return Err("node filter exceeds 4096 bytes".into());
        }
        Ok(())
    }

    /// Evaluate the declared predicates on a current node snapshot.
    pub fn matches(&self, node: &NodeSnapshot) -> bool {
        self.name.as_ref().is_none_or(|name| &node.name == name)
            && self.property.as_ref().is_none_or(|predicate| {
                node.properties
                    .get(&predicate.key)
                    .is_some_and(|value| property_json(value) == predicate.value)
            })
    }
}

/// Recover the JSON property value without wrapping it in SDK serde tags.
pub fn property_json(value: &PropertyValue) -> Value {
    match value {
        PropertyValue::String(value) => Value::String(value.clone()),
        PropertyValue::Number(value) => serde_json::json!(value),
        PropertyValue::Bool(value) => Value::Bool(*value),
        PropertyValue::Json(value) => value.clone(),
    }
}

/// Preserve arbitrary JSON, including integer precision, in the SDK property type.
pub fn json_property(value: Value) -> PropertyValue {
    match value {
        Value::String(value) => PropertyValue::String(value),
        Value::Bool(value) => PropertyValue::Bool(value),
        value => PropertyValue::Json(value),
    }
}

/// Serialize a snapshot with plain property JSON for script hosts.
pub fn snapshot_json(node: &NodeSnapshot) -> Value {
    let properties: serde_json::Map<String, Value> = node
        .properties
        .iter()
        .map(|(key, value)| (key.clone(), property_json(value)))
        .collect();
    serde_json::json!({"node_id":node.node_id,"petal_id":node.petal_id,"name":node.name,
        "position":node.position,"rotation":node.rotation,"scale":node.scale,"properties":properties})
}

/// The authoritative result of a completed atomic transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionReceipt {
    /// Number of operations actually committed.
    pub op_count: usize,
    /// Created node IDs in creation-operation order.
    pub created_node_ids: Vec<String>,
    /// Readings landed by the same flush; absent in receipts from hosts that predate readings.
    #[serde(default)]
    pub reading_count: usize,
}

/// A host-supplied reader; implementations recheck the bound petal at use.
pub trait NodeReader: Send {
    /// Resolve the stored full ancestry of a petal.
    fn petal_scope(&self, petal_id: &str) -> Result<String, String>;
    /// Return one live node in that exact petal, or no node.
    fn get_node(&self, petal_id: &str, node_id: &str) -> Result<Option<NodeSnapshot>, String>;
    /// Read a bounded, filtered page in deterministic node-ID order.
    fn query_nodes(&self, petal_id: &str, query: &NodeQuery) -> Result<Vec<NodeSnapshot>, String>;
}

/// A host-supplied atomic writer, bound to trusted plugin admission and capability limits.
pub trait TransactionSink: Send {
    /// Return only after atomic persistence; refusals have no effects and transport failures report an unknown outcome.
    fn commit(&mut self, petal_id: &str, ops: &[PendingOp]) -> Result<TransactionReceipt, String>;
}

/// Validate the complete transaction before any reader or sink side effects.
pub fn validate_transaction(ops: &[PendingOp]) -> Result<(), String> {
    if ops.len() > MAX_TRANSACTION_OPS {
        return Err("plugin transaction exceeds 64 operations".into());
    }
    if serde_json::to_vec(ops)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_HOST_BYTES
    {
        return Err("plugin transaction exceeds 65536 bytes".into());
    }
    for op in ops {
        match op {
            PendingOp::SetProperty {
                node_id,
                key,
                value,
            } => {
                crate::storage::validate_node_id(node_id).map_err(|error| error.to_string())?;
                crate::storage::validate_key(key).map_err(|error| error.to_string())?;
                crate::property_schema::validate_persistable_numbers(value)?;
            }
            PendingOp::CreateNode { petal_id, name } => {
                crate::storage::validate_node_id(petal_id).map_err(|error| error.to_string())?;
                if name.trim().is_empty() || name.len() > 1024 {
                    return Err("node name must be nonempty and at most 1024 bytes".into());
                }
            }
            PendingOp::DeleteNode { node_id } => {
                crate::storage::validate_node_id(node_id).map_err(|error| error.to_string())?
            }
            PendingOp::SetTransform {
                node_id,
                position,
                rotation,
                scale,
            } => {
                crate::storage::validate_node_id(node_id).map_err(|error| error.to_string())?;
                validate_transform_patch(*position, *rotation, *scale)?;
            }
        }
    }
    Ok(())
}

/// A patch must change something and stay finite, with a usable rotation and positive scale.
pub fn validate_transform_patch(
    position: Option<[f64; 3]>,
    rotation: Option<[f64; 4]>,
    scale: Option<[f64; 3]>,
) -> Result<(), String> {
    if position.is_none() && rotation.is_none() && scale.is_none() {
        return Err("transform patch changes nothing".into());
    }
    if position.is_some_and(|v| v.iter().any(|c| !c.is_finite())) {
        return Err("transform position must be finite".into());
    }
    if let Some(q) = rotation {
        let norm_sq: f64 = q.iter().map(|c| c * c).sum();
        if q.iter().any(|c| !c.is_finite()) || norm_sq < 1e-12 {
            return Err("transform rotation must be a finite nonzero quaternion".into());
        }
    }
    if scale.is_some_and(|v| v.iter().any(|c| !c.is_finite() || *c <= 0.0)) {
        return Err("transform scale must be finite and positive".into());
    }
    Ok(())
}

/// Validate one tick window's staged readings: count, shape and finiteness (anchors are a flush-time check).
pub fn validate_staged_readings(readings: &[StagedReading]) -> Result<(), String> {
    if readings.len() > MAX_STAGED_READINGS {
        return Err("plugin commit exceeds 1000 readings".into());
    }
    for reading in readings {
        crate::storage::validate_node_id(&reading.node_id).map_err(|error| error.to_string())?;
        let metric_ok = !reading.metric.is_empty()
            && reading.metric.len() <= MAX_METRIC_LEN
            && reading
                .metric
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'));
        if !metric_ok {
            return Err("reading metric must be 1..128 of [A-Za-z0-9_.:-]".into());
        }
        if !reading.value.is_finite() {
            return Err("reading value must be finite".into());
        }
        if reading.units.len() > MAX_UNITS_LEN {
            return Err("reading units exceed 64 bytes".into());
        }
        if matches!(&reading.recorded_at, Some(StagedTimestamp::Rfc3339(text)) if text.is_empty() || text.len() > 64)
        {
            return Err("reading recorded_at must be 1..64 bytes of RFC-3339".into());
        }
    }
    Ok(())
}

/// Validate a whole commit (ops plus readings) and their combined encoded size.
pub fn validate_commit(ops: &[PendingOp], readings: &[StagedReading]) -> Result<(), String> {
    validate_transaction(ops)?;
    validate_staged_readings(readings)?;
    let readings_len = serde_json::to_vec(readings)
        .map_err(|error| error.to_string())?
        .len();
    let ops_len = serde_json::to_vec(ops)
        .map_err(|error| error.to_string())?
        .len();
    if ops_len + readings_len > MAX_HOST_BYTES * 8 {
        return Err("plugin commit exceeds 524288 bytes".into());
    }
    Ok(())
}
