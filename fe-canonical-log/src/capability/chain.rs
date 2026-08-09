//! The three-key chain artifact and the nine ordered §2.4 verification steps.

use thiserror::Error;

use crate::cbor::{decode_canonical as decode_cbor, encode_canonical_checked, CborValue};
use crate::envelope::{Hash32, Identifier32, Scope};

use super::caveats::{CaveatField, Caveats};
use super::certificate::{
    certificate_id_of_bytes, Certificate, CertificateId, CAPABILITY_VERSION,
    MAXIMUM_CERTIFICATE_LIFETIME_MS,
};
use super::permissions::{self, PermissionContext, PermissionError};
use super::revalidation::AuthorizationView;
use super::verbs::{ObjectClass, ObjectClassSet, Verb, VerbSet};
use super::{bytes_at, entry, require_numeric_keys, unsigned_at, CapabilityError, Principal};

/// BLAKE3 identifier of the complete canonical chain map.
pub type ChainId = Hash32;

const CONTEXT: &str = "capability_chain";

/// Which principal slot of a certificate failed its DID-to-key binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalRole {
    /// Key 1, the signing principal.
    Issuer,
    /// Key 2, the principal allowed to exercise or attenuate the grant.
    Audience,
}

/// Which aspect of a certificate widened instead of attenuating (§2.4 step 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttenuationAspect {
    /// The child `grant_scope` is not contained by the parent's.
    GrantScope,
    /// The child verb bitset is not a subset of the parent's.
    Verbs,
    /// The child object-class bitset is not a subset of the parent's.
    ObjectClasses,
    /// The child `not_before_ms` is earlier than the parent's.
    NotBefore,
    /// The child `not_after_ms` is later than the parent's.
    NotAfter,
    /// The child `delegation_depth` exceeds `parent.delegation_depth - 1`.
    DelegationDepth,
    /// A caveat row became less restrictive.
    Caveat(
        /// The offending caveat row.
        CaveatField,
    ),
}

/// Every reason a capability chain fails one of the nine §2.4 steps, in step order.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ChainError {
    /// Step 1: the chain or one of its certificates broke the §2 grammar.
    #[error("capability grammar: {0}")]
    Encoding(#[from] CapabilityError),
    /// Step 1: the received chain bytes are not their own canonical re-encoding.
    #[error("chain bytes are not the canonical encoding of the chain they decode to")]
    NonCanonicalChainBytes,
    /// Step 1: a certificate entry broke the §2.2 grammar.
    #[error("certificate {index}: {source}")]
    CertificateEncoding {
        /// Position of the certificate, root first.
        index: usize,
        /// Underlying grammar failure.
        source: CapabilityError,
    },
    /// Step 1: a certificate entry is not its own canonical re-encoding.
    #[error("certificate {index} bytes are not their own canonical re-encoding")]
    NonCanonicalCertificateBytes {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 2: a principal's DID did not bind to its public key.
    #[error("certificate {index} {role:?} principal binding: {source}")]
    PrincipalBindingInvalid {
        /// Position of the certificate, root first.
        index: usize,
        /// Which principal slot failed.
        role: PrincipalRole,
        /// Underlying binding failure.
        source: CapabilityError,
    },
    /// Step 2: a certificate signature did not verify under its issuer key.
    #[error("certificate {index} signature did not verify under its issuer key")]
    CertificateSignatureInvalid {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 3: the first certificate named a parent.
    #[error("the root certificate must have a null parent_certificate_id")]
    RootHasParent,
    /// Step 3: the authorization view does not show the root issuer as Manager+.
    #[error("the root issuer is not a current Manager+ identity for epoch_scope")]
    RootIssuerNotManagerPlus,
    /// Step 3: the authorization view holds no record under the root `issuer_authority_id`.
    #[error("the authorization view holds no authority record for the root issuer_authority_id")]
    RootAuthorityRecordUnknown,
    /// Step 4: a non-root certificate named no parent.
    #[error("certificate {index} is not the root and must name a parent_certificate_id")]
    NonRootMissingParent {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 4: a certificate's parent link did not name its predecessor.
    #[error("certificate {index} parent_certificate_id does not name its predecessor")]
    ParentLinkMismatch {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 4: a certificate's issuer was not its predecessor's audience.
    #[error("certificate {index} issuer is not its predecessor's audience")]
    IssuerAudienceMismatch {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 4: a certificate rotated the root authority anchor.
    #[error("certificate {index} issuer_authority_id differs from the root value")]
    AuthorityAnchorMismatch {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 4: a certificate left the root epoch family.
    #[error("certificate {index} epoch_scope differs from the root value")]
    EpochScopeMismatch {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 4: a certificate claimed a different epoch than the root.
    #[error("certificate {index} scope_epoch differs from the root value")]
    ScopeEpochMismatch {
        /// Position of the certificate, root first.
        index: usize,
    },
    /// Step 5: a certificate widened its parent instead of attenuating it.
    #[error("certificate {index} widens its parent on {aspect:?}")]
    NotAttenuated {
        /// Position of the certificate, root first.
        index: usize,
        /// Which aspect widened.
        aspect: AttenuationAspect,
    },
    /// Step 5: a terminal certificate issued a child.
    #[error("certificate {index} was issued by a terminal certificate of zero delegation_depth")]
    DelegationDepthExhausted {
        /// Position of the child certificate, root first.
        index: usize,
    },
    /// Step 6: the leaf audience is neither the operation author nor the requester.
    #[error("the leaf audience does not bind to the acting principal")]
    LeafAudienceMismatch,
    /// Step 7: the effective window has not opened yet.
    #[error("the chain is not valid before {not_before_ms} ms; now is {now_ms} ms")]
    NotYetValid {
        /// Effective inclusive start.
        not_before_ms: u64,
        /// Caller-supplied wall clock.
        now_ms: u64,
    },
    /// Step 7: the effective window has closed.
    #[error("the chain expired at {not_after_ms} ms; now is {now_ms} ms")]
    Expired {
        /// Effective exclusive end.
        not_after_ms: u64,
        /// Caller-supplied wall clock.
        now_ms: u64,
    },
    /// Step 7: a certificate lifetime exceeded the bound in force.
    #[error("certificate {index} lifetime {lifetime_ms} ms exceeds the maximum {maximum_ms} ms")]
    LifetimeExceedsMaximum {
        /// Position of the certificate, root first.
        index: usize,
        /// Declared lifetime.
        lifetime_ms: u64,
        /// Bound in force.
        maximum_ms: u64,
    },
    /// Step 7: the operation relied on an epoch the leaf does not certify.
    #[error("operation scope_epoch {operation_scope_epoch} differs from leaf {leaf_scope_epoch}")]
    OperationEpochMismatch {
        /// Epoch named in the operation's capability reference.
        operation_scope_epoch: u64,
        /// Epoch the leaf certificate carries.
        leaf_scope_epoch: u64,
    },
    /// Step 8: the leaf does not grant the requested verb.
    #[error("the leaf grant does not include verb {verb:?}")]
    VerbNotGranted {
        /// Requested verb.
        verb: Verb,
    },
    /// Step 8: the leaf does not grant the requested object class.
    #[error("the leaf grant does not include object class {object_class:?}")]
    ObjectClassNotGranted {
        /// Requested object class.
        object_class: ObjectClass,
    },
    /// Step 8: the §3.3 table denied the pair.
    #[error("permission table: {0}")]
    Permission(#[from] PermissionError),
    /// Step 8: the requested target scope is outside the effective grant scope.
    #[error("the effective grant scope does not contain the requested target scope")]
    ScopeNotContained,
    /// Step 8: the requested resource is outside the `resource_allowlist` caveat.
    #[error("the requested resource is not in the resource_allowlist caveat")]
    ResourceNotAllowed,
    /// Step 8: the request exceeds the `max_payload_bytes` caveat.
    #[error("payload of {actual} bytes exceeds the max_payload_bytes caveat of {limit}")]
    PayloadTooLarge {
        /// Caveat limit.
        limit: u64,
        /// Requested size.
        actual: u64,
    },
    /// Step 8: the request exceeds the `max_segment_bytes` caveat.
    #[error("segment of {actual} bytes exceeds the max_segment_bytes caveat of {limit}")]
    SegmentTooLarge {
        /// Caveat limit.
        limit: u64,
        /// Requested size.
        actual: u64,
    },
    /// Step 8: the payload schema is outside the `allowed_schema_hashes` caveat.
    #[error("the requested schema hash is not in the allowed_schema_hashes caveat")]
    SchemaNotAllowed,
    /// Step 8: the operation kind is outside the `allowed_operation_kinds` caveat.
    #[error("operation kind {kind} is not in the allowed_operation_kinds caveat")]
    OperationKindNotAllowed {
        /// Requested operation kind.
        kind: u16,
    },
    /// Step 8: the requested preview rate exceeds the `max_preview_hz` caveat.
    #[error("preview rate {requested} Hz exceeds the max_preview_hz caveat of {limit}")]
    PreviewRateTooHigh {
        /// Caveat limit.
        limit: u16,
        /// Requested rate.
        requested: u16,
    },
    /// Step 9: the authorization view records no epoch at all for `epoch_scope`.
    #[error("the authorization view records no epoch for epoch_scope")]
    EpochScopeUnknown,
    /// Step 9: the authorization view's epoch has moved past the chain's epoch.
    #[error("epoch_scope is at epoch {current_epoch}; the chain certifies epoch {chain_epoch}")]
    EpochRevoked {
        /// Epoch the persistent authorization view reports.
        current_epoch: u64,
        /// Epoch the chain certifies.
        chain_epoch: u64,
    },
}

/// The three-key §2.4 chain map: root-first complete-certificate bytes and the leaf identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chain {
    /// MUST be [`CAPABILITY_VERSION`].
    pub capability_version: u64,
    /// Exact complete-certificate encodings, root first and leaf last.
    pub certificates: Vec<Vec<u8>>,
    /// BLAKE3 identifier of the final certificate.
    pub leaf_certificate_id: CertificateId,
}

impl Chain {
    /// Builds a chain from certificates already in root-to-leaf order.
    pub fn from_certificates(certificates: &[Certificate]) -> Result<Self, CapabilityError> {
        if certificates.is_empty() {
            return Err(CapabilityError::EmptyChain);
        }
        let encoded = certificates
            .iter()
            .map(Certificate::encode_canonical)
            .collect::<Result<Vec<Vec<u8>>, CapabilityError>>()?;
        let leaf_certificate_id = certificate_id_of_bytes(encoded.last().expect("non-empty"));
        Ok(Self {
            capability_version: CAPABILITY_VERSION,
            certificates: encoded,
            leaf_certificate_id,
        })
    }

    /// Enforces the §2.4 chain-map rules that need no cryptography.
    pub fn validate(&self) -> Result<(), CapabilityError> {
        if self.capability_version != CAPABILITY_VERSION {
            return Err(CapabilityError::UnsupportedCapabilityVersion {
                version: self.capability_version,
            });
        }
        let leaf = self
            .certificates
            .last()
            .ok_or(CapabilityError::EmptyChain)?;
        if certificate_id_of_bytes(leaf) != self.leaf_certificate_id {
            return Err(CapabilityError::LeafCertificateIdMismatch);
        }
        Ok(())
    }

    /// Encodes the three-key chain map.
    pub fn to_cbor(&self) -> Result<CborValue, CapabilityError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(self.capability_version)),
            (
                CborValue::Uint(1),
                CborValue::Array(
                    self.certificates
                        .iter()
                        .map(|bytes| CborValue::Bytes(bytes.clone()))
                        .collect(),
                ),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(self.leaf_certificate_id.0.to_vec()),
            ),
        ]))
    }

    /// Decodes the three-key chain map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CapabilityError> {
        require_numeric_keys(value, 3, CONTEXT)?;
        let capability_version = unsigned_at(value, 0, CONTEXT)?;
        if capability_version != CAPABILITY_VERSION {
            return Err(CapabilityError::UnsupportedCapabilityVersion {
                version: capability_version,
            });
        }
        let certificates = entry(value, 1, CONTEXT)?
            .as_array()
            .ok_or(CapabilityError::ExpectedArray {
                context: CONTEXT,
                key: 1,
            })?
            .iter()
            .map(|item| {
                item.as_bytes().map(|bytes| bytes.to_vec()).ok_or(
                    CapabilityError::ExpectedByteString {
                        context: CONTEXT,
                        key: 1,
                    },
                )
            })
            .collect::<Result<Vec<Vec<u8>>, CapabilityError>>()?;
        if certificates.is_empty() {
            return Err(CapabilityError::EmptyChain);
        }
        let chain = Self {
            capability_version,
            certificates,
            leaf_certificate_id: Hash32(bytes_at::<32>(value, 2, CONTEXT)?),
        };
        chain.validate()?;
        Ok(chain)
    }

    /// Encodes the exact bytes `chain_id` addresses.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CapabilityError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CapabilityError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }

    /// `chain_id = BLAKE3(complete_chain)` over the exact canonical outer map.
    pub fn chain_id(&self) -> Result<ChainId, CapabilityError> {
        Ok(Hash32::of(&self.encode_canonical()?))
    }
}

/// The action a chain is being asked to authorize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorizationRequest {
    /// Requested verb.
    pub verb: Verb,
    /// Requested object class.
    pub object_class: ObjectClass,
    /// Exact scope of the object being acted on.
    pub target_scope: Scope,
    /// Resource identifier being acted on, when the request names one.
    pub resource_id: Option<Identifier32>,
    /// Authenticated requester, bound to the leaf audience for every non-append verb.
    pub requester: Principal,
    /// Operation author, bound to the leaf audience for `append`.
    pub operation_author: Principal,
    /// Epoch named in the operation's §3.3 capability reference.
    pub operation_scope_epoch: u64,
    /// Operation kind being appended, when the request appends an operation.
    pub operation_kind: Option<u16>,
    /// Whether the appended operation is a structural authorization operation (§4.7).
    pub is_structural_operation: bool,
    /// Plaintext intent payload size, when the request carries one.
    pub payload_bytes: Option<u64>,
    /// Encrypted segment size, when the request seals one.
    pub segment_bytes: Option<u64>,
    /// Payload schema hash, when the request carries a payload.
    pub schema_hash: Option<Hash32>,
    /// Requested preview rate, when the request is a preview.
    pub preview_hz: Option<u16>,
    /// Caller-supplied wall clock in Unix milliseconds.
    pub now_ms: u64,
}

