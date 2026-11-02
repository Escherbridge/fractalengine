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
}

#[derive(Clone, Debug)]
pub struct FixtureCommand {
    pub operation: FixtureOp,
    pub deadline: Instant,
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
