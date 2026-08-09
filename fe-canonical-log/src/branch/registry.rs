//! The verse branch registry and the §2.1 mode transitions, as pure functions over injected
//! authority and DAG seams.

use std::collections::BTreeMap;

use crate::envelope::{Hash32, Identifier32};
use crate::frontier::SortedFrontier;

use super::{
    BranchControlAction, BranchControlOperation, BranchError, BranchMode, HeadAdmission,
    ManagerAppendOpAuthority, VerseDagView, BRANCH_CONTROL_OPERATION_KIND,
};

/// One branch's materialized state; fields are private so a detached selection cannot be
/// rewritten from outside this module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchRecord {
    branch_id: Identifier32,
    verse_id: Identifier32,
    mode: BranchMode,
    materialized_frontier: SortedFrontier,
    received_frontier: Option<SortedFrontier>,
    source_branch_id: Option<Identifier32>,
}

impl BranchRecord {
    /// The branch this record describes.
    pub const fn branch_id(&self) -> Identifier32 {
        self.branch_id
    }

    /// The verse this branch belongs to; a registry never spans verses.
    pub const fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// The current §2.1 mode.
    pub const fn mode(&self) -> BranchMode {
        self.mode
    }

    /// The committed selection the exposed projection and analytics position derive from.
    pub const fn materialized_frontier(&self) -> &SortedFrontier {
        &self.materialized_frontier
    }

    /// Evidence received while paused, which never becomes a committed selection on its own.
    pub fn received_frontier(&self) -> Option<&SortedFrontier> {
        self.received_frontier.as_ref()
    }

    /// The branch a paused, retargeted, or detached selection derives from.
    pub const fn source_branch_id(&self) -> Option<Identifier32> {
        self.source_branch_id
    }

    /// The D-CL19 commitment of the committed selection.
    pub fn materialized_frontier_commitment(&self) -> Hash32 {
        self.materialized_frontier.commitment()
    }

    /// True while this record's selection is pinned and immutable (§2.1 rule 3).
    pub const fn is_detached(&self) -> bool {
        matches!(self.mode, BranchMode::Detached)
    }
}

/// What an admitted branch-control operation did to the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchControlEffect {
    /// A new tracking branch was registered at its genesis.
    Created {
        /// The new branch.
        branch_id: Identifier32,
    },
    /// A tracking branch froze its committed selection.
    Paused {
        /// The paused branch.
        branch_id: Identifier32,
    },
    /// A tracking branch adopted a replay-verified selection.
    Retargeted {
        /// The retargeted branch.
        branch_id: Identifier32,
    },
    /// A new detached branch pinned an immutable selection.
    Detached {
        /// The new detached branch.
        branch_id: Identifier32,
        /// The branch its selection derives from.
        source_branch_id: Identifier32,
    },
}

/// One verse's branch registry (§1 rule 2: one registry per verse, never per petal).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BranchRegistry {
    verse_id: Identifier32,
    branches: BTreeMap<Identifier32, BranchRecord>,
}

impl BranchRegistry {
    /// An empty registry for one verse.
    pub fn new(verse_id: Identifier32) -> Self {
        Self {
            verse_id,
            branches: BTreeMap::new(),
        }
    }

    /// The verse this registry covers.
    pub const fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// The record for one branch.
    pub fn branch(&self, branch_id: Identifier32) -> Option<&BranchRecord> {
        self.branches.get(&branch_id)
    }

    /// Every branch in registration-independent identifier order.
    pub fn records(&self) -> impl Iterator<Item = &BranchRecord> {
        self.branches.values()
    }

    /// Number of registered branches.
    pub fn len(&self) -> usize {
        self.branches.len()
    }

    /// True while no branch is registered.
    pub fn is_empty(&self) -> bool {
        self.branches.is_empty()
    }

    /// Applies one admitted branch-control operation (§2.2 rules 3 through 5).
    ///
    /// The operation itself is immutable verse history; this derives its registry effect only
    /// after the header, the Manager+ gate, and every referenced operation check out.
    pub fn apply_control_operation(
        &mut self,
        control: &BranchControlOperation,
        authority: &dyn ManagerAppendOpAuthority,
        dag: &dyn VerseDagView,
    ) -> Result<BranchControlEffect, BranchError> {
        self.check_control_header(control)?;
        if !authority.permits_branch_control(&control.authority_request()) {
            return Err(BranchError::Unauthorized);
        }
        let payload = &control.payload;
        self.check_frontier_members(&payload.selected_frontier, dag)?;

        match payload.action {
            BranchControlAction::Create => self.create(control, dag),
            BranchControlAction::Pause => self.pause(control),
            BranchControlAction::Retarget => self.retarget(control, dag),
            BranchControlAction::Detach => self.detach(control),
        }
    }

