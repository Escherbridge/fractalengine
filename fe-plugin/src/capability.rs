//! Capability manifest and token system for plugin sandboxing.
//!
//! Each plugin declares a [`CapabilityManifest`] that specifies which verses,
//! petals, properties, and network resources it may access. At activation time
//! the host mints a [`CapabilityToken`] from the manifest. All runtime
//! operations are validated against the token before execution.

use fe_sdk::manifest_v2::{
    metric_allowed, ExtensionManifest, ExtensionSourceClass, SceneWrite, SourceClassSet,
};
use serde::{Deserialize, Serialize};

use crate::tick::TickBudget;

// ---------------------------------------------------------------------------
// Capability manifest (declared by the plugin author)
// ---------------------------------------------------------------------------

/// Declares the resources a plugin is allowed to access.
///
/// Parsed from a JSON manifest embedded in or alongside the plugin script.
/// The host validates the manifest against site policy before activation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// Verse scope patterns the plugin may access (e.g. `["VERSE#*"]`).
    #[serde(default)]
    pub verse_scope: Vec<String>,
    /// Petal scope patterns (e.g. `["VERSE#v1-FRACTAL#f1-PETAL#p1"]`).
    #[serde(default)]
    pub petal_scope: Vec<String>,
    /// Property key patterns the plugin may read/write (e.g. `["color", "health"]`).
    #[serde(default)]
    pub property_scope: Vec<String>,
    /// Named capability grants (e.g. `["storage.read", "storage.write",
    /// "query.select"]`). Absent = denied (fail closed). See the `fe_sdk`
    /// `CAP_*` constants for the canonical names.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Network access level; v1 grants only `"none"` (OD-1: [`CapabilityManifest::from_json`] rejects the rest).
    #[serde(default = "default_network_scope")]
    pub network_scope: String,
    /// Outbound HTTP request; v1 has no grant path, so `true` is rejected at parse and always denied at use.
    #[serde(default)]
    pub external_http: bool,
}

fn default_network_scope() -> String {
    "none".to_string()
}

impl CapabilityManifest {
    /// Parse a manifest from a JSON string, refusing any network grant v1 cannot honour (fe-plugin/src/AGENTS.md §network-deny).
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let manifest: Self = serde_json::from_str(json)?;
        if manifest.network_scope != "none" || manifest.external_http {
            return Err(serde::de::Error::custom(format!(
                "network access is not grantable in v1 (network_scope={:?}, external_http={})",
                manifest.network_scope, manifest.external_http
            )));
        }
        Ok(manifest)
    }
}

// ---------------------------------------------------------------------------
// Capability token (minted by the host at activation)
// ---------------------------------------------------------------------------

/// An opaque, non-forgeable token derived from a validated [`CapabilityManifest`].
///
/// In Phase 9A.1 we store the manifest claims directly. Phase 9A.2 will
/// upgrade this to a signed JWT so Wasm plugins cannot tamper with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    /// Plugin ID this token was minted for.
    pub plugin_id: String,
    /// Allowed verse scope patterns.
    pub verse_scope: Vec<String>,
    /// Allowed petal scope patterns.
    pub petal_scope: Vec<String>,
    /// Allowed property key patterns.
    pub property_scope: Vec<String>,
    /// Named capability grants (see [`CapabilityManifest::capabilities`]).
    pub capabilities: Vec<String>,
    /// Network scope level.
    pub network_scope: String,
    /// Whether outbound HTTP is allowed.
    pub external_http: bool,
    /// v2 tokens: an empty scope list grants nothing (v1 treats empty as a wildcard).
    #[serde(default)]
    pub strict_scopes: bool,
    /// Typed v2 grants; `None` on a v1 token.
    #[serde(default)]
    pub grants_v2: Option<V2Grants>,
}

/// Grants a manifest v2 adds on top of the v1 token fields (see src/AGENTS.md §tick-scheduler).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct V2Grants {
    /// Package id from the manifest.
    pub package_id: String,
    /// `scene.read`.
    pub scene_read: bool,
    /// Staged write kinds from `scene.write`.
    pub scene_write: Vec<SceneWrite>,
    /// Metrics `readings.stage` accepts, or `["*"]`; empty = no readings grant.
    pub metrics: Vec<String>,
    /// The ONE source class readings are written as (never inferred, never guest input).
    pub readings_class: Option<ExtensionSourceClass>,
    /// `routes` grant.
    pub routes: bool,
    /// Effective tick budget; `None` = no ticks and no flushes.
    pub tick: Option<TickBudget>,
}

/// The bind-time facts a v2 token needs beyond the manifest itself.
#[derive(Debug, Clone)]
pub struct V2Binding {
    /// Scope pattern of the bound verse (e.g. `VERSE#v1`).
    pub verse_scope: String,
    /// Full ancestry scope of the bound petal.
    pub petal_scope: String,
    /// The operator's pick when the manifest declares a class set.
    pub source_class: Option<ExtensionSourceClass>,
}

