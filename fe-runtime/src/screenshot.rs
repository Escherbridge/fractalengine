//! API→Bevy screenshot bridge types — see src/AGENTS.md §screenshot-bridge.

/// Bounded depth of the request channel; a full channel is a busy refusal, never a block.
pub const SCREENSHOT_BRIDGE_CAPACITY: usize = 4;

/// One capture request: the API thread's correlation id plus its private reply slot.
#[derive(Debug)]
pub struct ScreenshotRequest {
    pub correlation_id: String,
    pub reply_tx: tokio::sync::oneshot::Sender<ScreenshotReply>,
}

/// The capture outcome, echoing the request's correlation id (PNG bytes or an error text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotReply {
    pub correlation_id: String,
    pub result: Result<Vec<u8>, String>,
}

pub type ScreenshotRequestSender = crossbeam::channel::Sender<ScreenshotRequest>;
pub type ScreenshotRequestReceiver = crossbeam::channel::Receiver<ScreenshotRequest>;

/// A fresh bounded request channel (API holds the sender, the Bevy host the receiver).
pub fn screenshot_bridge() -> (ScreenshotRequestSender, ScreenshotRequestReceiver) {
    crossbeam::channel::bounded(SCREENSHOT_BRIDGE_CAPACITY)
}

impl ScreenshotRequest {
    /// Answer this request, echoing its correlation id. A dropped receiver (the
    /// API caller timed out) silently discards the late reply.
    pub fn reply(self, result: Result<Vec<u8>, String>) {
        let reply = ScreenshotReply {
            correlation_id: self.correlation_id,
            result,
        };
        if self.reply_tx.send(reply).is_err() {
            tracing::debug!("screenshot reply dropped — the API caller already timed out");
        }
    }
}
