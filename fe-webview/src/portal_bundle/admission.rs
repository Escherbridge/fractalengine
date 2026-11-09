//! Governance-backed admission (admission.md §2-§6): the verdict over governance's view, kill-list matching by hash, and `host/admission` effects on a live mount; see `portal_bundle/AGENTS.md` §admission.

use serde::{Deserialize, Serialize};

use super::mount::{AdmissionVerdict, BundleIdentity, MountMode};
use crate::ipc::{BlockedReason, BundleState, RevokedReason};

// ---------------------------------------------------------------------------
// Governance's records and view (A-2, A-15; governance.md §admission-record)
// ---------------------------------------------------------------------------

/// Governance's record state for one `(package_id, content_hash)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordState {
    Admitted,
    Disabled,
    PolicyDenied,
    Revoked,
    Removed,
}

/// The closed `reason` set of `host/admission` (bus.md §2.4); a new reason is a contract change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecordReason {
    Consented,
    Restored,
    LocalDisable,
    DisableAll,
    ConsentRevoked,
    PolicyRemoved,
    Ceiling,
    RoleLost,
    RevocationList,
    Uninstalled,
}

/// One admission record as governance's view holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRecord {
    pub state: RecordState,
    pub reason: RecordReason,
    /// The sealing DID consent was given to.
    pub publisher_did: String,
    /// The capability digest consent covers (README §7.2; sibling R19-R20).
    pub capability_digest: String,
}

/// How the accepted kill list (layer d) treats one digest on this peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KillListStatus {
    #[default]
    Clear,
    /// An accepted entry's `content_hash` equals the digest, whatever `package_id` the entry names.
    Listed,
    /// Listed, with an audited verse-Owner override for this peer (steer 2026-11-08 22:00).
    Overridden,
}

/// Governance's cached, synchronous view (A-15): every call is non-blocking and safe on the main thread.
pub trait GovernanceView: Send + Sync + 'static {
    /// Layer (b): Disable all, or `FE_EXT_DISABLE_ALL=1`.
    fn host_disabled(&self) -> bool;
    /// Layer (d) for one 64-hex digest; takes no package id, so an entry can never be narrowed by one.
    fn kill_list(&self, digest: &str) -> KillListStatus;
    /// The record keyed by this exact pair, if any.
    fn record(&self, package_id: &str, content_hash: &str) -> Option<AdmissionRecord>;
    /// May the local identity admit this build at `node_id` (trust root, verse policy, role; sibling R18-R19)?
    fn can_admit(&self, node_id: &str, identity: &BundleIdentity) -> bool;
    /// A verse-policy ceiling forbids rendering this build at `node_id` (A-12); none by default.
    fn render_ceiling(&self, _node_id: &str, _identity: &BundleIdentity) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// The verdict (admission.md §2)
// ---------------------------------------------------------------------------

/// Everything [`decide`] reads, gathered from a [`GovernanceView`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdmissionInputs {
    pub host_disabled: bool,
    /// Combined over the build's digests by [`kill_list_status`].
    pub kill_list: KillListStatus,
    pub record: Option<AdmissionRecord>,
    pub render_ceiling: bool,
    pub can_admit: bool,
}

