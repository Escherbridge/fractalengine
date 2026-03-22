use crate::keypair::NodeKeypair;

const SERVICE: &str = "fractalengine";

pub fn store_keypair(node_id: &str, keypair: &NodeKeypair) -> anyhow::Result<()> {
    let entry = keyring::Entry::new(SERVICE, node_id)?;
    let hex = hex::encode(keypair.seed_bytes());
    entry.set_password(&hex)?;
    Ok(())
}

pub fn load_keypair(node_id: &str) -> anyhow::Result<NodeKeypair> {
    let entry = keyring::Entry::new(SERVICE, node_id)?;
    let hex = entry.get_password()?;
    let bytes = hex::decode(&hex)?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| anyhow::anyhow!("invalid key length"))?;
    NodeKeypair::from_bytes(&arr)
}

pub fn delete_keypair(node_id: &str) -> anyhow::Result<()> {
    let entry = keyring::Entry::new(SERVICE, node_id)?;
    entry.delete_credential()?;
    Ok(())
}
