use libp2p::{
    kad::{self, store::MemoryStore},
    swarm::NetworkBehaviour,
    Swarm,
};

#[derive(NetworkBehaviour)]
pub struct FractalBehaviour {
    pub kademlia: kad::Behaviour<MemoryStore>,
}

pub struct SwarmHandle {
    pub swarm: Swarm<FractalBehaviour>,
}

pub async fn build_swarm(local_key: libp2p::identity::Keypair) -> anyhow::Result<SwarmHandle> {
    let swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_quic()
        .with_behaviour(|key: &libp2p::identity::Keypair| {
            let peer_id = key.public().to_peer_id();
            let store = MemoryStore::new(peer_id);
            let kademlia = kad::Behaviour::new(peer_id, store);
            Ok(FractalBehaviour { kademlia })
        })?
        .build();
    Ok(SwarmHandle { swarm })
}
