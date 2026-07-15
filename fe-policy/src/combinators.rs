//! Policy combinators — see fe-policy/AGENTS.md §combinators.

use std::sync::Arc;

use crate::engine::Policy;
use crate::types::{Action, AuthContext, Decision, Scope};

/// Allows iff at least one child allows; an empty set denies.
pub struct AnyOf {
    policies: Vec<Arc<dyn Policy>>,
}

impl AnyOf {
    pub fn new(policies: Vec<Arc<dyn Policy>>) -> Self {
        Self { policies }
    }
}

impl Policy for AnyOf {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        if self.policies.is_empty() {
            return Decision::deny("any-of: empty policy set (deny-by-default)");
        }
        let mut reasons = Vec::new();
        for policy in &self.policies {
            match policy.evaluate(subject, action, resource) {
                Decision::Allow => return Decision::Allow,
                Decision::Deny(reason) => reasons.push(reason),
            }
        }
        Decision::deny(format!("any-of: no child allowed [{}]", reasons.join("; ")))
    }

    fn name(&self) -> &'static str {
        "any-of"
    }
}

/// Allows iff every child allows; an empty set denies.
pub struct AllOf {
    policies: Vec<Arc<dyn Policy>>,
}

impl AllOf {
    pub fn new(policies: Vec<Arc<dyn Policy>>) -> Self {
        Self { policies }
    }
}

impl Policy for AllOf {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        if self.policies.is_empty() {
            return Decision::deny("all-of: empty policy set (deny-by-default)");
        }
        for policy in &self.policies {
            if let Decision::Deny(reason) = policy.evaluate(subject, action, resource) {
                return Decision::deny(format!("all-of: {}: {reason}", policy.name()));
            }
        }
        Decision::Allow
    }

    fn name(&self) -> &'static str {
        "all-of"
    }
}

/// Migration shim: evaluates the inner policy but converts denials into
/// warn-logged allows, so a gate can exist before its inputs are plumbed.
pub struct PermissiveMigrationPolicy {
    inner: Arc<dyn Policy>,
}

impl PermissiveMigrationPolicy {
    pub fn new(inner: Arc<dyn Policy>) -> Self {
        Self { inner }
    }
}

impl Policy for PermissiveMigrationPolicy {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        match self.inner.evaluate(subject, action, resource) {
            Decision::Allow => Decision::Allow,
            Decision::Deny(reason) => {
                tracing::warn!(
                    policy = self.inner.name(),
                    subject = %subject.subject_label(),
                    action = %action,
                    scope = %resource,
                    reason = %reason,
                    "policy would DENY — allowed in migration mode (enforcement not yet flipped)"
                );
                Decision::Allow
            }
        }
    }

    fn name(&self) -> &'static str {
        "permissive-migration"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_level::RoleLevel;

    struct Fixed(Decision);
    impl Policy for Fixed {
        fn evaluate(&self, _: &AuthContext, _: &Action, _: &Scope) -> Decision {
            self.0.clone()
        }
    }

    fn allow() -> Arc<dyn Policy> {
        Arc::new(Fixed(Decision::Allow))
    }

    fn deny() -> Arc<dyn Policy> {
        Arc::new(Fixed(Decision::deny("no")))
    }

    fn subject() -> AuthContext {
        AuthContext::Did {
            did: "did:key:z6MkTest".to_string(),
            role: RoleLevel::Viewer,
        }
    }

    #[test]
    fn any_of_empty_denies() {
        let d = AnyOf::new(vec![]).evaluate(&subject(), &Action::Read, &Scope::global());
        assert!(!d.is_allow());
    }

    #[test]
    fn any_of_one_allow_suffices() {
        let d = AnyOf::new(vec![deny(), allow()]).evaluate(&subject(), &Action::Read, &Scope::global());
        assert!(d.is_allow());
    }

    #[test]
    fn all_of_empty_denies() {
        let d = AllOf::new(vec![]).evaluate(&subject(), &Action::Read, &Scope::global());
        assert!(!d.is_allow());
    }

    #[test]
    fn all_of_one_deny_denies() {
        let d = AllOf::new(vec![allow(), deny()]).evaluate(&subject(), &Action::Read, &Scope::global());
        assert!(!d.is_allow());
    }

    #[test]
    fn all_of_all_allow_allows() {
        let d = AllOf::new(vec![allow(), allow()]).evaluate(&subject(), &Action::Read, &Scope::global());
        assert!(d.is_allow());
    }

    #[test]
    fn permissive_migration_converts_deny_to_allow() {
        let p = PermissiveMigrationPolicy::new(deny());
        let d = p.evaluate(&subject(), &Action::Write, &Scope::new("VERSE#v1"));
        assert!(d.is_allow(), "migration mode must allow (and warn-log) would-be denials");
    }

    #[test]
    fn permissive_migration_passes_allow_through() {
        let p = PermissiveMigrationPolicy::new(allow());
        assert!(p.evaluate(&subject(), &Action::Write, &Scope::global()).is_allow());
    }
}
