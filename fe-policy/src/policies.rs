//! Concrete policies wrapping the existing auth layers — see AGENTS.md §policies.

use crate::engine::Policy;
use crate::role_level::RoleLevel;
use crate::types::{Action, AuthContext, Decision, Scope};

/// Role-ordering policy: each action carries a minimum RoleLevel; unmapped actions deny.
pub struct RoleLevelPolicy {
    min_roles: Vec<(Action, RoleLevel)>,
}

impl RoleLevelPolicy {
    /// Empty rule set (denies everything until `require` rules are added).
    pub fn new() -> Self {
        Self {
            min_roles: Vec::new(),
        }
    }

    /// Add a minimum-role rule for an action.
    pub fn require(mut self, action: Action, min: RoleLevel) -> Self {
        self.min_roles.push((action, min));
        self
    }

    /// Standard verb map: Read/Query=Viewer+, Write/Install=Editor+, Publish/Manage=Manager+.
    pub fn standard() -> Self {
        Self::new()
            .require(Action::Read, RoleLevel::Viewer)
            .require(Action::Query, RoleLevel::Viewer)
            .require(Action::Write, RoleLevel::Editor)
            .require(Action::Install, RoleLevel::Editor)
            .require(Action::Publish, RoleLevel::Manager)
            .require(Action::Manage, RoleLevel::Manager)
    }
}

impl Default for RoleLevelPolicy {
    fn default() -> Self {
        Self::standard()
    }
}

impl Policy for RoleLevelPolicy {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        let Some((_, min)) = self.min_roles.iter().find(|(a, _)| a == action) else {
            return Decision::deny(format!("role-level: no rule for action '{action}'"));
        };
        let role = subject.role();
        if role.is_at_least(*min) {
            Decision::Allow
        } else {
            Decision::deny(format!(
                "role-level: '{}' has role '{role}' at scope '{resource}' — '{action}' requires '{min}'+",
                subject.subject_label()
            ))
        }
    }

    fn name(&self) -> &'static str {
        "role-level"
    }
}

/// Allows `Action::Read` for any subject (including Anonymous); denies everything else.
pub struct PublicReadPolicy;

impl Policy for PublicReadPolicy {
    fn evaluate(&self, _subject: &AuthContext, action: &Action, _resource: &Scope) -> Decision {
        match action {
            Action::Read => Decision::Allow,
            other => Decision::deny(format!("public-read: '{other}' is not a public read")),
        }
    }

    fn name(&self) -> &'static str {
        "public-read"
    }
}

/// Thin adapter over fe-plugin's capability grants: `Action::Custom(cap)` allows
/// iff the subject's capability token grants `cap`. Fail closed for everything else.
pub struct CapabilityPolicy;

impl Policy for CapabilityPolicy {
    fn evaluate(&self, subject: &AuthContext, action: &Action, _resource: &Scope) -> Decision {
        let Action::Custom(capability) = action else {
            return Decision::deny("capability: only named capability actions are evaluated");
        };
        if subject.has_capability(capability) {
            Decision::Allow
        } else {
            Decision::deny(format!(
                "capability: '{}' lacks grant '{capability}'",
                subject.subject_label()
            ))
        }
    }

    fn name(&self) -> &'static str {
        "capability"
    }
}

/// Allows any action iff the subject's role is at least the floor. A floor of
/// `RoleLevel::None` admits everyone (fe-api `require_role_level` semantics).
pub struct RoleFloorPolicy(pub RoleLevel);

impl Policy for RoleFloorPolicy {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        let role = subject.role();
        if role.is_at_least(self.0) {
            Decision::Allow
        } else {
            Decision::deny(format!(
                "role-floor: '{}' has role '{role}' at scope '{resource}' — '{action}' requires '{}'+",
                subject.subject_label(),
                self.0
            ))
        }
    }

    fn name(&self) -> &'static str {
        "role-floor"
    }
}

/// Action-agnostic API-token scope gate: allows iff the subject is an
/// `ApiToken` one of whose scopes contains `resource` (keyword-boundary prefix).
pub struct TokenScopePolicy;

