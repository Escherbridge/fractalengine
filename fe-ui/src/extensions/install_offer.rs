//! Install offer for an unresolved binding (bindings.md section 5.5): decides which single action a surface shows
//! and builds the consent request that carries the verse. Pure state; never installs on its own. See
//! `fe-ui/src/extensions/AGENTS.md` §install-offer.

use std::collections::HashSet;

use bevy::prelude::Resource;

use super::consent::{ConsentDecision, ConsentOrigin, ConsentRequest, KillListOverrideOffer};

/// Why a binding does not resolve on this host (the resolver's local states, bindings.md section 5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// No installation of the package exists here.
    NotInstalled,
    /// Installed, but not allowed in this verse (section 5.3).
    NotAccepted,
    /// The build is on an accepted kill list.
    Revoked,
    /// The binding names another peer (`runs_on`); never offered.
    Elsewhere,
    /// Policy forbids the package; never offered.
    PolicyDenied,
}

/// The single action a surface shows beside an unresolved binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferAction {
    /// Run the header phase and the consent dialog, then accept in the verse.
    Install,
    /// Already installed: record the local acceptance (consent still shown when the digest is new).
    AllowInVerse,
    /// Verse Owner only: the typed kill-list override.
    Override,
}

/// One unresolved binding as the host reports it (copied data; fe-ui has no fe-plugin dependency).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedBinding {
    pub verse_id: String,
    pub verse_name: String,
    /// Scope level the winning binding comes from, e.g. `petal Quay`.
    pub binding_scope: String,
    pub hook: String,
    pub package_id: String,
    pub version_req: String,
    pub reason: UnresolvedReason,
    /// Verse and local policy admit the package at package level.
    pub policy_admits: bool,
    /// A configured registry source has a build satisfying `version_req`.
    pub registry_has_build: bool,
    /// The local DID is the verse Owner (the kill-list override is reachable only then).
    pub local_is_owner: bool,
}

impl UnresolvedBinding {
    /// Key under which a dismissal is remembered: the binding version on this host.
    pub fn dismissal_key(&self) -> String {
        format!("{}|{}|{}|{}", self.verse_id, self.hook, self.package_id, self.version_req)
    }

    /// The one action to show, or `None` so there is never a dead control (a state without an offer).
    pub fn action(&self) -> Option<OfferAction> {
        match self.reason {
            UnresolvedReason::Elsewhere | UnresolvedReason::PolicyDenied => None,
            UnresolvedReason::Revoked => (self.local_is_owner && self.policy_admits).then_some(OfferAction::Override),
            UnresolvedReason::NotAccepted => self.policy_admits.then_some(OfferAction::AllowInVerse),
            UnresolvedReason::NotInstalled => (self.policy_admits && self.registry_has_build).then_some(OfferAction::Install),
        }
    }

    /// Consent request for the chosen action: the verse rides on the origin, a revoked build gets the override offer.
    /// `base` is the host's rendering of the package (sections, digest, signer); this only sets origin and kill list.
    pub fn consent_request(&self, mut base: ConsentRequest, revocation_reason: Option<String>) -> ConsentRequest {
        base.origin = ConsentOrigin::InstallOffer {
            verse_id: self.verse_id.clone(),
            verse_name: self.verse_name.clone(),
            binding_scope: self.binding_scope.clone(),
        };
        base.kill_list = match (self.reason, self.local_is_owner) {
            (UnresolvedReason::Revoked, true) => Some(KillListOverrideOffer {
                reason: revocation_reason.unwrap_or_default(),
                verse_id: self.verse_id.clone(),
                verse_name: self.verse_name.clone(),
            }),
            _ => None,
        };
        base
    }
}

/// Local dismissals; hide the offer for one binding version, never replicated (bindings.md section 5.5).
#[derive(Resource, Debug, Default)]
pub struct OfferDismissals {
    dismissed: HashSet<String>,
}

impl OfferDismissals {
    pub fn dismiss(&mut self, binding: &UnresolvedBinding) {
        self.dismissed.insert(binding.dismissal_key());
    }

    /// The action to render after dismissals; a changed `version_req` is a new binding version and offers again.
    pub fn visible_action(&self, binding: &UnresolvedBinding) -> Option<OfferAction> {
        if self.dismissed.contains(&binding.dismissal_key()) { None } else { binding.action() }
    }
}

/// What the host must record for an answered install offer: acceptance in the verse, or nothing.
pub fn acceptance_to_record(decision: &ConsentDecision) -> Option<(&str, &str)> {
    match decision {
        ConsentDecision::Approved { installation_id, accept_in_verse: Some(verse_id), .. } => {
            Some((installation_id.as_str(), verse_id.as_str()))
        }
        _ => None,
    }
}
