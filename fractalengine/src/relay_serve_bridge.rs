//! Desktop relay-capable mode glue (NET-30); see `fe-ui/src/AGENTS.md` §app-settings.

use std::collections::{BTreeMap, BTreeSet};

/// Hex namespace id derived from a hex namespace secret (same derivation as `fractalengine-relay/src/replicas.rs`).
#[cfg(feature = "relay-server")]
fn namespace_id_from_secret(secret_hex: &str) -> Option<String> {
    let bytes = hex::decode(secret_hex.trim()).ok()?;
    let secret: [u8; 32] = bytes.try_into().ok()?;
    Some(hex::encode(fe_database::derive_namespace_id(&secret)))
}

/// One pin per served verse that has a namespace id (row value, else derived from its stored secret).
#[cfg(feature = "relay-server")]
fn served_replica_pins(
    served: BTreeSet<String>,
    namespace_ids: &BTreeMap<String, Option<String>>,
    secret_store: &dyn fe_identity::SecretStore,
) -> Vec<fe_sync::relay_server::ServedReplica> {
    let mut pins = Vec::new();
    for verse_id in served {
        let secret = match fe_database::get_namespace_secret(secret_store, &verse_id) {
            Ok(secret) => secret,
            Err(error) => {
                tracing::warn!(verse_id, %error, "served verse namespace secret lookup failed");
                None
            }
        };
        let namespace_id = namespace_ids
            .get(&verse_id)
            .cloned()
            .flatten()
            .or_else(|| secret.as_deref().and_then(namespace_id_from_secret));
        let Some(namespace_id) = namespace_id else {
            tracing::warn!(
                verse_id,
                "served verse has no namespace id: its replica cannot open, so this relay refuses its members"
            );
            continue;
        };
        pins.push(fe_sync::relay_server::ServedReplica {
            verse_id,
            namespace_id,
            namespace_secret: secret,
        });
    }
    pins
}

/// Wakes the relay thread to re-run the served-set rule (a verse created or deleted); coalesced, never blocks a frame.
#[derive(bevy::prelude::Resource, Clone)]
pub struct ServedSetRefresh(crossbeam::channel::Sender<()>);

/// The refresh signal (a Bevy resource) and the receiver the desktop relay thread waits on.
pub fn served_set_refresh_channel() -> (ServedSetRefresh, crossbeam::channel::Receiver<()>) {
    let (sender, receiver) = crossbeam::channel::bounded(1);
    (ServedSetRefresh(sender), receiver)
}

/// Bevy system: a created or deleted verse asks the running relay to re-read its served set (RF-OPEN-4).
pub fn request_served_set_refresh(
    mut results: bevy::prelude::MessageReader<fe_runtime::messages::DbResult>,
    refresh: bevy::prelude::Res<ServedSetRefresh>,
) {
    use fe_runtime::messages::{DbResult, EntityType};
    let mut changed = false;
    for result in results.read() {
        changed |= matches!(
            result,
            DbResult::VerseCreated { .. } | DbResult::EntityDeleted { entity_type: EntityType::Verse, .. }
        );
    }
    // Full: a refresh is already pending. Disconnected: no relay is running.
    if changed && refresh.0.try_send(()).is_err() {
        tracing::trace!("served-set refresh already pending or no relay running");
    }
}

/// Wait before re-reading the owned verses after a failed read.
pub const OWNED_VERSES_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// Re-runs the served-set rule on every wake until `wakes` closes (RF-OPEN-4). A failed `read`
/// (`None`) keeps the set already applied and retries after `retry` with no new wake; an empty
/// set is applied only when the read succeeded (fe-sync `src/AGENTS.md` §peer-relay-server).
pub fn serve_refreshes(
    wakes: &crossbeam::channel::Receiver<()>,
    retry: std::time::Duration,
    mut read: impl FnMut() -> Option<BTreeMap<String, Option<String>>>,
    mut apply: impl FnMut(&BTreeMap<String, Option<String>>),
) {
    let mut retry_owed = false;
    loop {
        let woke = if retry_owed {
            !matches!(wakes.recv_timeout(retry), Err(crossbeam::channel::RecvTimeoutError::Disconnected))
        } else {
            wakes.recv().is_ok()
        };
        if !woke {
            return;
        }
        match read() {
            Some(owned) => {
                apply(&owned);
                retry_owed = false;
            }
            None => {
                tracing::warn!("desktop relay keeps its served verses: the owned-verse read failed, retrying");
                retry_owed = true;
            }
        }
    }
}

