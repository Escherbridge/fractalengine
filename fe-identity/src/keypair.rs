use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer};
use rand::rngs::OsRng;

pub struct NodeKeypair {
    signing_key: SigningKey,
}

impl NodeKeypair {
    pub fn generate() -> Self {
        Self { signing_key: SigningKey::generate(&mut OsRng) }
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, anyhow::Error> {
        Ok(Self { signing_key: SigningKey::from_bytes(bytes) })
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
        let seed = self.signing_key.to_bytes();
        let mut der = vec![
            0x30, 0x2e,
            0x02, 0x01, 0x00,
            0x30, 0x05,
            0x06, 0x03, 0x2b, 0x65, 0x70,
            0x04, 0x22,
            0x04, 0x20,
        ];
        der.extend_from_slice(&seed);
        der
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

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