impl CapabilityToken {
    /// Mint a v2 token: scopes come from the binding, an empty property scope grants no property.
    pub fn mint_v2(
        plugin_id: &str,
        manifest: &ExtensionManifest,
        binding: &V2Binding,
    ) -> Result<Self, String> {
        let readings_class = match &manifest.readings {
            None => None,
            Some(grant) => {
                let classes = grant.source_class.classes();
                match (binding.source_class, classes.as_slice()) {
                    (Some(pick), set) if set.contains(&pick) => Some(pick),
                    (Some(_), _) => return Err("source class pick is outside the declared set".into()),
                    (None, [only]) if matches!(grant.source_class, SourceClassSet::One(_)) => Some(*only),
                    (None, _) => return Err("a source class set needs the operator's pick at bind time".into()),
                }
            }
        };
        let scene = manifest.scene.clone().unwrap_or_default();
        let scene_write = scene.write.clone().unwrap_or_default();
        let mut capabilities = Vec::new();
        if scene.read {
            capabilities.push(fe_sdk::CAP_STORAGE_READ.to_string());
            capabilities.push(fe_sdk::CAP_QUERY_SELECT.to_string());
        }
        if !scene_write.is_empty() {
            // Writes authorise the petal through the reader, so they need read access to it.
            for name in [fe_sdk::CAP_STORAGE_READ, fe_sdk::CAP_STORAGE_WRITE] {
                if !capabilities.iter().any(|c| c == name) {
                    capabilities.push(name.to_string());
                }
            }
        }
        Ok(Self {
            plugin_id: plugin_id.to_string(),
            verse_scope: vec![binding.verse_scope.clone()],
            petal_scope: vec![binding.petal_scope.clone()],
            property_scope: scene.property_scope.clone().unwrap_or_default(),
            capabilities,
            network_scope: default_network_scope(),
            external_http: false,
            strict_scopes: true,
            grants_v2: Some(V2Grants {
                package_id: manifest.id.clone(),
                scene_read: scene.read,
                scene_write,
                metrics: manifest.readings.as_ref().map(|r| r.metrics.clone()).unwrap_or_default(),
                readings_class,
                routes: manifest.routes.unwrap_or(false),
                tick: manifest.tick.as_ref().map(|budget| crate::tick::TickBudget::from_manifest(budget, manifest)),
            }),
        })
    }

    /// Whether a v2 token allows staging this scene write kind; v1 tokens defer to `storage.write`.
    pub fn allows_scene_write(&self, kind: SceneWrite) -> bool {
        match &self.grants_v2 {
            Some(grants) => grants.scene_write.contains(&kind),
            None => self.has_capability(fe_sdk::CAP_STORAGE_WRITE),
        }
    }

    /// Whether a v2 token may stage a reading of `metric`; v1 tokens have no readings path.
    pub fn allows_reading(&self, metric: &str) -> bool {
        self.grants_v2.as_ref().is_some_and(|grants| {
            grants.readings_class.is_some() && metric_allowed(&grants.metrics, metric)
        })
    }

    /// The v2 effective tick budget, if the manifest declared one.
    pub fn tick_budget(&self) -> Option<&crate::tick::TickBudget> {
        self.grants_v2.as_ref().and_then(|grants| grants.tick.as_ref())
    }

    /// Mint a token from a validated manifest.
    pub fn mint(plugin_id: &str, manifest: &CapabilityManifest) -> Self {
        Self {
            plugin_id: plugin_id.to_string(),
            verse_scope: manifest.verse_scope.clone(),
            petal_scope: manifest.petal_scope.clone(),
            property_scope: manifest.property_scope.clone(),
            capabilities: manifest.capabilities.clone(),
            network_scope: manifest.network_scope.clone(),
            external_http: manifest.external_http,
            strict_scopes: false,
            grants_v2: None,
        }
    }

