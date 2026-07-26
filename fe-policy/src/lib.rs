//! Unified deny-by-default authorization engine — see fe-policy/AGENTS.md.

pub mod combinators;
pub mod engine;
pub mod node_lifecycle;
pub mod policies;
pub mod role_level;
pub mod types;

pub use combinators::{AllOf, AnyOf, PermissiveMigrationPolicy};
pub use engine::{Policy, PolicyEngine};
pub use node_lifecycle::{authorize_instance_promotion, authorize_node_delete, MIN_DELETE_ROLE};
pub use policies::{CapabilityPolicy, PublicReadPolicy, RoleLevelPolicy};
pub use role_level::RoleLevel;
pub use types::{Action, AuthContext, Decision, Scope};