/// admission.md §2 rules 3-5 and 10-12 for one validated build; the first matching rule wins.
pub fn decide(identity: &BundleIdentity, inputs: &AdmissionInputs) -> AdmissionVerdict {
    let record = inputs.record.as_ref();
    let record_is = |state: RecordState| record.is_some_and(|record| record.state == state);
    // 3. Host-wide disable, or this installation locally disabled.
    let locally_disabled = record.is_some_and(|record| {
        record.state == RecordState::Disabled
            && matches!(record.reason, RecordReason::LocalDisable | RecordReason::DisableAll)
    });
    if inputs.host_disabled || locally_disabled {
        return AdmissionVerdict::Revoked(RevokedReason::KillSwitch);
    }
    // 4. Kill-listed under any id (unless overridden), or governance's own record says so.
    if inputs.kill_list == KillListStatus::Listed || record_is(RecordState::Revoked) {
        return AdmissionVerdict::Revoked(RevokedReason::RevocationList);
    }
    // 5. The record was withdrawn: consent revoked, policy removal, role drop.
    if record_is(RecordState::Disabled) || record_is(RecordState::PolicyDenied) {
        return AdmissionVerdict::Revoked(RevokedReason::AdmissionWithdrawn);
    }
    // 10. A verse-policy ceiling forbids rendering it.
    if inputs.render_ceiling {
        return AdmissionVerdict::Blocked(BlockedReason::VersePolicy);
    }
    // 11. Admitted for this exact pair, sealed by the consented publisher, for exactly these capabilities.
    let sealed_by = identity.publisher_did.as_deref();
    if let (Some(record), Some(publisher)) = (record, sealed_by) {
        if record.state == RecordState::Admitted
            && record.publisher_did == publisher
            && record.capability_digest == identity.capability_digest
        {
            return AdmissionVerdict::Admitted {
                publisher_did: publisher.to_owned(),
            };
        }
    }
    // 12. Static sandbox; an unsealed build can never be admitted (D-X5).
    AdmissionVerdict::Unadmitted {
        can_admit: inputs.can_admit && sealed_by.is_some(),
    }
}

/// Combine the list status of one build's digests: an override on any digest names the build.
pub fn kill_list_status(view: &dyn GovernanceView, digests: &[&str]) -> KillListStatus {
    let statuses: Vec<KillListStatus> = digests.iter().map(|digest| view.kill_list(digest)).collect();
    if statuses.contains(&KillListStatus::Overridden) {
        KillListStatus::Overridden
    } else if statuses.contains(&KillListStatus::Listed) {
        KillListStatus::Listed
    } else {
        KillListStatus::Clear
    }
}

/// The full verdict for a build opened at `node_id`: the mount's admission source when governance is injected.
pub fn verdict(view: &dyn GovernanceView, node_id: &str, identity: &BundleIdentity) -> AdmissionVerdict {
    decide(
        identity,
        &AdmissionInputs {
            host_disabled: view.host_disabled(),
            kill_list: kill_list_status(view, &identity.digests()),
            record: view.record(&identity.package_id, &identity.content_hash),
            render_ceiling: view.render_ceiling(node_id, identity),
            can_admit: view.can_admit(node_id, identity),
        },
    )
}

/// The scope-free verdict behind the summary read's `admission` (README §8.5): no ceiling, no `can_admit`.
pub fn record_verdict(view: &dyn GovernanceView, identity: &BundleIdentity) -> AdmissionVerdict {
    decide(
        identity,
        &AdmissionInputs {
            host_disabled: view.host_disabled(),
            kill_list: kill_list_status(view, &identity.digests()),
            record: view.record(&identity.package_id, &identity.content_hash),
            render_ceiling: false,
            can_admit: false,
        },
    )
}

/// Rules 3-4 before any byte is read, from the asset row's blob hash: what a resolver puts in `BundleSource::revoked`.
pub fn revoked_before_bytes(view: &dyn GovernanceView, blob_hash: Option<&str>) -> Option<RevokedReason> {
    if view.host_disabled() {
        return Some(RevokedReason::KillSwitch);
    }
    blob_hash
        .filter(|digest| kill_list_status(view, &[*digest]) == KillListStatus::Listed)
        .map(|_| RevokedReason::RevocationList)
}

/// The mount mode a verdict allows for a build sealed by `sealed_by` (and its `can_admit`), or the state shown instead.
pub fn mount_mode(verdict: &AdmissionVerdict, sealed_by: Option<&str>) -> Result<(MountMode, bool), BundleState> {
    match verdict {
        AdmissionVerdict::Revoked(reason) => Err(BundleState::Revoked(*reason)),
        AdmissionVerdict::Blocked(reason) => Err(BundleState::Blocked(*reason)),
        AdmissionVerdict::Admitted { publisher_did } if sealed_by == Some(publisher_did.as_str()) => {
            Ok((MountMode::Admitted, false))
        }
        AdmissionVerdict::Admitted { .. } => Ok((MountMode::Static, false)),
        AdmissionVerdict::Unadmitted { can_admit } => Ok((MountMode::Static, *can_admit)),
    }
}

