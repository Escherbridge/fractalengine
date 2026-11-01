//! API→Bevy screenshot bridge types — see src/AGENTS.md §screenshot-bridge.

use crate::bridge::{bounded_bridge, BridgeReceiver, BridgeReply, BridgeRequest, BridgeSender};

/// Bounded depth of the request channel; a full channel is a busy refusal, never a block.
pub const SCREENSHOT_BRIDGE_CAPACITY: usize = 4;

/// The plain GET's only operation: capture the primary window (egui included).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapturePrimaryWindow;

/// PNG bytes, or the host's error text.
pub type ScreenshotResult = Result<Vec<u8>, String>;

/// One capture request on the shared correlated-bridge envelope
/// (§correlated-bridge): correlation id, caller scope (refused unless it
/// covers the active verse — agent_vision SEC-ADDENDUM), reply slot.
pub type ScreenshotRequest = BridgeRequest<CapturePrimaryWindow, ScreenshotResult>;

/// The capture outcome, echoing the request's correlation id.
pub type ScreenshotReply = BridgeReply<ScreenshotResult>;

/// The reply text of the uniform scope denial (mapped to 404 by fe-api;
/// identical for "out of scope" and "nothing open" — §existence-oracle).
pub const SCREENSHOT_SCOPE_DENIED: &str = "not found or not permitted";

pub type ScreenshotRequestSender = BridgeSender<CapturePrimaryWindow, ScreenshotResult>;
pub type ScreenshotRequestReceiver = BridgeReceiver<CapturePrimaryWindow, ScreenshotResult>;

/// A fresh bounded request channel (API holds the sender, the Bevy host the receiver).
pub fn screenshot_bridge() -> (ScreenshotRequestSender, ScreenshotRequestReceiver) {
    bounded_bridge(SCREENSHOT_BRIDGE_CAPACITY)
}
