//! Relay replica lifecycle (F4 / A7).
//!
//! The relay replicates every verse it hosts: at startup it asks the DB thread
//! for the verse hierarchy and opens one replica each (namespace secret looked
//! up from the relay's secret store), and it opens a replica whenever a verse
//! is created at runtime. Opening a replica also runs fe-sync's startup
//! reconciliation pass (doc snapshot → the same inbound apply path) so rows
//! that were denied while a role or the verse manifest had not yet converged
//! get a second chance to converge.
//!
//! The GUI binary opens replicas from navigation instead (fe-ui
//! `navigation_manager::open_replica`); both paths ride the same sync-thread
//! seam, so this module mirrors that wiring for the headless relay.

use fe_database::SecretStoreRes;
use fe_runtime::messages::{DbCommand, DbResult};
use fe_sync::SyncCommand;

/// Resolve a verse's namespace id + secret and send `OpenVerseReplica`.
///
/// `namespace_id` may be supplied directly (the startup hierarchy scan has
/// it); when absent it is derived from the stored namespace secret (the
/// `VerseCreated` path — the row and the secret are written together by the
/// DB handler, so deriving avoids a DB round-trip inside the Bevy system).
///
/// Returns `true` when a replica open command was actually sent.
pub fn open_replica(
    sync_tx: &crossbeam::channel::Sender<SyncCommand>,
    secret_store: &dyn fe_identity::SecretStore,
    verse_id: &str,
    namespace_id: Option<String>,
) -> bool {
    let secret = match fe_database::get_namespace_secret(secret_store, verse_id) {
        Ok(secret) => secret,
        Err(e) => {
            tracing::warn!(verse_id, "namespace secret lookup failed: {e}");
            None
        }
    };
    let ns_id = namespace_id.or_else(|| secret.as_deref().and_then(namespace_id_from_secret));
    let Some(ns_id) = ns_id else {
        tracing::warn!(
            verse_id,
            "no namespace_id available (row has none and no secret stored) — replica not opened"
        );
        return false;
    };
    match sync_tx.send(SyncCommand::OpenVerseReplica {
        verse_id: verse_id.to_string(),
        namespace_id: ns_id,
        namespace_secret: secret,
        // Env `FE_SYNC_BOOTSTRAP` peers are merged inside the sync thread
        // (`sync_thread.rs::bootstrap_peers_from_env`), so the relay sends none
        // explicitly — the same behaviour as the GUI.
        bootstrap_peers: Vec::new(),
    }) {
        Ok(()) => {
            tracing::info!(verse_id, "Relay opened verse replica");
            true
        }
        Err(e) => {
            tracing::warn!(
                verse_id,
                "sync command channel closed — replica not opened: {e}"
            );
            false
        }
    }
}

/// Derive the hex namespace id from a hex namespace secret (the DB handler's
/// own derivation: `blake3::keyed_hash` over the 32-byte secret).
fn namespace_id_from_secret(secret_hex: &str) -> Option<String> {
    let bytes = hex::decode(secret_hex.trim()).ok()?;
    let arr: [u8; 32] = bytes.try_into().ok()?;
    Some(hex::encode(fe_database::derive_namespace_id(&arr)))
}

/// Bevy system: open a replica whenever the DB thread reports a new verse.
///
/// The relay pumps `DbResult`s into Bevy `Messages` via
/// `fe_runtime::app::setup_core_systems`; this reads the same stream the GUI's
/// `VerseManagerPlugin` consumes (no fe-ui in the relay).
pub fn open_replica_on_verse_created(
    mut results: bevy::prelude::MessageReader<DbResult>,
    sync: bevy::prelude::Res<fe_sync::SyncCommandSenderRes>,
    secret_store: bevy::prelude::Res<SecretStoreRes>,
) {
    for result in results.read() {
        if let DbResult::VerseCreated { id, name } = result {
            tracing::info!(verse_id = %id, name = %name, "VerseCreated — opening replica");
            open_replica(&sync.0, secret_store.0.as_ref(), id, None);
        }
    }
}

