//! Correlated request/reply bridge — the ONE API→Bevy seam shape shared by the
//! screenshot, vision (capture + camera read-back) and input surfaces. See
//! src/AGENTS.md §correlated-bridge.

/// One request: the API thread's correlation id, the caller's token scope
/// (the host authorizes against it — agent_vision SEC-ADDENDUM), the
/// operation, and its private reply slot (Request-Reply with a Correlation
/// Identifier).
#[derive(Debug)]
pub struct BridgeRequest<Op, Output> {
    pub correlation_id: String,
    pub caller_scope: String,
    pub op: Op,
    pub reply_tx: tokio::sync::oneshot::Sender<BridgeReply<Output>>,
}

/// The host's answer, echoing the request's correlation id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeReply<Output> {
    pub correlation_id: String,
    pub result: Output,
}

pub type BridgeSender<Op, Output> = crossbeam::channel::Sender<BridgeRequest<Op, Output>>;
pub type BridgeReceiver<Op, Output> = crossbeam::channel::Receiver<BridgeRequest<Op, Output>>;

/// A fresh bounded request channel (API holds the sender, the Bevy host the
/// receiver); a full channel is a busy refusal on the API side, never a block.
pub fn bounded_bridge<Op, Output>(
    capacity: usize,
) -> (BridgeSender<Op, Output>, BridgeReceiver<Op, Output>) {
    crossbeam::channel::bounded(capacity)
}

impl<Op, Output> BridgeRequest<Op, Output> {
    /// A request plus the receiver its reply will arrive on.
    pub fn new(
        correlation_id: impl Into<String>,
        caller_scope: impl Into<String>,
        op: Op,
    ) -> (Self, tokio::sync::oneshot::Receiver<BridgeReply<Output>>) {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let request = Self {
            correlation_id: correlation_id.into(),
            caller_scope: caller_scope.into(),
            op,
            reply_tx,
        };
        (request, reply_rx)
    }

    /// Answer this request, echoing its correlation id. A dropped receiver
    /// (the API caller already timed out) silently discards the late reply.
    pub fn reply(self, result: Output) {
        let reply = BridgeReply {
            correlation_id: self.correlation_id,
            result,
        };
        if let Err(late) = self.reply_tx.send(reply) {
            tracing::debug!(
                correlation_id = %late.correlation_id,
                "bridge reply dropped — the API caller already timed out"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_echoes_the_correlation_id_and_a_late_one_is_dropped() {
        let (request, mut reply_rx) = BridgeRequest::<&str, u8>::new("c1", "VERSE#v", "op");
        assert_eq!((request.caller_scope.as_str(), request.op), ("VERSE#v", "op"));
        request.reply(7);
        let reply = reply_rx.try_recv().expect("the reply arrives on its own slot");
        assert_eq!(reply, BridgeReply { correlation_id: "c1".into(), result: 7 });

        let (late, reply_rx) = BridgeRequest::<(), u8>::new("c2", "VERSE#v", ());
        drop(reply_rx);
        late.reply(1); // must not panic: the caller timed out
    }

    #[test]
    fn the_bridge_is_bounded_at_its_capacity() {
        let (tx, _rx) = bounded_bridge::<(), ()>(2);
        for i in 0..2 {
            tx.try_send(BridgeRequest::new(format!("c{i}"), "VERSE#v", ()).0)
                .expect("within capacity");
        }
        assert!(matches!(
            tx.try_send(BridgeRequest::new("overflow", "VERSE#v", ()).0),
            Err(crossbeam::channel::TrySendError::Full(_))
        ));
    }
}
