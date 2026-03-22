use crate::types::GossipMessage;
use ed25519_dalek::{VerifyingKey, Signature, Verifier};

pub fn verify_gossip_message<T: serde::de::DeserializeOwned + serde::Serialize>(
    msg: &GossipMessage<T>,
    expected_pub_key: Option<&[u8; 32]>,
) -> anyhow::Result<()> {
    let pub_key = VerifyingKey::from_bytes(&msg.pub_key)?;
    if let Some(expected) = expected_pub_key {
        if &msg.pub_key != expected {
            anyhow::bail!("Public key mismatch");
        }
    }
    let payload_bytes = serde_json::to_vec(&msg.payload)?;
    let sig = Signature::from_bytes(&msg.sig);
    pub_key.verify_strict(&payload_bytes, &sig).map_err(Into::into)
}

pub async fn subscribe_petal(petal_id: &str) -> anyhow::Result<()> {
    // CROSS-CRATE: wire to iroh-gossip HyParView topic subscription in Sprint 5B
    tracing::info!("Subscribing to petal topic: {}", petal_id);
    Ok(())
}

pub async fn broadcast<T: serde::Serialize>(msg: GossipMessage<T>) -> anyhow::Result<()> {
    // CRITICAL: verify signature BEFORE any other logic on INBOUND messages
    // CROSS-CRATE: wire to iroh-gossip broadcast in Sprint 5B
    tracing::info!("Broadcasting gossip message");
    Ok(())
}