    /// Absorbs one admitted operation into a tracking branch's frontier (§2.1 tracking row).
    ///
    /// The new frontier is the old heads minus this operation's parents, plus this operation:
    /// a set, never a receipt-order winner. Causal completeness is the SPEC-4 admission
    /// layer's obligation, so a late-arriving sibling of an already-absorbed head correctly
    /// widens the frontier instead of being refused here.
    pub fn advance_tracking_frontier(
        &mut self,
        branch_id: Identifier32,
        op_id: Hash32,
        parents: &[Hash32],
        dag: &dyn VerseDagView,
    ) -> Result<&SortedFrontier, BranchError> {
        let verse_id = self.verse_id;
        check_head_admission(verse_id, op_id, dag)?;
        let record = self.mutable_record(branch_id)?;
        if record.mode != BranchMode::Tracking {
            return Err(BranchError::NotTracking {
                branch_id,
                mode: record.mode,
            });
        }
        record.materialized_frontier = advance(&record.materialized_frontier, op_id, parents)?;
        Ok(&record.materialized_frontier)
    }

    /// Records evidence received while paused (§2.1 rule 2).
    ///
    /// It advances the received frontier only. The materialized frontier — and therefore every
    /// exposed projection and committed analytics position — stays at the last committed
    /// selection until [`BranchRegistry::resume_tracking`] succeeds.
    pub fn record_paused_evidence(
        &mut self,
        branch_id: Identifier32,
        op_id: Hash32,
        parents: &[Hash32],
        dag: &dyn VerseDagView,
    ) -> Result<&SortedFrontier, BranchError> {
        let verse_id = self.verse_id;
        check_head_admission(verse_id, op_id, dag)?;
        let record = self.mutable_record(branch_id)?;
        if record.mode != BranchMode::Paused {
            return Err(BranchError::NotPaused {
                branch_id,
                mode: record.mode,
            });
        }
        let received = record
            .received_frontier
            .clone()
            .unwrap_or_else(|| record.materialized_frontier.clone());
        record.received_frontier = Some(advance(&received, op_id, parents)?);
        Ok(record
            .received_frontier
            .as_ref()
            .expect("the received frontier was just assigned"))
    }

    /// Returns a paused branch to tracking against a replay-verified selection (§2.1 rule 5).
    pub fn resume_tracking(
        &mut self,
        branch_id: Identifier32,
        replay_verified_frontier: &SortedFrontier,
        dag: &dyn VerseDagView,
    ) -> Result<(), BranchError> {
        let verse_id = self.verse_id;
        self.check_frontier_members(replay_verified_frontier, dag)?;
        if !dag.frontier_is_replay_verified(verse_id, branch_id, replay_verified_frontier) {
            return Err(BranchError::FrontierNotReplayVerified { branch_id });
        }
        let record = self.mutable_record(branch_id)?;
        if record.mode != BranchMode::Paused {
            return Err(BranchError::NotPaused {
                branch_id,
                mode: record.mode,
            });
        }
        record.mode = BranchMode::Tracking;
        record.materialized_frontier = replay_verified_frontier.clone();
        record.received_frontier = None;
        Ok(())
    }

    fn check_control_header(&self, control: &BranchControlOperation) -> Result<(), BranchError> {
        if control.operation_kind != BRANCH_CONTROL_OPERATION_KIND {
            return Err(BranchError::ControlOperationKindNotIntent {
                operation_kind: control.operation_kind,
            });
        }
        if control.scope.petal_id().is_some() || control.scope.resource_id().is_some() {
            return Err(BranchError::ControlScopeNotVerseWide);
        }
        if control.scope.verse_id() != self.verse_id {
            return Err(BranchError::ControlVerseMismatch);
        }
        Ok(())
    }

    fn check_frontier_members(
        &self,
        frontier: &SortedFrontier,
        dag: &dyn VerseDagView,
    ) -> Result<(), BranchError> {
        for op_id in frontier.as_slice() {
            check_head_admission(self.verse_id, *op_id, dag)?;
        }
        Ok(())
    }

