//! Unified deny-by-default authorization engine — see fe-policy/AGENTS.md.

pub mod combinators;
pub mod engine;
pub mod node_lifecycle;
pub mod policies;
pub mod role_grants;
pub mod role_level;
pub mod scope;
pub mod types;

pub use combinators::{AllOf, AnyOf, PermissiveMigrationPolicy};
pub use engine::{Policy, PolicyEngine};
pub use node_lifecycle::{
    authorize_instance_promotion, authorize_node_delete, authorize_node_edit, MIN_DELETE_ROLE,
};
pub use policies::{
    CapabilityPolicy, OwnershipPolicy, PublicReadPolicy, RoleFloorPolicy, RoleLevelPolicy,
    TokenScopePolicy,
};
pub use role_grants::{
    latest_per_grantor, materialize, validate_statement, CacheRow, ChangedDids, EffectiveRoles,
    GrantedRole, RejectReason, RoleStatement, RoleStatementPayload, StatementKey,
};
pub use role_level::RoleLevel;
// Scope-string parser — single home (W5b, OF-3); fe-database's `scope` module
// re-exports these so `fe_database::{parse_scope, build_scope, ...}` is unchanged.
pub use scope::{
    build_scope, parent_scope, parse_scope, scope_level, ScopeError, ScopeLevel, ScopeParts,
};
pub use types::{scope_contains, Action, AuthContext, Decision, Scope};
