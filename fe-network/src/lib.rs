use fe_runtime::messages::{NetworkCommand, NetworkEvent};

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
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to build network Tokio runtime");
        rt.block_on(async {
            // TODO Sprint 3B: initialise libp2p Swarm and iroh endpoint here
            tracing::info!("Network thread started");
            tx.send(NetworkEvent::Started).ok();
            #[allow(clippy::while_let_loop)]
            loop {
                match rx.recv() {
                    Ok(NetworkCommand::Ping) => {
                        tx.send(NetworkEvent::Pong).ok();
                    }
                    Ok(NetworkCommand::Shutdown) | Err(_) => break,
                }
            }
            tracing::info!("Network thread shutting down");
            tx.send(NetworkEvent::Stopped).ok();
        });
    })
}
