//! Per-verse persisted address book of peers this node last synced with
//! (QA-0086); see fe-sync `src/AGENTS.md` §peer-book.

use std::path::{Path, PathBuf};

/// At most this many peers are remembered per verse.
const MAX_PEERS: usize = 16;

/// How often a live replica refreshes its remembered peers.
const REFRESH: std::time::Duration = std::time::Duration::from_secs(10);

fn book_path(p2p_dir: &Path, verse_id: &str) -> PathBuf {
    p2p_dir
        .join("peer-book")
        .join(format!("{}.json", hex::encode(blake3::hash(verse_id.as_bytes()).as_bytes())))
}

/// The remembered dialable addresses for `verse_id`; empty when none or unreadable.
pub(crate) fn load(p2p_dir: &Path, verse_id: &str) -> Vec<iroh::EndpointAddr> {
    let Ok(bytes) = std::fs::read(book_path(p2p_dir, verse_id)) else {
        return Vec::new();
    };
    let mut peers: Vec<iroh::EndpointAddr> = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        tracing::warn!(verse_id, "peer book unreadable — ignored: {error}");
        Vec::new()
    });
    peers.truncate(MAX_PEERS);
    peers
}

fn store(p2p_dir: &Path, verse_id: &str, peers: &[iroh::EndpointAddr]) -> std::io::Result<()> {
    let path = book_path(p2p_dir, verse_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(peers)?)?;
    std::fs::rename(temporary, path)
}

/// Merge `remembered` into `peers` (deduplicated by endpoint id, explicit entries first).
pub(crate) fn merge(peers: &mut Vec<iroh::EndpointAddr>, remembered: Vec<iroh::EndpointAddr>) {
    for addr in remembered {
        if !peers.iter().any(|known| known.id == addr.id) {
            peers.push(addr);
        }
    }
}

/// While the replica's roster stays live, remember the current addresses of
/// every peer it synced with, so a restart can dial them without relays or
/// `FE_SYNC_BOOTSTRAP`. Ends when the roster closes (replica closed/reopened).
pub(crate) fn spawn_recorder(
    endpoint: iroh::Endpoint,
    roster: crate::replicator::SceneProviderRoster,
    p2p_dir: PathBuf,
    verse_id: String,
) {
    tokio::spawn(async move {
        let mut remembered = load(&p2p_dir, &verse_id);
        loop {
            tokio::time::sleep(REFRESH).await;
            let Ok(synced) = roster.peers() else { return };
            let mut changed = false;
            for id in synced {
                let Some(info) = endpoint.remote_info(id).await else { continue };
                let addr = iroh::EndpointAddr::from_parts(id, info.into_addrs().map(|addr| addr.into_addr()));
                if addr.is_empty() {
                    continue;
                }
                let room = remembered.len() < MAX_PEERS;
                match remembered.iter_mut().find(|known| known.id == id) {
                    Some(known) if *known == addr => {}
                    Some(known) => {
                        *known = addr;
                        changed = true;
                    }
                    None if room => {
                        remembered.push(addr);
                        changed = true;
                    }
                    None => {}
                }
            }
            if changed {
                if let Err(error) = store(&p2p_dir, &verse_id, &remembered) {
                    tracing::warn!(verse_id, "peer book write failed: {error}");
                }
            }
        }
    });
}
