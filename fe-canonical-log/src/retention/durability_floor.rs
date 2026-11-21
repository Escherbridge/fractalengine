//! The D-CL39 durability floor: R distinct qualifying record relays per manifest (SPEC-9 §4,
//! SPEC-5 §5.2 rule 6). Evidence is assessed fresh and is a required input of
//! `checkpoint::compaction_decision`.

use std::collections::{BTreeMap, BTreeSet};

use crate::capability::Principal;
use crate::envelope::{Hash32, Identifier32};
use crate::segment::custody_receipt::{
    verify_custody_receipt, CustodySet, WAREHOUSE_FORMAT_EPOCH,
};

/// Current durable relay facts an assessor consults on every assessment (Q6, Q7).
pub trait RelayQualificationView {
    /// Q6: the DID is on the verse's current `record_relays` list.
    fn is_listed_record_relay(&self, verse_id: Identifier32, relay_did: &str) -> bool;

    /// Q7: the principal currently holds a valid `seed` capability for class `segment` at the
    /// verse-wide scope and current epoch; a cache miss answers `false`.
    fn holds_current_segment_seed(&self, verse_id: Identifier32, relay: &Principal) -> bool;
}

/// What the assessor itself knows about one manifest's custody set (§2 rule 2, §4 rule 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestCustody {
    /// The assessor walked the manifest under the header scope key; a decoded seed commitment
    /// here is assessed as unknown.
    Computed(CustodySet),
    /// The assessor cannot compute the set; that manifest's floor is unknown, never met.
    Unavailable {
        /// The manifest whose set is unknown.
        manifest_id: Hash32,
    },
}

impl ManifestCustody {
    /// The manifest this entry describes.
    pub fn manifest_id(&self) -> Hash32 {
        match self {
            Self::Computed(set) => set.manifest_id(),
            Self::Unavailable { manifest_id } => *manifest_id,
        }
    }
}

/// The release under assessment: verse, the admin's prune setting, manifests, and artifacts.
#[derive(Clone, Copy, Debug)]
pub struct FloorSubject<'a> {
    /// The verse whose floor is assessed.
    pub verse_id: Identifier32,
    /// The verse's `peer_prune_after_floor` setting (default off).
    pub peer_prune_after_floor: bool,
    /// Every manifest whose custody might cover a released artifact.
    pub manifests: &'a [ManifestCustody],
    /// Every artifact the release would drop locally.
    pub released_artifacts: &'a [Hash32],
}

/// Why one receipt adds nothing to a floor count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptDisqualification {
    /// Q1: decoding, DID binding, or signature failed.
    NotVerified,
    /// Q2: the receipt names another verse.
    ForeignVerse,
    /// Q3: the receipt names a manifest this assessment does not cover.
    UnrelatedManifest,
    /// Q4: the assessor cannot compute the manifest's custody set.
    CustodySetUnknown,
    /// Q4: the attested custody set is not the one the assessor computed.
    CustodySetMismatch,
    /// Q5: the relay's format epoch is not this binary's.
    FormatEpochMismatch,
    /// Q6: the relay is not on the verse's `record_relays` list.
    RelayNotListed,
    /// Q7: the relay holds no current `seed` capability for the verse.
    NoSeedCapability,
    /// §4 rule 3: this relay already counted for the manifest.
    DuplicateRelay,
}

/// One receipt that did not count, by its index in the assessed slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RejectedReceipt {
    /// Position in the `receipts` slice handed to [`DurabilityFloorEvidence::assess`].
    pub receipt_index: usize,
    /// Why it did not count.
    pub reason: ReceiptDisqualification,
}

/// Why a manifest's floor is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloorUnknownReason {
    /// R is missing, zero, or unreadable (§4 rule 1).
    ReplicationFactorUnknown,
    /// The assessor cannot compute the custody set (§4 rule 5).
    CustodySetUnavailable,
}