impl Policy for TokenScopePolicy {
    fn evaluate(&self, subject: &AuthContext, _action: &Action, resource: &Scope) -> Decision {
        let AuthContext::ApiToken { scopes, .. } = subject else {
            return Decision::deny("token-scope: subject carries no token scope");
        };
        if scopes
            .iter()
            .any(|granted| Scope::new(granted.as_str()).contains(resource))
        {
            Decision::Allow
        } else {
            Decision::deny(format!(
                "token-scope: '{}' scopes do not cover '{resource}'",
                subject.subject_label()
            ))
        }
    }

    fn name(&self) -> &'static str {
        "token-scope"
    }
}

/// Owner-only gate over pre-fetched ownership data: allows iff the subject's
/// DID (or API-token subject) equals `owner_did`. Anonymous/Capability deny.
pub struct OwnershipPolicy {
    owner_did: String,
}

impl OwnershipPolicy {
    pub fn new(owner_did: impl Into<String>) -> Self {
        Self {
            owner_did: owner_did.into(),
        }
    }
}

impl Policy for OwnershipPolicy {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        let identity = match subject {
            AuthContext::Did { did, .. } => did,
            AuthContext::ApiToken { subject, .. } => subject,
            AuthContext::Anonymous | AuthContext::Capability { .. } => {
                return Decision::deny("ownership: subject carries no owner identity");
            }
        };
        if *identity == self.owner_did {
            Decision::Allow
        } else {
            Decision::deny(format!(
                "ownership: '{}' is not the owner of '{resource}' — '{action}' is owner-only",
                subject.subject_label()
            ))
        }
    }

    fn name(&self) -> &'static str {
        "ownership"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn did(role: RoleLevel) -> AuthContext {
        AuthContext::Did {
            did: "did:key:z6MkTest".to_string(),
            role,
        }
    }

    #[test]
    fn role_level_allows_at_or_above_minimum() {
        let policy = RoleLevelPolicy::standard();
        let scope = Scope::new("PETAL#p1");
        assert!(policy
            .evaluate(&did(RoleLevel::Editor), &Action::Write, &scope)
            .is_allow());
        assert!(policy
            .evaluate(&did(RoleLevel::Owner), &Action::Write, &scope)
            .is_allow());
        assert!(policy
            .evaluate(&did(RoleLevel::Viewer), &Action::Read, &scope)
            .is_allow());
    }

    #[test]
    fn role_level_denies_below_minimum() {
        let policy = RoleLevelPolicy::standard();
        let scope = Scope::new("PETAL#p1");
        assert!(!policy
            .evaluate(&did(RoleLevel::Viewer), &Action::Write, &scope)
            .is_allow());
        assert!(!policy
            .evaluate(&did(RoleLevel::Editor), &Action::Manage, &scope)
            .is_allow());
        assert!(!policy
            .evaluate(&did(RoleLevel::None), &Action::Read, &scope)
            .is_allow());
    }

    #[test]
    fn role_level_denies_anonymous() {
        let policy = RoleLevelPolicy::standard();
        let d = policy.evaluate(&AuthContext::Anonymous, &Action::Write, &Scope::global());
        assert!(!d.is_allow());
    }

    #[test]
    fn role_level_denies_unmapped_action() {
        let policy = RoleLevelPolicy::new().require(Action::Read, RoleLevel::Viewer);
        let d = policy.evaluate(&did(RoleLevel::Owner), &Action::Write, &Scope::global());
        assert!(!d.is_allow(), "unmapped action must deny even for Owner");
    }

    #[test]
    fn public_read_allows_anonymous_read_only() {
        let policy = PublicReadPolicy;
        assert!(policy
            .evaluate(&AuthContext::Anonymous, &Action::Read, &Scope::global())
            .is_allow());
        assert!(!policy
            .evaluate(&AuthContext::Anonymous, &Action::Write, &Scope::global())
            .is_allow());
        assert!(!policy
            .evaluate(&did(RoleLevel::Owner), &Action::Install, &Scope::global())
            .is_allow());
    }

    #[test]
    fn capability_policy_mirrors_token_grants() {
        let subject = AuthContext::Capability {
            plugin_id: "ext-a".to_string(),
            capabilities: vec!["storage.read".to_string()],
        };
        let policy = CapabilityPolicy;
        assert!(policy
            .evaluate(
                &subject,
                &Action::Custom("storage.read".into()),
                &Scope::global()
            )
            .is_allow());
        assert!(!policy
            .evaluate(
                &subject,
                &Action::Custom("storage.write".into()),
                &Scope::global()
            )
            .is_allow());
        // Non-capability subjects fail closed.
        assert!(!policy
            .evaluate(
                &did(RoleLevel::Owner),
                &Action::Custom("storage.read".into()),
                &Scope::global()
            )
            .is_allow());
        // Non-custom actions fail closed.
        assert!(!policy
            .evaluate(&subject, &Action::Read, &Scope::global())
            .is_allow());
    }

    fn token(scope: &str, role: RoleLevel) -> AuthContext {
        AuthContext::ApiToken {
            subject: "did:key:z6MkNode".to_string(),
            scopes: vec![scope.to_string()],
            role,
        }
    }

    #[test]
    fn token_scope_table() {
        let policy = TokenScopePolicy;
        let v1 = token("VERSE#v1", RoleLevel::Editor);
        // (subject, resource, allowed)
        let table = [
            (&v1, "VERSE#v1-FRACTAL#f-PETAL#p", true),
            (&v1, "VERSE#v1", true),
            (&v1, "VERSE#v2-FRACTAL#f-PETAL#p", false),
            (&v1, "VERSE#v10-FRACTAL#f", false),
            (&v1, "*", false),
        ];
        for (subject, resource, allowed) in table {
            assert_eq!(
                policy
                    .evaluate(subject, &Action::Write, &Scope::new(resource))
                    .is_allow(),
                allowed,
                "{resource}"
            );
        }
        let petal_token = token("VERSE#v1-FRACTAL#f-PETAL#p", RoleLevel::Owner);
        assert!(!policy
            .evaluate(&petal_token, &Action::Read, &Scope::new("VERSE#v1"))
            .is_allow());
    }

    #[test]
    fn token_scope_denies_non_token_subjects() {
        let resource = Scope::new("VERSE#v1-FRACTAL#f-PETAL#p");
        for subject in [
            did(RoleLevel::Owner),
            AuthContext::Anonymous,
            AuthContext::Capability {
                plugin_id: "ext".into(),
                capabilities: vec!["VERSE#v1".into()],
            },
        ] {
            let d = TokenScopePolicy.evaluate(&subject, &Action::Read, &resource);
            assert_eq!(
                d.reason(),
                Some("token-scope: subject carries no token scope")
            );
        }
    }

    #[test]
    fn ownership_table() {
        let owner = "did:key:z6MkOwner";
        let policy = OwnershipPolicy::new(owner);
        let scope = Scope::new("crate:x");
        let table = [
            (
                AuthContext::Did {
                    did: owner.into(),
                    role: RoleLevel::None,
                },
                true,
            ),
            (
                AuthContext::ApiToken {
                    subject: owner.into(),
                    scopes: vec![],
                    role: RoleLevel::Viewer,
                },
                true,
            ),
            (
                AuthContext::Did {
                    did: "did:key:z6MkOther".into(),
                    role: RoleLevel::Owner,
                },
                false,
            ),
            (AuthContext::Anonymous, false),
            (
                AuthContext::Capability {
                    plugin_id: owner.into(),
                    capabilities: vec![],
                },
                false,
            ),
        ];
        for (subject, allowed) in table {
            assert_eq!(
                policy
                    .evaluate(&subject, &Action::Publish, &scope)
                    .is_allow(),
                allowed,
                "{subject:?}"
            );
        }
    }

    #[test]
    fn role_floor_table() {
        let scope = Scope::global();
        // (floor, subject role, allowed)
        let table = [
            (RoleLevel::Editor, RoleLevel::Viewer, false),
            (RoleLevel::Editor, RoleLevel::Editor, true),
            (RoleLevel::Owner, RoleLevel::Manager, false),
            (RoleLevel::Owner, RoleLevel::Owner, true),
            (RoleLevel::None, RoleLevel::None, true),
        ];
        for (floor, role, allowed) in table {
            assert_eq!(
                RoleFloorPolicy(floor)
                    .evaluate(&did(role), &Action::Custom("any".into()), &scope)
                    .is_allow(),
                allowed,
                "floor {floor} role {role}"
            );
        }
        assert!(!RoleFloorPolicy(RoleLevel::Viewer)
            .evaluate(&AuthContext::Anonymous, &Action::Read, &scope)
            .is_allow());
    }
}
