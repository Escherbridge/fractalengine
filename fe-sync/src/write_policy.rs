//! Sync write-path policy gate. See fe-sync/src/AGENTS.md §write-policy.

use std::sync::Arc;

use bevy::prelude::Resource;
use fe_policy::{Action, AuthContext, Decision, Policy, RoleLevel, RoleLevelPolicy, Scope};

/// Shared handle to the policy engine gating replica row writes.
#[derive(Resource, Clone)]
pub struct PolicyHandle(pub Arc<dyn Policy>);

impl PolicyHandle {
    /// Strict role-based enforcement for replica writes.
    pub fn strict() -> Self {
        Self(Arc::new(RoleLevelPolicy::standard()))
    }

    /// Gate a row write authored by `author_did` into `verse_id`'s replica.
    /// `role` is the author's role at that scope if the caller has it (None until plumbed).
    pub fn allow_write(&self, author_did: &str, role: Option<RoleLevel>, verse_id: &str) -> bool {
        let subject = AuthContext::Did {
            did: author_did.to_string(),
            role: role.unwrap_or(RoleLevel::None),
        };
        let scope = Scope::new(format!("VERSE#{verse_id}"));
        match self.0.evaluate(&subject, &Action::Write, &scope) {
            Decision::Allow => true,
            Decision::Deny(reason) => {
                tracing::warn!(author_did, verse_id, reason = %reason, "sync write DENIED by policy");
                false
            }
        }
    }
}

impl Default for PolicyHandle {
    fn default() -> Self {
        Self::strict()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_denies_unplumbed_writes() {
        let policy = PolicyHandle::default();
        assert!(!policy.allow_write("did:key:z6MkPeer", None, "verse-1"));
    }

    #[test]
    fn strict_denies_writes_without_editor_role() {
        let policy = PolicyHandle::strict();
        assert!(!policy.allow_write("did:key:z6MkPeer", None, "verse-1"));
        assert!(!policy.allow_write("did:key:z6MkPeer", Some(RoleLevel::Viewer), "verse-1"));
    }

    #[test]
    fn strict_allows_editor_and_above() {
        let policy = PolicyHandle::strict();
        assert!(policy.allow_write("did:key:z6MkPeer", Some(RoleLevel::Editor), "verse-1"));
        assert!(policy.allow_write("did:key:z6MkPeer", Some(RoleLevel::Owner), "verse-1"));
    }
}