/// One manifest's floor state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestFloorState {
    /// At least R distinct qualifying relays hold it.
    Met,
    /// Fewer than R distinct qualifying relays hold it.
    BelowFloor,
    /// The floor cannot be evaluated; never read as met.
    Unknown(FloorUnknownReason),
}

/// One manifest's assessment, as the warehouse status read shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ManifestFloor {
    /// The assessed manifest.
    pub manifest_id: Hash32,
    /// Its floor state.
    pub state: ManifestFloorState,
    /// Distinct qualifying relays counted.
    pub qualifying_relays: u64,
}

/// Why a release is not covered by the floor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FloorShortfall {
    /// The verse's `peer_prune_after_floor` setting is off.
    PeerPruneAfterFloorOff,
    /// R is missing, zero, or unreadable.
    ReplicationFactorUnknown,
    /// The release names no artifact; coverage is never vacuous (SPEC-5 §5.2 rule 6.5).
    NothingReleased,
    /// A released artifact sits only in custody sets whose floor is not met.
    BelowFloor {
        /// The released artifact.
        artifact_id: Hash32,
        /// A manifest covering it.
        manifest_id: Hash32,
        /// Distinct qualifying relays that manifest has.
        qualifying_relays: u64,
        /// R.
        required: u64,
    },
    /// A released artifact is in no known custody set (bulk, unindexed, or unknown set).
    ArtifactNotCovered {
        /// The released artifact.
        artifact_id: Hash32,
    },
}

/// Fresh floor evidence for one release; obtainable only from [`Self::assess`].
///
/// Deliberately not `Clone`: assess again at the moment of release rather than keep a verdict.
#[derive(Debug, PartialEq, Eq)]
pub struct DurabilityFloorEvidence {
    verse_id: Identifier32,
    required: Option<u64>,
    manifests: Vec<ManifestFloor>,
    rejected: Vec<RejectedReceipt>,
    released: Vec<Hash32>,
    verdict: Result<(), FloorShortfall>,
}