/// `{state, reason}` codes for the summary read (README §8.5).
pub fn summary_codes(verdict: &AdmissionVerdict) -> (&'static str, Option<&'static str>) {
    match verdict {
        AdmissionVerdict::Admitted { .. } => ("admitted", None),
        AdmissionVerdict::Unadmitted { .. } => ("static_sandbox", None),
        AdmissionVerdict::Revoked(reason) => (
            "revoked",
            Some(match reason {
                RevokedReason::KillSwitch => "kill_switch",
                RevokedReason::RevocationList => "revocation_list",
                RevokedReason::AdmissionWithdrawn => "admission_withdrawn",
            }),
        ),
        AdmissionVerdict::Blocked(reason) => (
            "blocked",
            Some(match reason {
                BlockedReason::Integrity => "integrity",
                BlockedReason::MalformedManifest => "malformed_manifest",
                BlockedReason::TooLarge => "too_large",
                BlockedReason::VersePolicy => "verse_policy",
                BlockedReason::UnsupportedPlatform => "unsupported_platform",
            }),
        ),
    }
}

// ---------------------------------------------------------------------------
// Signals and their effect on a live mount (admission.md §3, §5)
// ---------------------------------------------------------------------------

/// One `host/admission` delta (bus.md §2.4; A-16); unknown fields (`kind`, `verse_id`) are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionEvent {
    pub package_id: String,
    pub content_hash: String,
    pub state: RecordState,
    pub reason: RecordReason,
}

/// What reaches the revoke listener (`plugin::apply_portal_admission`); the binary posts both.
#[derive(Debug, Clone, PartialEq, Eq, bevy::prelude::Message)]
pub enum AdmissionSignal {
    /// One `host/admission` delta.
    Event(AdmissionEvent),
    /// Governance accepted a revocation list; a bundle with no record gets no `host/admission` event (A-17).
    ListAccepted,
}

/// The facts about a mounted build that admission events are matched against.
#[derive(Debug, Clone, Copy)]
pub struct MountedBuild<'a> {
    pub package_id: &'a str,
    pub content_hash: &'a str,
    pub archive_digest: &'a str,
    pub mode: MountMode,
}

/// The state an event forces on a mounted build, matched without the view (so a view lagging the bus cannot keep it loaded).
pub fn revocation_for(event: &AdmissionEvent, build: MountedBuild<'_>) -> Option<BundleState> {
    let same_pair = event.package_id == build.package_id && event.content_hash == build.content_hash;
    let names_build = event.content_hash == build.content_hash || event.content_hash == build.archive_digest;
    let reason = match (event.state, event.reason) {
        // Layer (b) is host-wide: it unloads whatever is mounted, static included.
        (RecordState::Disabled, RecordReason::DisableAll) => RevokedReason::KillSwitch,
        // Layer (d) matches by hash under any package id.
        (RecordState::Revoked, _) if names_build => RevokedReason::RevocationList,
        (RecordState::Disabled, RecordReason::LocalDisable) if same_pair => RevokedReason::KillSwitch,
        (RecordState::Disabled | RecordState::PolicyDenied, _) if same_pair => RevokedReason::AdmissionWithdrawn,
        (RecordState::Removed, _) if same_pair && build.mode == MountMode::Admitted => RevokedReason::AdmissionWithdrawn,
        _ => return None,
    };
    Some(BundleState::Revoked(reason))
}

/// True when `event` is a fresh consent for exactly this pair: an explicit local Admit (`restored` never remounts).
pub fn consents(event: &AdmissionEvent, package_id: &str, content_hash: &str) -> bool {
    event.state == RecordState::Admitted
        && event.reason == RecordReason::Consented
        && event.package_id == package_id
        && event.content_hash == content_hash
}

/// What re-judging a live (or just-prepared) mount does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejudge {
    Keep,
    /// Unload now and show this state (§5).
    Refuse(BundleState),
    /// A Static mount became admissible: reload it on its admitted label (the user's Open is still in force).
    Promote,
    /// Same mount, new chrome state (`can_admit` changed).
    Restate(BundleState),
}

