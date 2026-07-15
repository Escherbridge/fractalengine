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
}
