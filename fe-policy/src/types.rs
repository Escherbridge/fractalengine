//! Plain-data evaluation inputs/outputs — see fe-policy/AGENTS.md §types.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::role_level::RoleLevel;

/// Unified subject: wraps the existing did:key/JWT, API-token, and plugin-capability identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthContext {
    /// No authenticated subject (public/anonymous access).
    Anonymous,
    /// A did:key subject with its role for the scope under evaluation pre-fetched.
    Did { did: String, role: RoleLevel },
    /// An API-token subject with granted scope strings and a pre-fetched role.
    ApiToken {
        subject: String,
        scopes: Vec<String>,
        role: RoleLevel,
    },
    /// A plugin capability-token subject (named capability grants, fail closed).
    Capability {
        plugin_id: String,
        capabilities: Vec<String>,
    },
}

impl AuthContext {
    /// Stable subject label for decision logs.
    pub fn subject_label(&self) -> String {
        match self {
            AuthContext::Anonymous => "anonymous".to_string(),
            AuthContext::Did { did, .. } => did.clone(),
            AuthContext::ApiToken { subject, .. } => format!("api-token:{subject}"),
            AuthContext::Capability { plugin_id, .. } => format!("plugin:{plugin_id}"),
        }
    }

    /// Effective role level; Anonymous and Capability subjects carry none.
    pub fn role(&self) -> RoleLevel {
        match self {
            AuthContext::Did { role, .. } | AuthContext::ApiToken { role, .. } => *role,
            AuthContext::Anonymous | AuthContext::Capability { .. } => RoleLevel::None,
        }
    }

    /// Whether a named capability is granted (false for non-capability subjects).
    pub fn has_capability(&self, capability: &str) -> bool {
        match self {
            AuthContext::Capability { capabilities, .. } => {
                capabilities.iter().any(|c| c == capability)
            }
            _ => false,
        }
    }
}

/// Verb being attempted against a resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Read,
    Write,
    Query,
    Install,
    Publish,
    Manage,
    /// Named action outside the fixed verb set (e.g. a plugin capability string).
    Custom(String),
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::Read => write!(f, "read"),
            Action::Write => write!(f, "write"),
            Action::Query => write!(f, "query"),
            Action::Install => write!(f, "install"),
            Action::Publish => write!(f, "publish"),
            Action::Manage => write!(f, "manage"),
            Action::Custom(name) => write!(f, "{name}"),
        }
    }
}

/// Hierarchical resource scope string (`VERSE#v-FRACTAL#f-PETAL#p` convention).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope(String);

impl Scope {
    pub fn new(scope: impl Into<String>) -> Self {
        Self(scope.into())
    }

    /// Wildcard scope for checks that are not resource-specific.
    pub fn global() -> Self {
        Self("*".to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Outcome of a policy evaluation. Absence of a matching policy is Deny.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    Allow,
    Deny(String),
}

impl Decision {
    pub fn deny(reason: impl Into<String>) -> Self {
        Decision::Deny(reason.into())
    }

    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Decision::Allow => None,
            Decision::Deny(reason) => Some(reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_has_no_role_and_no_capabilities() {
        let subject = AuthContext::Anonymous;
        assert_eq!(subject.role(), RoleLevel::None);
        assert!(!subject.has_capability("storage.read"));
        assert_eq!(subject.subject_label(), "anonymous");
    }

    #[test]
    fn did_subject_carries_role() {
        let subject = AuthContext::Did {
            did: "did:key:z6MkTest".to_string(),
            role: RoleLevel::Editor,
        };
        assert_eq!(subject.role(), RoleLevel::Editor);
        assert_eq!(subject.subject_label(), "did:key:z6MkTest");
    }

    #[test]
    fn capability_subject_fail_closed() {
        let subject = AuthContext::Capability {
            plugin_id: "ext-a".to_string(),
            capabilities: vec!["storage.read".to_string()],
        };
        assert!(subject.has_capability("storage.read"));
        assert!(!subject.has_capability("storage.write"));
        assert_eq!(subject.role(), RoleLevel::None);
    }

    #[test]
    fn decision_helpers() {
        assert!(Decision::Allow.is_allow());
        let deny = Decision::deny("nope");
        assert!(!deny.is_allow());
        assert_eq!(deny.reason(), Some("nope"));
    }

    #[test]
    fn action_display() {
        assert_eq!(Action::Write.to_string(), "write");
        assert_eq!(
            Action::Custom("query.select".into()).to_string(),
            "query.select"
        );
    }
}