impl DurabilityFloorEvidence {
    /// Assesses every receipt against Q1-Q7 and the release against R (`ts_replication_factor`).
    ///
    /// `receipts` are complete receipt bytes; each is re-verified here, so a stored receipt is
    /// evidence and never a cached verdict (§4 rule 6). A `None` or zero R is unknown, never met.
    pub fn assess<B: AsRef<[u8]>>(
        subject: &FloorSubject<'_>,
        receipts: &[B],
        replication_factor: Option<u64>,
        view: &dyn RelayQualificationView,
    ) -> Self {
        let required = replication_factor.filter(|r| *r >= 1);

        // A set whose identifier cannot be computed is as unknown as an unavailable one (§4 rule 5),
        // and so is a set decoded from a peer's seed commitment (§2 rule 2).
        let mut custody: BTreeMap<Hash32, Option<&CustodySet>> = BTreeMap::new();
        for entry in subject.manifests {
            let known = match entry {
                ManifestCustody::Computed(set) if set.is_walked() => {
                    set.custody_set_id().ok().map(|_| set)
                }
                ManifestCustody::Computed(_) => None,
                ManifestCustody::Unavailable { .. } => None,
            };
            custody
                .entry(entry.manifest_id())
                .and_modify(|existing| {
                    if *existing != known {
                        *existing = None;
                    }
                })
                .or_insert(known);
        }

        let mut expected_set_ids: BTreeMap<Hash32, Option<Hash32>> = BTreeMap::new();
        for (manifest_id, set) in &custody {
            let id = (*set).and_then(|set| set.custody_set_id().ok());
            expected_set_ids.insert(*manifest_id, id);
        }

        let mut counted: BTreeMap<Hash32, BTreeSet<[u8; 32]>> = custody
            .keys()
            .map(|manifest_id| (*manifest_id, BTreeSet::new()))
            .collect();
        let mut rejected = Vec::new();
        for (receipt_index, bytes) in receipts.iter().enumerate() {
            let reason = match verify_custody_receipt(bytes.as_ref()) {
                Err(_) => Some(ReceiptDisqualification::NotVerified),
                Ok(receipt) => {
                    if receipt.verse_id() != subject.verse_id {
                        Some(ReceiptDisqualification::ForeignVerse)
                    } else {
                        match expected_set_ids.get(&receipt.manifest_id()) {
                            None => Some(ReceiptDisqualification::UnrelatedManifest),
                            Some(None) => Some(ReceiptDisqualification::CustodySetUnknown),
                            Some(Some(expected)) if *expected != receipt.custody_set_id() => {
                                Some(ReceiptDisqualification::CustodySetMismatch)
                            }
                            Some(Some(_)) => {
                                let relay = receipt.relay_principal();
                                if receipt.warehouse_format_epoch() != WAREHOUSE_FORMAT_EPOCH {
                                    Some(ReceiptDisqualification::FormatEpochMismatch)
                                } else if !view.is_listed_record_relay(subject.verse_id, &relay.did)
                                {
                                    Some(ReceiptDisqualification::RelayNotListed)
                                } else if !view.holds_current_segment_seed(subject.verse_id, relay)
                                {
                                    Some(ReceiptDisqualification::NoSeedCapability)
                                } else if !counted
                                    .entry(receipt.manifest_id())
                                    .or_default()
                                    .insert(relay.public_key)
                                {
                                    Some(ReceiptDisqualification::DuplicateRelay)
                                } else {
                                    None
                                }
                            }
                        }
                    }
                }
            };
            if let Some(reason) = reason {
                rejected.push(RejectedReceipt {
                    receipt_index,
                    reason,
                });
            }
        }

        let manifests: Vec<ManifestFloor> = custody
            .iter()
            .map(|(manifest_id, set)| {
                let qualifying_relays = counted.get(manifest_id).map_or(0, |relays| relays.len()) as u64;
                let state = match (required, set) {
                    (None, _) => ManifestFloorState::Unknown(FloorUnknownReason::ReplicationFactorUnknown),
                    (Some(_), None) => {
                        ManifestFloorState::Unknown(FloorUnknownReason::CustodySetUnavailable)
                    }
                    (Some(required), Some(_)) if qualifying_relays >= required => {
                        ManifestFloorState::Met
                    }
                    (Some(_), Some(_)) => ManifestFloorState::BelowFloor,
                };
                ManifestFloor {
                    manifest_id: *manifest_id,
                    state,
                    qualifying_relays,
                }
            })
            .collect();

        let verdict = release_verdict(subject, required, &custody, &manifests);
        let mut released = subject.released_artifacts.to_vec();
        released.sort_unstable();
        released.dedup();
        Self {
            verse_id: subject.verse_id,
            required,
            manifests,
            rejected,
            released,
            verdict,
        }
    }

    /// The assessed release, ascending and deduplicated; the only artifacts a permit may drop.
    pub fn released_artifacts(&self) -> &[Hash32] {
        &self.released
    }

    /// The assessed verse.
    pub fn verse_id(&self) -> Identifier32 {
        self.verse_id
    }

    /// R as read, or `None` when unknown.
    pub fn required_relays(&self) -> Option<u64> {
        self.required
    }

    /// Per-manifest floor states, ascending by manifest ID.
    pub fn manifests(&self) -> &[ManifestFloor] {
        &self.manifests
    }

    /// Receipts that added nothing, with the reason.
    pub fn rejected_receipts(&self) -> &[RejectedReceipt] {
        &self.rejected
    }

    /// Whether every released artifact is covered by a met floor and pruning is enabled.
    pub fn is_met(&self) -> bool {
        self.verdict.is_ok()
    }

    /// The first reason the release is not covered, if any.
    pub fn shortfall(&self) -> Option<&FloorShortfall> {
        self.verdict.as_ref().err()
    }
}

