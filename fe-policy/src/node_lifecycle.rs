//! Node-lifecycle authorization (node_lifecycle_addressing_20260725 FR-1).
//! See AGENTS.md §node-lifecycle.

use crate::policies::RoleLevelPolicy;
use crate::role_level::RoleLevel;
use crate::types::{Action, AuthContext, Decision, Scope};
use crate::Policy;

/// Minimum role required to delete (tombstone) or cascade-delete a node.
/// Delete is a mutation, so it sits at the Editor tier (same as write) — a
/// Viewer must never be able to remove data (N-5: authz lives here, never UI).
pub const MIN_DELETE_ROLE: RoleLevel = RoleLevel::Editor;

/// Authorize a node delete / cascade-delete at `scope` for `subject` (FR-1).
///
/// Reuses the standard role-ordering policy: delete maps to `Action::Write`
/// (Editor+). Deny-by-default — an anonymous or Viewer subject is rejected.
/// The caller (DB thread) enforces this before writing the tombstone op-log
/// entry; the UI receives only pre-authorized deletes.
pub fn authorize_node_delete(subject: &AuthContext, scope: &Scope) -> Decision {
    RoleLevelPolicy::standard().evaluate(subject, &Action::Write, scope)
}

/// Authorize a promotion (materializing a stamp instance into a node, FR-5).
/// Promotion writes a new row, so it is an Editor+ write as well.
pub fn authorize_instance_promotion(subject: &AuthContext, scope: &Scope) -> Decision {
    RoleLevelPolicy::standard().evaluate(subject, &Action::Write, scope)
}

/// Authorize a node edit (rename / duplicate) — an Editor+ write like the rest
/// of the lifecycle family. Shared by `RenameNode` and `DuplicateNode`.
pub fn authorize_node_edit(subject: &AuthContext, scope: &Scope) -> Decision {
    RoleLevelPolicy::standard().evaluate(subject, &Action::Write, scope)
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

    fn scope() -> Scope {
        Scope::new("VERSE#v1-FRACTAL#f1-PETAL#p1")
    }

    #[test]
    fn editor_and_above_may_delete() {
        assert!(authorize_node_delete(&did(RoleLevel::Editor), &scope()).is_allow());
        assert!(authorize_node_delete(&did(RoleLevel::Manager), &scope()).is_allow());
        assert!(authorize_node_delete(&did(RoleLevel::Owner), &scope()).is_allow());
    }

    #[test]
    fn viewer_and_below_may_not_delete() {
        assert!(!authorize_node_delete(&did(RoleLevel::Viewer), &scope()).is_allow());
        assert!(!authorize_node_delete(&did(RoleLevel::None), &scope()).is_allow());
    }

    #[test]
    fn anonymous_may_not_delete() {
        assert!(!authorize_node_delete(&AuthContext::Anonymous, &scope()).is_allow());
    }

    #[test]
    fn deny_reason_is_informative() {
        let d = authorize_node_delete(&did(RoleLevel::Viewer), &scope());
        assert!(d.reason().unwrap_or_default().contains("requires 'editor'"));
    }

    #[test]
    fn promotion_shares_the_editor_gate() {
        assert!(authorize_instance_promotion(&did(RoleLevel::Editor), &scope()).is_allow());
        assert!(!authorize_instance_promotion(&did(RoleLevel::Viewer), &scope()).is_allow());
    }

    #[test]
    fn edit_shares_the_editor_gate() {
        assert!(authorize_node_edit(&did(RoleLevel::Editor), &scope()).is_allow());
        assert!(!authorize_node_edit(&did(RoleLevel::Viewer), &scope()).is_allow());
        assert!(!authorize_node_edit(&AuthContext::Anonymous, &scope()).is_allow());
    }

    #[test]
    fn min_delete_role_is_editor() {
        assert_eq!(MIN_DELETE_ROLE, RoleLevel::Editor);
    }
}
