pub async fn start_discovery(handle: &mut super::swarm::SwarmHandle) -> anyhow::Result<()> {
    handle.swarm.behaviour_mut().kademlia.bootstrap()
        .map_err(|e| anyhow::anyhow!("Kademlia bootstrap failed: {:?}", e))?;
    Ok(())
}
