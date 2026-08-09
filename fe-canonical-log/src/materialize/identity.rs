//! SPEC-4 §1.5 materializer version identity and the keys derived from it; see
//! `src/AGENTS.md` §module-ownership and `materialize/AGENTS.md`.

use crate::envelope::{Hash32, Identifier32};

/// An explicit, hand-authored materializer identity (SPEC-4 §1.5).
///
/// MUST NOT be derived from `CARGO_PKG_VERSION`, `env!`, or any other build-time identifier:
/// §1.5 requires a materializer change to be an explicit author decision, not a side effect of
/// a crate version bump that never touched reduction logic.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MaterializerVersion {
    /// Stable name of the materializer implementation, for example `"scene-graph"`.
    pub materializer_id: String,
    /// Hand-incremented version of that materializer's reduction logic.
    pub version: u32,
}

impl MaterializerVersion {
    /// Builds an explicit materializer identity.
    pub fn new(materializer_id: impl Into<String>, version: u32) -> Self {
        Self {
            materializer_id: materializer_id.into(),
            version,
        }
    }
}

/// A materializer version bound to the branch it projects (SPEC-4 §1.5, D-CL19).
///
/// Part of checkpoint identity: two checkpoints for the same branch under different
/// materializer versions are checkpoints of different projections and must never be compared
/// or substituted for one another.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectionIdentity {
    /// The materializer whose reduction logic produced this projection.
    pub materializer_version: MaterializerVersion,
    /// The branch this projection tracks.
    pub branch_id: Identifier32,
}

impl ProjectionIdentity {
    /// Builds a projection identity.
    pub fn new(materializer_version: MaterializerVersion, branch_id: Identifier32) -> Self {
        Self {
            materializer_version,
            branch_id,
        }
    }
}

/// Binds a projection identity to one admitted operation, for the durable apply marker
/// (SPEC-4 §3.2-§3.3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ApplyMarkerKey {
    /// The projection this marker records progress for.
    pub projection_identity: ProjectionIdentity,
    /// The operation this marker attests was applied, or explicitly excluded.
    pub op_id: Hash32,
}

impl ApplyMarkerKey {
    /// Builds an apply-marker key.
    pub fn new(projection_identity: ProjectionIdentity, op_id: Hash32) -> Self {
        Self {
            projection_identity,
            op_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn op(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    #[test]
    fn a_materializer_version_bump_yields_a_distinct_projection_identity_and_apply_marker_key() {
        let branch_id = branch(0x11);
        let op_id = op(0x22);
        let v1 = MaterializerVersion::new("scene-graph", 1);
        let v2 = MaterializerVersion::new("scene-graph", 2);

        let identity_v1 = ProjectionIdentity::new(v1, branch_id);
        let identity_v2 = ProjectionIdentity::new(v2, branch_id);
        assert_ne!(identity_v1, identity_v2);

        let marker_v1 = ApplyMarkerKey::new(identity_v1.clone(), op_id);
        let marker_v2 = ApplyMarkerKey::new(identity_v2.clone(), op_id);
        assert_ne!(marker_v1, marker_v2);

        // Same materializer id and version, same branch: identity is stable and reproducible.
        let identity_v1_again =
            ProjectionIdentity::new(MaterializerVersion::new("scene-graph", 1), branch_id);
        assert_eq!(identity_v1, identity_v1_again);
    }

    #[test]
    fn materializer_id_alone_also_distinguishes_projection_identity() {
        let branch_id = branch(0x33);
        let a = ProjectionIdentity::new(MaterializerVersion::new("scene-graph", 1), branch_id);
        let b = ProjectionIdentity::new(MaterializerVersion::new("terrain-index", 1), branch_id);
        assert_ne!(a, b);
    }

    #[test]
    fn a_different_branch_also_distinguishes_projection_identity() {
        let version = MaterializerVersion::new("scene-graph", 1);
        let a = ProjectionIdentity::new(version.clone(), branch(0x01));
        let b = ProjectionIdentity::new(version, branch(0x02));
        assert_ne!(a, b);
    }
}