/// What the verified authorization view records for one Manager+ question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityState {
    /// The view records the subject as a current Manager+ identity for that exact scope.
    ManagerPlus,
    /// The view resolves the subject and it is not Manager+ there.
    NotManagerPlus,
    /// The view holds no record to answer with; a missing answer is never an allow.
    Unknown,
}

impl AuthorityState {
    /// Whether this is the one state that authorizes.
    pub const fn is_manager_plus(self) -> bool {
        matches!(self, Self::ManagerPlus)
    }
}

/// The verified authorization view's Manager+ answers, asked question by question.
///
/// The verifier calls these itself with values it read out of the chain, so no caller can
/// pre-compute an answer for a different anchor, principal, or scope; see
/// `src/capability/AGENTS.md` §step-3-asks-the-view-itself.
pub trait ManagerAuthorityView: AuthorizationView {
    /// Whether `authority_id` records `issuer` as Manager+ for `epoch_scope` (§2.4 step 3).
    fn authority_is_manager_plus(
        &self,
        authority_id: &Hash32,
        issuer: &Principal,
        epoch_scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorityState;

    /// Whether `principal` is a current Manager+ identity for `epoch_scope` (§3.3).
    fn principal_is_manager_plus(
        &self,
        principal: &Principal,
        epoch_scope: &Scope,
        scope_epoch: u64,
    ) -> AuthorityState;
}

/// The one authorization answer this crate cannot ask for itself, injected per operation.
///
/// Causal validity depends on the operation's position in the bump's transitive parent closure,
/// which is DAG state no chain field identifies. Every other §2.4 view question is asked through
/// [`ManagerAuthorityView`] instead of being handed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthorizationSnapshot {
    /// Whether the operation sits in the epoch bump's parent closure (§5.2 rule 2).
    pub epoch_causally_valid_for_operation: bool,
}

/// Caller-supplied verification policy; there is deliberately no `Default`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VerificationOptions {
    /// A stricter-than-protocol lifetime bound; the 24-hour maximum always applies too.
    pub stricter_maximum_lifetime_ms: Option<u64>,
    /// Whether §5.2's historical-replay exception may excuse a stale epoch.
    pub allow_causal_replay: bool,
}

impl VerificationOptions {
    /// The live-traffic policy: the protocol lifetime bound only, no replay exception.
    pub const fn live() -> Self {
        Self {
            stricter_maximum_lifetime_ms: None,
            allow_causal_replay: false,
        }
    }

    /// The historical-replay policy of §5.2 rule 2.
    pub const fn causal_replay() -> Self {
        Self {
            stricter_maximum_lifetime_ms: None,
            allow_causal_replay: true,
        }
    }
}

