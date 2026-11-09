//! Extension manifest v2: serde types plus the ONE validator; see src/AGENTS.md §manifest-v2.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ui::UiSlot;

/// Host ceiling for `tick.fuel`.
pub const HOST_FUEL_CEILING: u64 = 20_000_000;
/// Host floor for `tick.cadence_ms`.
pub const HOST_CADENCE_FLOOR_MS: u64 = 100;
/// Host ceiling for `tick.max_ops`.
pub const HOST_MAX_OPS_CEILING: u32 = 64;
/// Host ceiling for `tick.max_readings`.
pub const HOST_MAX_READINGS_CEILING: u32 = 1_000;
/// Largest `args_schema` in bytes.
pub const MAX_ARGS_SCHEMA_BYTES: usize = 4_096;

/// The distinct refusal reasons of the validation table (docs/extensions/manifest-v2.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// V1: structure, type, pattern, bound or args_schema subset.
    Schema,
    /// V2: contract is not `fractalengine:plugin@2.x.y`.
    UnsupportedContract,
    /// V3: `web-bundle` kind or a stray `portal` section.
    ReservedKind,
    /// V4: a scope list is present and empty.
    EmptyScope,
    /// V5: non-loopback plaintext URL, or userinfo/query on any endpoint.
    InsecureEndpoint,
    /// V6: cadence under the host floor.
    CadenceBelowFloor,
    /// V7: writes or readings without a tick block.
    CadenceRequired,
    /// V8: source class outside the kind's set.
    SourceClassMismatch,
    /// V9: UI command not exposed by the package.
    UndeclaredCommand,
    /// V10: UI or consume stream not exposed by the package.
    UndeclaredStream,
    /// V11: unknown `host/*` source or a source consumed twice.
    UnknownSource,
    /// V12: named query fails the static guard.
    InvalidQuery,
    /// V13: scope conflict between sections.
    ScopeConflict,
    /// V14: duplicate id.
    DuplicateId,
    /// V15: a number above a host ceiling.
    OverHostCeiling,
}

impl Refusal {
    /// The kebab-case reason `fe-ext validate` and REST install both print.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Schema => "schema",
            Self::UnsupportedContract => "unsupported-contract",
            Self::ReservedKind => "reserved-kind",
            Self::EmptyScope => "empty-scope",
            Self::InsecureEndpoint => "insecure-endpoint",
            Self::CadenceBelowFloor => "cadence-below-floor",
            Self::CadenceRequired => "cadence-required",
            Self::SourceClassMismatch => "source-class-mismatch",
            Self::UndeclaredCommand => "undeclared-command",
            Self::UndeclaredStream => "undeclared-stream",
            Self::UnknownSource => "unknown-source",
            Self::InvalidQuery => "invalid-query",
            Self::ScopeConflict => "scope-conflict",
            Self::DuplicateId => "duplicate-id",
            Self::OverHostCeiling => "over-host-ceiling",
        }
    }
}

/// A refused manifest: the reason plus a human detail naming the offending member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestError {
    /// Distinct reason.
    pub reason: Refusal,
    /// What was wrong, for the author.
    pub detail: String,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.reason.as_str(), self.detail)
    }
}

impl std::error::Error for ManifestError {}

fn refuse<T>(reason: Refusal, detail: impl Into<String>) -> Result<T, ManifestError> {
    Err(ManifestError {
        reason,
        detail: detail.into(),
    })
}

fn schema<T>(detail: impl Into<String>) -> Result<T, ManifestError> {
    refuse(Refusal::Schema, detail)
}

/// Content type inside the Plugin family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionKind {
    /// Moves simulated actors and emits simulated readings.
    Simulation,
    /// Turns device traffic into readings and transforms.
    DeviceAdapter,
    /// Pushes data to a business-intelligence endpoint.
    BiConnector,
    /// Anything else.
    General,
    /// Reserved for the portal program; refused here.
    WebBundle,
}

/// A source class an extension may write as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSourceClass {
    /// The simulation lab.
    Simulation,
    /// A field device.
    Device,
    /// A configured connector.
    Connector,
}

impl From<ExtensionSourceClass> for crate::source_class::SourceClass {
    fn from(class: ExtensionSourceClass) -> Self {
        match class {
            ExtensionSourceClass::Simulation => Self::Simulation,
            ExtensionSourceClass::Device => Self::Device,
            ExtensionSourceClass::Connector => Self::Connector,
        }
    }
}

/// One class, or the set the operator picks ONE from at bind time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SourceClassSet {
    /// A single fixed class.
    One(ExtensionSourceClass),
    /// The operator picks exactly one.
    Many(Vec<ExtensionSourceClass>),
}

impl SourceClassSet {
    /// Every class named, in declaration order.
    pub fn classes(&self) -> Vec<ExtensionSourceClass> {
        match self {
            Self::One(class) => vec![*class],
            Self::Many(classes) => classes.clone(),
        }
    }
}

/// Minimum role a caller needs at the bound petal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoleFloor {
    /// Read-only.
    Viewer,
    /// Edit content.
    Editor,
    /// Manage members.
    Manager,
    /// Full control.
    Owner,
}

/// Scene access grants.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneGrant {
    /// `scene.get-node`, `scene.query-nodes` and `host/node-changes`.
    #[serde(default)]
    pub read: bool,
    /// Staged write kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<Vec<SceneWrite>>,
    /// Property keys readable and writable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property_scope: Option<Vec<String>>,
}

/// A staged scene write kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneWrite {
    /// Create a data node.
    CreateNode,
    /// Patch a transform.
    SetTransform,
    /// Replace a property.
    SetProperty,
    /// Soft-delete a plain node.
    DeleteNode,
}

/// Readings grant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadingsGrant {
    /// Class or class set.
    pub source_class: SourceClassSet,
    /// Metric names or `["*"]`.
    pub metrics: Vec<String>,
}

/// The cadence budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TickBudget {
    /// `on-tick` period in milliseconds.
    pub cadence_ms: u64,
    /// Scene ops per flush.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ops: Option<u32>,
    /// Readings per flush.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_readings: Option<u32>,
    /// Fuel per hook call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuel: Option<u64>,
}

/// Host-local key-value grant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KvGrant {
    /// Byte budget, at most 1 MiB.
    pub max_bytes: u64,
}

/// Host-side credential slot reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSlot {
    /// Scheme.
    pub scheme: AuthScheme,
    /// Slot name in the host credential store.
    pub credential: String,
    /// Header name, required for `header`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
}

