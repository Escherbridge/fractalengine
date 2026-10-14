//! The production SPEC-7 §2.2 `CapabilityVerifier`: SPEC-3 §2.4 chain verification for a
//! requester bound when the authenticated connection was built. See `src/AGENTS.md`
//! §composed-verifiers.

use async_trait::async_trait;

use crate::capability::{
    verify_chain_bytes, AuthorizationRequest, AuthorizationSnapshot, Certificate, Chain,
    ManagerAuthorityView, ObjectClass, Principal, Verb, VerificationOptions,
};
use crate::envelope::Scope;
use crate::wire::error::WireError;
use crate::wire::session::{CapabilityVerifier, ObjectClassBits, VerbBits, VerifiedAuthorization};

use super::verifier::Clock;

const AUTHORIZE_CONTEXT: &str = "authorize";

/// Verifies `authorize` chains on behalf of ONE authenticated requester.
///
/// The requester is fixed at construction by whoever authenticated the connection and is never
/// read from the chain: SPEC-3 §2.4 step 6 forbids exercising a chain "merely because it can
/// fetch the chain artifact", so a chain whose leaf audience is someone else is refused here.
/// There is deliberately no constructor without a requester.
#[derive(Debug)]
pub struct ChainCapabilityVerifier<V, C> {
    requester: Principal,
    view: V,
    clock: C,
}

impl<V: ManagerAuthorityView, C: Clock> ChainCapabilityVerifier<V, C> {
    /// A verifier for the connection authenticated as `requester`.
    pub fn for_requester(requester: Principal, view: V, clock: C) -> Self {
        Self {
            requester,
            view,
            clock,
        }
    }

    /// The authenticated requester every chain is checked against.
    pub fn requester(&self) -> &Principal {
        &self.requester
    }
}

/// The one verb or class a wire bitmask names; zero, unknown, or several bits are malformed.
fn single_bit<T>(bits: u8, from_bit: fn(u8) -> Option<T>, key: u64) -> Result<T, WireError> {
    from_bit(bits).ok_or(WireError::WrongShape {
        context: AUTHORIZE_CONTEXT,
        key,
    })
}

/// The leaf's `scope_epoch`, which a session (unlike an operation) has no reference of its own
/// for; step 9 still compares it against the persistent view.
fn leaf_scope_epoch(capability_chain_bytes: &[u8]) -> Option<u64> {
    let chain = Chain::decode_canonical(capability_chain_bytes).ok()?;
    let leaf = Certificate::decode_canonical(chain.certificates.last()?).ok()?;
    Some(leaf.unsigned.scope_epoch)
}

#[async_trait]
impl<V: ManagerAuthorityView, C: Clock> CapabilityVerifier for ChainCapabilityVerifier<V, C> {
    async fn verify(
        &self,
        capability_chain_bytes: &[u8],
        requested_verb: VerbBits,
        requested_object_class: ObjectClassBits,
        requested_scope: &Scope,
    ) -> Result<VerifiedAuthorization, WireError> {
        let verb = single_bit(requested_verb, Verb::from_bit, 2)?;
        let object_class = single_bit(requested_object_class, ObjectClass::from_bit, 3)?;
        let operation_scope_epoch =
            leaf_scope_epoch(capability_chain_bytes).ok_or(WireError::CapabilityNotVerified)?;

        // A session states no operation, so every per-operation caveat input is absent; a leaf
        // whose caveats demand one refuses here, and each operation is re-checked at admission.
        let request = AuthorizationRequest {
            verb,
            object_class,
            target_scope: *requested_scope,
            resource_id: requested_scope.resource_id(),
            requester: self.requester.clone(),
            operation_author: self.requester.clone(),
            operation_scope_epoch,
            operation_kind: None,
            is_structural_operation: false,
            payload_bytes: None,
            segment_bytes: None,
            schema_hash: None,
            preview_hz: None,
            now_ms: self.clock.now_ms(),
        };
        let verified = verify_chain_bytes(
            capability_chain_bytes,
            &request,
            &self.view,
            &AuthorizationSnapshot {
                epoch_causally_valid_for_operation: false,
            },
            &VerificationOptions::live(),
        )
        .map_err(|_| WireError::CapabilityNotVerified)?;

        Ok(VerifiedAuthorization {
            leaf_principal: verified.leaf_audience,
            chain_id: verified.chain_id,
            epoch_scope: verified.epoch_scope,
            scope_epoch: verified.scope_epoch,
            expires_at_ms: verified.effective_not_after_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::compose::fixtures::{
        principal, root_chain, verse_scope, FixedClock, ManagerView, AUTHOR_SEED, EPOCH,
        NOT_BEFORE_MS, NOW_MS, ONE_HOUR_MS, STRANGER_SEED,
    };
    use crate::envelope::Hash32;

    #[tokio::test]
    async fn a_wire_authorize_cannot_borrow_someone_elses_chain() {
        let chain = root_chain(AUTHOR_SEED, &[Verb::Fetch], &[ObjectClass::Operation]);
        let fetch = Verb::Fetch.bit();
        let operation = ObjectClass::Operation.bit();

        // The author's own connection binds its own chain.
        let owner = ChainCapabilityVerifier::for_requester(
            principal(AUTHOR_SEED),
            ManagerView::at_epoch(EPOCH),
            FixedClock(NOW_MS),
        );
        assert_eq!(
            owner.verify(&chain, fetch, operation, &verse_scope()).await,
            Ok(VerifiedAuthorization {
                leaf_principal: principal(AUTHOR_SEED),
                chain_id: Hash32::of(&chain),
                epoch_scope: verse_scope(),
                scope_epoch: EPOCH,
                expires_at_ms: NOT_BEFORE_MS + ONE_HOUR_MS,
            })
        );

        // Another authenticated principal presenting the SAME valid bytes is refused (§2.4
        // step 6): holding the artifact is not holding the grant.
        let borrower = ChainCapabilityVerifier::for_requester(
            principal(STRANGER_SEED),
            ManagerView::at_epoch(EPOCH),
            FixedClock(NOW_MS),
        );
        assert_eq!(
            borrower
                .verify(&chain, fetch, operation, &verse_scope())
                .await,
            Err(WireError::CapabilityNotVerified)
        );

        // A request naming two verbs at once names no single §2.4 request: malformed, not denied.
        assert_eq!(
            owner
                .verify(
                    &chain,
                    fetch | Verb::Append.bit(),
                    operation,
                    &verse_scope()
                )
                .await,
            Err(WireError::WrongShape {
                context: AUTHORIZE_CONTEXT,
                key: 2,
            })
        );
    }
}