    fn mutable_record(
        &mut self,
        branch_id: Identifier32,
    ) -> Result<&mut BranchRecord, BranchError> {
        let record = self
            .branches
            .get_mut(&branch_id)
            .ok_or(BranchError::UnknownBranch { branch_id })?;
        if record.is_detached() {
            return Err(BranchError::DetachedSelectionIsImmutable { branch_id });
        }
        Ok(record)
    }

    fn create(
        &mut self,
        control: &BranchControlOperation,
        dag: &dyn VerseDagView,
    ) -> Result<BranchControlEffect, BranchError> {
        let branch_id = control.payload.target_branch_id;
        if self.branches.contains_key(&branch_id) {
            return Err(BranchError::BranchAlreadyExists { branch_id });
        }
        let genesis = dag
            .admitted_branch_genesis(self.verse_id, branch_id)
            .ok_or(BranchError::BranchGenesisNotAdmitted { branch_id })?;
        if control.payload.selected_frontier.as_slice() != [genesis] {
            return Err(BranchError::CreateFrontierIsNotGenesis);
        }
        self.branches.insert(
            branch_id,
            BranchRecord {
                branch_id,
                verse_id: self.verse_id,
                mode: BranchMode::Tracking,
                materialized_frontier: control.payload.selected_frontier.clone(),
                received_frontier: None,
                source_branch_id: None,
            },
        );
        Ok(BranchControlEffect::Created { branch_id })
    }

    fn pause(
        &mut self,
        control: &BranchControlOperation,
    ) -> Result<BranchControlEffect, BranchError> {
        let branch_id = control.payload.target_branch_id;
        let source_branch_id = self.require_known_source(control)?;
        let record = self.mutable_record(branch_id)?;
        if record.mode != BranchMode::Tracking {
            return Err(BranchError::NotTracking {
                branch_id,
                mode: record.mode,
            });
        }
        if control.payload.selected_frontier != record.materialized_frontier {
            return Err(BranchError::PausedFrontierMismatch);
        }
        record.mode = BranchMode::Paused;
        record.received_frontier = Some(record.materialized_frontier.clone());
        record.source_branch_id = Some(source_branch_id);
        Ok(BranchControlEffect::Paused { branch_id })
    }

    fn retarget(
        &mut self,
        control: &BranchControlOperation,
        dag: &dyn VerseDagView,
    ) -> Result<BranchControlEffect, BranchError> {
        let branch_id = control.payload.target_branch_id;
        let source_branch_id = self.require_known_source(control)?;
        let verse_id = self.verse_id;
        if !dag.frontier_is_replay_verified(verse_id, branch_id, &control.payload.selected_frontier)
        {
            return Err(BranchError::FrontierNotReplayVerified { branch_id });
        }
        let record = self.mutable_record(branch_id)?;
        if record.mode != BranchMode::Tracking {
            return Err(BranchError::NotTracking {
                branch_id,
                mode: record.mode,
            });
        }
        record.materialized_frontier = control.payload.selected_frontier.clone();
        record.received_frontier = None;
        record.source_branch_id = Some(source_branch_id);
        Ok(BranchControlEffect::Retargeted { branch_id })
    }

    /// Pins a new immutable selection; this module offers no path that later changes it.
    ///
    /// Detached work re-enters a tracking branch only through the SPEC-1 kind-3 merge
    /// operation, whose admissibility and CRDT reduction belong to SPEC-4, not here.
    fn detach(
        &mut self,
        control: &BranchControlOperation,
    ) -> Result<BranchControlEffect, BranchError> {
        let branch_id = control.payload.target_branch_id;
        let source_branch_id = self.require_known_source(control)?;
        if self.branches.contains_key(&branch_id) {
            return Err(BranchError::BranchAlreadyExists { branch_id });
        }
        self.branches.insert(
            branch_id,
            BranchRecord {
                branch_id,
                verse_id: self.verse_id,
                mode: BranchMode::Detached,
                materialized_frontier: control.payload.selected_frontier.clone(),
                received_frontier: None,
                source_branch_id: Some(source_branch_id),
            },
        );
        Ok(BranchControlEffect::Detached {
            branch_id,
            source_branch_id,
        })
    }

    fn require_known_source(
        &self,
        control: &BranchControlOperation,
    ) -> Result<Identifier32, BranchError> {
        let source_branch_id =
            control
                .payload
                .source_branch_id
                .ok_or(BranchError::SourceBranchRequired {
                    action: control.payload.action,
                })?;
        if !self.branches.contains_key(&source_branch_id) {
            return Err(BranchError::UnknownSourceBranch {
                branch_id: source_branch_id,
            });
        }
        Ok(source_branch_id)
    }
}