/// Credential scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthScheme {
    /// `Authorization: Bearer`.
    Bearer,
    /// HTTP basic.
    Basic,
    /// A named header.
    Header,
    /// MQTT password.
    MqttPassword,
}

/// HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Method {
    /// GET.
    Get,
    /// POST.
    Post,
    /// PUT.
    Put,
    /// PATCH.
    Patch,
    /// DELETE.
    Delete,
}

/// MQTT quality of service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Qos {
    /// Fire and forget.
    AtMostOnce,
    /// Acknowledged.
    AtLeastOnce,
}

/// Reconnect backoff bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconnectMs {
    /// Smallest wait.
    pub min: u64,
    /// Largest wait.
    pub max: u64,
}

/// An ingress endpoint, tagged by `transport`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport")]
pub enum Ingress {
    /// HTTP server mount.
    #[serde(rename = "http-server")]
    HttpServer(HttpServerIngress),
    /// Websocket upgrade on the same mount.
    #[serde(rename = "websocket-accept")]
    WebsocketAccept(WebsocketAcceptIngress),
    /// Host dials a websocket.
    #[serde(rename = "websocket-dial")]
    WebsocketDial(WebsocketDialIngress),
    /// Host subscribes to an MQTT broker.
    #[serde(rename = "mqtt")]
    Mqtt(MqttIngress),
}

/// HTTP server ingress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpServerIngress {
    /// Endpoint id.
    pub id: String,
    /// Allowed methods.
    pub methods: Vec<Method>,
    /// Allowed content types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_types: Option<Vec<String>>,
    /// Body cap.
    pub max_body_bytes: u64,
    /// Per-caller rate.
    pub rate_hz: f64,
    /// Role needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_floor: Option<RoleFloor>,
    /// Accept sub-paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_paths: Option<bool>,
    /// Headers forwarded to the guest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forward_headers: Option<Vec<String>>,
}

/// Websocket accept ingress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebsocketAcceptIngress {
    /// Endpoint id.
    pub id: String,
    /// Frame cap.
    pub max_frame_bytes: u64,
    /// Frames per second per connection.
    pub rate_hz: f64,
    /// Connection cap.
    pub max_connections: u32,
    /// Role needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_floor: Option<RoleFloor>,
}

/// Websocket dial ingress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebsocketDialIngress {
    /// Endpoint id.
    pub id: String,
    /// Target URL.
    pub url: String,
    /// Frame cap.
    pub max_frame_bytes: u64,
    /// Frames per second.
    pub rate_hz: f64,
    /// Credential slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSlot>,
    /// Redial backoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect_ms: Option<ReconnectMs>,
}

/// MQTT ingress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MqttIngress {
    /// Endpoint id.
    pub id: String,
    /// Broker URL.
    pub broker: String,
    /// Subscribed topic filters.
    pub topics: Vec<String>,
    /// Quality of service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qos: Option<Qos>,
    /// Payload cap.
    pub max_payload_bytes: u64,
    /// Messages per second.
    pub rate_hz: f64,
    /// Credential slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSlot>,
}

/// An egress endpoint, tagged by `transport`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "transport")]
pub enum Egress {
    /// Outbound HTTP.
    #[serde(rename = "http")]
    Http(HttpEgress),
    /// Outbound MQTT publish.
    #[serde(rename = "mqtt")]
    Mqtt(MqttEgress),
}

/// HTTP egress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpEgress {
    /// Endpoint id.
    pub id: String,
    /// Base URL.
    pub url: String,
    /// Allowed methods.
    pub methods: Vec<Method>,
    /// Request cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_request_bytes: Option<u64>,
    /// Response cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_response_bytes: Option<u64>,
    /// Timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Requests per second.
    pub rate_hz: f64,
    /// Credential slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSlot>,
}

/// MQTT egress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MqttEgress {
    /// Endpoint id.
    pub id: String,
    /// Broker URL.
    pub broker: String,
    /// Publish topic filters.
    pub topics: Vec<String>,
    /// Quality of service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qos: Option<Qos>,
    /// Retain flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retain: Option<bool>,
    /// Payload cap.
    pub max_payload_bytes: u64,
    /// Publishes per second.
    pub rate_hz: f64,
    /// Credential slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSlot>,
}

/// The integration-bus section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusSection {
    /// What this package publishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose: Option<BusExpose>,
    /// Sources this package subscribes to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consume: Option<Vec<ConsumeEntry>>,
    /// Foreign commands it may call, `<package_id>/<command>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<Vec<String>>,
}

/// Exposed streams and commands.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BusExpose {
    /// Streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streams: Option<Vec<ExposedStream>>,
    /// Commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<ExposedCommand>>,
}

/// An exposed stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposedStream {
    /// Stream id.
    pub id: String,
    /// Presentation text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Publish rate cap.
    pub max_rate_hz: f64,
    /// Event size cap.
    pub max_event_bytes: u64,
    /// Role needed to subscribe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_floor: Option<RoleFloor>,
    /// Reachable by remote DIDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
    /// Restrict to these consumer packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumers: Option<Vec<String>>,
}

/// An exposed command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposedCommand {
    /// Command id.
    pub id: String,
    /// Presentation text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Role needed to call.
    pub role_floor: RoleFloor,
    /// Call rate cap.
    pub rate_hz: f64,
    /// Reachable by remote DIDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
    /// Argument schema in the bus.md subset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args_schema: Option<Value>,
}

/// One consumed source; `host/query` carries `id` + `sql`, `host/readings` may carry `metrics`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsumeEntry {
    /// `host/<x>` or `<package_id>/<stream>`.
    pub source: String,
    /// Named query id (`host/query`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// One SELECT, or `*` (`host/query`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// Minimum refresh interval (`host/query`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_interval_ms: Option<u64>,
    /// Metric filter (`host/readings`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Vec<String>>,
}

/// Context-menu node filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiWhenSpec {
    /// Property that must be present.
    pub property: String,
    /// Value it must equal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equals: Option<Value>,
}

/// A declarative UI contribution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiEntry {
    /// Contribution id.
    pub id: String,
    /// Target slot.
    pub slot: UiSlot,
    /// Display label.
    pub label: String,
    /// Icon name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Exposed command id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Property rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<String>>,
    /// Exposed stream id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    /// JSON Pointer into the stream event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Context-menu filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<UiWhenSpec>,
}

