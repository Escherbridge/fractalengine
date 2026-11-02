//! Trusted plugin admission and direct typed DB replies; see src/AGENTS.md.

use fe_sdk::plugin_host::{NodeQuery, TransactionReceipt};
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
        })
    }
    /// Admitted plugin identity, for attribution rather than user-supplied authority.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
    /// Exact petal bound by the host.
    pub fn petal_id(&self) -> &str {
        &self.petal_id
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
    /// Check the admitted property patterns.
    pub fn allows_property(&self, key: &str) -> bool {
        patterns_allow(&self.property_scope, key)
    }
    /// Check admitted verse and full petal ancestry patterns.
    pub fn allows_scope(&self, verse: &str, scope: &str) -> bool {
        patterns_allow(&self.verse_scope, verse) && patterns_allow(&self.petal_scope, scope)
    }
}

fn patterns_allow(patterns: &[String], value: &str) -> bool {
    patterns.is_empty()
        || patterns
            .iter()
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => value.starts_with(prefix),
                None => pattern == value,
            })
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
    /// An atomic persisted batch.
    Committed(TransactionReceipt),
}
