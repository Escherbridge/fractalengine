//! Bounded immutable native navigation read ownership; see AGENTS.md.

use super::Guard;
use fe_runtime::{messages::{DbCommand, DbResult}, native_navigation::{PetalTarget, PetalView}};
use std::{sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}}, time::{Duration, Instant}};

pub const MAX_TARGETS: usize = 16;
const LIFETIME: Duration = Duration::from_secs(1);
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
static EXHAUSTED: AtomicBool = AtomicBool::new(false);

fn fresh_epoch() -> Option<u64> {
    if EXHAUSTED.load(Ordering::SeqCst) { return None; }
    match NEXT_EPOCH.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
        value.checked_add(1).filter(|next| *next <= isize::MAX as u64)
    }) {
        Ok(epoch) => Some(epoch),
        Err(_) => { EXHAUSTED.store(true, Ordering::SeqCst); None }
    }
}

/// Current base source, independent of this owner's proof-set generation.
#[derive(Clone, PartialEq, Eq)]
pub struct Source {
    context: u128,
    observed_generation: u64,
    navigation: u64,
    scene: u128,
    hierarchy: u64,
    access: Option<u128>,
    selection: u64,
    role: Option<fe_database::RoleLevel>,
    verse: Option<String>,
    fractal: Option<String>,
    eligible: bool,
}

impl Source {
    pub(crate) fn eligible(&self) -> bool { self.eligible }
    pub(crate) fn observed_at(&self, generation: u64) -> bool { self.observed_generation == generation }
    pub fn capture(
        context_lifetime: u128,
        observed_generation: u64,
        guard: Guard,
        nav: &crate::navigation_manager::NavigationManager,
        manager: &crate::verse_manager::VerseManager,
        nodes: &crate::node_manager::NodeManager,
    ) -> Self {
        let verse = nav.active_verse_id.as_deref();
        let mut matches = manager.verses.iter().filter(|entry| Some(entry.id.as_str()) == verse);
        let current = matches.next();
        let ancestry = current.is_some() && matches.next().is_none() && current.is_some_and(|entry| {
            nav.active_fractal_id.as_deref().is_none_or(|fractal| {
                entry.fractals.iter().filter(|entry| entry.id == fractal).count() == 1
            })
        });
        Self {
            context: context_lifetime,
            observed_generation,
            navigation: guard.navigation,
            scene: guard.scene,
            hierarchy: guard.hierarchy,
            access: guard.access,
            selection: guard.selection,
            role: guard.role,
            verse: nav.active_verse_id.clone(),
            fractal: nav.active_fractal_id.clone(),
            eligible: context_lifetime != 0 && guard.ready && guard.capture_view && guard.capture_labels_clear
                && guard.access.is_some() && guard.role.is_some_and(|role| role.can_view())
                && !manager.scene_access_blocked() && nav.has_valid_ancestry()
                && nav.active_petal_id.is_none() && nodes.selection_count() == 0 && ancestry,
        }
    }

    fn allows(&self, target: &PetalTarget) -> bool {
        self.eligible && self.context != 0 && self.access.is_some()
            && self.role.is_some_and(|role| role.can_view()) && target.is_bounded()
            && self.verse.as_deref() == Some(target.verse_id.as_str())
            && self.fractal.as_deref().is_none_or(|id| id == target.fractal_id)
    }
}

/// Identity supplied by the actual sidebar Open response, never by label parsing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub widget: u64,
    pub petal: PetalTarget,
}

#[derive(Clone)]
struct Demand {
    request: String,
    source: Source,
    target: Target,
    admitted_at: Instant,
    deadline: Instant,
    target_epoch: u64,
}

impl Demand {
    fn admit(source: &Source, target: Target, now: Instant) -> Option<Self> {
        Some(Self {
            request: ulid::Ulid::new().to_string(),
            source: source.clone(),
            target,
            admitted_at: now,
            deadline: now.checked_add(LIFETIME)?,
            target_epoch: fresh_epoch()?,
        })
    }
}

