//! Scope-key identifier derivation (SPEC-3 §10.1.3); see `src/crypto/AGENTS.md` §key-id.

use zeroize::Zeroize;

use crate::capability::topic::scope_key_context;
use crate::envelope::{Identifier32, Scope};
use crate::signing::domain_preimage;

use super::aead::SealingKey;
use super::CryptoError;

/// NUL-terminated ASCII domain separator for scope-key identifiers (§10.1.3).
pub const SCOPE_KEY_ID_DOMAIN: &[u8] = b"fe-scope-key-id-v1\0";

/// `key_id = BLAKE3("fe-scope-key-id-v1" || 0x00 || canonical_scope_key_context || scope_key)`.
///
/// The same `canonical_scope_key_context` derives the §6 blinded topic key, so a scope or epoch
/// change rotates the identifier and the topic label together. The preimage carries raw key
/// bytes and is zeroized before this function returns.
pub fn derive_key_id(
    scope: &Scope,
    scope_epoch: u64,
    scope_key: &SealingKey,
) -> Result<Identifier32, CryptoError> {
    let context = scope_key_context(scope, scope_epoch)?;
    let mut preimage = domain_preimage(SCOPE_KEY_ID_DOMAIN, &context);
    preimage.extend_from_slice(scope_key.expose_bytes());
    let key_id = Identifier32(*blake3::hash(&preimage).as_bytes());
    preimage.zeroize();
    Ok(key_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::crypto::aead::SEALING_KEY_LENGTH;
    use crate::envelope::Identifier32 as Id;

    fn verse(filler: u8) -> Scope {
        Scope::verse_wide(Id([filler; 32]))
    }

    fn key(filler: u8) -> SealingKey {
        SealingKey::from_bytes([filler; SEALING_KEY_LENGTH])
    }

    #[test]
    fn the_domain_separator_is_nul_terminated_ascii() {
        assert_eq!(
            hex::encode(SCOPE_KEY_ID_DOMAIN),
            "66652d73636f70652d6b65792d69642d763100"
        );
        assert!(SCOPE_KEY_ID_DOMAIN.ends_with(&[0]));
    }

    #[test]
    fn derivation_is_deterministic() {
        assert_eq!(
            derive_key_id(&verse(0x11), 4, &key(0x5a)).expect("key id"),
            derive_key_id(&verse(0x11), 4, &key(0x5a)).expect("key id")
        );
    }

    #[test]
    fn a_different_scope_epoch_or_key_derives_a_different_identifier() {
        let baseline = derive_key_id(&verse(0x11), 4, &key(0x5a)).expect("key id");
        assert_ne!(
            baseline,
            derive_key_id(&verse(0x12), 4, &key(0x5a)).expect("key id")
        );
        assert_ne!(
            baseline,
            derive_key_id(&verse(0x11), 5, &key(0x5a)).expect("key id")
        );
        assert_ne!(
            baseline,
            derive_key_id(&verse(0x11), 4, &key(0x5b)).expect("key id")
        );
    }

    #[test]
    fn the_identifier_is_not_the_key_and_does_not_contain_it() {
        let material = key(0x5a);
        let key_id = derive_key_id(&verse(0x11), 4, &material).expect("key id");
        assert_ne!(&key_id.0, material.expose_bytes());
        assert!(!key_id
            .0
            .windows(4)
            .any(|window| window == &material.expose_bytes()[..4]));
    }

    #[test]
    fn a_petal_scope_and_its_verse_derive_different_identifiers() {
        let petal = Scope::new(Id([0x11; 32]), Some(Id([0x22; 32])), None).expect("petal scope");
        assert_ne!(
            derive_key_id(&petal, 4, &key(0x5a)).expect("key id"),
            derive_key_id(&verse(0x11), 4, &key(0x5a)).expect("key id")
        );
    }
}