/// Per-artifact coverage (SPEC-5 §5.2 rule 6.5): each released artifact needs a met manifest.
fn release_verdict(
    subject: &FloorSubject<'_>,
    required: Option<u64>,
    custody: &BTreeMap<Hash32, Option<&CustodySet>>,
    manifests: &[ManifestFloor],
) -> Result<(), FloorShortfall> {
    if !subject.peer_prune_after_floor {
        return Err(FloorShortfall::PeerPruneAfterFloorOff);
    }
    let Some(required) = required else {
        return Err(FloorShortfall::ReplicationFactorUnknown);
    };
    if subject.released_artifacts.is_empty() {
        return Err(FloorShortfall::NothingReleased);
    }
    for artifact_id in subject.released_artifacts {
        let covering: Vec<&ManifestFloor> = manifests
            .iter()
            .filter(|floor| {
                custody
                    .get(&floor.manifest_id)
                    .copied()
                    .flatten()
                    .is_some_and(|set| set.contains(*artifact_id))
            })
            .collect();
        if covering
            .iter()
            .any(|floor| floor.state == ManifestFloorState::Met)
        {
            continue;
        }
        return Err(match covering.first() {
            Some(floor) => FloorShortfall::BelowFloor {
                artifact_id: *artifact_id,
                manifest_id: floor.manifest_id,
                qualifying_relays: floor.qualifying_relays,
                required,
            },
            None => FloorShortfall::ArtifactNotCovered {
                artifact_id: *artifact_id,
            },
        });
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Real assessments for in-crate tests that need a met or unmet floor.

    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::segment::custody_receipt::{issue_custody_receipt, RecordRelayBinding};
    use crate::segment::store::{InMemorySealedArtifactStore, SealedArtifactStore};

    struct EveryRelayQualifies;

    impl RelayQualificationView for EveryRelayQualifies {
        fn is_listed_record_relay(&self, _verse_id: Identifier32, _relay_did: &str) -> bool {
            true
        }

        fn holds_current_segment_seed(&self, _verse_id: Identifier32, _relay: &Principal) -> bool {
            true
        }
    }

    /// An assessment with no receipts and pruning off: never met.
    pub(crate) fn unmet_floor(verse_id: Identifier32) -> DurabilityFloorEvidence {
        let none: [Vec<u8>; 0] = [];
        DurabilityFloorEvidence::assess(
            &FloorSubject {
                verse_id,
                peer_prune_after_floor: false,
                manifests: &[],
                released_artifacts: &[],
            },
            &none,
            None,
            &EveryRelayQualifies,
        )
    }

    /// Two relays hold a one-artifact manifest with R = 2, and the release drops that artifact.
    pub(crate) fn met_floor(verse_id: Identifier32) -> DurabilityFloorEvidence {
        let manifest_bytes = b"sealed manifest bytes".to_vec();
        let manifest_id = Hash32::of(&manifest_bytes);
        let mut store = InMemorySealedArtifactStore::new();
        store
            .put_verified(manifest_id, &manifest_bytes)
            .expect("held manifest");
        let set = CustodySet::for_manifest(manifest_id, [(manifest_id, manifest_bytes.len() as u64)])
            .expect("custody set");
        let binding = RecordRelayBinding::current(verse_id);
        let receipts: Vec<Vec<u8>> = [0x51u8, 0x52]
            .into_iter()
            .map(|seed| {
                issue_custody_receipt(&SigningKey::from_bytes(&[seed; 32]), &binding, &set, &store, 1)
                    .expect("receipt")
                    .encode_canonical()
                    .expect("bytes")
            })
            .collect();
        let evidence = DurabilityFloorEvidence::assess(
            &FloorSubject {
                verse_id,
                peer_prune_after_floor: true,
                manifests: &[ManifestCustody::Computed(set)],
                released_artifacts: &[manifest_id],
            },
            receipts.as_slice(),
            Some(2),
            &EveryRelayQualifies,
        );
        assert!(evidence.is_met(), "fixture floor must be met");
        evidence
    }
}
