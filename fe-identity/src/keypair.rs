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

    pub(crate) fn seed_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
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
    fn test_tampered_message_rejected() {
        let kp = NodeKeypair::generate();
        let sig = kp.sign(b"hello");
        assert!(kp.verifying_key().verify_strict(b"world", &sig).is_err());
    }
}