/// The whole manifest (`plugin/extension.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    /// Always 2.
    pub manifest_version: u32,
    /// Reverse-DNS package id.
    pub id: String,
    /// SemVer.
    pub version: String,
    /// Display name.
    pub name: String,
    /// Display description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Kind.
    pub kind: ExtensionKind,
    /// WIT package the component targets.
    pub contract: String,
    /// Always `petal`.
    pub binding: String,
    /// Scene grants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<SceneGrant>,
    /// Readings grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readings: Option<ReadingsGrant>,
    /// Route geometry grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routes: Option<bool>,
    /// Cadence budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick: Option<TickBudget>,
    /// Key-value grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv: Option<KvGrant>,
    /// Ingress endpoints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ingress: Vec<Ingress>,
    /// Egress endpoints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub egress: Vec<Egress>,
    /// Bus section.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus: Option<BusSection>,
    /// UI contributions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<UiEntry>,
    /// Reserved for the portal program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portal: Option<Value>,
}

/// A manifest that passed schema and semantics, with its consent digest.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedManifest {
    /// The typed manifest.
    pub manifest: ExtensionManifest,
    /// Lowercase-hex blake3 of the canonical capability projection.
    pub capability_digest: String,
}

/// Run schema then semantics on raw manifest bytes; the one entry `fe-ext validate` and REST install share.
pub fn parse_and_validate(bytes: &[u8]) -> Result<ValidatedManifest, ManifestError> {
    let raw: Value = serde_json::from_slice(bytes).map_err(|error| ManifestError {
        reason: Refusal::Schema,
        detail: format!("not JSON: {error}"),
    })?;
    let manifest: ExtensionManifest =
        serde_json::from_value(raw.clone()).map_err(|error| ManifestError {
            reason: Refusal::Schema,
            detail: error.to_string(),
        })?;
    manifest.validate()?;
    Ok(ValidatedManifest {
        manifest,
        capability_digest: capability_digest(&raw),
    })
}

impl ExtensionManifest {
    /// V1..V15 in table order; the first failing row is reported.
    pub fn validate(&self) -> Result<(), ManifestError> {
        self.check_structure()?; // V1
        self.check_contract()?; // V2
        self.check_reserved_kind()?; // V3
        self.check_empty_scopes()?; // V4
        self.check_endpoints()?; // V5
        self.check_cadence()?; // V6, V7
        self.check_source_class()?; // V8
        self.check_ui_references()?; // V9, V10
        self.check_consume()?; // V10 (own stream), V11, V12
        self.check_scope_conflicts()?; // V13
        self.check_duplicates()?; // V14
        self.check_ceilings() // V15
    }

    fn check_structure(&self) -> Result<(), ManifestError> {
        if self.manifest_version != 2 {
            return schema("manifest_version must be 2");
        }
        if !is_package_id(&self.id) {
            return schema("id must be a reverse-DNS package id");
        }
        if !is_semver(&self.version) {
            return schema("version must be SemVer");
        }
        if self.name.is_empty() || self.name.chars().count() > 80 {
            return schema("name must be 1..80 characters");
        }
        if self.description.as_ref().is_some_and(|d| d.chars().count() > 1000) {
            return schema("description exceeds 1000 characters");
        }
        if !is_contract(&self.contract) {
            return schema("contract must be fractalengine:plugin@<major>.<minor>.<patch>");
        }
        if self.binding != "petal" {
            return schema("binding must be \"petal\"");
        }
        if let Some(scene) = &self.scene {
            if let Some(write) = &scene.write {
                if has_duplicates(write.iter()) {
                    return schema("scene.write has duplicates");
                }
            }
            if let Some(scope) = &scene.property_scope {
                if scope.len() > 64 || has_duplicates(scope.iter()) {
                    return schema("scene.property_scope must be unique and at most 64");
                }
                if scope.iter().any(|p| !is_property_pattern(p)) {
                    return schema("scene.property_scope has a malformed pattern");
                }
            }
        }
        if let Some(readings) = &self.readings {
            if let SourceClassSet::Many(classes) = &readings.source_class {
                if classes.is_empty() || has_duplicates(classes.iter()) {
                    return schema("readings.source_class list must be unique and nonempty");
                }
            }
            check_metrics(&readings.metrics)?;
        }
        if let Some(tick) = &self.tick {
            if !(1..=3_600_000).contains(&tick.cadence_ms) {
                return schema("tick.cadence_ms must be 1..3600000");
            }
            if tick.max_ops.is_some_and(|v| v > 64) || tick.max_readings.is_some_and(|v| v > 1000) {
                return schema("tick.max_ops <= 64 and tick.max_readings <= 1000");
            }
            if tick.fuel.is_some_and(|v| !(1..=1_000_000_000).contains(&v)) {
                return schema("tick.fuel must be 1..1000000000");
            }
        }
        if self.kv.as_ref().is_some_and(|kv| !(1..=1_048_576).contains(&kv.max_bytes)) {
            return schema("kv.max_bytes must be 1..1048576");
        }
        if self.ingress.len() > 16 || self.egress.len() > 16 || self.ui.len() > 32 {
            return schema("ingress/egress at most 16, ui at most 32");
        }
        for endpoint in &self.ingress {
            check_ingress(endpoint)?;
        }
        for endpoint in &self.egress {
            check_egress(endpoint)?;
        }
        if let Some(bus) = &self.bus {
            check_bus(bus)?;
        }
        for entry in &self.ui {
            check_ui(entry)?;
        }
        Ok(())
    }

    fn check_contract(&self) -> Result<(), ManifestError> {
        let major = self.contract.rsplit('@').next().and_then(|v| v.split('.').next());
        if major != Some("2") {
            return refuse(
                Refusal::UnsupportedContract,
                format!("{} is not fractalengine:plugin@2.x.y", self.contract),
            );
        }
        Ok(())
    }

    fn check_reserved_kind(&self) -> Result<(), ManifestError> {
        if self.kind == ExtensionKind::WebBundle {
            return refuse(Refusal::ReservedKind, "kind web-bundle belongs to the portal program");
        }
        if self.portal.is_some() {
            return refuse(Refusal::ReservedKind, "a portal section requires kind web-bundle");
        }
        Ok(())
    }

