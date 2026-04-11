use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;

pub struct NodeKeypair {
    signing_key: SigningKey,
}

impl NodeKeypair {
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, anyhow::Error> {
        Ok(Self {
            signing_key: SigningKey::from_bytes(bytes),
        })
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing_key.verifying_key()
    }

    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }

    pub fn to_did_key(&self) -> String {
        crate::did_key::pub_key_to_did_key(&self.signing_key.verifying_key())
    }

    /// The raw 32-byte seed (private scalar) of the ed25519 key.
    ///
    /// Exposed publicly so that `fe-sync` can derive an [`iroh::SecretKey`]
    /// from the same seed without depending on `fe-identity` internals.
    pub fn seed_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }

    /// Convert to an [`iroh::SecretKey`] sharing the same ed25519 seed.
    ///
    /// Both `ed25519-dalek` and `iroh` derive the full keypair deterministically
    /// from a 32-byte seed, so the resulting `NodeId` matches `self.verifying_key()`.
    pub fn to_iroh_secret(&self) -> iroh::SecretKey {
        iroh::SecretKey::from_bytes(&self.seed_bytes())
    }

    pub(crate) fn pkcs8_der(&self) -> Vec<u8> {
        use ed25519_dalek::pkcs8::EncodePrivateKey;
        self.signing_key
            .to_pkcs8_der()
            .expect("PKCS#8 encoding")
            .as_bytes()
            .to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_sign() {
        let kp = NodeKeypair::generate();
        let msg = b"hello fractalengine";
        let sig = kp.sign(msg);
        kp.verifying_key().verify_strict(msg, &sig).unwrap();
    }

    #[test]
    fn test_iroh_secret_key_matches_public_key() {
        let kp = NodeKeypair::generate();
        let iroh_secret = kp.to_iroh_secret();
        // Both derive the public key from the same 32-byte seed.
        assert_eq!(
            &kp.verifying_key().to_bytes(),
            iroh_secret.public().as_bytes(),
            "iroh public key must match ed25519-dalek verifying key"
        );
    }

    #[test]
    fn test_tampered_message_rejected() {
        let kp = NodeKeypair::generate();
        let sig = kp.sign(b"hello");
        assert!(kp.verifying_key().verify_strict(b"world", &sig).is_err());
    }
}
