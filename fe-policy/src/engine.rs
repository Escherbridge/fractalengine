//! Policy trait + deny-by-default engine with tracing decision logs — see AGENTS.md §engine.

use std::sync::Arc;

use crate::types::{Action, AuthContext, Decision, Scope};

/// Single evaluation point: entry points call `evaluate`, never their own role math.
pub trait Policy: Send + Sync {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision;

    /// Short name used in decision logs.
    fn name(&self) -> &'static str {
        "policy"
    }
}

/// Deny-by-default policy set: allows iff at least one member policy allows.
#[derive(Default, Clone)]
pub struct PolicyEngine {
    policies: Vec<Arc<dyn Policy>>,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_policy(mut self, policy: Arc<dyn Policy>) -> Self {
        self.policies.push(policy);
        self
    }

    /// Evaluate and emit the decision log (allow=debug, deny=warn).
    pub fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        let decision = self.evaluate_unlogged(subject, action, resource);
        log_decision("engine", subject, action, resource, &decision);
        decision
    }

    fn evaluate_unlogged(
        &self,
        subject: &AuthContext,
        action: &Action,
        resource: &Scope,
    ) -> Decision {
        if self.policies.is_empty() {
            return Decision::deny("deny-by-default: no policy configured");
        }
        let mut reasons = Vec::new();
        for policy in &self.policies {
            match policy.evaluate(subject, action, resource) {
                Decision::Allow => return Decision::Allow,
                Decision::Deny(reason) => reasons.push(format!("{}: {reason}", policy.name())),
            }
        }
        Decision::deny(format!("deny-by-default: no policy allowed [{}]", reasons.join("; ")))
    }
}

impl Policy for PolicyEngine {
    fn evaluate(&self, subject: &AuthContext, action: &Action, resource: &Scope) -> Decision {
        PolicyEngine::evaluate(self, subject, action, resource)
    }

    fn name(&self) -> &'static str {
        "engine"
    }
}

/// Decision log: who/what/why at debug for allows, warn for denials (security event).
pub fn log_decision(
    policy: &str,
    subject: &AuthContext,
    action: &Action,
    resource: &Scope,
    decision: &Decision,
) {
    match decision {
        Decision::Allow => tracing::debug!(
            policy,
            subject = %subject.subject_label(),
            action = %action,
            scope = %resource,
            "policy decision: allow"
        ),
        Decision::Deny(reason) => tracing::warn!(
            policy,
            subject = %subject.subject_label(),
            action = %action,
            scope = %resource,
            reason = %reason,
            "policy decision: deny"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role_level::RoleLevel;

    struct AllowAll;
    impl Policy for AllowAll {
        fn evaluate(&self, _: &AuthContext, _: &Action, _: &Scope) -> Decision {
            Decision::Allow
        }
        fn name(&self) -> &'static str {
            "allow-all"
        }
    }

    struct DenyAll;
    impl Policy for DenyAll {
        fn evaluate(&self, _: &AuthContext, _: &Action, _: &Scope) -> Decision {
            Decision::deny("denied by deny-all")
        }
        fn name(&self) -> &'static str {
            "deny-all"
        }
    }

    fn subject() -> AuthContext {
        AuthContext::Did {
            did: "did:key:z6MkTest".to_string(),
            role: RoleLevel::Owner,
        }
    }

    #[test]
    fn empty_engine_denies_by_default() {
        let engine = PolicyEngine::new();
        let decision = engine.evaluate(&subject(), &Action::Read, &Scope::global());
        assert!(!decision.is_allow(), "empty policy set must deny");
        assert!(decision.reason().unwrap_or_default().contains("deny-by-default"));
    }

    #[test]
    fn empty_engine_denies_even_owner_write() {
        let engine = PolicyEngine::new();
        let decision = engine.evaluate(&subject(), &Action::Write, &Scope::new("PETAL#p1"));
        assert!(!decision.is_allow());
    }

    #[test]
    fn any_member_allow_wins() {
        let engine = PolicyEngine::new()
            .with_policy(Arc::new(DenyAll))
            .with_policy(Arc::new(AllowAll));
        assert!(engine
            .evaluate(&subject(), &Action::Read, &Scope::global())
            .is_allow());
    }

    #[test]
    fn all_members_deny_denies_with_reasons() {
        let engine = PolicyEngine::new().with_policy(Arc::new(DenyAll));
        let decision = engine.evaluate(&subject(), &Action::Read, &Scope::global());
        assert!(decision.reason().unwrap_or_default().contains("deny-all"));
    }

    #[test]
    fn engine_nests_as_policy() {
        let inner = PolicyEngine::new().with_policy(Arc::new(AllowAll));
        let outer = PolicyEngine::new().with_policy(Arc::new(inner));
        assert!(outer
            .evaluate(&subject(), &Action::Read, &Scope::global())
            .is_allow());
    }
}