    fn check_empty_scopes(&self) -> Result<(), ManifestError> {
        let empty = |what: &str| refuse(Refusal::EmptyScope, format!("{what} is empty; omit it instead"));
        if let Some(scene) = &self.scene {
            if scene.write.as_ref().is_some_and(Vec::is_empty) {
                return empty("scene.write");
            }
            if scene.property_scope.as_ref().is_some_and(Vec::is_empty) {
                return empty("scene.property_scope");
            }
        }
        if self.readings.as_ref().is_some_and(|r| r.metrics.is_empty()) {
            return empty("readings.metrics");
        }
        for endpoint in &self.ingress {
            match endpoint {
                Ingress::HttpServer(e) => {
                    if e.methods.is_empty() {
                        return empty("ingress methods");
                    }
                    if e.content_types.as_ref().is_some_and(Vec::is_empty) {
                        return empty("ingress content_types");
                    }
                    if e.forward_headers.as_ref().is_some_and(Vec::is_empty) {
                        return empty("ingress forward_headers");
                    }
                }
                Ingress::Mqtt(e) if e.topics.is_empty() => return empty("ingress topics"),
                _ => {}
            }
        }
        for endpoint in &self.egress {
            match endpoint {
                Egress::Http(e) if e.methods.is_empty() => return empty("egress methods"),
                Egress::Mqtt(e) if e.topics.is_empty() => return empty("egress topics"),
                _ => {}
            }
        }
        if let Some(bus) = &self.bus {
            if bus.call.as_ref().is_some_and(Vec::is_empty) {
                return empty("bus.call");
            }
            if let Some(streams) = bus.expose.as_ref().and_then(|e| e.streams.as_ref()) {
                if streams.iter().any(|s| s.consumers.as_ref().is_some_and(Vec::is_empty)) {
                    return empty("stream consumers");
                }
            }
            if let Some(consume) = &bus.consume {
                if consume.iter().any(|c| c.metrics.as_ref().is_some_and(Vec::is_empty)) {
                    return empty("consume metrics");
                }
            }
        }
        Ok(())
    }

    fn check_endpoints(&self) -> Result<(), ManifestError> {
        let urls = self
            .ingress
            .iter()
            .filter_map(|e| match e {
                Ingress::WebsocketDial(d) => Some(d.url.as_str()),
                Ingress::Mqtt(m) => Some(m.broker.as_str()),
                _ => None,
            })
            .chain(self.egress.iter().map(|e| match e {
                Egress::Http(h) => h.url.as_str(),
                Egress::Mqtt(m) => m.broker.as_str(),
            }));
        for url in urls {
            let parts = split_url(url).ok_or_else(|| ManifestError {
                reason: Refusal::Schema,
                detail: format!("malformed URL {url}"),
            })?;
            if parts.has_userinfo || parts.has_query {
                return refuse(Refusal::InsecureEndpoint, format!("{url} carries userinfo or a query"));
            }
            let plaintext = matches!(parts.scheme, "http" | "ws" | "mqtt");
            if plaintext && !parts.loopback {
                return refuse(Refusal::InsecureEndpoint, format!("{url} is plaintext to a non-loopback host"));
            }
        }
        Ok(())
    }

    fn check_cadence(&self) -> Result<(), ManifestError> {
        if let Some(tick) = &self.tick {
            if tick.cadence_ms < HOST_CADENCE_FLOOR_MS {
                return refuse(
                    Refusal::CadenceBelowFloor,
                    format!("tick.cadence_ms {} is below the host floor {HOST_CADENCE_FLOOR_MS}", tick.cadence_ms),
                );
            }
        }
        let writes = self
            .scene
            .as_ref()
            .and_then(|s| s.write.as_ref())
            .is_some_and(|w| !w.is_empty());
        if (writes || self.readings.is_some()) && self.tick.is_none() {
            return refuse(Refusal::CadenceRequired, "scene.write and readings are flushed only by a tick");
        }
        Ok(())
    }

    fn check_source_class(&self) -> Result<(), ManifestError> {
        let Some(readings) = &self.readings else {
            return Ok(());
        };
        use ExtensionSourceClass::{Connector, Device, Simulation};
        let allowed: &[ExtensionSourceClass] = match self.kind {
            ExtensionKind::Simulation => &[Simulation],
            ExtensionKind::DeviceAdapter => &[Device, Connector, Simulation],
            ExtensionKind::BiConnector => &[Connector],
            ExtensionKind::General | ExtensionKind::WebBundle => &[Device, Connector, Simulation],
        };
        if let Some(bad) = readings.source_class.classes().into_iter().find(|c| !allowed.contains(c)) {
            return refuse(
                Refusal::SourceClassMismatch,
                format!("{bad:?} is not allowed for kind {:?}", self.kind),
            );
        }
        Ok(())
    }

    fn exposed_command_ids(&self) -> BTreeSet<&str> {
        self.bus
            .iter()
            .filter_map(|b| b.expose.as_ref()?.commands.as_ref())
            .flatten()
            .map(|c| c.id.as_str())
            .collect()
    }

    fn exposed_stream_ids(&self) -> BTreeSet<&str> {
        self.bus
            .iter()
            .filter_map(|b| b.expose.as_ref()?.streams.as_ref())
            .flatten()
            .map(|s| s.id.as_str())
            .collect()
    }

    fn check_ui_references(&self) -> Result<(), ManifestError> {
        let commands = self.exposed_command_ids();
        let streams = self.exposed_stream_ids();
        for entry in &self.ui {
            if let Some(command) = &entry.command {
                if !commands.contains(command.as_str()) {
                    return refuse(Refusal::UndeclaredCommand, format!("ui {} names undeclared command {command}", entry.id));
                }
            }
            if let Some(stream) = &entry.stream {
                if !streams.contains(stream.as_str()) {
                    return refuse(Refusal::UndeclaredStream, format!("ui {} names undeclared stream {stream}", entry.id));
                }
            }
        }
        Ok(())
    }

    fn check_consume(&self) -> Result<(), ManifestError> {
        let streams = self.exposed_stream_ids();
        let own_prefix = format!("{}/", self.id);
        let mut seen = BTreeSet::new();
        for entry in self.bus.iter().filter_map(|b| b.consume.as_ref()).flatten() {
            if let Some(name) = entry.source.strip_prefix("host/") {
                if !matches!(name, "query" | "readings" | "node-changes" | "admission") {
                    return refuse(Refusal::UnknownSource, format!("unknown host source {}", entry.source));
                }
            } else if let Some(stream) = entry.source.strip_prefix(&own_prefix) {
                if !streams.contains(stream) {
                    return refuse(Refusal::UndeclaredStream, format!("consumes undeclared own stream {}", entry.source));
                }
            }
            let key = match (&entry.source[..], &entry.id) {
                ("host/query", Some(id)) => format!("host/query#{id}"),
                (source, _) => source.to_string(),
            };
            if !seen.insert(key) {
                return refuse(Refusal::UnknownSource, format!("{} is consumed twice", entry.source));
            }
            if entry.source == "host/query" {
                if let Some(sql) = entry.sql.as_deref().filter(|s| *s != "*") {
                    if !crate::query::is_select_only(sql) {
                        return refuse(Refusal::InvalidQuery, format!("named query {} fails the SELECT guard", entry.id.as_deref().unwrap_or("?")));
                    }
                }
            }
        }
        Ok(())
    }

