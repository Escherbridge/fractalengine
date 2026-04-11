//! Command and event types for the sync thread (P2P Mycelium Phase D).
//!
//! The main thread sends [`SyncCommand`]s to the sync thread over a crossbeam
//! channel; the sync thread sends [`SyncEvent`]s back.

use fe_runtime::blob_store::BlobHash;

/// Commands sent **to** the sync thread.
#[derive(Debug, Clone)]
pub enum SyncCommand {
    /// Request a blob from the network for a given verse.
    ///
    /// The sync thread will first check the local blob store; if present it
    /// immediately emits [`SyncEvent::BlobReady`].  Otherwise it attempts a
    /// peer fetch (stub in Phase D — actual discovery comes in Phase F).
    FetchBlob { hash: BlobHash, verse_id: String },
    /// Open (or join) an iroh-docs replica for a verse.
    ///
    /// Phase E: creates an `IrohDocsReplicator` (stub) and inserts it into the
    /// sync thread's replica map. The `namespace_secret` is `Some` if the
    /// local node is the owner; `None` for read-only join.
    OpenVerseReplica {
        verse_id: String,
        namespace_id: String,
        namespace_secret: Option<String>,
    },
    /// Close a previously-opened verse replica.
    CloseVerseReplica { verse_id: String },
    /// Write a row entry to the verse's replica.
    ///
    /// The `content_hash` references a blob in the shared blob store containing
    /// the serialised row JSON.
    WriteRowEntry {
        verse_id: String,
        table: String,
        record_id: String,
        content_hash: BlobHash,
    },
    /// Gracefully shut down the sync thread.
    Shutdown,
}

/// Events emitted **from** the sync thread.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    /// The sync thread has started.
    ///
    /// `online == true` means the iroh endpoint bound successfully and the
    /// node is reachable (at least via relay).  `false` means the thread is
    /// running in offline/local-only mode.
    Started { online: bool },
    /// A blob is now available in the local blob store.
    BlobReady { hash: BlobHash },
    /// The sync thread has shut down.
    Stopped,
}

/// Sender half for sync commands (type alias for ergonomics).
pub type SyncCommandSender = crossbeam::channel::Sender<SyncCommand>;
/// Receiver half for sync commands.
pub type SyncCommandReceiver = crossbeam::channel::Receiver<SyncCommand>;
/// Sender half for sync events.
pub type SyncEventSender = crossbeam::channel::Sender<SyncEvent>;
/// Receiver half for sync events.
pub type SyncEventReceiver = crossbeam::channel::Receiver<SyncEvent>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_command_debug_clone() {
        let cmd = SyncCommand::FetchBlob {
            hash: [0u8; 32],
            verse_id: "test-verse".into(),
        };
        let _ = format!("{:?}", cmd.clone());
        let _ = format!("{:?}", SyncCommand::Shutdown.clone());

        // Phase E variants
        let _ = format!(
            "{:?}",
            SyncCommand::OpenVerseReplica {
                verse_id: "v1".into(),
                namespace_id: "ns1".into(),
                namespace_secret: Some("secret".into()),
            }
            .clone()
        );
        let _ = format!(
            "{:?}",
            SyncCommand::CloseVerseReplica {
                verse_id: "v1".into(),
            }
            .clone()
        );
        let _ = format!(
            "{:?}",
            SyncCommand::WriteRowEntry {
                verse_id: "v1".into(),
                table: "verse".into(),
                record_id: "r1".into(),
                content_hash: [1u8; 32],
            }
            .clone()
        );
    }

    #[test]
    fn sync_event_debug_clone() {
        let _ = format!("{:?}", SyncEvent::Started { online: true }.clone());
        let _ = format!("{:?}", SyncEvent::BlobReady { hash: [1u8; 32] }.clone());
        let _ = format!("{:?}", SyncEvent::Stopped.clone());
    }

    #[test]
    fn channel_roundtrip() {
        let (tx, rx) = crossbeam::channel::bounded(1);
        tx.send(SyncCommand::Shutdown).unwrap();
        assert!(matches!(rx.recv().unwrap(), SyncCommand::Shutdown));
    }
}
