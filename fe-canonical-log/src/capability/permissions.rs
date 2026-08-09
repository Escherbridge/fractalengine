//! The §3.3 effective permission table and its two documented extras.

use thiserror::Error;

use super::verbs::{ObjectClass, Verb};

/// Every reason the §3.3 table denies a verb/class pair.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum PermissionError {
    /// The §3.3 cell for this pair is a dash.
    #[error("verb {verb:?} is never permitted for object class {object_class:?}")]
    VerbNotPermittedForClass {
        /// Requested verb.
        verb: Verb,
        /// Requested object class.
        object_class: ObjectClass,
    },
    /// The cell additionally requires current Manager+ authority from the authorization view.
    #[error("verb {verb:?} on {object_class:?} requires current Manager+ authority")]
    ManagerPlusRequired {
        /// Requested verb.
        verb: Verb,
        /// Requested object class.
        object_class: ObjectClass,
    },
    /// A structural append named no operation kind.
    #[error("a structural append must name its operation kind")]
    StructuralOperationKindMissing,
    /// A structural append reached a leaf with no `allowed_operation_kinds` caveat (§4.7).
    #[error("a structural append requires an explicit allowed_operation_kinds caveat")]
    StructuralOperationRequiresKindAllowlist,
}

/// The inputs the §3.3 table and its extras consult.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionContext {
    /// Requested verb.
    pub verb: Verb,
    /// Requested object class.
    pub object_class: ObjectClass,
    /// Operation kind being appended, when the request appends an operation.
    pub operation_kind: Option<u16>,
    /// Whether the appended operation is a structural authorization operation (§4.7).
    pub is_structural_operation: bool,
    /// Manager+ authority for the actor, read from the persistent authorization view.
    pub actor_is_manager_plus: bool,
    /// Whether the effective leaf caveats carry an `allowed_operation_kinds` allowlist.
    pub has_operation_kind_allowlist: bool,
}

/// The §3.3 table: every cell is permitted except the `preview` row, which is all dashes.
///
/// Preview is deliberately not an object class (D-CL13): it is a separate message type with
/// no `op_id`, no durable storage route, and no materializer entry point.
pub const fn permitted(verb: Verb, object_class: ObjectClass) -> bool {
    !matches!((verb, object_class), (Verb::Preview, _))
}

/// `append/checkpoint` additionally requires current Manager+ authority (§3.3).
pub const fn requires_manager_plus(verb: Verb, object_class: ObjectClass) -> bool {
    matches!(
        (verb, object_class),
        (Verb::Append, ObjectClass::Checkpoint)
    )
}

/// `append/op` additionally requires the kind allowlist when the operation is structural (§4.7).
pub const fn requires_operation_kind_allowlist(verb: Verb, object_class: ObjectClass) -> bool {
    matches!((verb, object_class), (Verb::Append, ObjectClass::Operation))
}

/// Applies the §3.3 table and both extras; membership in the allowlist is checked by the chain.
pub fn evaluate(context: &PermissionContext) -> Result<(), PermissionError> {
    if !permitted(context.verb, context.object_class) {
        return Err(PermissionError::VerbNotPermittedForClass {
            verb: context.verb,
            object_class: context.object_class,
        });
    }
    if requires_manager_plus(context.verb, context.object_class) && !context.actor_is_manager_plus {
        return Err(PermissionError::ManagerPlusRequired {
            verb: context.verb,
            object_class: context.object_class,
        });
    }
    if requires_operation_kind_allowlist(context.verb, context.object_class)
        && context.is_structural_operation
    {
        if context.operation_kind.is_none() {
            return Err(PermissionError::StructuralOperationKindMissing);
        }
        if !context.has_operation_kind_allowlist {
            return Err(PermissionError::StructuralOperationRequiresKindAllowlist);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(verb: Verb, object_class: ObjectClass) -> PermissionContext {
        PermissionContext {
            verb,
            object_class,
            operation_kind: None,
            is_structural_operation: false,
            actor_is_manager_plus: false,
            has_operation_kind_allowlist: false,
        }
    }

    #[test]
    fn preview_is_denied_for_every_object_class() {
        for object_class in ObjectClass::ALL {
            assert!(!permitted(Verb::Preview, object_class));
            assert_eq!(
                evaluate(&context(Verb::Preview, object_class)),
                Err(PermissionError::VerbNotPermittedForClass {
                    verb: Verb::Preview,
                    object_class,
                })
            );
        }
    }

    #[test]
    fn every_non_preview_cell_of_the_table_is_populated() {
        for verb in Verb::ALL {
            for object_class in ObjectClass::ALL {
                assert_eq!(permitted(verb, object_class), verb != Verb::Preview);
            }
        }
    }

    #[test]
    fn appending_a_checkpoint_requires_manager_plus() {
        let denied = context(Verb::Append, ObjectClass::Checkpoint);
        assert_eq!(
            evaluate(&denied),
            Err(PermissionError::ManagerPlusRequired {
                verb: Verb::Append,
                object_class: ObjectClass::Checkpoint,
            })
        );

        let allowed = PermissionContext {
            actor_is_manager_plus: true,
            ..denied
        };
        assert_eq!(evaluate(&allowed), Ok(()));

        for object_class in ObjectClass::ALL {
            if object_class != ObjectClass::Checkpoint {
                assert!(!requires_manager_plus(Verb::Append, object_class));
            }
            assert!(!requires_manager_plus(Verb::Fetch, object_class));
        }
    }

    #[test]
    fn a_structural_append_needs_a_named_kind_and_an_explicit_allowlist() {
        let missing_kind = PermissionContext {
            is_structural_operation: true,
            ..context(Verb::Append, ObjectClass::Operation)
        };
        assert_eq!(
            evaluate(&missing_kind),
            Err(PermissionError::StructuralOperationKindMissing)
        );

        let missing_allowlist = PermissionContext {
            operation_kind: Some(4),
            ..missing_kind
        };
        assert_eq!(
            evaluate(&missing_allowlist),
            Err(PermissionError::StructuralOperationRequiresKindAllowlist)
        );

        let allowed = PermissionContext {
            has_operation_kind_allowlist: true,
            ..missing_allowlist
        };
        assert_eq!(evaluate(&allowed), Ok(()));
    }

    #[test]
    fn a_non_structural_append_does_not_need_the_kind_allowlist() {
        assert_eq!(
            evaluate(&context(Verb::Append, ObjectClass::Operation)),
            Ok(())
        );
        assert!(requires_operation_kind_allowlist(
            Verb::Append,
            ObjectClass::Operation
        ));
        assert!(!requires_operation_kind_allowlist(
            Verb::Fetch,
            ObjectClass::Operation
        ));
        assert!(!requires_operation_kind_allowlist(
            Verb::Append,
            ObjectClass::Segment
        ));
    }
}
