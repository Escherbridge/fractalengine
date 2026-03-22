use bevy::prelude::Resource;
use ed25519_dalek::{VerifyingKey, Signature};
use crate::keypair::NodeKeypair;

#[derive(Resource)]
pub struct NodeIdentity {
    keypair: NodeKeypair,
    did_key: String,
}

impl NodeIdentity {
    pub fn new(keypair: NodeKeypair) -> Self {
        let did_key = keypair.to_did_key();
        Self { keypair, did_key }
    }

    pub fn did_key(&self) -> &str {
        &self.did_key
    }

    pub fn verifying_key(&self) -> VerifyingKey {
        self.keypair.verifying_key()
    }

    pub fn sign(&self, msg: &[u8]) -> Signature {
        self.keypair.sign(msg)
    }
}
