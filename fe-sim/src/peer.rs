//! `SimPeer` — a simulated peer (F8/A19): a harness `TestPeer` spawned on
//! the virtual transport instead of the real iroh one.
//!
//! Deliberately NOT a parallel peer model (decision D3): everything below
//! the transport — the DB thread and its command loop, the blob store, the
//! DB→sync replication bridge, the sync thread's full command loop (pending
//! writes, inbound pump, seed reconciliation, fabric bookkeeping) — is the
//! same machinery the real-transport scenarios run. Only the transport is
//! swapped (fe-test-harness §peer-model), which is what makes the sim lab
//! honest: prod and sim cannot drift by construction.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::net::{SimNet, SimTransportFactory};

/// A simulated in-process peer: a `TestPeer` whose sync thread binds no
/// iroh endpoint and sources every verse replica from the shared
/// [`SimNet`] hub.
pub struct SimPeer {
    /// The peer's fleet name (script membership key).
    pub name: String,
    /// The wrapped harness peer (DB + sync threads, blob store, channels).
    pub peer: fractalengine_test_harness::peer::TestPeer,
}

impl SimPeer {
    /// Spawn a simulated peer under `root_dir/{name}` on the shared hub.
    ///
    /// The peer's sync thread binds no iroh endpoint (the factory installs
    /// the virtual transport), so nothing in a sim scenario touches the
    /// real network — pinned in `scenario.rs` via
    /// `fe_sync::bound_endpoint_count`.
    pub fn spawn(net: &Arc<SimNet>, name: &str, root_dir: &Path) -> Result<Self> {
        let factory = SimTransportFactory::new(net.clone());
        let peer = fractalengine_test_harness::peer::TestPeer::spawn_with_transport(
            name,
            root_dir,
            Some(factory),
        )?;
        Ok(Self {
            name: name.to_string(),
            peer,
        })
    }

    /// This peer's DID (`did:key:…`) — the author identity its replica rows
    /// carry on the hub, and the membership key for scripted churn
    /// (`SimNet::set_peer_online`).
    pub fn did(&self) -> String {
        self.peer.keypair.to_did_key()
    }
}
