//! Summary-fed model behind the Repository inspector section. See `fe-ui/src/AGENTS.md` §portal.

use std::collections::HashMap;

use bevy::prelude::Resource;
use fe_webview::ipc::{BlockedReason, BundleState, RevokedReason};
use serde_json::Value;

use super::repository::RepositoryView;

/// One node's last portal-bundle summary (README §8.5 `data`), reduced to what the section shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleSummary {
    pub asset_id: String,
    pub title: String,
    pub package_id: String,
    pub version: String,
    pub content_hash: String,
    pub sealed_by: Option<String>,
    pub trust_root_name: Option<String>,
    /// Granted capability strings (ipc-v1 `capabilities`); empty when none are declared.
    pub capabilities: Vec<String>,
    /// Local-identity-only `admission.{state, reason}`; `None` for a remote read.
    pub admission: Option<(String, Option<String>)>,
}

/// Summary cache keyed by repository node id; fed by the binary's summary read, never fetched here.
#[derive(Resource, Debug, Default)]
pub struct RepositoryViewState {
    summaries: HashMap<String, BundleSummary>,
    /// Node ids whose summary should be (re)read; the binary drains this.
    wanted: Vec<String>,
}

impl RepositoryViewState {
    /// Ask for a summary read unless one is cached or already queued.
    pub fn request(&mut self, node_id: &str) {
        if !self.summaries.contains_key(node_id) && !self.wanted.iter().any(|queued| queued == node_id) {
            self.wanted.push(node_id.to_owned());
        }
    }

    /// Drain the queued node ids (binary side).
    pub fn take_wanted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.wanted)
    }

    /// Store a parsed summary for `node_id`.
    pub fn ingest(&mut self, node_id: &str, summary: BundleSummary) {
        self.summaries.insert(node_id.to_owned(), summary);
    }

    /// Drop a node's cache (its `portal.bundle_asset` changed or bytes arrived) and queue a re-read.
    pub fn invalidate(&mut self, node_id: &str) {
        self.summaries.remove(node_id);
        self.request(node_id);
    }

    /// The section's data for `node_id`; `None` until a summary arrives.
    pub fn view(&self, node_id: &str, live: Option<&BundleState>, can_replace: bool) -> Option<RepositoryView> {
        let summary = self.summaries.get(node_id)?;
        Some(build_repository_view(summary, live, can_replace))
    }
}

/// Pure: parse a summary `data` object; `None` when the identifying fields are missing.
pub fn parse_summary(data: &Value) -> Option<BundleSummary> {
    let text = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_owned);
    let admission = data.get("admission").and_then(|admission| {
        let state = admission.get("state")?.as_str()?.to_owned();
        let reason = admission.get("reason").and_then(Value::as_str).map(str::to_owned);
        Some((state, reason))
    });
    Some(BundleSummary {
        asset_id: text("asset_id")?,
        title: text("title").unwrap_or_default(),
        package_id: text("package_id")?,
        version: text("version").unwrap_or_default(),
        content_hash: text("content_hash")?,
        sealed_by: text("sealed_by"),
        trust_root_name: text("trust_root_name"),
        capabilities: data
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default(),
        admission,
    })
}

/// Pure: admission `{state, reason}` strings -> chrome state; unknown values fail closed to a non-admittable static view.
pub fn admission_to_state(state: &str, reason: Option<&str>, publisher: Option<&str>) -> BundleState {
    match state {
        "admitted" => match publisher {
            Some(did) => BundleState::Admitted { publisher: did.to_owned() },
            None => BundleState::StaticSandbox { can_admit: false },
        },
        "static" | "static_sandbox" => BundleState::StaticSandbox { can_admit: reason != Some("cannot_admit") },
        "waiting_for_bytes" => BundleState::WaitingForBytes,
        "unavailable" => BundleState::Unavailable,
        "blocked" => BundleState::Blocked(match reason {
            Some("malformed_manifest") => BlockedReason::MalformedManifest,
            Some("too_large") => BlockedReason::TooLarge,
            Some("verse_policy") => BlockedReason::VersePolicy,
            Some("unsupported_platform") => BlockedReason::UnsupportedPlatform,
            _ => BlockedReason::Integrity,
        }),
        "revoked" => BundleState::Revoked(match reason {
            Some("kill_switch") => RevokedReason::KillSwitch,
            Some("revocation_list") => RevokedReason::RevocationList,
            _ => RevokedReason::AdmissionWithdrawn,
        }),
        _ => BundleState::StaticSandbox { can_admit: false },
    }
}

/// Pure: plain-language name for one granted capability string (ux.md §2 "Access").
pub fn capability_label(capability: &str) -> String {
    match capability {
        "subscribe:node" => "Read node changes".into(),
        "subscribe:readings" => "Read sensor readings".into(),
        "act:set_property" => "Set properties".into(),
        "intent:focus_node" => "Focus nodes".into(),
        "intent:select_node" => "Select nodes".into(),
        other => match other.split_once(':') {
            Some(("subscribe", target)) => match target.strip_prefix("query/") {
                Some(name) => format!("Read query {name}"),
                None => format!("Read {target}"),
            },
            Some(("invoke", target)) => format!("Run {target}"),
            Some(("emit", target)) => format!("Publish {target}"),
            _ => other.to_owned(),
        },
    }
}

/// Pure: a live mounted state wins over the summary's admission; without either the section reads as a plain static view.
pub fn build_repository_view(summary: &BundleSummary, live: Option<&BundleState>, can_replace: bool) -> RepositoryView {
    let from_summary = summary
        .admission
        .as_ref()
        .map(|(state, reason)| admission_to_state(state, reason.as_deref(), summary.sealed_by.as_deref()))
        .unwrap_or(BundleState::StaticSandbox { can_admit: false });
    let state = match live {
        Some(BundleState::Closed) | None => from_summary,
        Some(live) => live.clone(),
    };
    RepositoryView {
        title: summary.title.clone(),
        publisher_did: summary.sealed_by.clone(),
        trust_root_name: summary.trust_root_name.clone(),
        version: summary.version.clone(),
        content_hash: summary.content_hash.clone(),
        state,
        capabilities: summary.capabilities.iter().map(|capability| capability_label(capability)).collect(),
        can_replace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_feeds_the_section_and_live_state_wins() {
        let data = serde_json::json!({
            "asset_id": "a1", "package_id": "docs.manual", "title": "Manual", "version": "1.2.0",
            "content_hash": "0123456789abcdef", "sealed_by": "did:key:z6Mkexample0000",
            "capabilities": ["subscribe:node", "act:set_property"],
            "admission": {"state": "admitted", "reason": null}
        });
        let summary = parse_summary(&data).expect("parses");
        let mut cache = RepositoryViewState::default();
        cache.request("n1");
        assert_eq!(cache.take_wanted(), ["n1"]);
        assert!(cache.view("n1", None, false).is_none());
        cache.ingest("n1", summary);
        let view = cache.view("n1", None, true).unwrap();
        assert!(matches!(view.state, BundleState::Admitted { .. }));
        assert_eq!(view.capabilities, ["Read node changes", "Set properties"]);
        let live = BundleState::Revoked(RevokedReason::KillSwitch);
        assert_eq!(cache.view("n1", Some(&live), false).unwrap().state, live);
        cache.invalidate("n1");
        assert!(cache.view("n1", None, false).is_none());
        assert_eq!(cache.take_wanted(), ["n1"]);
    }
}
