//! Trusted plugin admission and direct typed DB replies; see src/AGENTS.md.

use fe_sdk::manifest_v2::SceneWrite;
use fe_sdk::plugin_host::{NodeQuery, TransactionReceipt, MAX_METRIC_LEN};
use fe_sdk::source_class::SourceClass;
use fe_sdk::NodeSnapshot;

/// Host-minted capability binding; never deserialized from plugin request data.
#[derive(Debug, Clone)]
pub struct PluginAdmission {
    plugin_id: String,
    petal_id: String,
    capabilities: Vec<String>,
    property_scope: Vec<String>,
    verse_scope: Vec<String>,
    petal_scope: Vec<String>,
    scope: Option<String>,
    lease: fe_sdk::plugin_host::PluginLease,
    package: Option<PackageIdentity>,
    readings: Option<ReadingsAdmission>,
    scene_writes: Option<Vec<SceneWrite>>,
}

impl PluginAdmission {
    /// Bind the manifest admitted by the trusted host to one petal.
    pub fn new(
        plugin_id: String,
        petal_id: String,
        capabilities: Vec<String>,
        property_scope: Vec<String>,
        verse_scope: Vec<String>,
        petal_scope: Vec<String>,
    ) -> Result<Self, String> {
        if plugin_id.is_empty()
            || plugin_id.len() > 256
            || petal_id.is_empty()
            || petal_id.len() > 256
        {
            return Err("invalid plugin admission identity".into());
        }
        if [&capabilities, &property_scope, &verse_scope, &petal_scope]
            .iter()
            .any(|list| list.len() > 64 || list.iter().any(|item| item.len() > 1024))
        {
            return Err("plugin admission exceeds manifest bounds".into());
        }
        Ok(Self {
            plugin_id,
            petal_id,
            capabilities,
            property_scope,
            verse_scope,
            petal_scope,
            scope: None,
            lease: fe_sdk::plugin_host::PluginLease::default(),
            package: None,
            readings: None,
            scene_writes: None,
        })
    }
    /// Attach the admitted context's shared lease before sending DB requests.
    pub fn with_lease(mut self, lease: fe_sdk::plugin_host::PluginLease) -> Self {
        self.lease = lease;
        self
    }
    /// Mark a sealed-package (manifest v2) admission; its empty property scope grants nothing (R3).
    pub fn with_package(mut self, package: PackageIdentity) -> Self {
        self.package = Some(package);
        self
    }
    /// Bind the manifest's readings grant with the one source class the operator picked.
    pub fn with_readings(mut self, readings: ReadingsAdmission) -> Self {
        self.readings = Some(readings);
        self
    }
    /// Narrow staged scene writes to the manifest's declared `scene.write` kinds.
    pub fn with_scene_writes(mut self, kinds: Vec<SceneWrite>) -> Result<Self, String> {
        if kinds.len() > 4 {
            return Err("plugin admission declares more than four write kinds".into());
        }
        self.scene_writes = Some(kinds);
        Ok(self)
    }
    /// Check revocation at DB use and before releasing read payloads.
    pub fn require_active(&self) -> Result<(), String> {
        self.lease.require_active()
    }

    /// Admitted plugin identity, for attribution rather than user-supplied authority.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
    /// Exact petal bound by the host.
    pub fn petal_id(&self) -> &str {
        &self.petal_id
    }
    /// The sealed package this instance runs, absent for first-party and 1.0 contexts.
    pub fn package(&self) -> Option<&PackageIdentity> {
        self.package.as_ref()
    }
    /// The bound readings grant, absent when the manifest declares no readings.
    pub fn readings(&self) -> Option<&ReadingsAdmission> {
        self.readings.as_ref()
    }
    /// Bind the admitted instance to its DB-resolved full ancestry.
    pub fn bind_scope(mut self, scope: String) -> Result<Self, String> {
        if scope.len() > 4096 {
            return Err("plugin scope exceeds 4096 bytes".into());
        }
        self.scope = Some(scope);
        Ok(self)
    }
    /// The pinned ancestry, absent only during the initial scope handshake.
    pub fn bound_scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }
    /// Whether a named capability was admitted.
    pub fn allows(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|item| item == capability)
    }
    /// Whether one staged write kind is admitted: listed kinds for a package, all kinds for 1.0.
    pub fn allows_write(&self, kind: SceneWrite) -> bool {
        self.allows(fe_sdk::CAP_STORAGE_WRITE)
            && match &self.scene_writes {
                Some(kinds) => kinds.contains(&kind),
                None => self.package.is_none(),
            }
    }
    /// Check the admitted property patterns.
    pub fn allows_property(&self, key: &str) -> bool {
        patterns_allow(&self.property_scope, key, self.package.is_none())
    }
    /// Check admitted verse and full petal ancestry patterns.
    pub fn allows_scope(&self, verse: &str, scope: &str) -> bool {
        patterns_allow(&self.verse_scope, verse, true) && patterns_allow(&self.petal_scope, scope, true)
    }
}

