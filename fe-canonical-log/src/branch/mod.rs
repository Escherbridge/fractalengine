//! Branch modes, control operations, and the verse branch registry (SPEC-5 §2); see
//! `src/branch/AGENTS.md`.

pub mod control_payload;
pub mod registry;

use thiserror::Error;

use crate::cbor::CborError;
use crate::envelope::{Author, CapabilityRef, Hash32, Identifier32, Scope, UnsignedEnvelope};
use crate::frontier::{FrontierError, SortedFrontier};

pub use control_payload::{BranchControlAction, BranchControlPayload};
pub use registry::{BranchControlEffect, BranchRecord, BranchRegistry};

/// The `operation_kind` a branch-control operation MUST carry (§2.2 rule 3, normal intent).
pub const BRANCH_CONTROL_OPERATION_KIND: u16 = 1;

/// The §2.1 branch modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BranchMode {
    /// Follows newly admitted eligible operations; concurrent heads stay a frontier.
    Tracking,
    /// Retains the committed selection and MAY accumulate received evidence only.
    Paused,
    /// Pins one immutable selection and never follows a later tracking update.
    Detached,
}

/// Whether one operation may enter a branch frontier, per the receiver's admission state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadAdmission {
    /// Admitted into this verse's DAG and free of an equivocation conflict.
    Admitted,
    /// Not an admitted operation of this verse.
    NotInVerse,
    /// Shares a §3.4 `EquivocationKey` with another candidate; both stay quarantined.
    QuarantinedEquivocation {
        /// The other operation that claims the same author/wall/counter identity.
        conflicting_op_id: Hash32,
    },
}

/// Every reason a branch control operation or registry transition is refused.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum BranchError {
    /// The payload bytes were not canonical CBOR.
    #[error("canonical CBOR: {0}")]
    Cbor(#[from] CborError),
    /// The selected frontier was empty or repeated an operation.
    #[error("selected frontier: {0}")]
    Frontier(#[from] FrontierError),
    /// The branch-control payload was not a CBOR map.
    #[error("branch_control payload is not a CBOR map")]
    ExpectedMap,
    /// The payload map did not hold exactly keys 0..4, once each, and nothing else.
    #[error("branch_control payload must hold exactly the integer keys 0..4 once each")]
    UnexpectedMapKeys,
    /// A listed key was absent.
    #[error("branch_control payload key {key} is missing")]
    MissingKey {
        /// The absent key.
        key: u64,
    },
    /// A key held something other than an unsigned integer.
    #[error("branch_control payload key {key} is not an unsigned integer")]
    ExpectedUnsignedInteger {
        /// The offending key.
        key: u64,
    },
    /// A key held something other than a byte string.
    #[error("branch_control payload key {key} is not a byte string")]
    ExpectedByteString {
        /// The offending key.
        key: u64,
    },
    /// A key held something other than an array.
    #[error("branch_control payload key {key} is not an array")]
    ExpectedArray {
        /// The offending key.
        key: u64,
    },
    /// A nullable identifier slot held neither `null` nor a byte string.
    #[error("branch_control payload key {key} must be null or a byte string")]
    ExpectedNullOrByteString {
        /// The offending key.
        key: u64,
    },
    /// A byte string had the wrong fixed length.
    #[error("branch_control payload key {key} holds {actual} byte(s), expected {expected}")]
    WrongByteLength {
        /// The offending key.
        key: u64,
        /// Length the grammar requires.
        expected: usize,
        /// Length actually present.
        actual: usize,
    },
    /// `action` was outside 0..=3.
    #[error("branch_control action {action} is not one of create, pause, retarget, detach")]
    UnknownAction {
        /// The unrecognized action discriminant.
        action: u64,
    },
    /// The encoded frontier array was not strictly ascending, so its bytes are not canonical.
    #[error("selected_frontier[{index}] does not strictly follow its predecessor")]
    FrontierNotStrictlyAscending {
        /// Index of the offending member.
        index: usize,
    },
    /// A create action named a source branch.
    #[error("branch_control create must not name a source branch")]
    SourceBranchForbidden,
    /// A pause, retarget, or detach action named no source branch.
    #[error("branch_control {action:?} requires a source branch")]
    SourceBranchRequired {
        /// The action that requires the source.
        action: BranchControlAction,
    },
    /// The control operation was not a normal intent operation.
    #[error("branch control requires operation_kind 1, found {operation_kind}")]
    ControlOperationKindNotIntent {
        /// The offending operation kind.
        operation_kind: u16,
    },
    /// The control operation's header scope was not exactly `(verse_id, null, null)`.
    #[error("branch control scope must be exactly (verse_id, null, null)")]
    ControlScopeNotVerseWide,
    /// The control operation named a verse other than the registry's.
    #[error("branch control names another verse")]
    ControlVerseMismatch,
    /// The author did not hold a current Manager+ `append/op` capability for the schema.
    #[error("branch control author lacks a Manager+ append/op capability for this schema")]
    Unauthorized,
    /// A create or detach action named a branch the registry already holds.
    #[error("branch {branch_id:?} already exists")]
    BranchAlreadyExists {
        /// The colliding branch.
        branch_id: Identifier32,
    },
    /// The action targeted a branch the registry does not hold.
    #[error("branch {branch_id:?} is not in this registry")]
    UnknownBranch {
        /// The absent branch.
        branch_id: Identifier32,
    },
    /// The action named a source branch the registry does not hold.
    #[error("source branch {branch_id:?} is not in this registry")]
    UnknownSourceBranch {
        /// The absent source branch.
        branch_id: Identifier32,
    },
    /// A create action had no matching admitted `branch_genesis`.
    #[error("branch {branch_id:?} has no admitted branch_genesis operation")]
    BranchGenesisNotAdmitted {
        /// The branch whose genesis is missing.
        branch_id: Identifier32,
    },
    /// A create action's selected frontier was not exactly its admitted genesis operation.
    #[error("branch create must select exactly its admitted branch_genesis operation")]
    CreateFrontierIsNotGenesis,
    /// A referenced operation is not an admitted operation of this verse DAG.
    #[error("operation {op_id:?} is not admitted in this verse")]
    OperationNotInVerse {
        /// The offending operation.
        op_id: Hash32,
    },
    /// A referenced operation is one of two candidates sharing an equivocation key (§3.4).
    #[error("operation {op_id:?} equivocates with {conflicting_op_id:?}; both stay quarantined")]
    EquivocatingHead {
        /// The candidate that was offered as a head.
        op_id: Hash32,
        /// The other candidate sharing its `EquivocationKey`.
        conflicting_op_id: Hash32,
    },
    /// The supplied frontier was not replay-verified under SPEC-4.
    #[error("the supplied frontier is not replay-verified for branch {branch_id:?}")]
    FrontierNotReplayVerified {
        /// The branch whose selection was refused.
        branch_id: Identifier32,
    },
    /// A pause action did not record the target's current selected frontier.
    #[error("branch pause must record the target's current selected frontier")]
    PausedFrontierMismatch,
    /// A mutation was attempted against a detached selection.
    #[error("detached branch {branch_id:?} has an immutable selection")]
    DetachedSelectionIsImmutable {
        /// The detached branch.
        branch_id: Identifier32,
    },
    /// A tracking-only transition was attempted against a branch in another mode.
    #[error("branch {branch_id:?} is {mode:?}, not tracking")]
    NotTracking {
        /// The branch.
        branch_id: Identifier32,
        /// The mode it is actually in.
        mode: BranchMode,
    },
    /// A paused-only transition was attempted against a branch in another mode.
    #[error("branch {branch_id:?} is {mode:?}, not paused")]
    NotPaused {
        /// The branch.
        branch_id: Identifier32,
        /// The mode it is actually in.
        mode: BranchMode,
    },
}

/// The header facts an authority needs to decide a §2.2 rule 3 branch-control append.
#[derive(Clone, Copy, Debug)]
pub struct BranchControlRequest<'a> {
    /// Signing author of the control operation.
    pub author: &'a Author,
    /// Header scope, which §2.2 rule 3 fixes at `(verse_id, null, null)`.
    pub scope: Scope,
    /// Capability chain and epoch the author relied on.
    pub capability: &'a CapabilityRef,
    /// Header `operation_kind`, which §2.2 rule 3 fixes at 1.
    pub operation_kind: u16,
    /// Hash of the registered branch-control schema.
    pub schema_hash: Hash32,
    /// The control action being requested.
    pub action: BranchControlAction,
}

