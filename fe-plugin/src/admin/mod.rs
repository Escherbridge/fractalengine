//! Local extension-administration channel shared by the app (listener) and `fe-ext admin` (client).
//! Contract: docs/extensions/admin-channel.md; rationale: `src/admin/AGENTS.md`.

pub mod protocol;
pub mod transport;

#[cfg(windows)]
mod pipe_windows;
#[cfg(unix)]
mod socket_unix;

#[cfg(test)]
mod tests;

use std::time::Duration;

pub use protocol::{AdminReply, AdminRequest, AdminVerb};
pub use transport::{connect_local, listen_local, AdminListener, AdminStream, LocalEndpoint};

/// Env switch: `0` turns the channel off; anything else (or unset) leaves it on whenever the GUI runs.
pub const ENV_ADMIN_CHANNEL: &str = "FE_EXT_ADMIN_CHANNEL";
/// Connections the app serves at once; one more is refused with `busy`.
pub const MAX_CLIENTS: usize = 4;
/// How long the app waits for a request frame on an open connection before closing it.
pub const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the app waits on the GUI consent dialog before answering `consent-timeout` (the dialog stays open).
pub const CONSENT_WAIT: Duration = Duration::from_secs(15 * 60);

/// Whether [`ENV_ADMIN_CHANNEL`] leaves the channel on.
pub fn channel_enabled() -> bool {
    channel_enabled_by(std::env::var(ENV_ADMIN_CHANNEL).ok().as_deref())
}

/// [`channel_enabled`] over an explicit value (exactly `0` disables).
pub fn channel_enabled_by(value: Option<&str>) -> bool {
    value.map(str::trim) != Some("0")
}
