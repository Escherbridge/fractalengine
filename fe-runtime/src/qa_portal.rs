//! Fixed native fixture operations; see src/AGENTS.md section QA Portal evidence.

use crate::bridge::{bounded_bridge, BridgeReceiver, BridgeSender};
use crate::qa_debug::QaFailure;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

pub const JOB_LIMIT: Duration = Duration::from_secs(5);
pub const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DIMENSION: u32 = 2048;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixtureOp {
    Observe,
    ActivateSecondLink,
    ActivateRefusedThirdLink,
    Capture,
    /// Bundle page: title, probe digest and named probe texts.
    PortalObserve,
    /// Bundle page: activate `data-fe-slot` number 1..=32.
    PortalSlot(u8),
    /// Host IPC outcome counters.
    PortalIpcStats,
    /// Engage or restore the portal resolver's host-disabled state (the Disable all path); served by the binary.
    PortalKillSwitch { engage: bool },
    /// Post one fixed PJ2 IPC row through the open bundle's bridge; `expect` pins the mount it must hit; served by the binary.
    PortalIpcPost { row: IpcPostRowName, expect: MountExpectation },
}

/// The fixed PJ2 rows, mirrored from `fe_webview::portal_bundle::qa_rows::IpcPostRow` (fe-runtime has no fe-webview dependency).
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum IpcPostRowName {
    Unsigned,
    Unadmitted,
    External,
    ForeignFrame,
    Hidden,
    Stale,
    Control,
}

/// Optional identity the open mount must have (BLAKE3 of the package id; the content hash's 32 bytes); `None` = any.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct MountExpectation {
    pub package_digest: Option<[u8; 32]>,
    pub content_hash: Option<[u8; 32]>,
}

impl MountExpectation {
    /// From the request's optional strings; `None` when `content_hash` is not 64 hex characters.
    pub fn from_request(package_id: Option<&str>, content_hash: Option<&str>) -> Option<Self> {
        let content_hash = match content_hash {
            Some(raw) => Some(decode_content_hash(raw)?),
            None => None,
        };
        Some(Self {
            package_digest: package_id.map(package_digest),
            content_hash,
        })
    }

    /// True when the open mount's `package_id` and hex `content_hash` satisfy every pinned field.
    pub fn matches(&self, package_id: &str, content_hash: &str) -> bool {
        self.package_digest.is_none_or(|digest| digest == package_digest(package_id))
            && self
                .content_hash
                .is_none_or(|expected| decode_content_hash(content_hash) == Some(expected))
    }
}

fn package_digest(package_id: &str) -> [u8; 32] {
    *blake3::hash(package_id.as_bytes()).as_bytes()
}

fn decode_content_hash(raw: &str) -> Option<[u8; 32]> {
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(raw, &mut bytes).ok()?;
    Some(bytes)
}

#[derive(Clone, Debug)]
pub struct FixtureCommand {
    pub operation: FixtureOp,
    pub deadline: Instant,
}

impl FixtureCommand {
    pub fn is_current(&self) -> bool {
        let now = Instant::now();
        self.deadline > now && self.deadline.duration_since(now) <= JOB_LIMIT
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureObservation {
    pub page: String,
    pub title: String,
    pub heading: String,
    pub second_link: String,
    pub third_link: String,
    pub ready: bool,
    pub invoked: bool,
}

impl FixtureObservation {
    pub fn validate(&self, raw: &str) -> Result<(), QaFailure> {
        let Some(page) = raw
            .rsplit('/')
            .next()
            .filter(|page| matches!(*page, "one" | "two"))
        else {
            return Err(unavailable());
        };
        if self.page != page
            || self.title != format!("QA Portal {page}")
            || self.heading != self.title
            || self.second_link != "/web/two"
            || self.third_link != "/web/three"
            || !self.ready
        {
            return Err(failure(
                "portal_fixture_document_mismatch",
                "Native document does not match the fixed fixture",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FixtureReceipt {
    pub scope: String,
    pub operation: FixtureOp,
    pub popup_session: u64,
    pub page_generation: u64,
    pub url: String,
    pub observation: FixtureObservation,
    pub link_invoked: bool,
    pub native_refusal_observed: bool,
    pub capture_source: Option<&'static str>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub sha256: Option<String>,
    /// Bundle-page evidence (title, probe digest, slot/IPC counters); `None` for the legacy fixture.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portal: Option<serde_json::Value>,
}

#[derive(Debug)]
pub struct FixtureResult {
    pub receipt: FixtureReceipt,
    pub png: Option<Vec<u8>>,
}
pub type FixtureOutcome = Result<FixtureResult, QaFailure>;
pub type FixtureSender = BridgeSender<FixtureCommand, FixtureOutcome>;
pub type FixtureReceiver = BridgeReceiver<FixtureCommand, FixtureOutcome>;

pub fn fixture_bridge() -> (FixtureSender, FixtureReceiver) {
    bounded_bridge(4)
}
pub fn unavailable() -> QaFailure {
    failure(
        "portal_fixture_unavailable",
        "A live owned native fixture Portal is required",
    )
}
pub fn stale() -> QaFailure {
    failure(
        "portal_fixture_changed",
        "Portal, scope, page, helper or request lifetime changed",
    )
}
pub fn failure(code: &'static str, message: &'static str) -> QaFailure {
    QaFailure { code, message }
}
