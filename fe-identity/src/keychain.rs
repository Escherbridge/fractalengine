use std::sync::Arc;

use crate::keypair::NodeKeypair;
use crate::secret_store::SecretStore;

const SERVICE: &str = "fractalengine";

/// Store a node keypair's 32-byte seed as hex in the given secret store.
pub fn store_keypair(
    store: &Arc<dyn SecretStore>,
    node_id: &str,
    keypair: &NodeKeypair,
) -> anyhow::Result<()> {
    let hex = hex::encode(keypair.seed_bytes());
    store
        .set(SERVICE, node_id, &hex)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Load a node keypair from the secret store by reading the hex-encoded seed.
pub fn load_keypair(store: &Arc<dyn SecretStore>, node_id: &str) -> anyhow::Result<NodeKeypair> {
    let hex = store
        .get(SERVICE, node_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .ok_or_else(|| anyhow::anyhow!("no keypair found for {node_id}"))?;
    let bytes = hex::decode(&hex)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid key length"))?;
    NodeKeypair::from_bytes(&arr)
}

/// Delete a node keypair from the secret store.
pub fn delete_keypair(store: &Arc<dyn SecretStore>, node_id: &str) -> anyhow::Result<()> {
    store
        .delete(SERVICE, node_id)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::InMemoryBackend;

    #[test]
    fn roundtrip_store_load_keypair() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let kp = NodeKeypair::generate();
        store_keypair(&store, "test-node", &kp).unwrap();
        let loaded = load_keypair(&store, "test-node").unwrap();
        assert_eq!(kp.seed_bytes(), loaded.seed_bytes());
    }

    #[test]
    fn load_missing_returns_error() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        assert!(load_keypair(&store, "missing").is_err());
    }

    #[test]
    fn delete_keypair_removes() {
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        let kp = NodeKeypair::generate();
        store_keypair(&store, "test-node", &kp).unwrap();
        delete_keypair(&store, "test-node").unwrap();
        assert!(load_keypair(&store, "test-node").is_err());
    }
}