    fn check_scope_conflicts(&self) -> Result<(), ManifestError> {
        let scene = self.scene.as_ref();
        let scope = scene.and_then(|s| s.property_scope.as_deref()).unwrap_or(&[]);
        let reads = scene.is_some_and(|s| s.read);
        let writes_property = scene
            .and_then(|s| s.write.as_ref())
            .is_some_and(|w| w.contains(&SceneWrite::SetProperty));
        if writes_property && scope.is_empty() {
            return refuse(Refusal::ScopeConflict, "set_property needs scene.property_scope");
        }
        for entry in &self.ui {
            if entry.slot == UiSlot::InspectorSection {
                if !reads {
                    return refuse(Refusal::ScopeConflict, format!("ui {} is an InspectorSection without scene.read", entry.id));
                }
                for key in entry.properties.iter().flatten() {
                    if !scope_allows(scope, key) {
                        return refuse(Refusal::ScopeConflict, format!("ui {} shows {key} outside property_scope", entry.id));
                    }
                }
            }
        }
        let consumes_changes = self
            .bus
            .iter()
            .filter_map(|b| b.consume.as_ref())
            .flatten()
            .any(|c| c.source == "host/node-changes");
        if consumes_changes && !reads {
            return refuse(Refusal::ScopeConflict, "host/node-changes needs scene.read");
        }
        let auths = self
            .ingress
            .iter()
            .filter_map(|e| match e {
                Ingress::WebsocketDial(d) => d.auth.as_ref(),
                Ingress::Mqtt(m) => m.auth.as_ref(),
                _ => None,
            })
            .chain(self.egress.iter().filter_map(|e| match e {
                Egress::Http(h) => h.auth.as_ref(),
                Egress::Mqtt(m) => m.auth.as_ref(),
            }));
        for auth in auths {
            if auth.scheme == AuthScheme::Header && auth.header.is_none() {
                return refuse(Refusal::ScopeConflict, "auth scheme header needs a header name");
            }
        }
        Ok(())
    }

    fn check_duplicates(&self) -> Result<(), ManifestError> {
        let endpoint_ids = self
            .ingress
            .iter()
            .map(|e| match e {
                Ingress::HttpServer(x) => x.id.as_str(),
                Ingress::WebsocketAccept(x) => x.id.as_str(),
                Ingress::WebsocketDial(x) => x.id.as_str(),
                Ingress::Mqtt(x) => x.id.as_str(),
            })
            .chain(self.egress.iter().map(|e| match e {
                Egress::Http(x) => x.id.as_str(),
                Egress::Mqtt(x) => x.id.as_str(),
            }));
        let expose = self.bus.as_ref().and_then(|b| b.expose.as_ref());
        let stream_ids = expose.and_then(|e| e.streams.as_ref()).into_iter().flatten().map(|s| s.id.as_str());
        let command_ids = expose.and_then(|e| e.commands.as_ref()).into_iter().flatten().map(|c| c.id.as_str());
        let query_ids = self
            .bus
            .iter()
            .filter_map(|b| b.consume.as_ref())
            .flatten()
            .filter(|c| c.source == "host/query")
            .filter_map(|c| c.id.as_deref());
        let ui_ids = self.ui.iter().map(|u| u.id.as_str());
        for (what, ids) in [
            ("endpoint", endpoint_ids.collect::<Vec<_>>()),
            ("stream", stream_ids.collect()),
            ("command", command_ids.collect()),
            ("named query", query_ids.collect()),
            ("ui", ui_ids.collect()),
        ] {
            let mut seen = BTreeSet::new();
            if let Some(dup) = ids.into_iter().find(|id| !seen.insert(*id)) {
                return refuse(Refusal::DuplicateId, format!("duplicate {what} id {dup}"));
            }
        }
        Ok(())
    }

    fn check_ceilings(&self) -> Result<(), ManifestError> {
        if let Some(tick) = &self.tick {
            if tick.fuel.is_some_and(|f| f > HOST_FUEL_CEILING) {
                return refuse(Refusal::OverHostCeiling, format!("tick.fuel exceeds {HOST_FUEL_CEILING}"));
            }
            if tick.max_ops.is_some_and(|v| v > HOST_MAX_OPS_CEILING)
                || tick.max_readings.is_some_and(|v| v > HOST_MAX_READINGS_CEILING)
            {
                return refuse(Refusal::OverHostCeiling, "tick budget exceeds the host ceiling");
            }
        }
        Ok(())
    }
}

/// True when `key` is allowed by an exact key, a `prefix*` or `*` pattern.
pub fn scope_allows(patterns: &[impl AsRef<str>], key: &str) -> bool {
    patterns.iter().any(|pattern| {
        let pattern = pattern.as_ref();
        pattern == "*"
            || pattern.strip_suffix('*').map_or(pattern == key, |prefix| key.starts_with(prefix))
    })
}

/// True when `metric` is listed in `metrics`, or `*` is declared.
pub fn metric_allowed(metrics: &[impl AsRef<str>], metric: &str) -> bool {
    metrics.iter().any(|m| m.as_ref() == "*" || m.as_ref() == metric)
}

/// blake3 hex of the canonical grant projection of the raw manifest (docs/extensions/manifest-v2.md).
pub fn capability_digest(raw: &Value) -> String {
    let mut projection = raw.clone();
    if let Some(object) = projection.as_object_mut() {
        for key in ["version", "name", "description", "tick"] {
            object.remove(key);
        }
        if let Some(expose) = object.get_mut("bus").and_then(|b| b.get_mut("expose")) {
            for group in ["streams", "commands"] {
                for item in expose.get_mut(group).and_then(Value::as_array_mut).into_iter().flatten() {
                    if let Some(item) = item.as_object_mut() {
                        item.remove("description");
                    }
                }
            }
        }
        for item in object.get_mut("ui").and_then(Value::as_array_mut).into_iter().flatten() {
            if let Some(item) = item.as_object_mut() {
                item.remove("label");
                item.remove("icon");
            }
        }
    }
    blake3::hash(canonical_json(&projection).as_bytes()).to_hex().to_string()
}