fn patterns_allow(patterns: &[String], value: &str, empty_grants_all: bool) -> bool {
    if patterns.is_empty() {
        return empty_grants_all;
    }
    patterns
        .iter()
        .any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => value.starts_with(prefix),
            None => pattern == value,
        })
}

/// A verified sealed package's identity; the provenance key `{package_id, content_hash}` (R24).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageIdentity {
    package_id: String,
    content_hash: String,
}

impl PackageIdentity {
    /// Bind a reverse-DNS package id and its lowercase-hex blake3 `content_hash`.
    pub fn new(package_id: String, content_hash: String) -> Result<Self, String> {
        let id_ok = (3..=128).contains(&package_id.len())
            && package_id.contains('.')
            && package_id.split('.').all(|label| {
                label.starts_with(|c: char| c.is_ascii_lowercase())
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            });
        let hash_ok = content_hash.len() == 64
            && content_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !id_ok || !hash_ok {
            return Err("invalid admitted package identity".into());
        }
        Ok(Self {
            package_id,
            content_hash,
        })
    }
    /// Reverse-DNS package id (the sealed hexon id).
    pub fn package_id(&self) -> &str {
        &self.package_id
    }
    /// Lowercase-hex blake3 over the signed manifest bytes.
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }
}

/// A manifest readings grant bound at admission; the class is chosen, never inferred (R6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingsAdmission {
    source_class: SourceClass,
    metrics: Vec<String>,
}

impl ReadingsAdmission {
    /// Bind one extension source class and the declared metric names (or `*`).
    pub fn new(source_class: SourceClass, metrics: Vec<String>) -> Result<Self, String> {
        if !matches!(
            source_class,
            SourceClass::Simulation | SourceClass::Device | SourceClass::Connector
        ) {
            return Err("extension readings must be simulation, device or connector".into());
        }
        if metrics.is_empty()
            || metrics.len() > 128
            || metrics
                .iter()
                .any(|metric| metric.is_empty() || metric.len() > MAX_METRIC_LEN)
        {
            return Err("readings grant must declare 1..128 metric names".into());
        }
        Ok(Self {
            source_class,
            metrics,
        })
    }
    /// The class every reading from this instance is written as.
    pub fn source_class(&self) -> SourceClass {
        self.source_class
    }
    /// Whether a staged metric is declared.
    pub fn allows_metric(&self, metric: &str) -> bool {
        fe_sdk::manifest_v2::metric_allowed(&self.metrics, metric)
    }
}

/// Structured plugin reads; no caller-authored SQL crosses this channel.
#[derive(Debug, Clone)]
pub enum PluginRead {
    /// Current stored full petal ancestry.
    PetalScope,
    /// One live node in the admitted petal.
    Node(String),
    /// One bounded structured node page.
    Nodes(NodeQuery),
}

/// A direct reply belongs only to the request's embedded sender.
#[derive(Debug, Clone)]
pub enum PluginReply {
    /// Stored petal ancestry.
    Scope(String),
    /// Current node, absent uniformly for foreign or missing targets.
    Node(Option<NodeSnapshot>),
    /// Bounded current nodes.
    Nodes(Vec<NodeSnapshot>),
    /// An atomic persisted flush: its scene batch and the readings that landed with it.
    Committed(TransactionReceipt),
}

/// Why the DB refused or could not confirm a plugin request; mirrors WIT `host-error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRefusal {
    /// The admission does not grant it; nothing landed.
    CapabilityDenied(String),
    /// An op or reading failed host-boundary validation, e.g. an unknown anchor; nothing landed.
    InvalidInput(String),
    /// The lease, the local role floor or the pinned ancestry no longer holds; nothing landed.
    Revoked(String),
    /// The store refused or a commit fence tripped; nothing landed.
    Backend(String),
    /// The scene batch committed but its readings did not all land; never retried.
    UnknownOutcome(String),
}

impl PluginRefusal {
    /// The WIT `host-error` case name.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CapabilityDenied(_) => "capability-denied",
            Self::InvalidInput(_) => "invalid-input",
            Self::Revoked(_) => "revoked",
            Self::Backend(_) => "backend",
            Self::UnknownOutcome(_) => "unknown-outcome",
        }
    }
    /// The human-readable reason.
    pub fn detail(&self) -> &str {
        match self {
            Self::CapabilityDenied(detail)
            | Self::InvalidInput(detail)
            | Self::Revoked(detail)
            | Self::Backend(detail)
            | Self::UnknownOutcome(detail) => detail,
        }
    }
}

impl std::fmt::Display for PluginRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind(), self.detail())
    }
}

impl std::error::Error for PluginRefusal {}

impl From<PluginRefusal> for String {
    fn from(refusal: PluginRefusal) -> Self {
        refusal.to_string()
    }
}

/// The typed reply every plugin DB request answers on its embedded sender.
pub type PluginResult = Result<PluginReply, PluginRefusal>;