/// Compare a fresh verdict with the mount it was prepared as (admission.md §3).
pub fn rejudge(verdict: &AdmissionVerdict, mode: MountMode, sealed_by: Option<&str>, can_admit: bool) -> Rejudge {
    match (mount_mode(verdict, sealed_by), mode) {
        (Err(state), _) => Rejudge::Refuse(state),
        // Uninstalled, or consent no longer covers this build: no IPC survives.
        (Ok((MountMode::Static, _)), MountMode::Admitted) => {
            Rejudge::Refuse(BundleState::Revoked(RevokedReason::AdmissionWithdrawn))
        }
        (Ok((MountMode::Admitted, _)), MountMode::Static) => Rejudge::Promote,
        (Ok((MountMode::Static, now)), MountMode::Static) if now != can_admit => {
            Rejudge::Restate(BundleState::StaticSandbox { can_admit: now })
        }
        _ => Rejudge::Keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLISHER: &str = "did:key:z6Mkpublisher";

    fn identity(publisher: Option<&str>, capability_digest: &str) -> BundleIdentity {
        BundleIdentity {
            package_id: "org.example.a".into(),
            content_hash: "h1".into(),
            archive_digest: "z1".into(),
            publisher_did: publisher.map(str::to_owned),
            capability_digest: capability_digest.into(),
        }
    }

    fn record(state: RecordState, reason: RecordReason) -> Option<AdmissionRecord> {
        Some(AdmissionRecord {
            state,
            reason,
            publisher_did: PUBLISHER.into(),
            capability_digest: "d1".into(),
        })
    }

    fn event(package_id: &str, content_hash: &str, state: RecordState, reason: RecordReason) -> AdmissionEvent {
        AdmissionEvent {
            package_id: package_id.into(),
            content_hash: content_hash.into(),
            state,
            reason,
        }
    }

    /// admission.md §2 precedence, then `host/admission` matching and re-judging (§3, §5).
    #[test]
    fn verdict_precedence_event_matching_and_rejudge() {
        use super::{RecordReason as Why, RecordState as State};
        let sealed = identity(Some(PUBLISHER), "d1");
        let admitted = AdmissionInputs {
            record: record(State::Admitted, Why::Consented),
            can_admit: true,
            ..AdmissionInputs::default()
        };
        let with = |record: Option<AdmissionRecord>| AdmissionInputs { record, ..admitted.clone() };
        let revoked = |reason| AdmissionVerdict::Revoked(reason);
        let rows: Vec<(&str, BundleIdentity, AdmissionInputs, AdmissionVerdict)> = vec![
            (
                "3: host-wide disable beats every later rule",
                sealed.clone(),
                AdmissionInputs { host_disabled: true, kill_list: KillListStatus::Listed, ..admitted.clone() },
                revoked(RevokedReason::KillSwitch),
            ),
            ("3: installation locally disabled", sealed.clone(), with(record(State::Disabled, Why::LocalDisable)), revoked(RevokedReason::KillSwitch)),
            (
                "4: a listed digest beats an Admitted record",
                sealed.clone(),
                AdmissionInputs { kill_list: KillListStatus::Listed, ..admitted.clone() },
                revoked(RevokedReason::RevocationList),
            ),
            ("4: governance's record says revoked", sealed.clone(), with(record(State::Revoked, Why::RevocationList)), revoked(RevokedReason::RevocationList)),
            (
                "4: an audited Owner override lets the record decide",
                sealed.clone(),
                AdmissionInputs { kill_list: KillListStatus::Overridden, ..admitted.clone() },
                AdmissionVerdict::Admitted { publisher_did: PUBLISHER.into() },
            ),
            ("5: consent revoked", sealed.clone(), with(record(State::Disabled, Why::ConsentRevoked)), revoked(RevokedReason::AdmissionWithdrawn)),
            ("5: role dropped below policy", sealed.clone(), with(record(State::PolicyDenied, Why::RoleLost)), revoked(RevokedReason::AdmissionWithdrawn)),
            (
                "10: a render ceiling",
                sealed.clone(),
                AdmissionInputs { render_ceiling: true, ..admitted.clone() },
                AdmissionVerdict::Blocked(BlockedReason::VersePolicy),
            ),
            ("11: exact pair, seal and consent", sealed.clone(), admitted.clone(), AdmissionVerdict::Admitted { publisher_did: PUBLISHER.into() }),
            ("11: a capability change needs re-consent", identity(Some(PUBLISHER), "d2"), admitted.clone(), AdmissionVerdict::Unadmitted { can_admit: true }),
            ("11: another publisher inherits nothing", identity(Some("did:key:other"), "d1"), admitted.clone(), AdmissionVerdict::Unadmitted { can_admit: true }),
            ("12: unsealed build with a stale Admitted record", identity(None, "d1"), admitted.clone(), AdmissionVerdict::Unadmitted { can_admit: false }),
            ("12: uninstalled", sealed.clone(), with(record(State::Removed, Why::Uninstalled)), AdmissionVerdict::Unadmitted { can_admit: true }),
            (
                "12: no record, no right to admit",
                sealed.clone(),
                AdmissionInputs { can_admit: false, ..with(None) },
                AdmissionVerdict::Unadmitted { can_admit: false },
            ),
        ];
        for (row, identity, inputs, expected) in rows {
            assert_eq!(decide(&identity, &inputs), expected, "{row}");
        }

        let build = MountedBuild {
            package_id: "org.example.a",
            content_hash: "h1",
            archive_digest: "z1",
            mode: MountMode::Admitted,
        };
        let state = |reason| Some(BundleState::Revoked(reason));
        for (row, event, expected) in [
            ("list entry under a renamed id", event("org.example.renamed", "h1", State::Revoked, Why::RevocationList), state(RevokedReason::RevocationList)),
            ("list entry naming the archive digest", event("org.example.x", "z1", State::Revoked, Why::RevocationList), state(RevokedReason::RevocationList)),
            ("list entry for another build", event("org.example.a", "h9", State::Revoked, Why::RevocationList), None),
            ("disable all reaches any mount", event("org.example.other", "h9", State::Disabled, Why::DisableAll), state(RevokedReason::KillSwitch)),
            ("local disable of this pair", event("org.example.a", "h1", State::Disabled, Why::LocalDisable), state(RevokedReason::KillSwitch)),
            ("local disable of another build", event("org.example.a", "h2", State::Disabled, Why::LocalDisable), None),
            ("consent revoked", event("org.example.a", "h1", State::Disabled, Why::ConsentRevoked), state(RevokedReason::AdmissionWithdrawn)),
            ("policy removed", event("org.example.a", "h1", State::PolicyDenied, Why::PolicyRemoved), state(RevokedReason::AdmissionWithdrawn)),
            ("uninstalled while admitted", event("org.example.a", "h1", State::Removed, Why::Uninstalled), state(RevokedReason::AdmissionWithdrawn)),
            ("consent changes nothing here", event("org.example.a", "h1", State::Admitted, Why::Consented), None),
        ] {
            assert_eq!(revocation_for(&event, build), expected, "{row}");
        }
        let consent = event("org.example.a", "h1", State::Admitted, Why::Consented);
        assert!(consents(&consent, "org.example.a", "h1"));
        assert!(!consents(&event("org.example.a", "h1", State::Admitted, Why::Restored), "org.example.a", "h1"), "restored never remounts");

        let admitted_verdict = AdmissionVerdict::Admitted { publisher_did: PUBLISHER.into() };
        for (row, verdict, mode, can_admit, expected) in [
            ("still admitted", admitted_verdict.clone(), MountMode::Admitted, false, Rejudge::Keep),
            ("revoked", revoked(RevokedReason::KillSwitch), MountMode::Static, true, Rejudge::Refuse(BundleState::Revoked(RevokedReason::KillSwitch))),
            (
                "admission gone under an admitted mount",
                AdmissionVerdict::Unadmitted { can_admit: true },
                MountMode::Admitted,
                false,
                Rejudge::Refuse(BundleState::Revoked(RevokedReason::AdmissionWithdrawn)),
            ),
            ("consented while static", admitted_verdict, MountMode::Static, true, Rejudge::Promote),
            (
                "may no longer admit",
                AdmissionVerdict::Unadmitted { can_admit: false },
                MountMode::Static,
                true,
                Rejudge::Restate(BundleState::StaticSandbox { can_admit: false }),
            ),
        ] {
            assert_eq!(rejudge(&verdict, mode, Some(PUBLISHER), can_admit), expected, "{row}");
        }
    }
}