/// Sorted-keys, no-whitespace JSON; the same rule as fe-format's manifest canonicalizer.
fn canonical_json(value: &Value) -> String {
    match value {
        Value::Array(items) => format!("[{}]", items.iter().map(canonical_json).collect::<Vec<_>>().join(",")),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let entries: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical_json(&map[k])))
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        other => other.to_string(),
    }
}

fn has_duplicates<T: Ord>(items: impl Iterator<Item = T>) -> bool {
    let mut seen = BTreeSet::new();
    items.into_iter().any(|item| !seen.insert(item))
}

fn is_label(label: &str, max: usize) -> bool {
    let mut chars = label.chars();
    label.len() <= max
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_package_id(id: &str) -> bool {
    id.len() <= 128 && id.split('.').count() >= 2 && id.split('.').all(|label| is_label(label, 128))
}

fn is_local_id(id: &str) -> bool {
    is_label(id, 32)
}

fn is_semver(version: &str) -> bool {
    let (rest, build) = version.split_once('+').map_or((version, None), |(r, b)| (r, Some(b)));
    let (core, pre) = rest.split_once('-').map_or((rest, None), |(c, p)| (c, Some(p)));
    let numbers: Vec<&str> = core.split('.').collect();
    let numeric = numbers.len() == 3
        && numbers.iter().all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && (*n == "0" || !n.starts_with('0')));
    let ident = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    version.len() <= 64 && numeric && pre.is_none_or(ident) && build.is_none_or(ident)
}

fn is_contract(contract: &str) -> bool {
    contract.strip_prefix("fractalengine:plugin@").is_some_and(|v| {
        let parts: Vec<&str> = v.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn is_key_chars(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

fn is_property_pattern(pattern: &str) -> bool {
    pattern == "*" || is_key_chars(pattern.strip_suffix('*').unwrap_or(pattern), 255)
}

fn check_metrics(metrics: &[String]) -> Result<(), ManifestError> {
    if metrics.len() > 128 || has_duplicates(metrics.iter()) {
        return schema("metrics must be unique and at most 128");
    }
    if metrics.iter().any(|m| m != "*" && !is_key_chars(m, 128)) {
        return schema("metrics has a malformed name");
    }
    Ok(())
}

struct UrlParts<'a> {
    scheme: &'a str,
    loopback: bool,
    has_userinfo: bool,
    has_query: bool,
}

fn split_url(url: &str) -> Option<UrlParts<'_>> {
    if url.len() > 2048 || url.chars().any(char::is_whitespace) || url.contains('#') {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme, "https" | "http" | "wss" | "ws" | "mqtts" | "mqtt") {
        return None;
    }
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    if authority.is_empty() || authority.contains(['?', '*']) {
        return None;
    }
    let has_userinfo = authority.contains('@');
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = if hostport.starts_with('[') {
        hostport.split_inclusive(']').next().unwrap_or(hostport)
    } else {
        hostport.split(':').next().unwrap_or(hostport)
    };
    Some(UrlParts {
        scheme,
        loopback: matches!(host, "127.0.0.1" | "[::1]" | "localhost"),
        has_userinfo,
        has_query: path.contains('?'),
    })
}

fn check_url(url: &str) -> Result<(), ManifestError> {
    split_url(url).map(|_| ()).ok_or_else(|| ManifestError {
        reason: Refusal::Schema,
        detail: format!("malformed endpoint URL {url}"),
    })
}

fn check_rate(rate: f64, max: f64) -> Result<(), ManifestError> {
    if rate.is_finite() && rate > 0.0 && rate <= max {
        Ok(())
    } else {
        schema(format!("rate_hz must be in (0, {max}]"))
    }
}

fn check_bound(value: u64, min: u64, max: u64, what: &str) -> Result<(), ManifestError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        schema(format!("{what} must be {min}..{max}"))
    }
}