fn check_head_admission(
    verse_id: Identifier32,
    op_id: Hash32,
    dag: &dyn VerseDagView,
) -> Result<(), BranchError> {
    match dag.head_admission(verse_id, op_id) {
        HeadAdmission::Admitted => Ok(()),
        HeadAdmission::NotInVerse => Err(BranchError::OperationNotInVerse { op_id }),
        HeadAdmission::QuarantinedEquivocation { conflicting_op_id } => {
            Err(BranchError::EquivocatingHead {
                op_id,
                conflicting_op_id,
            })
        }
    }
}

/// Old heads minus this operation's parents, plus this operation.
fn advance(
    current: &SortedFrontier,
    op_id: Hash32,
    parents: &[Hash32],
) -> Result<SortedFrontier, BranchError> {
    let mut heads: Vec<Hash32> = current
        .as_slice()
        .iter()
        .copied()
        .filter(|head| !parents.contains(head) && *head != op_id)
        .collect();
    heads.push(op_id);
    Ok(SortedFrontier::try_new(heads)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branch::{BranchControlPayload, BranchControlRequest};
    use crate::envelope::{
        Author, CapabilityRef, Hlc, PayloadRef, Scope, UnsignedEnvelope, PROTOCOL_VERSION,
    };
    use crate::kind::{validate_structural_rules, StructuralRuleError};
    use std::collections::BTreeSet;

    const VERSE: Identifier32 = Identifier32([0x0e; 32]);
    const MAIN: Identifier32 = Identifier32([0x0a; 32]);
    const SIDE: Identifier32 = Identifier32([0x0b; 32]);

    fn op_id(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn frontier(op_ids: impl IntoIterator<Item = Hash32>) -> SortedFrontier {
        SortedFrontier::try_new(op_ids).expect("frontier")
    }

    /// Admits every listed operation, resolves genesis from a map, and replay-verifies a set.
    #[derive(Default)]
    struct FakeVerseDag {
        admitted: BTreeSet<Hash32>,
        equivocating: BTreeMap<Hash32, Hash32>,
        genesis: BTreeMap<Identifier32, Hash32>,
        replay_verified: BTreeSet<Vec<Hash32>>,
    }

    impl FakeVerseDag {
        fn admitting(op_ids: impl IntoIterator<Item = Hash32>) -> Self {
            Self {
                admitted: op_ids.into_iter().collect(),
                ..Self::default()
            }
        }

        fn with_genesis(mut self, branch_id: Identifier32, genesis_op_id: Hash32) -> Self {
            self.admitted.insert(genesis_op_id);
            self.genesis.insert(branch_id, genesis_op_id);
            self
        }

        fn with_replay_verified(mut self, verified: &SortedFrontier) -> Self {
            self.replay_verified.insert(verified.as_slice().to_vec());
            self
        }

        fn with_equivocation(mut self, op_id: Hash32, conflicting_op_id: Hash32) -> Self {
            self.admitted.insert(op_id);
            self.equivocating.insert(op_id, conflicting_op_id);
            self
        }
    }

    impl VerseDagView for FakeVerseDag {
        fn head_admission(&self, verse_id: Identifier32, op_id: Hash32) -> HeadAdmission {
            if verse_id != VERSE || !self.admitted.contains(&op_id) {
                return HeadAdmission::NotInVerse;
            }
            match self.equivocating.get(&op_id) {
                Some(conflicting_op_id) => HeadAdmission::QuarantinedEquivocation {
                    conflicting_op_id: *conflicting_op_id,
                },
                None => HeadAdmission::Admitted,
            }
        }

        fn admitted_branch_genesis(
            &self,
            verse_id: Identifier32,
            branch_id: Identifier32,
        ) -> Option<Hash32> {
            (verse_id == VERSE)
                .then(|| self.genesis.get(&branch_id).copied())
                .flatten()
        }

        fn frontier_is_replay_verified(
            &self,
            verse_id: Identifier32,
            _branch_id: Identifier32,
            candidate: &SortedFrontier,
        ) -> bool {
            verse_id == VERSE && self.replay_verified.contains(candidate.as_slice())
        }
    }

    /// Grants or refuses every branch-control append.
    struct FakeAuthority {
        manager_plus: bool,
    }

    impl ManagerAppendOpAuthority for FakeAuthority {
        fn permits_branch_control(&self, request: &BranchControlRequest<'_>) -> bool {
            self.manager_plus
                && request.operation_kind == BRANCH_CONTROL_OPERATION_KIND
                && request.scope.petal_id().is_none()
        }
    }

    fn control(
        action: BranchControlAction,
        target_branch_id: Identifier32,
        selected_frontier: SortedFrontier,
        source_branch_id: Option<Identifier32>,
    ) -> BranchControlOperation {
        BranchControlOperation {
            op_id: op_id(0xc0),
            operation_kind: BRANCH_CONTROL_OPERATION_KIND,
            scope: Scope::verse_wide(VERSE),
            author: Author::from_public_key([0x22; 32]),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch: 4,
            },
            schema_hash: Hash32([0x44; 32]),
            payload: BranchControlPayload {
                action,
                target_branch_id,
                selected_frontier,
                source_branch_id,
            },
        }
    }

    /// A registry holding one tracking branch created from its admitted genesis.
    fn registry_with_main(genesis_op_id: Hash32) -> (BranchRegistry, FakeVerseDag) {
        let dag = FakeVerseDag::admitting([genesis_op_id]).with_genesis(MAIN, genesis_op_id);
        let mut registry = BranchRegistry::new(VERSE);
        let effect = registry
            .apply_control_operation(
                &control(
                    BranchControlAction::Create,
                    MAIN,
                    frontier([genesis_op_id]),
                    None,
                ),
                &FakeAuthority { manager_plus: true },
                &dag,
            )
            .expect("create");
        assert_eq!(effect, BranchControlEffect::Created { branch_id: MAIN });
        (registry, dag)
    }

    #[test]
    fn tracking_preserves_sorted_concurrent_frontier() {
        let genesis = op_id(0x10);
        let left = op_id(0x40);
        let right = op_id(0x20);
        let third = op_id(0x30);
        let dag =
            FakeVerseDag::admitting([genesis, left, right, third]).with_genesis(MAIN, genesis);

        let arrivals = [
            [(left, genesis), (right, genesis), (third, genesis)],
            [(right, genesis), (third, genesis), (left, genesis)],
            [(third, genesis), (left, genesis), (right, genesis)],
            [(third, genesis), (right, genesis), (left, genesis)],
            [(left, genesis), (third, genesis), (right, genesis)],
            [(right, genesis), (left, genesis), (third, genesis)],
        ];

        let mut commitments = BTreeSet::new();
        for permutation in arrivals {
            let (mut registry, _) = registry_with_main(genesis);
            for (arriving, parent) in permutation {
                registry
                    .advance_tracking_frontier(MAIN, arriving, &[parent], &dag)
                    .expect("advance");
            }
            let record = registry.branch(MAIN).expect("branch");
            assert_eq!(
                record.materialized_frontier().as_slice(),
                &[right, third, left],
                "concurrent heads stay a byte-sorted set, never a receipt-order winner"
            );
            commitments.insert(record.materialized_frontier_commitment());
        }
        assert_eq!(
            commitments.len(),
            1,
            "every arrival permutation commits to one D-CL19 frontier"
        );

        let (mut registry, _) = registry_with_main(genesis);
        registry
            .advance_tracking_frontier(MAIN, left, &[genesis], &dag)
            .expect("advance");
        registry
            .advance_tracking_frontier(MAIN, right, &[genesis], &dag)
            .expect("advance");
        registry
            .advance_tracking_frontier(MAIN, third, &[left, right], &dag)
            .expect("advance");
        assert_eq!(
            registry
                .branch(MAIN)
                .expect("branch")
                .materialized_frontier()
                .as_slice(),
            &[third],
            "an operation naming both heads as parents collapses them causally, not by receipt"
        );
    }

    #[test]
    fn paused_receipt_never_advances_materialized_projection() {
        let genesis = op_id(0x10);
        let later = op_id(0x50);
        let dag = FakeVerseDag::admitting([genesis, later])
            .with_genesis(MAIN, genesis)
            .with_replay_verified(&frontier([later]));
        let (mut registry, _) = registry_with_main(genesis);

        registry
            .apply_control_operation(
                &control(
                    BranchControlAction::Pause,
                    MAIN,
                    frontier([genesis]),
                    Some(MAIN),
                ),
                &FakeAuthority { manager_plus: true },
                &dag,
            )
            .expect("pause");
        assert_eq!(
            registry.branch(MAIN).expect("branch").mode(),
            BranchMode::Paused
        );

        let committed = registry
            .branch(MAIN)
            .expect("branch")
            .materialized_frontier_commitment();
        registry
            .record_paused_evidence(MAIN, later, &[genesis], &dag)
            .expect("evidence");
        let record = registry.branch(MAIN).expect("branch");
        assert_eq!(
            record.received_frontier().map(SortedFrontier::as_slice),
            Some(&[later][..])
        );
        assert_eq!(record.materialized_frontier().as_slice(), &[genesis]);
        assert_eq!(record.materialized_frontier_commitment(), committed);

        assert_eq!(
            registry.advance_tracking_frontier(MAIN, later, &[genesis], &dag),
            Err(BranchError::NotTracking {
                branch_id: MAIN,
                mode: BranchMode::Paused,
            })
        );

        assert_eq!(
            registry.resume_tracking(MAIN, &frontier([genesis]), &dag),
            Err(BranchError::FrontierNotReplayVerified { branch_id: MAIN }),
            "resume needs deterministic replay, not the queued arrival order"
        );
        registry
            .resume_tracking(MAIN, &frontier([later]), &dag)
            .expect("resume");
        let record = registry.branch(MAIN).expect("branch");
        assert_eq!(record.mode(), BranchMode::Tracking);
        assert_eq!(record.materialized_frontier().as_slice(), &[later]);
        assert_eq!(record.received_frontier(), None);
        assert_ne!(record.materialized_frontier_commitment(), committed);
    }

    #[test]
    fn detached_selection_is_immutable_and_reintegration_is_explicit() {
        let genesis = op_id(0x10);
        let pinned = op_id(0x20);
        let tracked = op_id(0x30);
        let merge = op_id(0x60);
        let dag = FakeVerseDag::admitting([genesis, pinned, tracked, merge])
            .with_genesis(MAIN, genesis)
            .with_replay_verified(&frontier([tracked]));
        let (mut registry, _) = registry_with_main(genesis);
        registry
            .advance_tracking_frontier(MAIN, pinned, &[genesis], &dag)
            .expect("advance");

        registry
            .apply_control_operation(
                &control(
                    BranchControlAction::Detach,
                    SIDE,
                    frontier([pinned]),
                    Some(MAIN),
                ),
                &FakeAuthority { manager_plus: true },
                &dag,
            )
            .expect("detach");
        let detached = registry.branch(SIDE).expect("branch").clone();
        assert!(detached.is_detached());
        assert_eq!(detached.source_branch_id(), Some(MAIN));

        registry
            .advance_tracking_frontier(MAIN, tracked, &[pinned], &dag)
            .expect("advance");
        assert_eq!(
            registry.branch(SIDE).expect("branch"),
            &detached,
            "a detached selection does not follow later tracking updates"
        );

        assert_eq!(
            registry.advance_tracking_frontier(SIDE, tracked, &[pinned], &dag),
            Err(BranchError::DetachedSelectionIsImmutable { branch_id: SIDE })
        );
        assert_eq!(
            registry.record_paused_evidence(SIDE, tracked, &[pinned], &dag),
            Err(BranchError::DetachedSelectionIsImmutable { branch_id: SIDE })
        );
        assert_eq!(
            registry.resume_tracking(SIDE, &frontier([tracked]), &dag),
            Err(BranchError::DetachedSelectionIsImmutable { branch_id: SIDE })
        );
        assert_eq!(
            registry.apply_control_operation(
                &control(
                    BranchControlAction::Retarget,
                    SIDE,
                    frontier([tracked]),
                    Some(MAIN),
                ),
                &FakeAuthority { manager_plus: true },
                &dag,
            ),
            Err(BranchError::DetachedSelectionIsImmutable { branch_id: SIDE })
        );
        assert_eq!(
            registry.branch(SIDE).expect("branch"),
            &detached,
            "no registry path rewrites a pinned selection"
        );

        let single_parent_merge = merge_envelope(&[pinned]);
        assert_eq!(
            validate_structural_rules(&single_parent_merge),
            Err(StructuralRuleError::DetachedMergeNeedsTwoParents { count: 1 }),
            "a mode toggle or single-parent operation is not a reintegration"
        );
        let mut reintegration = merge_envelope(&[pinned, tracked]);
        reintegration.parents.sort_unstable();
        validate_structural_rules(&reintegration).expect("kind-3 merge");

        registry
            .advance_tracking_frontier(MAIN, merge, &reintegration.parents, &dag)
            .expect("reintegrate");
        assert_eq!(
            registry
                .branch(MAIN)
                .expect("branch")
                .materialized_frontier()
                .as_slice(),
            &[merge],
            "the detached head enters tracking only through the multi-parent kind-3 merge"
        );
        assert_eq!(registry.branch(SIDE).expect("branch"), &detached);
    }

    /// A kind-3 detached-to-tracking merge envelope over the given parents.
    fn merge_envelope(parents: &[Hash32]) -> UnsignedEnvelope {
        UnsignedEnvelope {
            protocol_version: PROTOCOL_VERSION,
            operation_kind: 3,
            scope: Scope::verse_wide(VERSE),
            author: Author::from_public_key([0x22; 32]),
            capability: CapabilityRef {
                chain_id: Hash32([0x33; 32]),
                scope_epoch: 4,
            },
            schema_hash: Hash32([0x44; 32]),
            branch_id: MAIN,
            parents: parents.to_vec(),
            hlc: Hlc::new(1_700_000_000_000, 1),
            payload: PayloadRef::empty(),
        }
    }

    #[test]
    fn branch_control_requires_a_verse_wide_intent_header_and_manager_plus_authority() {
        let genesis = op_id(0x10);
        let (mut registry, dag) = registry_with_main(genesis);
        let authority = FakeAuthority { manager_plus: true };

        let mut wrong_kind = control(
            BranchControlAction::Pause,
            MAIN,
            frontier([genesis]),
            Some(MAIN),
        );
        wrong_kind.operation_kind = 4;
        assert_eq!(
            registry.apply_control_operation(&wrong_kind, &authority, &dag),
            Err(BranchError::ControlOperationKindNotIntent { operation_kind: 4 })
        );

        let mut petal_scoped = control(
            BranchControlAction::Pause,
            MAIN,
            frontier([genesis]),
            Some(MAIN),
        );
        petal_scoped.scope =
            Scope::new(VERSE, Some(Identifier32([0x77; 32])), None).expect("petal scope");
        assert_eq!(
            registry.apply_control_operation(&petal_scoped, &authority, &dag),
            Err(BranchError::ControlScopeNotVerseWide)
        );

        let mut other_verse = control(
            BranchControlAction::Pause,
            MAIN,
            frontier([genesis]),
            Some(MAIN),
        );
        other_verse.scope = Scope::verse_wide(Identifier32([0x99; 32]));
        assert_eq!(
            registry.apply_control_operation(&other_verse, &authority, &dag),
            Err(BranchError::ControlVerseMismatch)
        );

        assert_eq!(
            registry.apply_control_operation(
                &control(
                    BranchControlAction::Pause,
                    MAIN,
                    frontier([genesis]),
                    Some(MAIN)
                ),
                &FakeAuthority {
                    manager_plus: false
                },
                &dag,
            ),
            Err(BranchError::Unauthorized)
        );
        assert_eq!(
            registry.branch(MAIN).expect("branch").mode(),
            BranchMode::Tracking,
            "a refused control operation alters no branch selection"
        );
    }

    #[test]
    fn an_equivocating_candidate_never_enters_a_frontier() {
        let genesis = op_id(0x10);
        let equivocating = op_id(0x70);
        let conflicting = op_id(0x71);
        let dag = FakeVerseDag::admitting([genesis])
            .with_genesis(MAIN, genesis)
            .with_equivocation(equivocating, conflicting);
        let (mut registry, _) = registry_with_main(genesis);

        assert_eq!(
            registry.advance_tracking_frontier(MAIN, equivocating, &[genesis], &dag),
            Err(BranchError::EquivocatingHead {
                op_id: equivocating,
                conflicting_op_id: conflicting,
            })
        );
        assert_eq!(
            registry.apply_control_operation(
                &control(
                    BranchControlAction::Detach,
                    SIDE,
                    frontier([equivocating]),
                    Some(MAIN),
                ),
                &FakeAuthority { manager_plus: true },
                &dag,
            ),
            Err(BranchError::EquivocatingHead {
                op_id: equivocating,
                conflicting_op_id: conflicting,
            })
        );
        assert_eq!(registry.branch(SIDE), None);
        assert_eq!(
            registry
                .branch(MAIN)
                .expect("branch")
                .materialized_frontier()
                .as_slice(),
            &[genesis],
            "neither equivocating candidate is materialized"
        );

        assert_eq!(
            registry.advance_tracking_frontier(MAIN, op_id(0xee), &[genesis], &dag),
            Err(BranchError::OperationNotInVerse { op_id: op_id(0xee) })
        );
    }

    #[test]
    fn create_requires_a_matching_admitted_branch_genesis() {
        let genesis = op_id(0x10);
        let other = op_id(0x11);
        let dag = FakeVerseDag::admitting([genesis, other]).with_genesis(MAIN, genesis);
        let mut registry = BranchRegistry::new(VERSE);
        let authority = FakeAuthority { manager_plus: true };

        assert_eq!(
            registry.apply_control_operation(
                &control(BranchControlAction::Create, SIDE, frontier([other]), None),
                &authority,
                &dag,
            ),
            Err(BranchError::BranchGenesisNotAdmitted { branch_id: SIDE })
        );
        assert_eq!(
            registry.apply_control_operation(
                &control(BranchControlAction::Create, MAIN, frontier([other]), None),
                &authority,
                &dag,
            ),
            Err(BranchError::CreateFrontierIsNotGenesis)
        );
        registry
            .apply_control_operation(
                &control(BranchControlAction::Create, MAIN, frontier([genesis]), None),
                &authority,
                &dag,
            )
            .expect("create");
        assert_eq!(
            registry.apply_control_operation(
                &control(BranchControlAction::Create, MAIN, frontier([genesis]), None),
                &authority,
                &dag,
            ),
            Err(BranchError::BranchAlreadyExists { branch_id: MAIN })
        );
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());
        assert_eq!(registry.records().count(), 1);
        assert_eq!(registry.verse_id(), VERSE);
    }

    #[test]
    fn retarget_adopts_only_a_replay_verified_frontier_from_a_known_source() {
        let genesis = op_id(0x10);
        let verified = op_id(0x20);
        let unverified = op_id(0x21);
        let dag = FakeVerseDag::admitting([genesis, verified, unverified])
            .with_genesis(MAIN, genesis)
            .with_replay_verified(&frontier([verified]));
        let (mut registry, _) = registry_with_main(genesis);
        let authority = FakeAuthority { manager_plus: true };

        assert_eq!(
            registry.apply_control_operation(
                &control(
                    BranchControlAction::Retarget,
                    MAIN,
                    frontier([verified]),
                    Some(SIDE),
                ),
                &authority,
                &dag,
            ),
            Err(BranchError::UnknownSourceBranch { branch_id: SIDE })
        );
        assert_eq!(
            registry.apply_control_operation(
                &control(
                    BranchControlAction::Retarget,
                    MAIN,
                    frontier([unverified]),
                    Some(MAIN),
                ),
                &authority,
                &dag,
            ),
            Err(BranchError::FrontierNotReplayVerified { branch_id: MAIN })
        );
        assert_eq!(
            registry
                .branch(MAIN)
                .expect("branch")
                .materialized_frontier()
                .as_slice(),
            &[genesis]
        );

        registry
            .apply_control_operation(
                &control(
                    BranchControlAction::Retarget,
                    MAIN,
                    frontier([verified]),
                    Some(MAIN),
                ),
                &authority,
                &dag,
            )
            .expect("retarget");
        assert_eq!(
            registry
                .branch(MAIN)
                .expect("branch")
                .materialized_frontier()
                .as_slice(),
            &[verified]
        );
    }

    #[test]
    fn pause_must_record_the_targets_current_selected_frontier() {
        let genesis = op_id(0x10);
        let stale = op_id(0x22);
        let dag = FakeVerseDag::admitting([genesis, stale]).with_genesis(MAIN, genesis);
        let (mut registry, _) = registry_with_main(genesis);

        assert_eq!(
            registry.apply_control_operation(
                &control(
                    BranchControlAction::Pause,
                    MAIN,
                    frontier([stale]),
                    Some(MAIN)
                ),
                &FakeAuthority { manager_plus: true },
                &dag,
            ),
            Err(BranchError::PausedFrontierMismatch)
        );
        assert_eq!(
            registry.branch(MAIN).expect("branch").mode(),
            BranchMode::Tracking
        );
        assert_eq!(
            registry.record_paused_evidence(MAIN, stale, &[genesis], &dag),
            Err(BranchError::NotPaused {
                branch_id: MAIN,
                mode: BranchMode::Tracking,
            })
        );
        assert_eq!(
            registry.advance_tracking_frontier(SIDE, stale, &[genesis], &dag),
            Err(BranchError::UnknownBranch { branch_id: SIDE })
        );
    }
}