/// What the startup gate decided about serving a relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayServeDecision {
    /// Every condition holds: spawn the embedded relay.
    Start,
    /// The consent toggle is off; nothing happens.
    Inert,
    /// Toggle is on but the binary lacks the `relay-server` feature.
    RefusedNoFeature,
    /// Toggle is on but no passing reachability self-test is on record.
    RefusedSelfTest,
    /// Self-test passed but the signed dial-back did not confirm.
    RefusedDialback,
}

/// Pure startup gate: feature AND toggle AND `public` self-test AND passing dial-back.
pub fn decide(
    feature_built: bool,
    serve_relay: bool,
    self_test_verdict: Option<&str>,
    dialback_passed: bool,
) -> RelayServeDecision {
    if !serve_relay {
        return RelayServeDecision::Inert;
    }
    if !feature_built {
        return RelayServeDecision::RefusedNoFeature;
    }
    if self_test_verdict != Some("public") {
        return RelayServeDecision::RefusedSelfTest;
    }
    if !dialback_passed {
        return RelayServeDecision::RefusedDialback;
    }
    RelayServeDecision::Start
}

/// Keeps a running desktop relay alive for the process lifetime.
pub struct RelayServeHandle {
    #[cfg(feature = "relay-server")]
    _relay: fe_sync::relay_server::EmbeddedRelay,
    /// Served verses' replicas pinned open; released with the relay (sync-relay-1).
    #[cfg(feature = "relay-server")]
    _served_pins: Option<fe_sync::relay_server::ServedReplicaPins>,
}

impl RelayServeHandle {
    /// Binds the served set to `namespace_ids` (the verses this node owns) and pins their replicas open while this handle lives; call again whenever a verse is created or deleted (RF-OPEN-4). See `fe-sync/src/AGENTS.md` §peer-relay-server "served replicas".
    pub fn keep_served_replicas_open(
        &mut self,
        namespace_ids: &BTreeMap<String, Option<String>>,
        secret_store: &dyn fe_identity::SecretStore,
    ) {
        #[cfg(feature = "relay-server")]
        {
            // Gate first: a verse leaving the set stops admitting before its pin is released.
            let hosted: BTreeSet<String> = namespace_ids.keys().cloned().collect();
            let served = fe_sync::relay_server::rebind_hosted_verses(self._relay.gate(), hosted);
            let pins = served_replica_pins(served, namespace_ids, secret_store);
            if let Some(held) = &self._served_pins {
                held.replace(pins);
            } else {
                self._served_pins = Some(fe_sync::relay_server::keep_served_replicas_open(pins));
            }
        }
        #[cfg(not(feature = "relay-server"))]
        let _ = (namespace_ids, secret_store);
    }

    /// Verse ids the running relay admits members through.
    pub fn served_verse_ids(&self) -> BTreeSet<String> {
        #[cfg(feature = "relay-server")]
        {
            self._relay.gate().served_verse_ids()
        }
        #[cfg(not(feature = "relay-server"))]
        {
            BTreeSet::new()
        }
    }
}

