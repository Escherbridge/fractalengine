//! Fixed PJ2 IPC rows for the `ipc-post` QA operation: fixed sources and envelopes only; see `AGENTS.md` here §qa-rows.

use std::time::Instant;

use super::ipc::{Ingress, PortalBridge};
use super::mount::{MountMode, PortalMounts};
use super::origin::static_label;

/// Envelope id of every fixed row: far above any page counter, so the control reply never settles a page request.
pub const ROW_ENVELOPE_ID: u32 = 4_000_000_000;
/// The `external` row's sender: an ordinary web origin.
pub const EXTERNAL_ORIGIN: &str = "https://example.org/";
/// The `foreign_frame` row's sender when the bundle declares no https frame origin.
pub const FALLBACK_FRAME_ORIGIN: &str = "https://frame.example.org/";
/// A well-formed token that is never minted for the live document (tokens are 32 hex chars from a keyed hash).
const FOREIGN_TOKEN: &str = "00000000000000000000000000000000";

/// One fixed PJ2 row (portal-core step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcPostRow {
    /// Sender: the named build's static label origin, else the open build's (an unsigned bundle never gets the bridge).
    Unsigned,
    /// Sender: the named build's static label origin, else the open build's (signed but not admitted here).
    Unadmitted,
    /// Sender: [`EXTERNAL_ORIGIN`].
    External,
    /// Sender: the bundle's first declared https frame origin, else [`FALLBACK_FRAME_ORIGIN`].
    ForeignFrame,
    /// Sender: the admitted document with its current token, while the surface is hidden.
    Hidden,
    /// Sender: the admitted document, carrying a token that is not this generation's.
    Stale,
    /// Sender: the admitted, visible document with its current token; answered on the next pump.
    Control,
}

impl IpcPostRow {
    /// Every row, in scenario order.
    pub const ALL: [IpcPostRow; 7] = [
        Self::Unsigned,
        Self::Unadmitted,
        Self::External,
        Self::ForeignFrame,
        Self::Hidden,
        Self::Stale,
        Self::Control,
    ];

    /// True for the rows posted from the admitted document's own origin.
    pub fn needs_admitted_mount(self) -> bool {
        matches!(self, Self::Hidden | Self::Stale | Self::Control)
    }

    /// True for the rows whose sender is a named build's static origin: a content-hash pin names the sender, not the open mount.
    pub fn sender_is_named_build(self) -> bool {
        matches!(self, Self::Unsigned | Self::Unadmitted)
    }
}

/// What one row did to the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RowOutcome {
    pub row: IpcPostRow,
    /// Change in the process-total `dropped_origin` counter across the post.
    pub dropped_origin_delta: u64,
    /// Passed checks 1-3 and was queued for routing (`Ingress::Accepted`).
    pub delivered: bool,
    /// A check-3 refusal reply was queued at receipt (`Ingress::Replied`).
    pub replied: bool,
}

/// Post `row`'s fixed envelope from its fixed source through [`PortalBridge::ingress`].
///
/// `named_build` (hex content hash) is the `unsigned`/`unadmitted` sender's build, which need not be open; other rows
/// ignore it. `None` when the row needs a live mount (admitted, for the document rows) and there is none, or the
/// named build is not a valid hash; nothing is posted then.
/// The `hidden` row restores the surface's prior visibility afterwards.
pub fn post_row(
    bridge: &PortalBridge,
    mounts: &PortalMounts,
    row: IpcPostRow,
    named_build: Option<&str>,
    now: Instant,
) -> Option<RowOutcome> {
    let mount = mounts.current().filter(|mount| !mount.is_faulted());
    let admitted = mount.as_ref().filter(|mount| mount.mode == MountMode::Admitted);
    let foreign_body = envelope(FOREIGN_TOKEN);
    let (source, body) = match row {
        IpcPostRow::Unsigned | IpcPostRow::Unadmitted => {
            let label = match named_build {
                Some(content_hash) => static_label(content_hash)?,
                None => static_label(&mount.as_ref()?.content_hash)?,
            };
            (format!("{}/", label.https_origin()), foreign_body)
        }
        IpcPostRow::External => (EXTERNAL_ORIGIN.to_owned(), foreign_body),
        IpcPostRow::ForeignFrame => {
            let declared = mount
                .as_ref()?
                .frame_origins
                .iter()
                .find(|origin| origin.starts_with("https://"))
                .map(|origin| format!("{}/", origin.trim_end_matches('/')));
            (declared.unwrap_or_else(|| FALLBACK_FRAME_ORIGIN.to_owned()), foreign_body)
        }
        IpcPostRow::Hidden | IpcPostRow::Control => {
            let mount = admitted?;
            let token = bridge.document_token(mount, mounts.generation())?;
            (mount.entry_url.clone(), envelope(&token))
        }
        IpcPostRow::Stale => {
            let mount = admitted?;
            let token = bridge.document_token(mount, mounts.generation())?;
            // The live token cannot be FOREIGN_TOKEN short of a 128-bit collision; refuse rather than answer if it is.
            if token == FOREIGN_TOKEN {
                return None;
            }
            (mount.entry_url.clone(), foreign_body)
        }
    };
    let before = bridge.stats().total.dropped_origin;
    let ingress = if row == IpcPostRow::Hidden {
        let was_visible = bridge.is_visible();
        bridge.set_visible(false);
        let ingress = bridge.ingress(mounts, &source, &body, now);
        bridge.set_visible(was_visible);
        ingress
    } else {
        bridge.ingress(mounts, &source, &body, now)
    };
    let after = bridge.stats().total.dropped_origin;
    Some(RowOutcome {
        row,
        dropped_origin_delta: after.saturating_sub(before),
        delivered: ingress == Ingress::Accepted,
        replied: ingress == Ingress::Replied,
    })
}

/// The one fixed envelope: a `hello` (read-only, always answered when heard) under `token`.
fn envelope(token: &str) -> String {
    format!(r#"{{"v":1,"n":"{token}","id":{ROW_ENVELOPE_ID},"t":"hello"}}"#)
}
