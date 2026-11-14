//! Desktop relay-capable mode glue (NET-30); see `fe-ui/src/AGENTS.md` §app-settings.

use std::collections::BTreeSet;

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
        Ok(relay) => Some(RelayServeHandle { _relay: relay }),
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
    }
}
