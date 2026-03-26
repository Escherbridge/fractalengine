// TODO: Wire gossip to the running swarm handle once it is accessible from this context.
// Currently these functions are stubs that log only.
use crate::types::GossipMessage;
use ed25519_dalek::{Signature, VerifyingKey};

pub fn verify_gossip_message<T: serde::de::DeserializeOwned + serde::Serialize>(
    msg: &GossipMessage<T>,
    expected_pub_key: Option<&[u8; 32]>,
) -> anyhow::Result<()> {
    let pub_key_bytes: [u8; 32] = msg
        .pub_key
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid pub_key length"))?;
    let pub_key = VerifyingKey::from_bytes(&pub_key_bytes)?;
    if let Some(expected) = expected_pub_key {
        if pub_key_bytes != *expected {
            anyhow::bail!("Public key mismatch");
        }
    }
    let payload_bytes = serde_json::to_vec(&msg.payload)?;
    let sig_bytes: [u8; 64] = msg
        .sig
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid sig length"))?;
    let sig = Signature::from_bytes(&sig_bytes);
    pub_key
        .verify_strict(&payload_bytes, &sig)
        .map_err(Into::into)
}

pub async fn subscribe_petal(petal_id: &str) -> anyhow::Result<()> {
    tracing::info!("Subscribing to petal topic: {}", petal_id);
    Ok(())
}

pub async fn broadcast<T: serde::Serialize>(msg: GossipMessage<T>) -> anyhow::Result<()> {
    tracing::info!("Broadcasting gossip message");
    let _ = msg;
    Ok(())
}
