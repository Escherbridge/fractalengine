//! API end of every correlated API→Bevy bridge (screenshot, vision, input):
//! mint → `try_send` → bounded wait → check the echo — see
//! fe-runtime/src/AGENTS.md §correlated-bridge.

use std::time::Duration;

use fe_runtime::bridge::{BridgeRequest, BridgeSender};

/// Why a round trip produced no answer; each surface maps these to its own
/// typed error (and text) — the transport outcomes themselves are shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundTripError {
    /// The bounded channel is full — a busy refusal, never a blocked worker.
    Saturated,
    /// The Bevy host dropped its receiver (the render host stopped).
    HostGone,
    /// No answer within the surface's reply budget.
    TimedOut(Duration),
    /// The answer echoed a foreign correlation id.
    Uncorrelated,
    /// The host dropped the request without answering.
    Dropped,
}

/// One correlated round trip over `bridge` (Request-Reply with a Correlation
/// Identifier), carrying the caller's token scope for the host's
/// authorization. `bridge_name` only labels the saturation/host-gone WARNs.
pub async fn round_trip<Op, Output>(
    bridge: &BridgeSender<Op, Output>,
    caller_scope: &str,
    op: Op,
    timeout: Duration,
    bridge_name: &'static str,
) -> Result<Output, RoundTripError> {
    let correlation_id = ulid::Ulid::new().to_string();
    let (request, reply_rx) = BridgeRequest::new(correlation_id.clone(), caller_scope, op);
    match bridge.try_send(request) {
        Ok(()) => {}
        Err(crossbeam::channel::TrySendError::Full(_)) => {
            tracing::warn!("{bridge_name} bridge saturated — refusing");
            return Err(RoundTripError::Saturated);
        }
        Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
            tracing::warn!("{bridge_name} bridge closed (Bevy host gone)");
            return Err(RoundTripError::HostGone);
        }
    }
    match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(reply)) if reply.correlation_id == correlation_id => Ok(reply.result),
        Ok(Ok(_)) => Err(RoundTripError::Uncorrelated),
        Ok(Err(_)) => Err(RoundTripError::Dropped),
        Err(_) => Err(RoundTripError::TimedOut(timeout)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fe_runtime::bridge::bounded_bridge;

    /// Every transport outcome a surface must map: answered, foreign echo,
    /// dropped, silent host, full channel, host gone.
    #[tokio::test]
    async fn round_trip_outcomes_table() {
        let timeout = Duration::from_millis(50);

        let (tx, rx) = bounded_bridge::<u8, u8>(1);
        let host = std::thread::spawn(move || {
            let request = rx.recv().expect("one request");
            let op = request.op;
            request.reply(op + 1);
        });
        assert_eq!(round_trip(&tx, "VERSE#v", 1, Duration::from_secs(5), "test").await, Ok(2));
        host.join().expect("host thread");

        let (tx, rx) = bounded_bridge::<u8, u8>(1);
        let host = std::thread::spawn(move || {
            let mut request = rx.recv().expect("one request");
            request.correlation_id = "someone-else".into();
            request.reply(0);
        });
        assert_eq!(
            round_trip(&tx, "VERSE#v", 1, Duration::from_secs(5), "test").await,
            Err(RoundTripError::Uncorrelated)
        );
        host.join().expect("host thread");

        let (tx, rx) = bounded_bridge::<u8, u8>(1);
        let host = std::thread::spawn(move || drop(rx.recv().expect("one request")));
        assert_eq!(
            round_trip(&tx, "VERSE#v", 1, Duration::from_secs(5), "test").await,
            Err(RoundTripError::Dropped)
        );
        host.join().expect("host thread");

        let (tx, _rx) = bounded_bridge::<u8, u8>(1);
        assert_eq!(
            round_trip(&tx, "VERSE#v", 1, timeout, "test").await,
            Err(RoundTripError::TimedOut(timeout))
        );
        // The timed-out request still occupies the only slot.
        assert_eq!(round_trip(&tx, "VERSE#v", 1, timeout, "test").await, Err(RoundTripError::Saturated));

        let (tx, rx) = bounded_bridge::<u8, u8>(1);
        drop(rx);
        assert_eq!(round_trip(&tx, "VERSE#v", 1, timeout, "test").await, Err(RoundTripError::HostGone));
    }
}
