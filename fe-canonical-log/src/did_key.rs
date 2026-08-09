//! Self-contained `did:key` Ed25519 codec; see `src/AGENTS.md` §did_key.

use multibase::Base;
use thiserror::Error;

/// The `did:key` method prefix every canonical author DID starts with.
pub const DID_KEY_PREFIX: &str = "did:key:";

/// Multicodec prefix for an Ed25519 public key.
pub const ED25519_MULTICODEC_PREFIX: [u8; 2] = [0xed, 0x01];

/// Every reason a `did:key` string fails to yield an Ed25519 public key.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DidKeyError {
    /// The string did not start with `did:key:`.
    #[error("DID does not use the did:key method")]
    NotDidKeyMethod,
    /// The multibase body did not decode.
    #[error("did:key multibase body did not decode: {reason}")]
    MultibaseDecode {
        /// Underlying multibase failure text.
        reason: String,
    },
    /// The multibase body used a base other than base58btc.
    #[error("did:key must use base58btc ('z' prefix)")]
    NotBase58Btc,
    /// The decoded bytes did not begin with the Ed25519 multicodec prefix.
    #[error("did:key multicodec prefix is not 0xed 0x01 (Ed25519)")]
    WrongMulticodecPrefix,
    /// The decoded key was not exactly 32 bytes.
    #[error("did:key carries {length} key byte(s), expected 32")]
    WrongKeyLength {
        /// Number of key bytes actually present.
        length: usize,
    },
}

/// Encodes a raw Ed25519 public key as a canonical `did:key` string.
pub fn public_key_to_did(public_key: &[u8; 32]) -> String {
    let mut multicodec = Vec::with_capacity(ED25519_MULTICODEC_PREFIX.len() + public_key.len());
    multicodec.extend_from_slice(&ED25519_MULTICODEC_PREFIX);
    multicodec.extend_from_slice(public_key);
    format!(
        "{DID_KEY_PREFIX}{}",
        multibase::encode(Base::Base58Btc, &multicodec)
    )
}

/// Decodes a canonical `did:key` string back to its raw Ed25519 public key.
pub fn did_to_public_key(did: &str) -> Result<[u8; 32], DidKeyError> {
    let body = did
        .strip_prefix(DID_KEY_PREFIX)
        .ok_or(DidKeyError::NotDidKeyMethod)?;

    let (base, bytes) = multibase::decode(body).map_err(|error| DidKeyError::MultibaseDecode {
        reason: error.to_string(),
    })?;
    if base != Base::Base58Btc {
        return Err(DidKeyError::NotBase58Btc);
    }
    if bytes.len() < ED25519_MULTICODEC_PREFIX.len()
        || bytes[..ED25519_MULTICODEC_PREFIX.len()] != ED25519_MULTICODEC_PREFIX
    {
        return Err(DidKeyError::WrongMulticodecPrefix);
    }

    let key_bytes = &bytes[ED25519_MULTICODEC_PREFIX.len()..];
    key_bytes
        .try_into()
        .map_err(|_| DidKeyError::WrongKeyLength {
            length: key_bytes.len(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `signing_key.did` from `docs/spec/canonical-log/operation-envelope-v1.json`.
    const FIXTURE_DID: &str = "did:key:z6MkehRgf7yJbgaGfYsdoAsKdBPE3dj2CYhowQdcjqSJgvVd";

    /// `signing_key.public_key_hex` from `docs/spec/canonical-log/operation-envelope-v1.json`.
    const FIXTURE_PUBLIC_KEY_HEX: &str =
        "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8";

    fn fixture_public_key() -> [u8; 32] {
        hex::decode(FIXTURE_PUBLIC_KEY_HEX)
            .expect("fixture hex")
            .try_into()
            .expect("32 bytes")
    }

    #[test]
    fn fixture_public_key_encodes_to_the_fixture_did() {
        assert_eq!(public_key_to_did(&fixture_public_key()), FIXTURE_DID);
    }

    #[test]
    fn fixture_did_decodes_to_the_fixture_public_key() {
        assert_eq!(
            did_to_public_key(FIXTURE_DID).expect("fixture DID decodes"),
            fixture_public_key()
        );
    }

    #[test]
    fn round_trips_every_byte_pattern() {
        for filler in [0x00u8, 0x01, 0x7f, 0xfe, 0xff] {
            let key = [filler; 32];
            let did = public_key_to_did(&key);
            assert!(did.starts_with(DID_KEY_PREFIX));
            assert_eq!(did_to_public_key(&did).expect("round trip"), key);
        }
    }

    #[test]
    fn rejects_a_non_did_key_method() {
        assert_eq!(
            did_to_public_key("did:web:example.com"),
            Err(DidKeyError::NotDidKeyMethod)
        );
        assert_eq!(
            did_to_public_key("not-a-did"),
            Err(DidKeyError::NotDidKeyMethod)
        );
    }

    #[test]
    fn rejects_a_non_base58btc_multibase() {
        let mut multicodec = ED25519_MULTICODEC_PREFIX.to_vec();
        multicodec.extend_from_slice(&[0u8; 32]);
        let base64_body = multibase::encode(Base::Base64, &multicodec);
        assert_eq!(
            did_to_public_key(&format!("{DID_KEY_PREFIX}{base64_body}")),
            Err(DidKeyError::NotBase58Btc)
        );
    }

    #[test]
    fn rejects_a_wrong_multicodec_prefix() {
        let mut multicodec = vec![0xec, 0x01];
        multicodec.extend_from_slice(&[0u8; 32]);
        let body = multibase::encode(Base::Base58Btc, &multicodec);
        assert_eq!(
            did_to_public_key(&format!("{DID_KEY_PREFIX}{body}")),
            Err(DidKeyError::WrongMulticodecPrefix)
        );
    }

    #[test]
    fn rejects_a_key_that_is_not_thirty_two_bytes() {
        for key_length in [0usize, 31, 33] {
            let mut multicodec = ED25519_MULTICODEC_PREFIX.to_vec();
            multicodec.resize(ED25519_MULTICODEC_PREFIX.len() + key_length, 0);
            let body = multibase::encode(Base::Base58Btc, &multicodec);
            assert_eq!(
                did_to_public_key(&format!("{DID_KEY_PREFIX}{body}")),
                Err(DidKeyError::WrongKeyLength { length: key_length })
            );
        }
    }

    #[test]
    fn rejects_an_undecodable_multibase_body() {
        let error = did_to_public_key("did:key:z0OIl").expect_err("invalid base58 alphabet");
        assert!(
            matches!(error, DidKeyError::MultibaseDecode { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn rejects_a_body_shorter_than_the_multicodec_prefix() {
        let body = multibase::encode(Base::Base58Btc, [0xedu8]);
        assert_eq!(
            did_to_public_key(&format!("{DID_KEY_PREFIX}{body}")),
            Err(DidKeyError::WrongMulticodecPrefix)
        );
    }
}
