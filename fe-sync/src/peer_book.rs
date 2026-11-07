//! Per-verse persisted LRU address book of peers this node last synced with
//! (QA-0086, QA-0117); see fe-sync `src/AGENTS.md` §peer-book.

use std::path::{Path, PathBuf};

/// At most this many peers are remembered per verse — also the scene-provider cap.
pub(crate) const MAX_PEERS: usize = 16;

/// How often a live replica refreshes its remembered peers.
const REFRESH: std::time::Duration = std::time::Duration::from_secs(10);

/// Entries not synced for this long are dropped on load.
const STALE_AFTER_SECS: u64 = 7 * 24 * 60 * 60;

/// An unchanged address re-persists its `last_seen` at most this often.
const TOUCH_PERSIST_SECS: u64 = 60 * 60;

/// One remembered peer and when this node last synced with it (Unix seconds).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct BookEntry {
    pub(crate) addr: iroh::EndpointAddr,
    pub(crate) last_seen: u64,
}

/// The on-disk book: current entries, or the pre-QA-0117 bare address list.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum StoredBook {
    Entries(Vec<BookEntry>),
    Legacy(Vec<iroh::EndpointAddr>),
}

fn book_path(p2p_dir: &Path, verse_id: &str) -> PathBuf {
    p2p_dir
        .join("peer-book")
        .join(format!("{}.json", hex::encode(blake3::hash(verse_id.as_bytes()).as_bytes())))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

/// The fresh remembered entries for `verse_id`, most recently synced first;
/// a legacy book counts as seen `now`. Empty when none or unreadable.
pub(crate) fn load_entries(p2p_dir: &Path, verse_id: &str, now: u64) -> Vec<BookEntry> {
    let Ok(bytes) = std::fs::read(book_path(p2p_dir, verse_id)) else {
        return Vec::new();
    };
    let mut entries = match serde_json::from_slice::<StoredBook>(&bytes) {
        Ok(StoredBook::Entries(entries)) => entries,
        Ok(StoredBook::Legacy(addrs)) => addrs
            .into_iter()
            .map(|addr| BookEntry {
                addr,
                last_seen: now,
            })
            .collect(),
        Err(error) => {
            tracing::warn!(verse_id, "peer book unreadable — ignored: {error}");
            Vec::new()
        }
    };
    entries.retain(|entry| now.saturating_sub(entry.last_seen) <= STALE_AFTER_SECS);
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_seen));
    entries.truncate(MAX_PEERS);
    entries
}

/// The remembered dialable addresses for `verse_id`, most recently synced first.
pub(crate) fn load(p2p_dir: &Path, verse_id: &str) -> Vec<iroh::EndpointAddr> {
    load_entries(p2p_dir, verse_id, now_secs())
        .into_iter()
        .map(|entry| entry.addr)
        .collect()
}

pub(crate) fn store(
    p2p_dir: &Path,
    verse_id: &str,
    entries: &[BookEntry],
) -> std::io::Result<()> {
    let path = book_path(p2p_dir, verse_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec(entries)?)?;
    std::fs::rename(temporary, path)
}

/// Merge `remembered` into `peers` (deduplicated by endpoint id). Explicit
/// entries always win and the total never exceeds [`MAX_PEERS`], the limit the
/// scene-asset layer enforces on open (QA-0117).
pub(crate) fn merge(peers: &mut Vec<iroh::EndpointAddr>, remembered: Vec<iroh::EndpointAddr>) {
    for addr in remembered {
        if peers.len() >= MAX_PEERS {
            break;
        }
        if !peers.iter().any(|known| known.id == addr.id) {
            peers.push(addr);
        }
    }
}

/// Record one sync-confirmed peer address (LRU, QA-0117): a full book evicts
/// its least recently synced entry that is not in `synced`. Returns whether
/// the book should be persisted.
pub(crate) fn record(
    book: &mut Vec<BookEntry>,
    addr: iroh::EndpointAddr,
    synced: &[iroh::EndpointId],
    now: u64,
) -> bool {
    if let Some(known) = book.iter_mut().find(|known| known.addr.id == addr.id) {
        let due = known.addr != addr || now.saturating_sub(known.last_seen) >= TOUCH_PERSIST_SECS;
        if due {
            known.addr = addr;
            known.last_seen = now;
        }
        return due;
    }
    if book.len() >= MAX_PEERS {
        let Some(oldest) = book
            .iter()
            .enumerate()
            .filter(|(_, entry)| !synced.contains(&entry.addr.id))
            .min_by_key(|(_, entry)| entry.last_seen)
            .map(|(index, _)| index)
        else {
            return false;
        };
        book.remove(oldest);
    }
    book.push(BookEntry {
        addr,
        last_seen: now,
    });
    true
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
        let mut book = load_entries(&p2p_dir, &verse_id, now_secs());
        loop {
            tokio::time::sleep(REFRESH).await;
            // Sync-confirmed peers only (successful SyncFinished), never forwarded authors.
            let Ok(synced) = roster.peers() else { return };
            let now = now_secs();
            let mut changed = false;
            for &id in &synced {
                let Some(info) = endpoint.remote_info(id).await else { continue };
                let addr = iroh::EndpointAddr::from_parts(id, info.into_addrs().map(|addr| addr.into_addr()));
                if addr.is_empty() {
                    continue;
                }
                changed |= record(&mut book, addr, &synced, now);
            }
            if changed {
                if let Err(error) = store(&p2p_dir, &verse_id, &book) {
                    tracing::warn!(verse_id, "peer book write failed: {error}");
                }
            }
        }
    });
}