/// What a chain proves once all nine §2.4 steps pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedCapability {
    /// BLAKE3 identifier of the chain artifact.
    pub chain_id: ChainId,
    /// BLAKE3 identifier of the leaf certificate.
    pub leaf_certificate_id: CertificateId,
    /// The principal the leaf grant binds to.
    pub leaf_audience: Principal,
    /// The leaf `grant_scope`; the maximum scope this capability reaches.
    pub effective_scope: Scope,
    /// The scope whose epoch revokes this chain.
    pub epoch_scope: Scope,
    /// The epoch the chain certifies.
    pub scope_epoch: u64,
    /// The leaf verb bitset.
    pub effective_verbs: VerbSet,
    /// The leaf object-class bitset.
    pub effective_object_classes: ObjectClassSet,
    /// The leaf caveat map, already the most restrictive in the chain.
    pub effective_caveats: Caveats,
    /// The latest `not_before_ms` in the chain.
    pub effective_not_before_ms: u64,
    /// The earliest `not_after_ms` in the chain (§4.5).
    pub effective_not_after_ms: u64,
}

/// Verifies a chain this process already holds as a value.
pub fn verify_chain(
    chain: &Chain,
    request: &AuthorizationRequest,
    view: &dyn ManagerAuthorityView,
    snapshot: &AuthorizationSnapshot,
    options: &VerificationOptions,
) -> Result<VerifiedCapability, ChainError> {
    let bytes = chain.encode_canonical()?;
    verify_chain_bytes(&bytes, request, view, snapshot, options)
}

/// Verifies chain bytes as received, executing the nine §2.4 steps in order.
pub fn verify_chain_bytes(
    chain_bytes: &[u8],
    request: &AuthorizationRequest,
    view: &dyn ManagerAuthorityView,
    snapshot: &AuthorizationSnapshot,
    options: &VerificationOptions,
) -> Result<VerifiedCapability, ChainError> {
    // Step 1: canonical round-trip of the outer chain and of every certificate.
    let chain = Chain::decode_canonical(chain_bytes)?;
    if chain.encode_canonical()?.as_slice() != chain_bytes {
        return Err(ChainError::NonCanonicalChainBytes);
    }
    let mut certificates = Vec::with_capacity(chain.certificates.len());
    for (index, certificate_bytes) in chain.certificates.iter().enumerate() {
        let certificate = Certificate::decode_canonical(certificate_bytes)
            .map_err(|source| ChainError::CertificateEncoding { index, source })?;
        if certificate
            .encode_canonical()
            .map_err(|source| ChainError::CertificateEncoding { index, source })?
            .as_slice()
            != certificate_bytes.as_slice()
        {
            return Err(ChainError::NonCanonicalCertificateBytes { index });
        }
        certificates.push(certificate);
    }

    // Step 2: DID-to-key binding of both principals, then the issuer signature.
    for (index, certificate) in certificates.iter().enumerate() {
        verify_binding(index, PrincipalRole::Issuer, || {
            crate::signing::verify_author_binding(
                &certificate.unsigned.issuer.did,
                &certificate.unsigned.issuer.public_key,
            )
        })?;
        verify_binding(index, PrincipalRole::Audience, || {
            crate::signing::verify_author_binding(
                &certificate.unsigned.audience.did,
                &certificate.unsigned.audience.public_key,
            )
        })?;
        certificate
            .verify_issuer_signature()
            .map_err(|_| ChainError::CertificateSignatureInvalid { index })?;
    }

    // Step 3: the root has no parent, and the view itself is asked whether the root's own
    // issuer_authority_id makes the root's own issuer Manager+ for the root's own epoch_scope.
    let root = &certificates[0].unsigned;
    if root.parent_certificate_id.is_some() {
        return Err(ChainError::RootHasParent);
    }
    match view.authority_is_manager_plus(
        &root.issuer_authority_id,
        &root.issuer,
        &root.epoch_scope,
        root.scope_epoch,
    ) {
        AuthorityState::ManagerPlus => {}
        AuthorityState::NotManagerPlus => return Err(ChainError::RootIssuerNotManagerPlus),
        AuthorityState::Unknown => return Err(ChainError::RootAuthorityRecordUnknown),
    }

    // Step 4: parent links and epoch-family equality against the root.
    for index in 1..certificates.len() {
        let previous = &certificates[index - 1];
        let current = &certificates[index].unsigned;
        let expected_parent = certificate_id_of_bytes(&chain.certificates[index - 1]);
        match current.parent_certificate_id {
            None => return Err(ChainError::NonRootMissingParent { index }),
            Some(parent) if parent != expected_parent => {
                return Err(ChainError::ParentLinkMismatch { index })
            }
            Some(_) => {}
        }
        if current.issuer != previous.unsigned.audience {
            return Err(ChainError::IssuerAudienceMismatch { index });
        }
        if current.issuer_authority_id != root.issuer_authority_id {
            return Err(ChainError::AuthorityAnchorMismatch { index });
        }
        if current.epoch_scope != root.epoch_scope {
            return Err(ChainError::EpochScopeMismatch { index });
        }
        if current.scope_epoch != root.scope_epoch {
            return Err(ChainError::ScopeEpochMismatch { index });
        }
    }

    // Step 5: strict attenuation at every hop.
    for index in 1..certificates.len() {
        let parent = &certificates[index - 1].unsigned;
        let child = &certificates[index].unsigned;
        if !parent.grant_scope.contains(&child.grant_scope) {
            return Err(not_attenuated(index, AttenuationAspect::GrantScope));
        }
        if !child.verbs.is_subset_of(parent.verbs) {
            return Err(not_attenuated(index, AttenuationAspect::Verbs));
        }
        if !child.object_classes.is_subset_of(parent.object_classes) {
            return Err(not_attenuated(index, AttenuationAspect::ObjectClasses));
        }
        if child.not_before_ms < parent.not_before_ms {
            return Err(not_attenuated(index, AttenuationAspect::NotBefore));
        }
        if child.not_after_ms > parent.not_after_ms {
            return Err(not_attenuated(index, AttenuationAspect::NotAfter));
        }
        if parent.delegation_depth == 0 {
            return Err(ChainError::DelegationDepthExhausted { index });
        }
        if child.delegation_depth > parent.delegation_depth - 1 {
            return Err(not_attenuated(index, AttenuationAspect::DelegationDepth));
        }
        if let Some(field) = parent.caveats.attenuation_violation(&child.caveats) {
            return Err(not_attenuated(index, AttenuationAspect::Caveat(field)));
        }
    }

    let leaf = &certificates
        .last()
        .expect("a decoded chain always holds a certificate")
        .unsigned;

    // Step 6: the leaf audience binds to the acting principal.
    let acting_principal = match request.verb {
        Verb::Append => &request.operation_author,
        _ => &request.requester,
    };
    if leaf.audience != *acting_principal {
        return Err(ChainError::LeafAudienceMismatch);
    }

    // Step 7: per-certificate lifetime bound, the effective window, and the epoch binding.
    let maximum_lifetime_ms = match options.stricter_maximum_lifetime_ms {
        Some(stricter) => stricter.min(MAXIMUM_CERTIFICATE_LIFETIME_MS),
        None => MAXIMUM_CERTIFICATE_LIFETIME_MS,
    };
    for (index, certificate) in certificates.iter().enumerate() {
        certificate
            .unsigned
            .check_lifetime(maximum_lifetime_ms)
            .map_err(|error| match error {
                CapabilityError::LifetimeExceedsMaximum {
                    lifetime_ms,
                    maximum_ms,
                } => ChainError::LifetimeExceedsMaximum {
                    index,
                    lifetime_ms,
                    maximum_ms,
                },
                other => ChainError::Encoding(other),
            })?;
    }
    let effective_not_before_ms = certificates
        .iter()
        .map(|certificate| certificate.unsigned.not_before_ms)
        .max()
        .expect("a decoded chain always holds a certificate");
    let effective_not_after_ms = certificates
        .iter()
        .map(|certificate| certificate.unsigned.not_after_ms)
        .min()
        .expect("a decoded chain always holds a certificate");
    if request.now_ms < effective_not_before_ms {
        return Err(ChainError::NotYetValid {
            not_before_ms: effective_not_before_ms,
            now_ms: request.now_ms,
        });
    }
    if request.now_ms >= effective_not_after_ms {
        return Err(ChainError::Expired {
            not_after_ms: effective_not_after_ms,
            now_ms: request.now_ms,
        });
    }
    if request.operation_scope_epoch != leaf.scope_epoch {
        return Err(ChainError::OperationEpochMismatch {
            operation_scope_epoch: request.operation_scope_epoch,
            leaf_scope_epoch: leaf.scope_epoch,
        });
    }

    // Step 8: the requested action fits the effective leaf grant and every caveat. The Manager+
    // question §3.3 asks is about the principal step 6 just bound, for the leaf's own epoch scope.
    let actor_is_manager_plus = view
        .principal_is_manager_plus(acting_principal, &leaf.epoch_scope, leaf.scope_epoch)
        .is_manager_plus();
    check_request_fits_leaf(leaf, request, actor_is_manager_plus)?;

    // Step 9: the persistent view's epoch still matches, unless causal replay excuses it.
    let current_epoch = view
        .current_epoch(&leaf.epoch_scope)
        .ok_or(ChainError::EpochScopeUnknown)?;
    if current_epoch != leaf.scope_epoch
        && !(options.allow_causal_replay && snapshot.epoch_causally_valid_for_operation)
    {
        return Err(ChainError::EpochRevoked {
            current_epoch,
            chain_epoch: leaf.scope_epoch,
        });
    }

    Ok(VerifiedCapability {
        chain_id: chain.chain_id()?,
        leaf_certificate_id: chain.leaf_certificate_id,
        leaf_audience: leaf.audience.clone(),
        effective_scope: leaf.grant_scope,
        epoch_scope: leaf.epoch_scope,
        scope_epoch: leaf.scope_epoch,
        effective_verbs: leaf.verbs,
        effective_object_classes: leaf.object_classes,
        effective_caveats: leaf.caveats.clone(),
        effective_not_before_ms,
        effective_not_after_ms,
    })
}