/// Retained original request and successful sampled view; its deadline never moves.
pub struct Proof {
    demand: Demand,
    view: PetalView,
}

impl Proof {
    pub fn target(&self) -> &Target { &self.demand.target }
    pub fn target_epoch(&self) -> u64 { self.demand.target_epoch }
    #[cfg(test)]
    pub(super) fn original_deadline(&self) -> Instant { self.demand.deadline }
}

struct Slot {
    demand: Demand,
    attempted: bool,
    try_after: Instant,
    proof: Option<Arc<Proof>>,
}

impl Slot {
    fn admit(source: &Source, target: Target, now: Instant) -> Option<Self> {
        Some(Self {
            demand: Demand::admit(source, target, now)?,
            attempted: false,
            try_after: now,
            proof: None,
        })
    }
}

/// An immutable completed target set for later use-time checks.
#[derive(Clone)]
pub struct Bindings {
    epoch: u64,
    proofs: Vec<Arc<Proof>>,
}

impl Bindings {
    pub fn epoch(&self) -> u64 { self.epoch }
    pub fn proofs(&self) -> &[Arc<Proof>] { &self.proofs }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submission { NoWork, Queued, Backpressure, Disconnected }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reception { Foreign, Discarded, Admitted }

/// At most sixteen admitted reads and one accepted DB request, without input wiring.
#[derive(Default)]
pub struct ProofOwner {
    source: Option<Source>,
    slots: Vec<Slot>,
    in_flight: Option<Demand>,
    epoch: Option<u64>,
    disconnected: bool,
}

impl ProofOwner {
    pub fn retire(&mut self) {
        self.source = None;
        self.slots.clear();
        self.epoch = None;
    }

    fn advance(&mut self) -> bool {
        self.epoch = fresh_epoch();
        if self.epoch.is_none() { self.retire(); return false; }
        true
    }

    pub fn observe(&mut self, source: &Source, targets: &[Target], now: Instant) -> bool {
        let valid = !self.disconnected && !targets.is_empty() && targets.len() <= MAX_TARGETS
            && targets.iter().all(|target| source.allows(&target.petal))
            && targets.iter().enumerate().all(|(index, target)| {
                targets[..index].iter().all(|previous| {
                    previous.widget != target.widget && previous.petal.petal_id != target.petal.petal_id
                })
            });
        if !valid { self.retire(); return false; }
        let changed = self.source.as_ref() != Some(source)
            || self.slots.len() != targets.len()
            || self.slots.iter().zip(targets).any(|(slot, target)| slot.demand.target != *target);
        if changed {
            let Some(slots) = targets.iter().cloned().map(|target| Slot::admit(source, target, now))
                .collect::<Option<Vec<_>>>() else { self.retire(); return false; };
            self.source = Some(source.clone());
            self.slots = slots;
            return self.advance();
        }
        let mut renewed = false;
        let mut failed = false;
        for slot in &mut self.slots {
            if now >= slot.demand.deadline {
                let Some(replacement) = Slot::admit(source, slot.demand.target.clone(), now)
                    else { failed = true; break; };
                *slot = replacement;
                renewed = true;
            }
        }
        if failed { self.retire(); return false; }
        !renewed || self.advance()
    }

    pub fn submit(
        &mut self,
        source: &Source,
        now: Instant,
        sender: &crossbeam::channel::Sender<DbCommand>,
    ) -> Submission {
        if self.disconnected { return Submission::Disconnected; }
        if self.source.as_ref() != Some(source) || !source.eligible || self.in_flight.is_some() {
            return Submission::NoWork;
        }
        let Some(slot) = self.slots.iter_mut().find(|slot| {
            !slot.attempted && now >= slot.try_after && now >= slot.demand.admitted_at
                && now < slot.demand.deadline
        }) else { return Submission::NoWork; };
        let command = DbCommand::ReadNativePetalView {
            request_id: slot.demand.request.clone(),
            target: slot.demand.target.petal.clone(),
        };
        match sender.try_send(command) {
            Ok(()) => {
                slot.attempted = true;
                self.in_flight = Some(slot.demand.clone());
                Submission::Queued
            }
            Err(crossbeam::channel::TrySendError::Full(_)) => {
                slot.try_after = now.checked_add(Duration::from_millis(100))
                    .unwrap_or(slot.demand.deadline).min(slot.demand.deadline);
                Submission::Backpressure
            }
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                self.disconnected = true;
                self.retire();
                Submission::Disconnected
            }
        }
    }