fn check_auth(auth: &Option<AuthSlot>) -> Result<(), ManifestError> {
    let Some(auth) = auth else { return Ok(()) };
    let slot_ok = is_label(&auth.credential, 64);
    let header_ok = auth.header.as_ref().is_none_or(|h| {
        h.len() <= 64
            && h.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            && h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    if slot_ok && header_ok {
        Ok(())
    } else {
        schema("auth credential/header is malformed")
    }
}

fn check_topics(topics: &[String]) -> Result<(), ManifestError> {
    if topics.len() > 16 || has_duplicates(topics.iter()) || topics.iter().any(|t| t.is_empty() || t.len() > 256) {
        return schema("topics must be unique, at most 16, 1..256 bytes each");
    }
    Ok(())
}

fn check_role_list_bounds(items: &Option<Vec<String>>, max: usize, what: &str) -> Result<(), ManifestError> {
    if let Some(items) = items {
        if items.len() > max || has_duplicates(items.iter()) {
            return schema(format!("{what} must be unique and at most {max}"));
        }
    }
    Ok(())
}

fn check_ingress(endpoint: &Ingress) -> Result<(), ManifestError> {
    match endpoint {
        Ingress::HttpServer(e) => {
            check_local(&e.id)?;
            check_bound(e.max_body_bytes, 1, 1_048_576, "max_body_bytes")?;
            check_rate(e.rate_hz, 100.0)?;
            if has_duplicates(e.methods.iter()) {
                return schema("methods has duplicates");
            }
            if e.methods.iter().any(|m| !matches!(m, Method::Get | Method::Post | Method::Put)) {
                return schema("ingress methods are GET, POST or PUT");
            }
            check_role_list_bounds(&e.content_types, 8, "content_types")?;
            if e.content_types.iter().flatten().any(|c| c.len() > 128) {
                return schema("content type exceeds 128 bytes");
            }
            check_role_list_bounds(&e.forward_headers, 8, "forward_headers")?;
            if e.forward_headers.iter().flatten().any(|h| !is_label(h, 64)) {
                return schema("forward_headers must be lowercase header names");
            }
        }
        Ingress::WebsocketAccept(e) => {
            check_local(&e.id)?;
            check_bound(e.max_frame_bytes, 1, 1_048_576, "max_frame_bytes")?;
            check_rate(e.rate_hz, 100.0)?;
            check_bound(u64::from(e.max_connections), 1, 256, "max_connections")?;
        }
        Ingress::WebsocketDial(e) => {
            check_local(&e.id)?;
            check_url(&e.url)?;
            check_bound(e.max_frame_bytes, 1, 1_048_576, "max_frame_bytes")?;
            check_rate(e.rate_hz, 100.0)?;
            check_auth(&e.auth)?;
            if let Some(r) = &e.reconnect_ms {
                check_bound(r.min, 250, 600_000, "reconnect_ms.min")?;
                check_bound(r.max, 250, 600_000, "reconnect_ms.max")?;
            }
        }
        Ingress::Mqtt(e) => {
            check_local(&e.id)?;
            check_url(&e.broker)?;
            check_topics(&e.topics)?;
            check_bound(e.max_payload_bytes, 1, 1_048_576, "max_payload_bytes")?;
            check_rate(e.rate_hz, 100.0)?;
            check_auth(&e.auth)?;
        }
    }
    Ok(())
}

fn check_egress(endpoint: &Egress) -> Result<(), ManifestError> {
    match endpoint {
        Egress::Http(e) => {
            check_local(&e.id)?;
            check_url(&e.url)?;
            if has_duplicates(e.methods.iter()) {
                return schema("methods has duplicates");
            }
            if let Some(v) = e.max_request_bytes {
                check_bound(v, 0, 8_388_608, "max_request_bytes")?;
            }
            if let Some(v) = e.max_response_bytes {
                check_bound(v, 0, 8_388_608, "max_response_bytes")?;
            }
            if let Some(v) = e.timeout_ms {
                check_bound(v, 100, 30_000, "timeout_ms")?;
            }
            check_rate(e.rate_hz, 100.0)?;
            check_auth(&e.auth)?;
        }
        Egress::Mqtt(e) => {
            check_local(&e.id)?;
            check_url(&e.broker)?;
            check_topics(&e.topics)?;
            check_bound(e.max_payload_bytes, 1, 1_048_576, "max_payload_bytes")?;
            check_rate(e.rate_hz, 100.0)?;
            check_auth(&e.auth)?;
        }
    }
    Ok(())
}

fn check_local(id: &str) -> Result<(), ManifestError> {
    if is_local_id(id) {
        Ok(())
    } else {
        schema(format!("id {id} must match [a-z][a-z0-9-]{{0,31}}"))
    }
}

fn is_source(source: &str) -> bool {
    source.len() <= 161
        && source.split_once('/').is_some_and(|(head, tail)| {
            head.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
                && head.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
                && is_local_id(tail)
        })
}

fn check_bus(bus: &BusSection) -> Result<(), ManifestError> {
    if let Some(expose) = &bus.expose {
        if expose.streams.as_ref().is_some_and(|s| s.len() > 16) || expose.commands.as_ref().is_some_and(|c| c.len() > 32) {
            return schema("bus.expose allows 16 streams and 32 commands");
        }
        for stream in expose.streams.iter().flatten() {
            check_local(&stream.id)?;
            check_rate(stream.max_rate_hz, 50.0)?;
            check_bound(stream.max_event_bytes, 1, 65_536, "max_event_bytes")?;
            if stream.description.as_ref().is_some_and(|d| d.chars().count() > 200) {
                return schema("description exceeds 200 characters");
            }
            if let Some(consumers) = &stream.consumers {
                if consumers.len() > 32 || has_duplicates(consumers.iter()) || consumers.iter().any(|c| !is_package_id(c)) {
                    return schema("consumers must be unique package ids, at most 32");
                }
            }
        }
        for command in expose.commands.iter().flatten() {
            check_local(&command.id)?;
            check_rate(command.rate_hz, 10.0)?;
            if command.description.as_ref().is_some_and(|d| d.chars().count() > 200) {
                return schema("description exceeds 200 characters");
            }
            if let Some(args) = &command.args_schema {
                check_args_schema(args)?;
            }
        }
    }
    if let Some(consume) = &bus.consume {
        if consume.len() > 16 {
            return schema("bus.consume allows 16 entries");
        }
        for entry in consume {
            check_consume_shape(entry)?;
        }
    }
    if let Some(calls) = &bus.call {
        let valid = |c: &String| c.len() <= 161 && c.split_once('/').is_some_and(|(p, cmd)| is_package_id(p) && is_local_id(cmd));
        if calls.len() > 16 || has_duplicates(calls.iter()) || calls.iter().any(|c| !valid(c)) {
            return schema("bus.call must be unique <package_id>/<command>, at most 16");
        }
    }
    Ok(())
}

fn check_consume_shape(entry: &ConsumeEntry) -> Result<(), ManifestError> {
    if !is_source(&entry.source) {
        return schema(format!("consume source {} is malformed", entry.source));
    }
    let query_fields = entry.id.is_some() || entry.sql.is_some() || entry.min_interval_ms.is_some();
    match entry.source.as_str() {
        "host/query" => {
            let (Some(id), Some(sql)) = (&entry.id, &entry.sql) else {
                return schema("host/query needs id and sql");
            };
            check_local(id)?;
            if sql.is_empty() || sql.len() > 8192 || entry.metrics.is_some() {
                return schema("host/query sql must be 1..8192 bytes and takes no metrics");
            }
            if let Some(v) = entry.min_interval_ms {
                check_bound(v, 1000, 3_600_000, "min_interval_ms")?;
            }
        }
        "host/readings" => {
            if query_fields {
                return schema("host/readings takes only metrics");
            }
            if let Some(metrics) = &entry.metrics {
                check_metrics(metrics)?;
            }
        }
        _ => {
            if query_fields || entry.metrics.is_some() {
                return schema("this source takes no extra members");
            }
        }
    }
    Ok(())
}

/// Keywords of the bus.md `args_schema` subset.
const ARGS_KEYWORDS: [&str; 12] = [
    "type", "properties", "required", "additionalProperties", "enum", "const", "minimum", "maximum",
    "minLength", "maxLength", "items", "description",
];

fn check_args_schema(schema_value: &Value) -> Result<(), ManifestError> {
    if serde_json::to_vec(schema_value).map_or(true, |bytes| bytes.len() > MAX_ARGS_SCHEMA_BYTES) {
        return schema("args_schema exceeds 4 KiB");
    }
    check_args_node(schema_value, 1)
}

fn check_args_node(node: &Value, depth: usize) -> Result<(), ManifestError> {
    let Some(object) = node.as_object() else {
        return schema("args_schema nodes must be objects");
    };
    if depth > 4 {
        return schema("args_schema nests deeper than 4");
    }
    for (key, value) in object {
        match key.as_str() {
            "minItems" | "maxItems" => {}
            k if ARGS_KEYWORDS.contains(&k) => {}
            other => return schema(format!("args_schema keyword {other} is outside the subset")),
        }
        match key.as_str() {
            "additionalProperties" if !value.is_boolean() => return schema("additionalProperties must be boolean"),
            "properties" => {
                let Some(properties) = value.as_object() else {
                    return schema("properties must be an object");
                };
                for child in properties.values() {
                    check_args_node(child, depth + 1)?;
                }
            }
            "items" => check_args_node(value, depth + 1)?,
            _ => {}
        }
    }
    Ok(())
}

fn check_ui(entry: &UiEntry) -> Result<(), ManifestError> {
    check_local(&entry.id)?;
    if entry.label.is_empty() || entry.label.chars().count() > 48 {
        return schema("ui label must be 1..48 characters");
    }
    if entry.icon.as_ref().is_some_and(|i| !is_label(i, 32)) {
        return schema("ui icon must be a lowercase name");
    }
    for id in [&entry.command, &entry.stream].into_iter().flatten() {
        check_local(id)?;
    }
    if let Some(properties) = &entry.properties {
        if properties.is_empty() || properties.len() > 16 || has_duplicates(properties.iter()) || properties.iter().any(|p| !is_key_chars(p, 255)) {
            return schema("ui properties must be 1..16 unique property keys");
        }
    }
    if let Some(field) = &entry.field {
        if field.len() > 128 || !(field.is_empty() || field.starts_with('/')) {
            return schema("ui field must be a JSON Pointer of at most 128 bytes");
        }
    }
    if entry.when.as_ref().is_some_and(|w| !is_key_chars(&w.property, 255)) {
        return schema("ui when.property is malformed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLEET: &str = include_str!("../schema/examples/fleet-sim.extension.json");
    const DEVICE: &str = include_str!("../schema/examples/device-adapter.extension.json");
    const BI: &str = include_str!("../schema/examples/bi-push.extension.json");

    fn with(base: &str, edit: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut value: Value = serde_json::from_str(base).unwrap();
        edit(&mut value);
        serde_json::to_vec(&value).unwrap()
    }

    /// Journey J2: the three shipped examples pass, then one change per row yields its reason.
    #[test]
    fn examples_pass_and_each_edit_gets_its_reason() {
        for example in [FLEET, DEVICE, BI] {
            let ok = parse_and_validate(example.as_bytes()).expect("example validates");
            assert_eq!(ok.capability_digest.len(), 64);
        }
        let rows: Vec<(&str, Vec<u8>, Option<Refusal>)> = vec![
            ("empty scope", with(FLEET, |m| m["scene"]["property_scope"] = serde_json::json!([])), Some(Refusal::EmptyScope)),
            ("plain http egress", with(FLEET, |m| m["egress"] = serde_json::json!([{"id":"sink","transport":"http","url":"http://bi.example.org/","methods":["POST"],"rate_hz":1}])), Some(Refusal::InsecureEndpoint)),
            ("loopback http", with(FLEET, |m| m["egress"] = serde_json::json!([{"id":"sink","transport":"http","url":"http://127.0.0.1:9000/","methods":["POST"],"rate_hz":1}])), None),
            ("sub-floor cadence", with(FLEET, |m| m["tick"]["cadence_ms"] = serde_json::json!(50)), Some(Refusal::CadenceBelowFloor)),
            ("simulation as device", with(FLEET, |m| m["readings"]["source_class"] = serde_json::json!("device")), Some(Refusal::SourceClassMismatch)),
            ("undeclared command", with(FLEET, |m| m["ui"][0]["command"] = serde_json::json!("launch")), Some(Refusal::UndeclaredCommand)),
            ("undeclared own stream", with(FLEET, |m| m["bus"]["consume"] = serde_json::json!([{"source":"org.example.fleet-sim/tracks"}])), Some(Refusal::UndeclaredStream)),
            ("old contract", with(FLEET, |m| m["contract"] = serde_json::json!("fractalengine:plugin@1.0.0")), Some(Refusal::UnsupportedContract)),
            ("unknown field", with(FLEET, |m| m["surprise"] = serde_json::json!(true)), Some(Refusal::Schema)),
            ("web bundle", with(FLEET, |m| m["kind"] = serde_json::json!("web-bundle")), Some(Refusal::ReservedKind)),
            ("writes without tick", with(FLEET, |m| { m.as_object_mut().unwrap().remove("tick"); }), Some(Refusal::CadenceRequired)),
            ("fuel over ceiling", with(FLEET, |m| m["tick"]["fuel"] = serde_json::json!(30_000_000u64)), Some(Refusal::OverHostCeiling)),
            ("unknown host source", with(FLEET, |m| m["bus"]["consume"] = serde_json::json!([{"source":"host/secrets"}])), Some(Refusal::UnknownSource)),
            ("duplicate ui id", with(FLEET, |m| m["ui"][1]["id"] = serde_json::json!("start")), Some(Refusal::DuplicateId)),
            ("inspector key outside scope", with(FLEET, |m| m["ui"][3]["properties"] = serde_json::json!(["other.key"])), Some(Refusal::ScopeConflict)),
            ("mutating named query", with(BI, |m| m["bus"]["consume"][0]["sql"] = serde_json::json!("DELETE FROM iot_reading")), Some(Refusal::InvalidQuery)),
        ];
        for (row, bytes, expected) in rows {
            let got = parse_and_validate(&bytes).err().map(|e| e.reason);
            assert_eq!(got, expected, "row {row}");
        }
    }

    #[test]
    fn digest_ignores_presentation_and_tracks_grants() {
        let base = parse_and_validate(FLEET.as_bytes()).unwrap().capability_digest;
        let relabelled = with(FLEET, |m| {
            m["name"] = serde_json::json!("Renamed");
            m["version"] = serde_json::json!("0.2.0");
            m["ui"][0]["label"] = serde_json::json!("Go");
        });
        assert_eq!(parse_and_validate(&relabelled).unwrap().capability_digest, base);
        let widened = with(FLEET, |m| m["scene"]["property_scope"] = serde_json::json!(["*"]));
        assert_ne!(parse_and_validate(&widened).unwrap().capability_digest, base);
    }
}