    /// Whether this token grants a named capability. Fail closed: an absent
    /// grant returns `false` so the host never registers/executes the gated fn.
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }

    /// Bridge to the shared policy engine (see fe-plugin/src/AGENTS.md §capability-policy).
    pub fn to_auth_context(&self) -> fe_policy::AuthContext {
        fe_policy::AuthContext::Capability {
            plugin_id: self.plugin_id.clone(),
            capabilities: self.capabilities.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Access validation
// ---------------------------------------------------------------------------

/// Operations that can be checked against a capability token.
#[derive(Debug, Clone)]
pub enum CapabilityOperation {
    /// Read/write a node property.
    PropertyAccess { key: String },
    /// Access a petal scope.
    PetalAccess { scope: String },
    /// Access a verse scope.
    VerseAccess { scope: String },
    /// Outbound HTTP request.
    HttpRequest,
}

/// Validate whether a capability token permits the given operation.
///
/// Returns `Ok(())` if allowed, or an error describing the denial.
pub fn validate_access(
    token: &CapabilityToken,
    operation: &CapabilityOperation,
) -> Result<(), CapabilityError> {
    match operation {
        CapabilityOperation::PropertyAccess { key } => {
            if token.property_scope.is_empty() && !token.strict_scopes {
                // v1: empty = wildcard (all properties allowed); v2: empty grants nothing
                return Ok(());
            }
            if token.property_scope.iter().any(|p| pattern_matches(p, key)) {
                Ok(())
            } else {
                Err(CapabilityError::PropertyDenied {
                    plugin_id: token.plugin_id.clone(),
                    key: key.clone(),
                })
            }
        }
        CapabilityOperation::PetalAccess { scope } => {
            if token.petal_scope.is_empty() && !token.strict_scopes {
                return Ok(());
            }
            if token.petal_scope.iter().any(|p| pattern_matches(p, scope)) {
                Ok(())
            } else {
                Err(CapabilityError::ScopeDenied {
                    plugin_id: token.plugin_id.clone(),
                    scope: scope.clone(),
                })
            }
        }
        CapabilityOperation::VerseAccess { scope } => {
            if token.verse_scope.is_empty() && !token.strict_scopes {
                return Ok(());
            }
            if token.verse_scope.iter().any(|p| pattern_matches(p, scope)) {
                Ok(())
            } else {
                Err(CapabilityError::ScopeDenied {
                    plugin_id: token.plugin_id.clone(),
                    scope: scope.clone(),
                })
            }
        }
        // Deny-by-default with no grant path (OD-1): token network fields never widen this.
        CapabilityOperation::HttpRequest => Err(CapabilityError::HttpDenied {
            plugin_id: token.plugin_id.clone(),
        }),
    }
}

/// Simple pattern matching: `*` matches everything, otherwise exact match.
fn pattern_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value
    }
}