/// Lists the local user's verses (id, name) for the relay consent step; call once at startup, off the UI thread.
pub async fn publish_servable_verses(db: &fe_database::repo::Db, local_did: String) {
    let read = async {
        let mut response = db
            .query("SELECT verse_id, name FROM verse WHERE created_by = $did")
            .bind(("did", local_did))
            .await?
            .check()?;
        anyhow::Ok(response.take::<Vec<serde_json::Value>>(0)?)
    };
    match read.await {
        Ok(rows) => fe_ui::settings::record_relay_servable_verses(
            rows.into_iter()
                .filter_map(|row| {
                    let verse_id = row.get("verse_id")?.as_str()?.to_string();
                    let name = row.get("name").and_then(|value| value.as_str()).unwrap_or_default().to_string();
                    Some((verse_id, name))
                })
                .collect(),
        ),
        Err(error) => tracing::warn!(%error, "could not list servable verses for the relay consent step"),
    }
}

/// Registers the startup-read toggles with fe-sync and the Settings UI; call once before the sync thread starts.
pub fn register_settings(settings: &fe_ui::settings::AppSettings) {
    fe_sync::relay_config::set_auto_home_relay(settings.auto_home_relay);
    fe_sync::dialback_responder::set_act_as_prober(settings.act_as_prober);
    fe_ui::settings::set_relay_serve_built(cfg!(feature = "relay-server"));
}

/// Spawns the relay only when [`decide`] says `Start`; call from a plain (non-async) thread once a self-test verdict and dial-back result exist.
pub fn maybe_start(
    runtime: &tokio::runtime::Handle,
    settings: &fe_ui::settings::AppSettings,
    self_test_verdict: Option<&str>,
    dialback_passed: bool,
    hosted_verses: BTreeSet<String>,
) -> Option<RelayServeHandle> {
    register_settings(settings);
    if self_test_verdict.is_some() {
        fe_ui::settings::record_self_test_verdict(self_test_verdict.map(str::to_owned));
    }

    let decision = decide(
        cfg!(feature = "relay-server"),
        settings.serve_relay,
        self_test_verdict,
        dialback_passed,
    );
    match decision {
        RelayServeDecision::Inert => None,
        RelayServeDecision::RefusedNoFeature => {
            tracing::warn!("serve_relay ignored: this build lacks the `relay-server` feature");
            None
        }
        RelayServeDecision::RefusedSelfTest => {
            tracing::warn!("serve_relay ignored: no passing reachability self-test");
            None
        }
        RelayServeDecision::RefusedDialback => {
            tracing::warn!("serve_relay ignored: dial-back did not confirm this device is reachable");
            None
        }
        RelayServeDecision::Start => start(runtime, hosted_verses),
    }
}

#[cfg(feature = "relay-server")]
fn start(runtime: &tokio::runtime::Handle, hosted_verses: BTreeSet<String>) -> Option<RelayServeHandle> {
    use fe_sync::relay_server::{install_gate, EmbeddedRelay, RelayMembershipGate};

    // `block_on` inside a runtime would panic, and this binary aborts on panic: refuse instead.
    if tokio::runtime::Handle::try_current().is_ok() {
        tracing::warn!("desktop relay not started: maybe_start was called from inside an async runtime");
        return None;
    }

    let config = match fe_sync::relay_server::RelayServeConfig::from_env_desktop() {
        Ok(Some(config)) => config,
        Ok(None) => {
            tracing::warn!("serve_relay on but the desktop relay env config is absent; not serving");
            return None;
        }
        Err(error) => {
            tracing::warn!(reason = error.name(), "desktop relay refused to start");
            return None;
        }
    };
    let config = match config.with_hosted_verses(hosted_verses).and_then(|config| config.validate().map(|()| config)) {
        Ok(config) => config,
        Err(error) => {
            tracing::warn!(reason = error.name(), "desktop relay config refused");
            return None;
        }
    };
    tracing::warn!("serving a members-only relay: members see this host and this host sees their traffic metadata");
    let gate = RelayMembershipGate::for_config(&config);
    if !install_gate(gate.clone()) {
        tracing::warn!("desktop relay not started: a membership gate is already installed");
        return None;
    }
    match runtime.block_on(EmbeddedRelay::spawn(config, gate)) {
        Ok(relay) => {
            let handle = RelayServeHandle { _relay: relay, _served_pins: None };
            let served = handle.served_verse_ids();
            tracing::info!(verses = served.len(), "desktop relay serving verses");
            tracing::debug!(ids = ?served, "desktop relay served verse ids");
            let known_names = fe_ui::settings::relay_servable_verses();
            let published = served
                .into_iter()
                .map(|id| {
                    let name = known_names.iter().find(|(known_id, _)| *known_id == id).map(|(_, name)| name.clone()).unwrap_or_default();
                    (id, name)
                })
                .collect();
            fe_ui::settings::record_relay_servable_verses(published);
            Some(handle)
        }
        Err(error) => {
            tracing::warn!(reason = error.name(), "desktop relay failed to bind");
            None
        }
    }
}