/// Per-system state for [`startup_replica_scan`]: whether the one-time
/// hierarchy request has been sent, and which verses this process has already
/// opened (so later `HierarchyLoaded` replies — e.g. an API
/// `GET /api/v1/hierarchy` channel round-trip — never churn a live replica).
#[derive(Default)]
pub(crate) struct StartupScanState {
    hierarchy_requested: bool,
    opened: std::collections::HashSet<String>,
}

/// Bevy system (A7): at startup, open a replica for every verse the relay
/// hosts.
///
/// Why a `DbCommand::LoadHierarchy` round-trip instead of a direct SurrealKV
/// read: SurrealKV takes a per-handle file lock, so a second connection in
/// this process cannot be opened while the DB thread's writer connection is
/// alive (Windows os error 33 — the relay's `api_db_reader` legitimately
/// falls back to `None` there; see fe-database's `examples` note that the app
/// must not be running while they inspect the store). The channel round-trip
/// rides the DB thread's single writer instead and works with or without a
/// direct reader, and `VerseHierarchyData` carries each verse's
/// `namespace_id`. Each replica open also runs fe-sync's startup
/// reconciliation pass inside the sync thread.
pub fn startup_replica_scan(
    mut state: bevy::prelude::Local<StartupScanState>,
    db_tx: bevy::prelude::Res<fe_runtime::app::DbCommandSender>,
    mut results: bevy::prelude::MessageReader<DbResult>,
    sync: bevy::prelude::Res<fe_sync::SyncCommandSenderRes>,
    secret_store: bevy::prelude::Res<SecretStoreRes>,
) {
    if !state.hierarchy_requested {
        state.hierarchy_requested = true;
        if let Err(e) = db_tx.0.send(DbCommand::LoadHierarchy) {
            tracing::warn!("startup verse replica scan: DB command channel closed: {e}");
            return;
        }
        tracing::info!("Startup verse replica scan: requested hierarchy from DB thread");
    }
    for result in results.read() {
        let DbResult::HierarchyLoaded { verses } = result else {
            continue;
        };
        let mut opened_now = 0usize;
        for verse in verses {
            if !state.opened.insert(verse.id.clone()) {
                continue; // already opened by this process
            }
            if open_replica(
                &sync.0,
                secret_store.0.as_ref(),
                &verse.id,
                verse.namespace_id.clone(),
            ) {
                opened_now += 1;
            }
        }
        tracing::info!(
            verses = verses.len(),
            opened = opened_now,
            "Startup replica scan pass complete"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn namespace_id_from_secret_matches_db_derivation() {
        let secret = [7u8; 32];
        let hex_secret = hex::encode(secret);
        assert_eq!(
            namespace_id_from_secret(&hex_secret),
            Some(hex::encode(fe_database::derive_namespace_id(&secret)))
        );
    }

    #[test]
    fn namespace_id_from_secret_rejects_bad_hex() {
        assert_eq!(namespace_id_from_secret("not-hex"), None);
        assert_eq!(namespace_id_from_secret("abcd"), None);
    }

    /// A7 (`VerseCreated`): the real Bevy system turns a `DbResult::VerseCreated`
    /// into an `OpenVerseReplica` carrying the namespace id derived from the
    /// stored secret, so the relay replicates verses created at runtime.
    #[test]
    fn verse_created_opens_replica_with_derived_namespace() {
        use bevy::prelude::*;
        use fe_identity::{InMemoryBackend, SecretStore};

        let verse_id = "01TESTVERSE0000000000000000";
        let secret = [9u8; 32];
        let hex_secret = hex::encode(secret);

        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        store
            .set(
                &format!("fractalengine:verse:{verse_id}:ns_secret"),
                "fractalengine",
                &hex_secret,
            )
            .unwrap();

        let (sync_tx, sync_rx) = crossbeam::channel::bounded(4);
        let mut app = App::new();
        app.add_message::<DbResult>();
        app.insert_resource(fe_sync::SyncCommandSenderRes(sync_tx));
        app.insert_resource(SecretStoreRes(store));
        app.add_systems(Update, open_replica_on_verse_created);

        app.world_mut().write_message(DbResult::VerseCreated {
            id: verse_id.to_string(),
            name: "Runtime Verse".to_string(),
        });
        app.update();

        match sync_rx.try_recv().expect("OpenVerseReplica expected") {
            SyncCommand::OpenVerseReplica {
                verse_id: v,
                namespace_id,
                namespace_secret,
                ..
            } => {
                assert_eq!(v, verse_id);
                assert_eq!(
                    namespace_id,
                    hex::encode(fe_database::derive_namespace_id(&secret))
                );
                assert_eq!(namespace_secret, Some(hex_secret));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    /// A7 (startup scan): the system asks the DB thread for the hierarchy
    /// once, opens a replica per verse on `HierarchyLoaded` (namespace derived
    /// from the stored secret when the row has none), and never re-opens on a
    /// later `HierarchyLoaded` (an API `GET /api/v1/hierarchy` round-trip).
    #[test]
    fn startup_scan_opens_replicas_from_hierarchy_loaded() {
        use bevy::prelude::*;
        use fe_identity::{InMemoryBackend, SecretStore};
        use fe_runtime::messages::{DbCommand, DbResult, VerseHierarchyData};

        let verse_id = "01STARTUPSCANVERSE0000000";
        let other_id = "01STARTUPSCANVERSENOSEC000";
        let secret = [11u8; 32];
        let hex_secret = hex::encode(secret);

        let store: Arc<dyn SecretStore> = Arc::new(InMemoryBackend::new());
        store
            .set(
                &format!("fractalengine:verse:{verse_id}:ns_secret"),
                "fractalengine",
                &hex_secret,
            )
            .unwrap();

        let (sync_tx, sync_rx) = crossbeam::channel::bounded(8);
        let (db_tx, db_rx) = crossbeam::channel::bounded(8);
        let mut app = App::new();
        app.add_message::<DbResult>();
        app.insert_resource(fe_sync::SyncCommandSenderRes(sync_tx));
        app.insert_resource(SecretStoreRes(store));
        app.insert_resource(fe_runtime::app::DbCommandSender(db_tx));
        app.add_systems(Update, startup_replica_scan);

        // Frame 1: exactly one LoadHierarchy request.
        app.update();
        match db_rx.try_recv().unwrap() {
            DbCommand::LoadHierarchy => {}
            other => panic!("expected LoadHierarchy, got {other:?}"),
        }
        assert!(db_rx.try_recv().is_err(), "hierarchy requested only once");
        assert!(sync_rx.try_recv().is_err(), "no replica before the reply");

        // Frame 2: the reply opens one replica per verse. `other_id` has no
        // secret and no row namespace_id — it must be skipped loudly.
        app.world_mut().write_message(DbResult::HierarchyLoaded {
            verses: vec![
                VerseHierarchyData {
                    id: verse_id.to_string(),
                    name: "Startup Verse".into(),
                    namespace_id: None,
                    fractals: Vec::new(),
                },
                VerseHierarchyData {
                    id: other_id.to_string(),
                    name: "No Secret Verse".into(),
                    namespace_id: None,
                    fractals: Vec::new(),
                },
            ],
        });
        app.update();

        let opened = sync_rx.try_recv().expect("OpenVerseReplica expected");
        match opened {
            SyncCommand::OpenVerseReplica {
                verse_id: v,
                namespace_id,
                namespace_secret,
                ..
            } => {
                assert_eq!(v, verse_id);
                assert_eq!(
                    namespace_id,
                    hex::encode(fe_database::derive_namespace_id(&secret))
                );
                assert_eq!(namespace_secret, Some(hex_secret));
            }
            other => panic!("unexpected command: {other:?}"),
        }
        assert!(
            sync_rx.try_recv().is_err(),
            "verse without a secret must not open a replica"
        );

        // Frame 3: a later HierarchyLoaded (an API hierarchy round-trip) must
        // not churn the already-open replica.
        app.world_mut().write_message(DbResult::HierarchyLoaded {
            verses: vec![VerseHierarchyData {
                id: verse_id.to_string(),
                name: "Startup Verse".into(),
                namespace_id: None,
                fractals: Vec::new(),
            }],
        });
        app.update();
        assert!(
            sync_rx.try_recv().is_err(),
            "already-open verses are not re-opened"
        );
    }
}