    pub fn receive(&mut self, result: &DbResult, source: &Source, now: Instant) -> Reception {
        let DbResult::NativePetalViewRead { request_id, target, outcome } = result
            else { return Reception::Foreign; };
        if !self.in_flight.as_ref().is_some_and(|read| &read.request == request_id) {
            return Reception::Foreign;
        }
        let Some(demand) = self.in_flight.take() else { return Reception::Discarded; };
        let Ok(view) = outcome else { return Reception::Discarded; };
        if target != &demand.target.petal || view.target != *target {
            return Reception::Discarded;
        }
        let proof = Arc::new(Proof { demand, view: view.clone() });
        if !self.proof_is_current(&proof, source, now) { return Reception::Discarded; }
        let Some(slot) = self.slots.iter_mut().find(|slot| slot.demand.request == *request_id)
            else { return Reception::Discarded; };
        slot.proof = Some(proof);
        if !self.advance() { return Reception::Discarded; }
        Reception::Admitted
    }

    pub fn proof_is_current(&self, proof: &Proof, source: &Source, now: Instant) -> bool {
        !self.disconnected && !EXHAUSTED.load(Ordering::SeqCst)
            && self.source.as_ref() == Some(source) && &proof.demand.source == source
            && source.allows(&proof.demand.target.petal)
            && now >= proof.demand.admitted_at && now < proof.demand.deadline
            && proof.view.is_bounded() && proof.view.target == proof.demand.target.petal
            && self.slots.iter().any(|slot| {
                slot.demand.request == proof.demand.request
                    && slot.demand.target_epoch == proof.demand.target_epoch
                    && slot.demand.target == proof.demand.target
                    && slot.demand.deadline == proof.demand.deadline
                    && slot.demand.source == proof.demand.source
            })
    }

    pub fn bindings(&self, source: &Source, now: Instant) -> Option<Bindings> {
        if self.slots.is_empty() { return None; }
        let proofs = self.slots.iter().map(|slot| {
            let proof = slot.proof.as_ref()?;
            self.proof_is_current(proof, source, now).then(|| proof.clone())
        }).collect::<Option<Vec<_>>>()?;
        Some(Bindings { epoch: self.epoch?, proofs })
    }

    pub fn bindings_are_current(&self, bindings: &Bindings, source: &Source, now: Instant) -> bool {
        self.epoch == Some(bindings.epoch) && !bindings.proofs.is_empty()
            && bindings.proofs.len() == self.slots.len()
            && bindings.proofs.iter().all(|proof| self.proof_is_current(proof, source, now))
    }
}

