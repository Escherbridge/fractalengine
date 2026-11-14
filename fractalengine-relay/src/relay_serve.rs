//! Opt-in embedded members-only relay for fe-relay (P7, AC-1/AC-2); see `fractalengine-relay/AGENTS.md` §embedded-relay.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Relay listeners this process has bound and not yet shut down.
static BOUND_RELAY_LISTENERS: AtomicUsize = AtomicUsize::new(0);

/// Number of embedded-relay listeners currently bound by this process.
pub fn bound_relay_listener_count() -> usize {
    BOUND_RELAY_LISTENERS.load(Ordering::Acquire)
}

/// Keeps a running embedded relay alive; dropping it releases the listener count.
#[cfg(feature = "relay-server")]
pub struct EmbeddedRelayHandle {
    relay: Option<fe_sync::relay_server::EmbeddedRelay>,
    runtime: tokio::runtime::Handle,
}

/// Placeholder type so callers compile without the `relay-server` feature; never constructed.
#[cfg(not(feature = "relay-server"))]
pub struct EmbeddedRelayHandle {
    _never: std::convert::Infallible,
}

#[cfg(feature = "relay-server")]
impl EmbeddedRelayHandle {
    /// Gracefully stops the relay; call from a non-async thread.
    pub fn shutdown(mut self) {
        if let Some(relay) = self.relay.take() {
            self.runtime.block_on(relay.shutdown());
            // `Drop` sees `None` after the take, so the listener is released here.
            BOUND_RELAY_LISTENERS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(feature = "relay-server")]
impl Drop for EmbeddedRelayHandle {
    fn drop(&mut self) {
        if self.relay.take().is_some() {
            BOUND_RELAY_LISTENERS.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Starts the embedded relay only when `FE_RELAY_SERVE=1` and the `relay-server` feature is built; call from a non-async thread BEFORE `spawn_sync_thread`.
#[cfg(feature = "relay-server")]
pub fn maybe_spawn_embedded_relay(
    rt_handle: &tokio::runtime::Handle,
    hosted_verses: BTreeSet<String>,
) -> anyhow::Result<Option<EmbeddedRelayHandle>> {
    let config = fe_sync::relay_server::RelayServeConfig::from_env();
    spawn_from_config(rt_handle, config, hosted_verses)
}

/// Without the `relay-server` feature nothing is ever bound; a set opt-in only warns.
#[cfg(not(feature = "relay-server"))]
pub fn maybe_spawn_embedded_relay(
    _rt_handle: &tokio::runtime::Handle,
    _hosted_verses: BTreeSet<String>,
) -> anyhow::Result<Option<EmbeddedRelayHandle>> {
    if std::env::var("FE_RELAY_SERVE").as_deref() == Ok("1") {
        tracing::warn!(
            "FE_RELAY_SERVE=1 ignored: this fe-relay build lacks the `relay-server` feature; no relay listener is bound"
        );
    }
    Ok(None)
}

/// Config result to running relay: absent opt-in is `None`, a refused config is an error naming the reason (AC-2).
#[cfg(feature = "relay-server")]
fn spawn_from_config(
    rt_handle: &tokio::runtime::Handle,
    config: Result<Option<fe_sync::relay_server::RelayServeConfig>, fe_sync::relay_server::RelayServeError>,
    hosted_verses: BTreeSet<String>,
) -> anyhow::Result<Option<EmbeddedRelayHandle>> {
    use fe_sync::relay_server::{install_gate, EmbeddedRelay, RelayMembershipGate};

    let refused = |error: fe_sync::relay_server::RelayServeError| {
        anyhow::anyhow!("embedded relay refused to start: {} ({error:?})", error.name())
    };
    let Some(config) = config.map_err(refused)? else {
        return Ok(None);
    };
    let config = config.with_hosted_verses(hosted_verses).map_err(refused)?;
    config.validate().map_err(refused)?;

    tracing::warn!(
        "FE_RELAY_SERVE=1: this node serves a members-only relay; members see this host, \
         and this host sees relayed members' addresses and traffic metadata"
    );
    // Gate must be installed before the sync thread starts so it never misses a snapshot.
    let gate = RelayMembershipGate::for_config(&config);
    // A second gate would never receive snapshots and would deny every member: refuse instead.
    if !install_gate(gate.clone()) {
        anyhow::bail!("embedded relay refused to start: a relay membership gate is already installed in this process");
    }
    let relay = rt_handle
        .block_on(EmbeddedRelay::spawn(config, gate))
        .map_err(refused)?;
    BOUND_RELAY_LISTENERS.fetch_add(1, Ordering::AcqRel);
    Ok(Some(EmbeddedRelayHandle {
        relay: Some(relay),
        runtime: rt_handle.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_thread_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
    }

    #[test]
    fn relay_binary_binds_no_relay_listener_without_opt_in() {
        let runtime = current_thread_runtime();
        let handle = maybe_spawn_embedded_relay(runtime.handle(), BTreeSet::new())
            .expect("absent opt-in is not an error");
        assert!(handle.is_none(), "no relay handle without opt-in");
        assert_eq!(bound_relay_listener_count(), 0, "no relay listener bound");
    }

    #[cfg(feature = "relay-server")]
    #[test]
    fn refused_configs_exit_with_named_reason_and_bind_nothing() {
        use fe_sync::relay_server::{RelayServeConfig, RelayServeProfile};

        let runtime = current_thread_runtime();
        let cases: [(&[(&str, &str)], &str); 3] = [
            (&[("FE_RELAY_SERVE", "1")], "no_cert_mode"),
            (&[("FE_RELAY_SERVE", "1"), ("FE_RELAY_SERVE_CERT_MODE", "bogus")], "unknown_cert_mode"),
            (&[("FE_RELAY_SERVE", "1"), ("FE_RELAY_SERVE_ACCESS", "open")], "open_access_refused"),
        ];
        for (variables, expected_name) in cases {
            let lookup = |key: &str| {
                variables
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| value.to_string())
            };
            let config = RelayServeConfig::from_lookup(RelayServeProfile::Operator, &lookup);
            let error = spawn_from_config(runtime.handle(), config, BTreeSet::new())
                .err()
                .expect("refused config must be an error");
            assert!(
                error.to_string().contains(expected_name),
                "error {error} must name {expected_name}"
            );
        }
        assert_eq!(bound_relay_listener_count(), 0, "refused configs bind nothing");
    }
}
