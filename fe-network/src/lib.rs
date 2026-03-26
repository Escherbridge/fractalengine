use fe_runtime::messages::{NetworkCommand, NetworkEvent};
use libp2p::futures::StreamExt as _;

pub mod discovery;
pub mod gossip;
pub mod iroh_blobs;
pub mod iroh_docs;
pub mod swarm;
pub mod types;

pub use types::*;

pub fn spawn_network_thread(
    rx: crossbeam::channel::Receiver<NetworkCommand>,
    tx: crossbeam::channel::Sender<NetworkEvent>,
) -> std::thread::JoinHandle<()> {
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "spawn_network_thread must not be called from within a Tokio runtime"
    );
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to build network Tokio runtime");
        rt.block_on(async {
            tracing::info!("Network thread started");

            let local_key = libp2p::identity::Keypair::generate_ed25519();

            let mut swarm_handle = match swarm::build_swarm(local_key).await {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!("Failed to build swarm: {e}");
                    tx.send(NetworkEvent::Stopped).ok();
                    return;
                }
            };

            if let Err(e) = discovery::start_discovery(&mut swarm_handle).await {
                tracing::warn!("Discovery bootstrap error (continuing): {e}");
            }

            tx.send(NetworkEvent::Started).ok();

            loop {
                tokio::select! {
                    cmd = async { rx.recv() } => {
                        match cmd {
                            Ok(NetworkCommand::Ping) => {
                                tx.send(NetworkEvent::Pong).ok();
                            }
                            Ok(NetworkCommand::Shutdown) | Err(_) => break,
                        }
                    }
                    event = swarm_handle.swarm.select_next_some() => {
                        tracing::debug!("Swarm event: {:?}", event);
                    }
                }
            }

            tracing::info!("Network thread shutting down");
            tx.send(NetworkEvent::Stopped).ok();
        });
    })
}
