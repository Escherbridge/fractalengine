use ed25519_dalek::VerifyingKey;
use multibase::Base;

pub fn pub_key_to_did_key(pub_key: &VerifyingKey) -> String {
    let pub_bytes = pub_key.to_bytes();
    let mut multicodec = Vec::with_capacity(34);
    multicodec.push(0xed);
    multicodec.push(0x01);
    multicodec.extend_from_slice(&pub_bytes);
    let encoded = multibase::encode(Base::Base58Btc, &multicodec);
    format!("did:key:{}", encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keypair::NodeKeypair;

    #[test]
    fn test_did_key_format() {
        let kp = NodeKeypair::generate();
        let did = pub_key_to_did_key(&kp.verifying_key());
        assert!(did.starts_with("did:key:z6Mk"), "got: {}", did);
    }
}
