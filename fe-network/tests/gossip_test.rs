use ed25519_dalek::{Signer, SigningKey};
use fe_network::gossip::verify_gossip_message;
use fe_network::types::{AssetId, GossipMessage};
use rand::rngs::OsRng;

#[test]
fn test_gossip_valid_message() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let payload = "hello network";
    let payload_bytes = serde_json::to_vec(&payload).unwrap();
    let sig = signing_key.sign(&payload_bytes);
    let msg = GossipMessage {
        payload,
        sig: sig.to_bytes().to_vec(),
        pub_key: signing_key.verifying_key().to_bytes().to_vec(),
    };
    assert!(verify_gossip_message(&msg, None).is_ok());
}

#[test]
fn test_gossip_tampered_message() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let payload = "hello network";
    let payload_bytes = serde_json::to_vec(&payload).unwrap();
    let sig = signing_key.sign(&payload_bytes);
    let mut bad_sig = sig.to_bytes().to_vec();
    bad_sig[0] ^= 0xff;
    let msg = GossipMessage {
        payload,
        sig: bad_sig,
        pub_key: signing_key.verifying_key().to_bytes().to_vec(),
    };
    assert!(verify_gossip_message(&msg, None).is_err());
}

#[test]
fn test_asset_roundtrip() {
    let data = b"test asset bytes";
    let hash = blake3::hash(data);
    let asset_id = AssetId(*hash.as_bytes());
    let _ = asset_id;
}