/// The §2.2 rule 3 Manager+ `append/op` gate, injected so no capability type is imported.
pub trait ManagerAppendOpAuthority {
    /// True when the author holds a current Manager+ `append/op` capability that permits this
    /// schema, operation kind, and exact verse scope.
    fn permits_branch_control(&self, request: &BranchControlRequest<'_>) -> bool;
}

/// The verse DAG facts the registry cannot derive on its own; SPEC-4 storage implements it.
pub trait VerseDagView {
    /// Admission state of one operation within this verse.
    fn head_admission(&self, verse_id: Identifier32, op_id: Hash32) -> HeadAdmission;

    /// The admitted `branch_genesis` (kind 2) operation of a branch, if one exists.
    fn admitted_branch_genesis(
        &self,
        verse_id: Identifier32,
        branch_id: Identifier32,
    ) -> Option<Hash32>;

    /// True when deterministic SPEC-4 replay reproduces this exact selection for the branch.
    fn frontier_is_replay_verified(
        &self,
        verse_id: Identifier32,
        branch_id: Identifier32,
        frontier: &SortedFrontier,
    ) -> bool;
}

/// One admitted branch-control operation: its §2.2 rule 3 header facts and decrypted payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchControlOperation {
    /// Content address of the complete envelope.
    pub op_id: Hash32,
    /// Header `operation_kind`.
    pub operation_kind: u16,
    /// Header scope.
    pub scope: Scope,
    /// Header author.
    pub author: Author,
    /// Header capability reference.
    pub capability: CapabilityRef,
    /// Header schema hash.
    pub schema_hash: Hash32,
    /// The decrypted §2.2 rule 4 payload.
    pub payload: BranchControlPayload,
}

impl BranchControlOperation {
    /// Pairs an admitted envelope's header facts with its decrypted branch-control payload.
    pub fn from_admitted(
        op_id: Hash32,
        unsigned: &UnsignedEnvelope,
        payload: BranchControlPayload,
    ) -> Self {
        Self {
            op_id,
            operation_kind: unsigned.operation_kind,
            scope: unsigned.scope,
            author: unsigned.author.clone(),
            capability: unsigned.capability,
            schema_hash: unsigned.schema_hash,
            payload,
        }
    }

    /// The authority request these header facts describe.
    pub fn authority_request(&self) -> BranchControlRequest<'_> {
        BranchControlRequest {
            author: &self.author,
            scope: self.scope,
            capability: &self.capability,
            operation_kind: self.operation_kind,
            schema_hash: self.schema_hash,
            action: self.payload.action,
        }
    }
}