/// Errors returned when a capability check fails.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CapabilityError {
    #[error("Plugin '{plugin_id}' denied access to property '{key}'")]
    PropertyDenied { plugin_id: String, key: String },
    #[error("Plugin '{plugin_id}' denied access to scope '{scope}'")]
    ScopeDenied { plugin_id: String, scope: String },
    #[error("Plugin '{plugin_id}' denied outbound HTTP access")]
    HttpDenied { plugin_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_manifest_from_json() {
        let json = r#"{
            "verse_scope": ["VERSE#*"],
            "petal_scope": ["VERSE#v1-FRACTAL#f1-PETAL#p1"],
            "property_scope": ["color", "health"],
            "network_scope": "none",
            "external_http": false
        }"#;
        let manifest = CapabilityManifest::from_json(json).unwrap();
        assert_eq!(manifest.verse_scope, vec!["VERSE#*"]);
        assert_eq!(manifest.petal_scope.len(), 1);
        assert_eq!(manifest.property_scope, vec!["color", "health"]);
        assert!(!manifest.external_http);
    }

    #[test]
    fn parse_manifest_defaults() {
        let json = "{}";
        let manifest = CapabilityManifest::from_json(json).unwrap();
        assert!(manifest.verse_scope.is_empty());
        assert_eq!(manifest.network_scope, "none");
        assert!(!manifest.external_http);

        // v2 table: the shipped fleet manifest mints a strict token; an empty scope grants nothing.
        const FLEET: &str = include_str!("../../fe-sdk/schema/examples/fleet-sim.extension.json");
        let validated = fe_sdk::manifest_v2::parse_and_validate(FLEET.as_bytes()).unwrap();
        let binding = V2Binding { verse_scope: "VERSE#v1".into(), petal_scope: "VERSE#v1-FRACTAL#f1-PETAL#p1".into(), source_class: None };
        let token = CapabilityToken::mint_v2("fleet", &validated.manifest, &binding).unwrap();
        let key = |k: &str| CapabilityOperation::PropertyAccess { key: k.into() };
        assert!(validate_access(&token, &key("fleet.speed")).is_ok());
        assert!(validate_access(&token, &key("other")).is_err());
        assert!(token.allows_scene_write(SceneWrite::SetTransform) && !token.allows_scene_write(SceneWrite::DeleteNode));
        assert!(token.allows_reading("altitude_m") && !token.allows_reading("pressure"));
        assert_eq!(token.tick_budget().map(|b| (b.cadence_ms, b.max_readings, b.fuel)), Some((500, 256, 200_000)));
        let mut bare = validated.manifest.clone();
        bare.scene = None;
        bare.readings = None;
        bare.tick = None;
        let nothing = CapabilityToken::mint_v2("bare", &bare, &binding).unwrap();
        assert!(validate_access(&nothing, &key("anything")).is_err(), "no property_scope grants no property");
        assert!(!nothing.has_capability(fe_sdk::CAP_STORAGE_READ) && !nothing.allows_scene_write(SceneWrite::CreateNode));
        let mut pick = validated.manifest.clone();
        pick.readings.as_mut().unwrap().source_class = SourceClassSet::Many(vec![ExtensionSourceClass::Device, ExtensionSourceClass::Connector]);
        assert!(CapabilityToken::mint_v2("p", &pick, &binding).is_err(), "a class set needs the operator's pick");
        let picked = V2Binding { source_class: Some(ExtensionSourceClass::Device), ..binding.clone() };
        assert!(CapabilityToken::mint_v2("p", &pick, &picked).is_ok());
        let outside = V2Binding { source_class: Some(ExtensionSourceClass::Simulation), ..binding };
        assert!(CapabilityToken::mint_v2("p", &pick, &outside).is_err());
    }

    #[test]
    fn validate_property_access_allowed() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"property_scope": ["color", "health"]}"#).unwrap(),
        );
        assert!(validate_access(
            &token,
            &CapabilityOperation::PropertyAccess {
                key: "color".into()
            }
        )
        .is_ok());
    }

    #[test]
    fn validate_property_access_denied() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"property_scope": ["color"]}"#).unwrap(),
        );
        assert!(validate_access(
            &token,
            &CapabilityOperation::PropertyAccess {
                key: "secret".into()
            }
        )
        .is_err());
    }

    #[test]
    fn validate_wildcard_scope() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"verse_scope": ["VERSE#*"]}"#).unwrap(),
        );
        assert!(validate_access(
            &token,
            &CapabilityOperation::VerseAccess {
                scope: "VERSE#abc".into()
            }
        )
        .is_ok());
    }

    #[test]
    fn validate_http_denied() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"external_http": false}"#).unwrap(),
        );
        assert!(validate_access(&token, &CapabilityOperation::HttpRequest).is_err());
    }

    #[test]
    fn network_scope_table_allow_or_deny_for_none_local_internet() {
        // (network_scope, external_http): the manifest parse refuses every grant, and a
        // hand-built manifest that smuggles one past the parser is still denied at use.
        for (scope, http) in [("none", false), ("local", false), ("internet", true), ("none", true)] {
            let json = format!(r#"{{"network_scope": "{scope}", "external_http": {http}}}"#);
            let parsed = CapabilityManifest::from_json(&json);
            assert_eq!(parsed.is_ok(), scope == "none" && !http, "parse {scope}/{http}");

            let manifest = CapabilityManifest {
                verse_scope: vec![],
                petal_scope: vec![],
                property_scope: vec![],
                capabilities: vec![],
                network_scope: scope.into(),
                external_http: http,
            };
            let token = CapabilityToken::mint("test-plugin", &manifest);
            assert!(
                matches!(
                    validate_access(&token, &CapabilityOperation::HttpRequest),
                    Err(CapabilityError::HttpDenied { .. })
                ),
                "request must be denied for {scope}/{http}"
            );
        }
    }

    #[test]
    fn capability_grants_fail_closed() {
        // No capabilities declared => every named capability is denied.
        let token =
            CapabilityToken::mint("test-plugin", &CapabilityManifest::from_json("{}").unwrap());
        assert!(!token.has_capability(fe_sdk::CAP_STORAGE_READ));
        assert!(!token.has_capability(fe_sdk::CAP_QUERY_SELECT));

        // Explicit grants are honored; unlisted ones stay denied.
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"capabilities": ["storage.read", "query.select"]}"#)
                .unwrap(),
        );
        assert!(token.has_capability(fe_sdk::CAP_STORAGE_READ));
        assert!(token.has_capability(fe_sdk::CAP_QUERY_SELECT));
        assert!(!token.has_capability(fe_sdk::CAP_STORAGE_WRITE));
    }

    #[test]
    fn auth_context_bridge_mirrors_token_grants() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"capabilities": ["storage.read"]}"#).unwrap(),
        );
        let subject = token.to_auth_context();
        assert!(subject.has_capability("storage.read"));
        assert!(!subject.has_capability("storage.write"));
        assert_eq!(subject.subject_label(), "plugin:test-plugin");
    }

    #[test]
    fn validate_empty_scope_is_wildcard() {
        let token =
            CapabilityToken::mint("test-plugin", &CapabilityManifest::from_json("{}").unwrap());
        // Empty property_scope = wildcard
        assert!(validate_access(
            &token,
            &CapabilityOperation::PropertyAccess {
                key: "anything".into()
            }
        )
        .is_ok());
        // Empty petal_scope = wildcard
        assert!(validate_access(
            &token,
            &CapabilityOperation::PetalAccess {
                scope: "any".into()
            }
        )
        .is_ok());
    }
}