fn not_attenuated(index: usize, aspect: AttenuationAspect) -> ChainError {
    ChainError::NotAttenuated { index, aspect }
}

fn verify_binding(
    index: usize,
    role: PrincipalRole,
    check: impl FnOnce() -> Result<(), crate::signing::SigningError>,
) -> Result<(), ChainError> {
    check().map_err(|source| ChainError::PrincipalBindingInvalid {
        index,
        role,
        source: CapabilityError::Signing(source),
    })
}

/// §2.4 step 8: verb, class, table, scope, resource, sizes, schema, kind, and preview rate.
fn check_request_fits_leaf(
    leaf: &super::certificate::UnsignedCertificate,
    request: &AuthorizationRequest,
    actor_is_manager_plus: bool,
) -> Result<(), ChainError> {
    if !leaf.verbs.contains(request.verb) {
        return Err(ChainError::VerbNotGranted { verb: request.verb });
    }
    if !leaf.object_classes.contains(request.object_class) {
        return Err(ChainError::ObjectClassNotGranted {
            object_class: request.object_class,
        });
    }
    permissions::evaluate(&PermissionContext {
        verb: request.verb,
        object_class: request.object_class,
        operation_kind: request.operation_kind,
        is_structural_operation: request.is_structural_operation,
        actor_is_manager_plus,
        has_operation_kind_allowlist: leaf.caveats.allowed_operation_kinds.is_some(),
    })?;
    if !leaf.grant_scope.contains(&request.target_scope) {
        return Err(ChainError::ScopeNotContained);
    }
    if let Some(allowlist) = &leaf.caveats.resource_allowlist {
        match request.resource_id {
            Some(resource) if allowlist.contains(&resource) => {}
            _ => return Err(ChainError::ResourceNotAllowed),
        }
    }
    if let Some(limit) = leaf.caveats.max_payload_bytes {
        let actual = request.payload_bytes.unwrap_or(0);
        if actual > limit {
            return Err(ChainError::PayloadTooLarge { limit, actual });
        }
    }
    if let Some(limit) = leaf.caveats.max_segment_bytes {
        let actual = request.segment_bytes.unwrap_or(0);
        if actual > limit {
            return Err(ChainError::SegmentTooLarge { limit, actual });
        }
    }
    if let Some(allowlist) = &leaf.caveats.allowed_schema_hashes {
        match request.schema_hash {
            Some(schema_hash) if allowlist.contains(&schema_hash) => {}
            _ => return Err(ChainError::SchemaNotAllowed),
        }
    }
    if let Some(allowlist) = &leaf.caveats.allowed_operation_kinds {
        match request.operation_kind {
            Some(kind) if allowlist.contains(&kind) => {}
            Some(kind) => return Err(ChainError::OperationKindNotAllowed { kind }),
            None => return Err(ChainError::OperationKindNotAllowed { kind: 0 }),
        }
    }
    if let Some(limit) = leaf.caveats.max_preview_hz {
        let requested = request.preview_hz.unwrap_or(0);
        if requested > limit {
            return Err(ChainError::PreviewRateTooHigh { limit, requested });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor::CborError;
    use crate::segment::hashseq::LaneKey;
    use crate::segment::relay_policy::{
        authorize_scope_key_wrap, decide_fetch, decide_seed, relay_authorities, FetchDecision,
        PeerIdentity, RelayAuthorizationView, RelayRefusal, SeedDecision,
    };
    use crate::segment::SegmentError;
    use crate::signing::SigningError;
    use ed25519_dalek::SigningKey;

    use super::super::certificate::{UnsignedCertificate, CERTIFICATE_SIGNATURE_DOMAIN};

    const ROOT_ISSUER_SEED: u8 = 0x01;
    const DELEGATE_SEED: u8 = 0x02;
    const LEAF_SEED: u8 = 0x03;
    const STRANGER_SEED: u8 = 0x09;
    const NOT_BEFORE_MS: u64 = 1_700_000_000_000;
    const ONE_HOUR_MS: u64 = 3_600_000;
    const NOW_MS: u64 = NOT_BEFORE_MS + 1_000;
    const SCOPE_EPOCH: u64 = 7;

    fn signing_key(seed_byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed_byte; 32])
    }

    fn principal(seed_byte: u8) -> Principal {
        Principal::from_public_key(signing_key(seed_byte).verifying_key().to_bytes())
    }

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn verse() -> Identifier32 {
        identifier(0x11)
    }

    fn petal_scope() -> Scope {
        Scope::new(verse(), Some(identifier(0x22)), None).expect("petal scope")
    }

    fn root_unsigned() -> UnsignedCertificate {
        UnsignedCertificate {
            capability_version: CAPABILITY_VERSION,
            issuer: principal(ROOT_ISSUER_SEED),
            audience: principal(DELEGATE_SEED),
            parent_certificate_id: None,
            issuer_authority_id: Hash32([0xa0; 32]),
            grant_scope: Scope::verse_wide(verse()),
            epoch_scope: Scope::verse_wide(verse()),
            scope_epoch: SCOPE_EPOCH,
            verbs: VerbSet::from_verbs([Verb::Append, Verb::Fetch, Verb::Seed]).expect("verbs"),
            object_classes: ObjectClassSet::from_classes([
                ObjectClass::Operation,
                ObjectClass::Segment,
            ])
            .expect("classes"),
            not_before_ms: NOT_BEFORE_MS,
            not_after_ms: NOT_BEFORE_MS + ONE_HOUR_MS,
            delegation_depth: 2,
            caveats: Caveats {
                max_payload_bytes: Some(8192),
                max_segment_bytes: Some(1_048_576),
                allowed_schema_hashes: Some(vec![Hash32([0x50; 32]), Hash32([0x60; 32])]),
                resource_allowlist: None,
                allowed_operation_kinds: Some(vec![1, 4]),
                max_preview_hz: Some(30),
            },
        }
    }

    fn delegate_unsigned(parent: &Certificate) -> UnsignedCertificate {
        UnsignedCertificate {
            capability_version: CAPABILITY_VERSION,
            issuer: parent.unsigned.audience.clone(),
            audience: principal(LEAF_SEED),
            parent_certificate_id: Some(parent.certificate_id().expect("id")),
            issuer_authority_id: parent.unsigned.issuer_authority_id,
            grant_scope: petal_scope(),
            epoch_scope: parent.unsigned.epoch_scope,
            scope_epoch: parent.unsigned.scope_epoch,
            verbs: VerbSet::from_verbs([Verb::Append, Verb::Fetch]).expect("verbs"),
            object_classes: ObjectClassSet::from_classes([ObjectClass::Operation])
                .expect("classes"),
            not_before_ms: parent.unsigned.not_before_ms,
            not_after_ms: parent.unsigned.not_after_ms,
            delegation_depth: 0,
            caveats: Caveats {
                max_payload_bytes: Some(4096),
                max_segment_bytes: Some(1_048_576),
                allowed_schema_hashes: Some(vec![Hash32([0x50; 32])]),
                resource_allowlist: None,
                allowed_operation_kinds: Some(vec![1]),
                max_preview_hz: Some(10),
            },
        }
    }

    /// Builds a two-certificate chain, applying a mutation to each unsigned certificate first.
    fn chain_with(
        mutate_root: impl FnOnce(&mut UnsignedCertificate),
        mutate_delegate: impl FnOnce(&mut UnsignedCertificate),
    ) -> Chain {
        let mut root = root_unsigned();
        mutate_root(&mut root);
        let root = root
            .sign(&signing_key(ROOT_ISSUER_SEED))
            .expect("sign root");
        let mut delegate = delegate_unsigned(&root);
        mutate_delegate(&mut delegate);
        let delegate = delegate
            .sign(&signing_key(DELEGATE_SEED))
            .expect("sign delegate");
        Chain::from_certificates(&[root, delegate]).expect("chain")
    }

    fn chain_with_delegate(mutate: impl FnOnce(&mut UnsignedCertificate)) -> Chain {
        chain_with(|_| {}, mutate)
    }

    fn sample_chain() -> Chain {
        chain_with(|_| {}, |_| {})
    }

    fn signed_pair() -> (Certificate, Certificate) {
        let root = root_unsigned()
            .sign(&signing_key(ROOT_ISSUER_SEED))
            .expect("sign root");
        let delegate = delegate_unsigned(&root)
            .sign(&signing_key(DELEGATE_SEED))
            .expect("sign delegate");
        (root, delegate)
    }

    fn append_request() -> AuthorizationRequest {
        AuthorizationRequest {
            verb: Verb::Append,
            object_class: ObjectClass::Operation,
            target_scope: petal_scope(),
            resource_id: None,
            requester: principal(LEAF_SEED),
            operation_author: principal(LEAF_SEED),
            operation_scope_epoch: SCOPE_EPOCH,
            operation_kind: Some(1),
            is_structural_operation: false,
            payload_bytes: Some(1024),
            segment_bytes: None,
            schema_hash: Some(Hash32([0x50; 32])),
            preview_hz: None,
            now_ms: NOW_MS,
        }
    }

    fn snapshot() -> AuthorizationSnapshot {
        AuthorizationSnapshot {
            epoch_causally_valid_for_operation: false,
        }
    }

    /// An in-memory stand-in for the durable authorization view Wave 3 supplies.
    ///
    /// Authority records deliberately span epochs, so a bumped `current_epoch` still resolves
    /// step 3 and the chain reaches step 9 rather than short-circuiting earlier.
    #[derive(Clone, Debug)]
    struct FakeAuthorityView {
        authority_records: Vec<(Hash32, Principal)>,
        manager_principals: Vec<Principal>,
        known_epoch_scope: Option<Scope>,
        current_epoch: Option<u64>,
        version: u64,
    }

    impl AuthorizationView for FakeAuthorityView {
        fn current_epoch(&self, epoch_scope: &Scope) -> Option<u64> {
            if self.known_epoch_scope != Some(*epoch_scope) {
                return None;
            }
            self.current_epoch
        }

        fn version(&self) -> u64 {
            self.version
        }
    }

    impl ManagerAuthorityView for FakeAuthorityView {
        fn authority_is_manager_plus(
            &self,
            authority_id: &Hash32,
            issuer: &Principal,
            epoch_scope: &Scope,
            _scope_epoch: u64,
        ) -> AuthorityState {
            if self.known_epoch_scope != Some(*epoch_scope) {
                return AuthorityState::Unknown;
            }
            match self
                .authority_records
                .iter()
                .find(|(recorded, _)| recorded == authority_id)
            {
                Some((_, holder)) if holder == issuer => AuthorityState::ManagerPlus,
                Some(_) => AuthorityState::NotManagerPlus,
                None => AuthorityState::Unknown,
            }
        }

        fn principal_is_manager_plus(
            &self,
            principal: &Principal,
            epoch_scope: &Scope,
            _scope_epoch: u64,
        ) -> AuthorityState {
            if self.known_epoch_scope != Some(*epoch_scope) {
                return AuthorityState::Unknown;
            }
            if self.manager_principals.contains(principal) {
                AuthorityState::ManagerPlus
            } else {
                AuthorityState::NotManagerPlus
            }
        }
    }

    /// The view every happy-path test runs against: the root anchor resolves, nobody else does.
    fn view() -> FakeAuthorityView {
        FakeAuthorityView {
            authority_records: vec![(Hash32([0xa0; 32]), principal(ROOT_ISSUER_SEED))],
            manager_principals: Vec::new(),
            known_epoch_scope: Some(Scope::verse_wide(verse())),
            current_epoch: Some(SCOPE_EPOCH),
            version: 1,
        }
    }

    fn verify(chain: &Chain) -> Result<VerifiedCapability, ChainError> {
        verify_chain(
            chain,
            &append_request(),
            &view(),
            &snapshot(),
            &VerificationOptions::live(),
        )
    }

    #[test]
    fn a_chain_round_trips_and_addresses_its_own_canonical_bytes() {
        let chain = sample_chain();
        let bytes = chain.encode_canonical().expect("encode");
        let decoded = Chain::decode_canonical(&bytes).expect("decode");

        assert_eq!(decoded, chain);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);
        assert_eq!(chain.chain_id().expect("id"), Hash32::of(&bytes));
        assert_eq!(
            chain.leaf_certificate_id,
            certificate_id_of_bytes(chain.certificates.last().expect("leaf"))
        );

        let verified = verify(&chain).expect("verify");
        assert_eq!(verified.chain_id, chain.chain_id().expect("id"));
        assert_eq!(verified.leaf_certificate_id, chain.leaf_certificate_id);
        assert_eq!(verified.effective_scope, petal_scope());
        assert_eq!(verified.epoch_scope, Scope::verse_wide(verse()));
        assert_eq!(verified.scope_epoch, SCOPE_EPOCH);
        assert_eq!(verified.leaf_audience, principal(LEAF_SEED));
        assert_eq!(verified.effective_not_before_ms, NOT_BEFORE_MS);
        assert_eq!(verified.effective_not_after_ms, NOT_BEFORE_MS + ONE_HOUR_MS);
        assert_eq!(verified.effective_caveats.max_payload_bytes, Some(4096));
    }

    #[test]
    fn capability_chain_rejects_noncanonical_cbor() {
        let chain = sample_chain();
        let bytes = chain.encode_canonical().expect("encode");

        let mut trailing = bytes.clone();
        trailing.push(0x00);
        assert_eq!(
            verify_chain_bytes(
                &trailing,
                &append_request(),
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::Encoding(CapabilityError::Cbor(
                CborError::TrailingBytes { count: 1 }
            )))
        );

        let mut indefinite = bytes.clone();
        indefinite[0] = 0xbf;
        assert!(matches!(
            Chain::decode_canonical(&indefinite),
            Err(CapabilityError::Cbor(CborError::IndefiniteLength { .. }))
        ));

        // Keys 2, 0, 1 in that order: well formed CBOR that is not canonically sorted.
        assert!(matches!(
            Chain::decode_canonical(&[0xa3, 0x02, 0x40, 0x00, 0x01, 0x01, 0x80]),
            Err(CapabilityError::Cbor(CborError::UnsortedMapKeys { .. }))
        ));

        // Key 0 repeated.
        assert!(matches!(
            Chain::decode_canonical(&[0xa3, 0x00, 0x01, 0x00, 0x01, 0x02, 0x40]),
            Err(CapabilityError::Cbor(CborError::DuplicateMapKey { .. }))
        ));

        // A half-precision float, which the profile has no value model for.
        assert!(matches!(
            Chain::decode_canonical(&[0xf9, 0x3c, 0x00]),
            Err(CapabilityError::Cbor(
                CborError::UnsupportedSimpleValue { .. }
            ))
        ));

        // The text string "e" followed by U+0301, which is not Unicode NFC.
        assert!(matches!(
            Chain::decode_canonical(&[0x63, 0x65, 0xcc, 0x81]),
            Err(CapabilityError::Cbor(CborError::NotNormalizedNfc { .. }))
        ));

        let unknown_key = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(CAPABILITY_VERSION)),
            (
                CborValue::Uint(1),
                CborValue::Array(
                    chain
                        .certificates
                        .iter()
                        .map(|certificate| CborValue::Bytes(certificate.clone()))
                        .collect(),
                ),
            ),
            (
                CborValue::Uint(2),
                CborValue::Bytes(chain.leaf_certificate_id.0.to_vec()),
            ),
            (CborValue::Uint(3), CborValue::Uint(0)),
        ]);
        assert_eq!(
            Chain::from_cbor(&unknown_key),
            Err(CapabilityError::UnexpectedMapKeys {
                context: "capability_chain",
                expected_key_count: 3,
            })
        );

        let mismatched_did = chain_with_delegate(|delegate| {
            delegate.audience.did = principal(STRANGER_SEED).did;
        });
        assert_eq!(
            verify(&mismatched_did),
            Err(ChainError::PrincipalBindingInvalid {
                index: 1,
                role: PrincipalRole::Audience,
                source: CapabilityError::Signing(SigningError::AuthorBindingMismatch),
            })
        );
    }

    #[test]
    fn capability_chain_rejects_link_or_signature_substitution() {
        let changed_parent = chain_with_delegate(|delegate| {
            delegate.parent_certificate_id = Some(Hash32([0xee; 32]));
        });
        assert_eq!(
            verify(&changed_parent),
            Err(ChainError::ParentLinkMismatch { index: 1 })
        );

        let dropped_parent = chain_with_delegate(|delegate| {
            delegate.parent_certificate_id = None;
        });
        assert_eq!(
            verify(&dropped_parent),
            Err(ChainError::NonRootMissingParent { index: 1 })
        );

        let changed_issuer = chain_with_delegate(|delegate| {
            delegate.issuer = principal(STRANGER_SEED);
        });
        assert_eq!(
            verify(&changed_issuer),
            Err(ChainError::CertificateSignatureInvalid { index: 1 }),
            "a substituted issuer no longer matches the delegate signing key"
        );

        let stranger_issuer_and_key = {
            let root = root_unsigned()
                .sign(&signing_key(ROOT_ISSUER_SEED))
                .expect("sign root");
            let mut delegate = delegate_unsigned(&root);
            delegate.issuer = principal(STRANGER_SEED);
            let delegate = delegate
                .sign(&signing_key(STRANGER_SEED))
                .expect("sign delegate");
            Chain::from_certificates(&[root, delegate]).expect("chain")
        };
        assert_eq!(
            verify(&stranger_issuer_and_key),
            Err(ChainError::IssuerAudienceMismatch { index: 1 }),
            "a correctly signed certificate still needs the issuer/audience link"
        );

        let changed_anchor = chain_with_delegate(|delegate| {
            delegate.issuer_authority_id = Hash32([0xbb; 32]);
        });
        assert_eq!(
            verify(&changed_anchor),
            Err(ChainError::AuthorityAnchorMismatch { index: 1 })
        );

        let mut changed_leaf_id = sample_chain();
        changed_leaf_id.leaf_certificate_id = Hash32([0xcd; 32]);
        assert_eq!(
            changed_leaf_id.encode_canonical(),
            Err(CapabilityError::LeafCertificateIdMismatch)
        );

        let (root, delegate) = signed_pair();
        let root_signature = root.signature;
        let mut forged = delegate;
        forged.signature = root_signature;
        let forged_chain = Chain::from_certificates(&[root, forged]).expect("chain");
        assert_eq!(
            verify(&forged_chain),
            Err(ChainError::CertificateSignatureInvalid { index: 1 })
        );

        let (root, delegate) = signed_pair();
        let wrong_domain_signature = crate::signing::sign_domain(
            &signing_key(DELEGATE_SEED),
            crate::signing::SIGNATURE_DOMAIN,
            &delegate.unsigned.encode_canonical().expect("encode"),
        );
        let mut wrong_domain = delegate;
        wrong_domain.signature = wrong_domain_signature;
        let wrong_domain_chain = Chain::from_certificates(&[root, wrong_domain]).expect("chain");
        assert_eq!(
            verify(&wrong_domain_chain),
            Err(ChainError::CertificateSignatureInvalid { index: 1 }),
            "the operation-envelope domain must never authorize a certificate"
        );
    }

    #[test]
    fn capability_delegation_only_attenuates() {
        let widened_scope = chain_with(
            |root| root.grant_scope = petal_scope(),
            |delegate| delegate.grant_scope = Scope::verse_wide(verse()),
        );
        assert_eq!(
            verify(&widened_scope),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::GrantScope,
            })
        );

        let widened_verbs = chain_with_delegate(|delegate| {
            delegate.verbs =
                VerbSet::from_verbs([Verb::Append, Verb::Fetch, Verb::Decrypt]).expect("verbs");
        });
        assert_eq!(
            verify(&widened_verbs),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::Verbs,
            })
        );

        let widened_classes = chain_with_delegate(|delegate| {
            delegate.object_classes = ObjectClassSet::from_classes([
                ObjectClass::Operation,
                ObjectClass::Segment,
                ObjectClass::Tile,
            ])
            .expect("classes");
        });
        assert_eq!(
            verify(&widened_classes),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::ObjectClasses,
            })
        );

        let earlier_start = chain_with_delegate(|delegate| {
            delegate.not_before_ms -= 1;
        });
        assert_eq!(
            verify(&earlier_start),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::NotBefore,
            })
        );

        let extended_expiry = chain_with_delegate(|delegate| {
            delegate.not_after_ms += 1;
        });
        assert_eq!(
            verify(&extended_expiry),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::NotAfter,
            })
        );

        let widened_depth = chain_with_delegate(|delegate| {
            delegate.delegation_depth = 2;
        });
        assert_eq!(
            verify(&widened_depth),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::DelegationDepth,
            })
        );

        let widened_limit = chain_with_delegate(|delegate| {
            delegate.caveats.max_payload_bytes = Some(8193);
        });
        assert_eq!(
            verify(&widened_limit),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::Caveat(CaveatField::MaxPayloadBytes),
            })
        );

        let widened_schemas = chain_with_delegate(|delegate| {
            delegate.caveats.allowed_schema_hashes =
                Some(vec![Hash32([0x50; 32]), Hash32([0x70; 32])]);
        });
        assert_eq!(
            verify(&widened_schemas),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::Caveat(CaveatField::AllowedSchemaHashes),
            })
        );

        let widened_resources = chain_with(
            |root| root.caveats.resource_allowlist = Some(vec![identifier(0x31)]),
            |delegate| {
                delegate.caveats.resource_allowlist =
                    Some(vec![identifier(0x31), identifier(0x32)]);
            },
        );
        assert_eq!(
            verify(&widened_resources),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::Caveat(CaveatField::ResourceAllowlist),
            })
        );

        let widened_kinds = chain_with_delegate(|delegate| {
            delegate.caveats.allowed_operation_kinds = Some(vec![1, 2, 4]);
        });
        assert_eq!(
            verify(&widened_kinds),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::Caveat(CaveatField::AllowedOperationKinds),
            })
        );

        let dropped_caveat = chain_with_delegate(|delegate| {
            delegate.caveats.max_preview_hz = None;
        });
        assert_eq!(
            verify(&dropped_caveat),
            Err(ChainError::NotAttenuated {
                index: 1,
                aspect: AttenuationAspect::Caveat(CaveatField::MaxPreviewHz),
            }),
            "a non-null parent caveat may never become null in a child"
        );

        assert!(
            verify(&sample_chain()).is_ok(),
            "the honest chain still passes"
        );
    }

    #[test]
    fn a_terminal_certificate_cannot_issue_a_child() {
        let root = root_unsigned()
            .sign(&signing_key(ROOT_ISSUER_SEED))
            .expect("sign root");
        let delegate = delegate_unsigned(&root)
            .sign(&signing_key(DELEGATE_SEED))
            .expect("sign delegate");
        let mut grandchild = delegate_unsigned(&delegate);
        grandchild.audience = principal(STRANGER_SEED);
        let grandchild = grandchild
            .sign(&signing_key(LEAF_SEED))
            .expect("sign grandchild");

        let chain = Chain::from_certificates(&[root, delegate, grandchild]).expect("chain");
        assert_eq!(
            verify(&chain),
            Err(ChainError::DelegationDepthExhausted { index: 2 })
        );
    }

    #[test]
    fn capability_ttl_and_leaf_binding_are_enforced() {
        let chain = sample_chain();

        let mut early = append_request();
        early.now_ms = NOT_BEFORE_MS - 1;
        assert_eq!(
            verify_chain(
                &chain,
                &early,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::NotYetValid {
                not_before_ms: NOT_BEFORE_MS,
                now_ms: NOT_BEFORE_MS - 1,
            })
        );

        let mut at_start = append_request();
        at_start.now_ms = NOT_BEFORE_MS;
        assert!(
            verify_chain(
                &chain,
                &at_start,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            )
            .is_ok(),
            "not_before_ms is inclusive"
        );

        let mut at_expiry = append_request();
        at_expiry.now_ms = NOT_BEFORE_MS + ONE_HOUR_MS;
        assert_eq!(
            verify_chain(
                &chain,
                &at_expiry,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::Expired {
                not_after_ms: NOT_BEFORE_MS + ONE_HOUR_MS,
                now_ms: NOT_BEFORE_MS + ONE_HOUR_MS,
            }),
            "not_after_ms is exclusive"
        );

        let stricter = VerificationOptions {
            stricter_maximum_lifetime_ms: Some(60_000),
            allow_causal_replay: false,
        };
        assert_eq!(
            verify_chain(&chain, &append_request(), &view(), &snapshot(), &stricter),
            Err(ChainError::LifetimeExceedsMaximum {
                index: 0,
                lifetime_ms: ONE_HOUR_MS,
                maximum_ms: 60_000,
            })
        );

        let over_long = {
            let mut root = root_unsigned();
            root.not_after_ms = root.not_before_ms + MAXIMUM_CERTIFICATE_LIFETIME_MS + 1;
            let signature = crate::signing::sign_domain(
                &signing_key(ROOT_ISSUER_SEED),
                CERTIFICATE_SIGNATURE_DOMAIN,
                &root.encode_canonical().expect("encode"),
            );
            Chain::from_certificates(&[Certificate {
                unsigned: root,
                signature,
            }])
            .expect("chain")
        };
        let mut root_audience_request = append_request();
        root_audience_request.operation_author = principal(DELEGATE_SEED);
        root_audience_request.requester = principal(DELEGATE_SEED);
        assert_eq!(
            verify_chain(
                &over_long,
                &root_audience_request,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::LifetimeExceedsMaximum {
                index: 0,
                lifetime_ms: MAXIMUM_CERTIFICATE_LIFETIME_MS + 1,
                maximum_ms: MAXIMUM_CERTIFICATE_LIFETIME_MS,
            }),
            "a peer-supplied over-long certificate is refused at §2.4 step 7"
        );

        let mut wrong_author = append_request();
        wrong_author.operation_author = principal(STRANGER_SEED);
        assert_eq!(
            verify_chain(
                &chain,
                &wrong_author,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::LeafAudienceMismatch),
            "append binds the leaf audience to the operation author"
        );

        let mut fetch_by_stranger = append_request();
        fetch_by_stranger.verb = Verb::Fetch;
        fetch_by_stranger.requester = principal(STRANGER_SEED);
        assert_eq!(
            verify_chain(
                &chain,
                &fetch_by_stranger,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::LeafAudienceMismatch),
            "a non-append verb binds the leaf audience to the authenticated requester"
        );

        let mut stale_epoch = append_request();
        stale_epoch.operation_scope_epoch = SCOPE_EPOCH - 1;
        assert_eq!(
            verify_chain(
                &chain,
                &stale_epoch,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::OperationEpochMismatch {
                operation_scope_epoch: SCOPE_EPOCH - 1,
                leaf_scope_epoch: SCOPE_EPOCH,
            })
        );
    }

    #[test]
    fn the_root_must_be_parentless_and_manager_plus() {
        let orphan_root = chain_with(
            |root| root.parent_certificate_id = Some(Hash32([0x01; 32])),
            |_| {},
        );
        assert_eq!(verify(&orphan_root), Err(ChainError::RootHasParent));

        // The root anchor resolves, but to somebody other than this chain's root issuer.
        let other_holder = FakeAuthorityView {
            authority_records: vec![(Hash32([0xa0; 32]), principal(STRANGER_SEED))],
            ..view()
        };
        assert_eq!(
            verify_chain(
                &sample_chain(),
                &append_request(),
                &other_holder,
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::RootIssuerNotManagerPlus)
        );

        // A context-free "yes" is exactly what this must not accept: the view records the root
        // issuer as Manager+, but under an authority id this chain does not anchor itself to.
        let manager_under_another_anchor = FakeAuthorityView {
            authority_records: vec![(Hash32([0xa1; 32]), principal(ROOT_ISSUER_SEED))],
            ..view()
        };
        assert_eq!(
            verify_chain(
                &sample_chain(),
                &append_request(),
                &manager_under_another_anchor,
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::RootAuthorityRecordUnknown)
        );

        // The same record, asked about a different epoch scope, is not an answer either.
        let other_epoch_scope = FakeAuthorityView {
            known_epoch_scope: Some(petal_scope()),
            ..view()
        };
        assert_eq!(
            verify_chain(
                &sample_chain(),
                &append_request(),
                &other_epoch_scope,
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::RootAuthorityRecordUnknown)
        );

        let no_view_at_all = FakeAuthorityView {
            authority_records: Vec::new(),
            ..view()
        };
        assert_eq!(
            verify_chain(
                &sample_chain(),
                &append_request(),
                &no_view_at_all,
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::RootAuthorityRecordUnknown)
        );
    }

    #[test]
    fn a_delegate_cannot_escape_the_root_epoch_family() {
        let other_epoch_scope = chain_with_delegate(|delegate| {
            delegate.epoch_scope = petal_scope();
        });
        assert_eq!(
            verify(&other_epoch_scope),
            Err(ChainError::EpochScopeMismatch { index: 1 })
        );

        let other_epoch = chain_with_delegate(|delegate| {
            delegate.scope_epoch = SCOPE_EPOCH + 1;
        });
        assert_eq!(
            verify(&other_epoch),
            Err(ChainError::ScopeEpochMismatch { index: 1 })
        );
    }

    #[test]
    fn the_permission_table_and_caveats_deny_a_request_that_does_not_fit() {
        let chain = sample_chain();

        let mut ungranted_preview = append_request();
        ungranted_preview.verb = Verb::Preview;
        assert_eq!(
            verify_chain(
                &chain,
                &ungranted_preview,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::VerbNotGranted {
                verb: Verb::Preview
            }),
            "the leaf never granted preview, so the grant check precedes the table"
        );

        let previewable = chain_with(
            |root| {
                root.verbs =
                    VerbSet::from_verbs([Verb::Append, Verb::Fetch, Verb::Preview]).expect("verbs");
            },
            |delegate| {
                delegate.verbs =
                    VerbSet::from_verbs([Verb::Append, Verb::Fetch, Verb::Preview]).expect("verbs");
            },
        );
        let mut preview_request = append_request();
        preview_request.verb = Verb::Preview;
        preview_request.preview_hz = Some(5);
        assert_eq!(
            verify_chain(
                &previewable,
                &preview_request,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::Permission(
                PermissionError::VerbNotPermittedForClass {
                    verb: Verb::Preview,
                    object_class: ObjectClass::Operation,
                }
            )),
            "§3.3 dashes the whole preview row even when the chain granted the verb"
        );

        let mut outside_scope = append_request();
        outside_scope.target_scope =
            Scope::new(verse(), Some(identifier(0x33)), None).expect("other petal");
        assert_eq!(
            verify_chain(
                &chain,
                &outside_scope,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::ScopeNotContained)
        );

        let mut oversized = append_request();
        oversized.payload_bytes = Some(4097);
        assert_eq!(
            verify_chain(
                &chain,
                &oversized,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::PayloadTooLarge {
                limit: 4096,
                actual: 4097,
            })
        );

        let mut unlisted_schema = append_request();
        unlisted_schema.schema_hash = Some(Hash32([0x60; 32]));
        assert_eq!(
            verify_chain(
                &chain,
                &unlisted_schema,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::SchemaNotAllowed)
        );

        let mut unlisted_kind = append_request();
        unlisted_kind.operation_kind = Some(4);
        assert_eq!(
            verify_chain(
                &chain,
                &unlisted_kind,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::OperationKindNotAllowed { kind: 4 })
        );

        let mut segment_request = append_request();
        segment_request.object_class = ObjectClass::Segment;
        assert_eq!(
            verify_chain(
                &chain,
                &segment_request,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::ObjectClassNotGranted {
                object_class: ObjectClass::Segment,
            })
        );

        let resource_bound = chain_with_delegate(|delegate| {
            delegate.caveats.resource_allowlist = Some(vec![identifier(0x31)]);
        });
        assert_eq!(
            verify(&resource_bound),
            Err(ChainError::ResourceNotAllowed),
            "a resource_allowlist caveat denies a request that names no resource"
        );

        let mut named_resource = append_request();
        named_resource.resource_id = Some(identifier(0x31));
        assert!(verify_chain(
            &resource_bound,
            &named_resource,
            &view(),
            &snapshot(),
            &VerificationOptions::live()
        )
        .is_ok());

        let mut wrong_resource = append_request();
        wrong_resource.resource_id = Some(identifier(0x32));
        assert_eq!(
            verify_chain(
                &resource_bound,
                &wrong_resource,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::ResourceNotAllowed)
        );

        let segment_bound = chain_with(
            |root| {
                root.caveats.max_segment_bytes = Some(1024);
                root.object_classes =
                    ObjectClassSet::from_classes([ObjectClass::Operation, ObjectClass::Segment])
                        .expect("classes");
            },
            |delegate| {
                delegate.caveats.max_segment_bytes = Some(1024);
                delegate.object_classes =
                    ObjectClassSet::from_classes([ObjectClass::Operation, ObjectClass::Segment])
                        .expect("classes");
            },
        );
        let mut oversized_segment = append_request();
        oversized_segment.object_class = ObjectClass::Segment;
        oversized_segment.segment_bytes = Some(1025);
        assert_eq!(
            verify_chain(
                &segment_bound,
                &oversized_segment,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::SegmentTooLarge {
                limit: 1024,
                actual: 1025,
            })
        );
    }

    #[test]
    fn a_structural_append_needs_an_explicit_operation_kind_allowlist() {
        let unrestricted_kinds = chain_with(
            |root| root.caveats.allowed_operation_kinds = None,
            |delegate| delegate.caveats.allowed_operation_kinds = None,
        );
        let mut structural = append_request();
        structural.is_structural_operation = true;
        structural.operation_kind = Some(4);
        assert_eq!(
            verify_chain(
                &unrestricted_kinds,
                &structural,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::Permission(
                PermissionError::StructuralOperationRequiresKindAllowlist
            ))
        );

        let kind_four = chain_with(
            |root| root.caveats.allowed_operation_kinds = Some(vec![1, 4]),
            |delegate| delegate.caveats.allowed_operation_kinds = Some(vec![4]),
        );
        assert!(verify_chain(
            &kind_four,
            &structural,
            &view(),
            &snapshot(),
            &VerificationOptions::live()
        )
        .is_ok());
    }

    #[test]
    fn appending_a_checkpoint_requires_manager_plus_from_the_authorization_view() {
        let chain = chain_with(
            |root| {
                root.object_classes =
                    ObjectClassSet::from_classes([ObjectClass::Operation, ObjectClass::Checkpoint])
                        .expect("classes");
            },
            |delegate| {
                delegate.object_classes =
                    ObjectClassSet::from_classes([ObjectClass::Operation, ObjectClass::Checkpoint])
                        .expect("classes");
            },
        );

        let mut request = append_request();
        request.object_class = ObjectClass::Checkpoint;
        assert_eq!(
            verify_chain(
                &chain,
                &request,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::Permission(
                PermissionError::ManagerPlusRequired {
                    verb: Verb::Append,
                    object_class: ObjectClass::Checkpoint,
                }
            ))
        );

        let manager = FakeAuthorityView {
            manager_principals: vec![principal(LEAF_SEED)],
            ..view()
        };
        assert!(verify_chain(
            &chain,
            &request,
            &manager,
            &snapshot(),
            &VerificationOptions::live()
        )
        .is_ok());

        // The question is asked about the principal step 6 bound, for the leaf's own epoch scope.
        let manager_elsewhere = FakeAuthorityView {
            manager_principals: vec![principal(STRANGER_SEED)],
            ..view()
        };
        assert_eq!(
            verify_chain(
                &chain,
                &request,
                &manager_elsewhere,
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::Permission(
                PermissionError::ManagerPlusRequired {
                    verb: Verb::Append,
                    object_class: ObjectClass::Checkpoint,
                }
            ))
        );
    }

    #[test]
    fn a_bumped_epoch_revokes_the_chain_unless_causal_replay_is_allowed() {
        let chain = sample_chain();
        let bumped = FakeAuthorityView {
            current_epoch: Some(SCOPE_EPOCH + 1),
            version: 2,
            ..view()
        };
        assert_eq!(
            verify_chain(
                &chain,
                &append_request(),
                &bumped,
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::EpochRevoked {
                current_epoch: SCOPE_EPOCH + 1,
                chain_epoch: SCOPE_EPOCH,
            })
        );

        assert_eq!(
            verify_chain(
                &chain,
                &append_request(),
                &bumped,
                &snapshot(),
                &VerificationOptions::causal_replay()
            ),
            Err(ChainError::EpochRevoked {
                current_epoch: SCOPE_EPOCH + 1,
                chain_epoch: SCOPE_EPOCH,
            }),
            "a revoked-concurrent operation is never excused"
        );

        let in_closure = AuthorizationSnapshot {
            epoch_causally_valid_for_operation: true,
        };
        assert!(verify_chain(
            &chain,
            &append_request(),
            &bumped,
            &in_closure,
            &VerificationOptions::causal_replay()
        )
        .is_ok());

        assert_eq!(
            verify_chain(
                &chain,
                &append_request(),
                &bumped,
                &in_closure,
                &VerificationOptions::live()
            ),
            Err(ChainError::EpochRevoked {
                current_epoch: SCOPE_EPOCH + 1,
                chain_epoch: SCOPE_EPOCH,
            }),
            "live traffic never takes the historical-replay exception"
        );

        let no_epoch_recorded = FakeAuthorityView {
            current_epoch: None,
            ..view()
        };
        assert_eq!(
            verify_chain(
                &chain,
                &append_request(),
                &no_epoch_recorded,
                &in_closure,
                &VerificationOptions::causal_replay()
            ),
            Err(ChainError::EpochScopeUnknown),
            "a view that records no epoch for the scope is never an allow, replay or not"
        );
    }

    #[test]
    fn a_single_certificate_chain_verifies_for_its_own_audience() {
        let root = root_unsigned()
            .sign(&signing_key(ROOT_ISSUER_SEED))
            .expect("sign");
        let chain = Chain::from_certificates(&[root]).expect("chain");
        let mut request = append_request();
        request.operation_author = principal(DELEGATE_SEED);
        request.requester = principal(DELEGATE_SEED);
        request.target_scope = Scope::verse_wide(verse());
        assert!(verify_chain(
            &chain,
            &request,
            &view(),
            &snapshot(),
            &VerificationOptions::live()
        )
        .is_ok());
    }

    /// A relay view that grants `seed` to one peer and nothing else to anybody.
    struct SeedOnlyRelayView {
        seeder: PeerIdentity,
        epoch: u64,
    }

    impl RelayAuthorizationView for SeedOnlyRelayView {
        fn current_scope_epoch(&self, _lane: &LaneKey) -> Option<u64> {
            Some(self.epoch)
        }

        fn current_key_id(&self, _lane: &LaneKey) -> Option<Identifier32> {
            Some(identifier(0x41))
        }

        fn has_seed_capability(&self, peer: &PeerIdentity, _: &LaneKey, epoch: u64) -> bool {
            *peer == self.seeder && epoch == self.epoch
        }

        fn has_fetch_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }

        fn may_wrap_scope_key_for_device(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }
    }

    #[test]
    fn relay_seeding_never_implies_decrypt_or_disclosure() {
        // The certificate half: a leaf that grants only `seed` grants only `seed`.
        let seed_only = chain_with(
            |_| {},
            |delegate| {
                delegate.verbs = VerbSet::from_verbs([Verb::Seed]).expect("verbs");
            },
        );
        let mut seed_request = append_request();
        seed_request.verb = Verb::Seed;
        assert!(verify_chain(
            &seed_only,
            &seed_request,
            &view(),
            &snapshot(),
            &VerificationOptions::live()
        )
        .is_ok());

        for denied in [Verb::Decrypt, Verb::Append, Verb::Materialize, Verb::Fetch] {
            let mut request = seed_request.clone();
            request.verb = denied;
            assert_eq!(
                verify_chain(
                    &seed_only,
                    &request,
                    &view(),
                    &snapshot(),
                    &VerificationOptions::live()
                ),
                Err(ChainError::VerbNotGranted { verb: denied }),
                "seeding authority never widens into {denied:?}"
            );
        }

        let mut checkpoint_request = seed_request.clone();
        checkpoint_request.object_class = ObjectClass::Checkpoint;
        assert_eq!(
            verify_chain(
                &seed_only,
                &checkpoint_request,
                &view(),
                &snapshot(),
                &VerificationOptions::live()
            ),
            Err(ChainError::ObjectClassNotGranted {
                object_class: ObjectClass::Checkpoint,
            }),
            "a seeder is never a checkpoint authority"
        );

        // The relay half: the same separation on the SPEC-6 §6 disclosure path.
        let seeder = PeerIdentity([0x07; 32]);
        let lane = LaneKey::Header { verse_id: verse() };
        let relay_view = SeedOnlyRelayView {
            seeder,
            epoch: SCOPE_EPOCH,
        };
        let authorities = relay_authorities(&relay_view, &seeder, &lane, SCOPE_EPOCH);

        assert!(authorities.may_seed());
        assert!(!authorities.may_fetch());
        assert!(!authorities.may_decrypt());
        assert!(!authorities.may_append());
        assert!(!authorities.may_authorize_checkpoint());
        assert_eq!(
            decide_seed(&relay_view, &seeder, &lane, SCOPE_EPOCH),
            SeedDecision::Accept
        );
        assert_eq!(
            decide_fetch(
                &relay_view,
                &seeder,
                &lane,
                SCOPE_EPOCH,
                identifier(0x41),
                true
            ),
            FetchDecision::Refuse(RelayRefusal::NoFetchCapability),
            "holding the bytes and being allowed to seed them is not disclosure authority"
        );
        assert_eq!(
            authorize_scope_key_wrap(&relay_view, &seeder, &lane, SCOPE_EPOCH),
            Err(SegmentError::Unauthorized),
            "a seed-authorized peer obtains no scope-key wrap"
        );
    }

    #[test]
    fn an_empty_chain_is_rejected() {
        assert_eq!(
            Chain::from_certificates(&[]),
            Err(CapabilityError::EmptyChain)
        );
        let empty = CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(CAPABILITY_VERSION)),
            (CborValue::Uint(1), CborValue::Array(Vec::new())),
            (CborValue::Uint(2), CborValue::Bytes(vec![0u8; 32])),
        ]);
        assert_eq!(Chain::from_cbor(&empty), Err(CapabilityError::EmptyChain));
    }
}
