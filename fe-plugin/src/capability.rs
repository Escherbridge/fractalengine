//! Capability manifest and token system for plugin sandboxing.
//!
//! Each plugin declares a [`CapabilityManifest`] that specifies which verses,
//! petals, properties, and network resources it may access. At activation time
//! the host mints a [`CapabilityToken`] from the manifest. All runtime
//! operations are validated against the token before execution.

use serde::{Deserialize, Serialize};

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
    /// Network access level: `"none"`, `"local"`, `"internet"`.
    #[serde(default = "default_network_scope")]
    pub network_scope: String,
    /// Whether the plugin may issue outbound HTTP requests.
    #[serde(default)]
    pub external_http: bool,
}

fn default_network_scope() -> String {
    "none".to_string()
}

impl CapabilityManifest {
    /// Parse a manifest from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
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
    /// Network scope level.
    pub network_scope: String,
    /// Whether outbound HTTP is allowed.
    pub external_http: bool,
}

impl CapabilityToken {
    /// Mint a token from a validated manifest.
    pub fn mint(plugin_id: &str, manifest: &CapabilityManifest) -> Self {
        Self {
            plugin_id: plugin_id.to_string(),
            verse_scope: manifest.verse_scope.clone(),
            petal_scope: manifest.petal_scope.clone(),
            property_scope: manifest.property_scope.clone(),
            network_scope: manifest.network_scope.clone(),
            external_http: manifest.external_http,
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
            if token.property_scope.is_empty() {
                // Empty = wildcard (all properties allowed)
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
            if token.petal_scope.is_empty() {
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
            if token.verse_scope.is_empty() {
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
        CapabilityOperation::HttpRequest => {
            if token.external_http {
                Ok(())
            } else {
                Err(CapabilityError::HttpDenied {
                    plugin_id: token.plugin_id.clone(),
                })
            }
        }
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
    }

    #[test]
    fn validate_property_access_allowed() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"property_scope": ["color", "health"]}"#).unwrap(),
        );
        assert!(validate_access(&token, &CapabilityOperation::PropertyAccess { key: "color".into() }).is_ok());
    }

    #[test]
    fn validate_property_access_denied() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"property_scope": ["color"]}"#).unwrap(),
        );
        assert!(validate_access(&token, &CapabilityOperation::PropertyAccess { key: "secret".into() }).is_err());
    }

    #[test]
    fn validate_wildcard_scope() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"verse_scope": ["VERSE#*"]}"#).unwrap(),
        );
        assert!(validate_access(&token, &CapabilityOperation::VerseAccess { scope: "VERSE#abc".into() }).is_ok());
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
    fn validate_http_allowed() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json(r#"{"external_http": true}"#).unwrap(),
        );
        assert!(validate_access(&token, &CapabilityOperation::HttpRequest).is_ok());
    }

    #[test]
    fn validate_empty_scope_is_wildcard() {
        let token = CapabilityToken::mint(
            "test-plugin",
            &CapabilityManifest::from_json("{}").unwrap(),
        );
        // Empty property_scope = wildcard
        assert!(validate_access(&token, &CapabilityOperation::PropertyAccess { key: "anything".into() }).is_ok());
        // Empty petal_scope = wildcard
        assert!(validate_access(&token, &CapabilityOperation::PetalAccess { scope: "any".into() }).is_ok());
    }
}