#[cfg(test)]
pub(super) fn extend_existing_journey() {
    use fe_runtime::native_navigation::Unavailable;
    let now = Instant::now();
    let source = Source {
        context: 1, observed_generation: 1, navigation: 1, scene: 1, hierarchy: 1,
        access: Some(1), selection: 1, role: Some(fe_database::RoleLevel::Viewer),
        verse: Some("verse".into()), fractal: Some("fractal".into()), eligible: true,
    };
    let target = Target { widget: 7, petal: PetalTarget {
        verse_id: "verse".into(), fractal_id: "fractal".into(),
        petal_id: "petal".into(), name: "Declared fixture".into(),
    } };
    let (sender, receiver) = crossbeam::channel::bounded(1);
    let mut owner = ProofOwner::default();
    assert!(owner.observe(&source, &[target.clone()], now));
    let original_request = owner.slots[0].demand.request.clone();
    let deadline = owner.slots[0].demand.deadline;
    sender.try_send(DbCommand::LoadHierarchy).unwrap();
    assert_eq!(owner.submit(&source, now, &sender), Submission::Backpressure);
    assert_eq!(owner.slots[0].demand.deadline, deadline, "Full never restarts admission time");
    receiver.try_recv().unwrap();
    let submitted_at = now + Duration::from_millis(100);
    assert_eq!(owner.submit(&source, submitted_at, &sender), Submission::Queued);
    let DbCommand::ReadNativePetalView { request_id, target: requested } = receiver.try_recv().unwrap()
        else { panic!("dedicated local Viewer read required"); };
    assert_eq!(request_id, original_request);
    assert_eq!(requested, target.petal);
    let reply = DbResult::NativePetalViewRead {
        request_id: request_id.clone(), target: target.petal.clone(),
        outcome: Ok(PetalView { target: target.petal.clone(), source_fence: "a".repeat(64) }),
    };
    assert_eq!(owner.receive(&reply, &source, now + Duration::from_millis(500)), Reception::Admitted);
    let bindings = owner.bindings(&source, now + Duration::from_millis(500)).unwrap();
    let proof = bindings.proofs()[0].clone();
    let epoch = bindings.epoch();
    let mut changed = source.clone();
    changed.observed_generation += 1;
    assert!(!owner.proof_is_current(&proof, &changed, now + Duration::from_millis(500)));
    for field in 0..10 {
        let mut stale = source.clone();
        match field {
            0 => stale.context += 1,
            1 => stale.navigation += 1,
            2 => stale.scene += 1,
            3 => stale.hierarchy += 1,
            4 => stale.access = Some(2),
            5 => stale.selection += 1,
            6 => stale.role = Some(fe_database::RoleLevel::None),
            7 => stale.verse = None,
            8 => stale.fractal = Some("different".into()),
            _ => stale.eligible = false,
        }
        assert!(!owner.proof_is_current(&proof, &stale, now + Duration::from_millis(500)),
            "every original base source component stays bound");
    }
    for offset in [500, 600, 700, 800, 900, 999] {
        assert!(owner.proof_is_current(&proof, &source, now + Duration::from_millis(offset)));
        assert!(owner.bindings_are_current(&bindings, &source, now + Duration::from_millis(offset)));
    }
    assert!(!owner.proof_is_current(&proof, &source, deadline));
    assert!(!owner.bindings_are_current(&bindings, &source, deadline));
    assert_eq!(owner.epoch, Some(epoch), "expiry refuses without pumping or changing generation");
    assert!(owner.observe(&source, &[target.clone()], deadline));
    assert_ne!(owner.slots[0].demand.request, request_id);
    assert!(!owner.proof_is_current(&proof, &source, deadline), "a new read cannot renew retained proof");

    for delay in [1000, 1500] {
        let mut delayed = ProofOwner::default();
        assert!(delayed.observe(&source, &[target.clone()], now));
        assert_eq!(delayed.submit(&source, now + Duration::from_millis(delay), &sender), Submission::NoWork);
        assert!(receiver.try_recv().is_err(), "late unsent read is unavailable");
        let mut delayed = ProofOwner::default();
        assert!(delayed.observe(&source, &[target.clone()], now));
        assert_eq!(delayed.submit(&source, now, &sender), Submission::Queued);
        let DbCommand::ReadNativePetalView { request_id, .. } = receiver.try_recv().unwrap()
            else { panic!("native read expected"); };
        let late = DbResult::NativePetalViewRead { request_id, target: target.petal.clone(),
            outcome: Ok(PetalView { target: target.petal.clone(), source_fence: "b".repeat(64) }) };
        assert_eq!(delayed.receive(&late, &source, now + Duration::from_millis(delay)), Reception::Discarded);
        assert!(delayed.bindings(&source, now + Duration::from_millis(delay)).is_none());
    }

    let mut pending = ProofOwner::default();
    assert!(pending.observe(&source, &[target.clone()], now));
    assert_eq!(pending.submit(&source, now, &sender), Submission::Queued);
    let DbCommand::ReadNativePetalView { request_id, .. } = receiver.try_recv().unwrap()
        else { panic!("native read expected"); };
    assert!(pending.observe(&changed, &[target.clone()], now + Duration::from_millis(100)));
    assert_eq!(pending.submit(&changed, now + Duration::from_millis(100), &sender), Submission::NoWork,
        "source retirement retains the one accepted read until its exact reply");
    let old = DbResult::NativePetalViewRead { request_id, target: target.petal.clone(),
        outcome: Ok(PetalView { target: target.petal.clone(), source_fence: "c".repeat(64) }) };
    assert_eq!(pending.receive(&old, &changed, now + Duration::from_millis(100)), Reception::Discarded);
    assert!(pending.bindings(&changed, now + Duration::from_millis(100)).is_none());

    let mut malformed = ProofOwner::default();
    assert!(malformed.observe(&source, &[target.clone()], now));
    assert_eq!(malformed.submit(&source, now, &sender), Submission::Queued);
    let DbCommand::ReadNativePetalView { request_id, .. } = receiver.try_recv().unwrap()
        else { panic!("native read expected"); };
    let foreign = DbResult::NativePetalViewRead { request_id: "foreign".into(), target: target.petal.clone(),
        outcome: Err(Unavailable::Target) };
    assert_eq!(malformed.receive(&foreign, &source, now), Reception::Foreign);
    assert!(malformed.in_flight.is_some());
    let corrupt = DbResult::NativePetalViewRead { request_id, target: target.petal.clone(),
        outcome: Ok(PetalView { target: target.petal.clone(), source_fence: "not-a-fence".into() }) };
    assert_eq!(malformed.receive(&corrupt, &source, now), Reception::Discarded);
    assert!(malformed.bindings(&source, now).is_none());

    let mut owner = ProofOwner::default();
    let bounded: Vec<_> = (0..MAX_TARGETS).map(|index| Target { widget: index as u64,
        petal: PetalTarget { petal_id: format!("petal-{index}"), ..target.petal.clone() } }).collect();
    assert!(owner.observe(&source, &bounded, now));
    assert!(owner.bindings(&source, now).is_none(), "a partial target set cannot publish");
    for invalid in [
        vec![target.clone(), target.clone()],
        vec![target.clone(), Target { widget: 8, ..target.clone() }],
        vec![target.clone(), Target { petal: PetalTarget { petal_id: "other".into(), ..target.petal.clone() }, ..target.clone() }],
        (0..=MAX_TARGETS).map(|index| Target { widget: index as u64,
            petal: PetalTarget { petal_id: format!("petal-{index}"), ..target.petal.clone() } }).collect(),
        vec![Target { petal: PetalTarget { fractal_id: "moved".into(), ..target.petal.clone() }, ..target.clone() }],
    ] {
        assert!(!owner.observe(&source, &invalid, now));
        assert!(owner.bindings(&source, now).is_none());
    }
    let mut no_access = source.clone();
    no_access.eligible = false;
    assert!(!owner.observe(&no_access, &[target.clone()], now));
    no_access.eligible = true;
    no_access.role = Some(fe_database::RoleLevel::None);
    assert!(!owner.observe(&no_access, &[target.clone()], now));
    no_access.role = None;
    assert!(!owner.observe(&no_access, &[target.clone()], now));
    let (gone, receiver) = crossbeam::channel::bounded(1);
    drop(receiver);
    assert!(owner.observe(&source, &[target.clone()], now));
    assert_eq!(owner.submit(&source, now, &gone), Submission::Disconnected);
    assert!(!owner.observe(&source, &[target], now));
}