#[cfg(not(feature = "relay-server"))]
fn start(_runtime: &tokio::runtime::Handle, _hosted_verses: BTreeSet<String>) -> Option<RelayServeHandle> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_toggle_is_inert_without_feature_and_refuses_without_passing_self_test() {
        use RelayServeDecision::*;
        // (feature, toggle, verdict, dialback, expected)
        let rows = [
            (true, false, Some("public"), true, Inert),
            (false, false, None, false, Inert),
            (false, true, Some("public"), true, RefusedNoFeature),
            (true, true, None, true, RefusedSelfTest),
            (true, true, Some("behind_nat"), true, RefusedSelfTest),
            (true, true, Some("public"), false, RefusedDialback),
            (true, true, Some("public"), true, Start),
        ];
        for (feature, toggle, verdict, dialback, expected) in rows {
            assert_eq!(decide(feature, toggle, verdict, dialback), expected, "{feature} {toggle} {verdict:?} {dialback}");
        }

        // RF-OPEN-4: creating or deleting a verse wakes the running relay to re-read its served set; other results do not.
        use fe_runtime::messages::{DbResult, EntityType};
        let (refresh, refresh_requests) = served_set_refresh_channel();
        let mut app = bevy::prelude::App::new();
        app.add_message::<DbResult>()
            .insert_resource(refresh)
            .add_systems(bevy::prelude::Update, request_served_set_refresh);
        let mut deliver = |result: DbResult| {
            app.world_mut().write_message(result);
            app.update();
            refresh_requests.try_recv().is_ok()
        };
        let created = DbResult::VerseCreated { id: "verse-new".into(), name: "New".into(), namespace_id: Some("ns".into()) };
        assert!(deliver(created), "a created verse re-runs the served-set rule");
        let fractal_deleted = DbResult::EntityDeleted { entity_type: EntityType::Fractal, entity_id: "fractal-1".into() };
        assert!(!deliver(fractal_deleted), "a fractal delete leaves the served set alone");
        let verse_deleted = DbResult::EntityDeleted { entity_type: EntityType::Verse, entity_id: "verse-new".into() };
        assert!(deliver(verse_deleted), "a deleted verse re-runs the rule, which releases it");

        // A failed owned-verse read keeps the served set (no empty set is applied) and is retried
        // without a new wake; a successful read, even an empty one, is applied.
        let owned = |ids: &[&str]| -> BTreeMap<String, Option<String>> {
            ids.iter().map(|id| (id.to_string(), Some(format!("ns-{id}")))).collect()
        };
        let mut reads = std::collections::VecDeque::from([Some(owned(&["verse-a"])), None, Some(owned(&[]))]);
        let applied = std::sync::Mutex::new(Vec::new());
        let (wake, wakes) = crossbeam::channel::unbounded();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                serve_refreshes(
                    &wakes,
                    std::time::Duration::from_millis(10),
                    || reads.pop_front().flatten(),
                    |set| applied.lock().unwrap().push(set.clone()),
                )
            });
            wake.send(()).unwrap();
            wake.send(()).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while applied.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            drop(wake);
        });
        assert_eq!(
            applied.into_inner().unwrap(),
            vec![owned(&["verse-a"]), owned(&[])],
            "the failed read applied nothing; its retry applied the real (empty) set"
        );
    }
}
